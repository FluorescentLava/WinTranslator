use crate::model::*;
use anyhow::{Context, Result};
use std::{ffi::c_void, fs, path::PathBuf};

pub fn data_dir() -> PathBuf {
    std::env::var_os("LINGUA_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_else(|| ".".into()))
                .join("TranslationTool")
        })
}
fn write_json(path: PathBuf, value: &impl serde::Serialize) -> Result<()> {
    fs::create_dir_all(path.parent().context("没有数据目录")?)?;
    let temp = path.with_extension("tmp");
    fs::write(&temp, serde_json::to_vec_pretty(value)?)?;
    fs::rename(temp, path)?;
    Ok(())
}
pub fn load() -> (Settings, Vec<Translation>, Option<String>) {
    let dir = data_dir();
    let mut warnings = Vec::new();
    let mut settings: Settings = match fs::read(dir.join("settings.json")) {
        Ok(data) => serde_json::from_slice(&data).unwrap_or_else(|_| {
            warnings.push("设置文件无法读取，已使用默认设置");
            Settings::default()
        }),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                warnings.push("设置文件读取失败，已使用默认设置");
            }
            Settings::default()
        }
    };
    match fs::read(dir.join("credentials.dpapi")) {
        Ok(data) => match unprotect(&data) {
            Ok(plaintext) => match serde_json::from_slice(&plaintext) {
                Ok(credentials) => settings.credentials = credentials,
                Err(_) => warnings.push("API 凭据文件格式有误，请检查本机配置"),
            },
            Err(_) => warnings.push("无法解密 API 凭据，请使用保存凭据时的 Windows 账户"),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => warnings.push("API 凭据文件读取失败，请检查文件访问权限"),
    }
    let history = fs::read(dir.join("history.json"))
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default();
    let warning = (!warnings.is_empty()).then(|| warnings.join("；"));
    (settings, history, warning)
}
pub fn save_settings(settings: &Settings) -> Result<()> {
    let protected = protect(&serde_json::to_vec(&settings.credentials)?)?;
    let dir = data_dir();
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("credentials.dpapi.tmp"), protected)?;
    fs::rename(
        dir.join("credentials.dpapi.tmp"),
        dir.join("credentials.dpapi"),
    )?;
    write_json(dir.join("settings.json"), settings)
}
pub fn save_history(history: &[Translation]) -> Result<()> {
    write_json(data_dir().join("history.json"), &history)
}
// This local trace contains provenance and response metadata, never credentials or source text.
pub fn diagnostic(mut value: serde_json::Value) {
    use std::io::Write;
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let Ok(_guard) = LOCK.lock() else {
        return;
    };
    value["time"] = chrono::Local::now().to_rfc3339().into();
    value["process_id"] = std::process::id().into();
    let dir = data_dir();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("translation-diagnostics.jsonl");
    if fs::metadata(&path).is_ok_and(|m| m.len() > 1_048_576) {
        let previous = dir.join("translation-diagnostics.previous.jsonl");
        let _ = fs::remove_file(&previous);
        let _ = fs::rename(&path, previous);
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{value}");
    }
}

#[repr(C)]
struct Blob {
    len: u32,
    ptr: *mut u8,
}
#[link(name = "crypt32")]
unsafe extern "system" {
    fn CryptProtectData(
        input: *const Blob,
        description: *const u16,
        entropy: *const Blob,
        reserved: *const c_void,
        prompt: *const c_void,
        flags: u32,
        output: *mut Blob,
    ) -> i32;
    fn CryptUnprotectData(
        input: *const Blob,
        description: *mut *mut u16,
        entropy: *const Blob,
        reserved: *const c_void,
        prompt: *const c_void,
        flags: u32,
        output: *mut Blob,
    ) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LocalFree(memory: *mut c_void) -> *mut c_void;
}
fn crypt(data: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    anyhow::ensure!(data.len() <= u32::MAX as usize, "密钥数据过长");
    let input = Blob {
        len: data.len() as u32,
        ptr: data.as_ptr() as *mut u8,
    };
    let mut output = Blob {
        len: 0,
        ptr: std::ptr::null_mut(),
    };
    // DPAPI owns the output allocation; UI_FORBIDDEN keeps this operation noninteractive.
    unsafe {
        let ok = if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                &mut output,
            )
        };
        anyhow::ensure!(
            ok != 0,
            "Windows 密钥加解密失败：{}",
            std::io::Error::last_os_error()
        );
        let result = std::slice::from_raw_parts(output.ptr, output.len as usize).to_vec();
        LocalFree(output.ptr.cast());
        Ok(result)
    }
}
pub fn protect(data: &[u8]) -> Result<Vec<u8>> {
    crypt(data, true)
}
pub fn unprotect(data: &[u8]) -> Result<Vec<u8>> {
    crypt(data, false)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_are_encrypted_for_current_windows_user() {
        let original = b"test-only-secret";
        let encrypted = protect(original).unwrap();
        assert!(!encrypted.windows(original.len()).any(|x| x == original));
        assert_eq!(unprotect(&encrypted).unwrap(), original);
    }
}

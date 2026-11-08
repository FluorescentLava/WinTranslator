use crate::model::*;
use anyhow::{Context, Result};
use std::{
    ffi::c_void,
    fs,
    path::{Path, PathBuf},
};

fn failed_credential_reads() -> &'static std::sync::Mutex<std::collections::HashSet<PathBuf>> {
    static FAILED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<PathBuf>>> =
        std::sync::OnceLock::new();
    FAILED.get_or_init(Default::default)
}

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
    let (settings, history, warning, report) = load_from(&dir);
    diagnostic(report);
    (settings, history, warning)
}

fn file_report(bytes: Option<usize>, status: &str, os_error: Option<i32>) -> serde_json::Value {
    serde_json::json!({"status": status, "bytes": bytes, "os_error": os_error})
}

fn load_from(
    dir: &Path,
) -> (
    Settings,
    Vec<Translation>,
    Option<String>,
    serde_json::Value,
) {
    let mut warnings = Vec::new();
    let settings_report;
    let mut settings: Settings = match fs::read(dir.join("settings.json")) {
        Ok(data) => match serde_json::from_slice(&data) {
            Ok(settings) => {
                settings_report = file_report(Some(data.len()), "loaded", None);
                settings
            }
            Err(_) => {
                settings_report = file_report(Some(data.len()), "invalid_json", None);
                warnings.push("设置文件无法读取，已使用默认设置");
                Settings::default()
            }
        },
        Err(error) => {
            settings_report = file_report(
                None,
                if error.kind() == std::io::ErrorKind::NotFound {
                    "missing"
                } else {
                    "read_failed"
                },
                error.raw_os_error(),
            );
            if error.kind() != std::io::ErrorKind::NotFound {
                warnings.push("设置文件读取失败，已使用默认设置");
            }
            Settings::default()
        }
    };
    let credentials_report = match fs::read(dir.join("credentials.dpapi")) {
        Ok(data) => match unprotect(&data) {
            Ok(plaintext) => match serde_json::from_slice(&plaintext) {
                Ok(credentials) => {
                    settings.credentials = credentials;
                    file_report(Some(data.len()), "loaded", None)
                }
                Err(_) => {
                    warnings.push("API 凭据文件格式有误，请检查本机配置");
                    file_report(Some(data.len()), "invalid_json", None)
                }
            },
            Err(error) => {
                warnings.push("无法解密 API 凭据，请使用保存凭据时的 Windows 账户");
                file_report(
                    Some(data.len()),
                    "decrypt_failed",
                    error
                        .downcast_ref::<std::io::Error>()
                        .and_then(std::io::Error::raw_os_error),
                )
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            file_report(None, "missing", error.raw_os_error())
        }
        Err(error) => {
            warnings.push("API 凭据文件读取失败，请检查文件访问权限");
            file_report(None, "read_failed", error.raw_os_error())
        }
    };
    if let Ok(mut failed) = failed_credential_reads().lock() {
        match credentials_report["status"].as_str() {
            Some("loaded") => {
                failed.remove(dir);
            }
            Some("read_failed" | "decrypt_failed" | "invalid_json") => {
                failed.insert(dir.to_path_buf());
            }
            _ => {}
        }
    }
    let history = fs::read(dir.join("history.json"))
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or_default();
    let warning = (!warnings.is_empty()).then(|| warnings.join("；"));
    let report = serde_json::json!({
        "event": "configuration_load", "data_directory": dir,
        "settings_file": settings_report, "credentials_file": credentials_report,
        "load_warning": warning.is_some(), "selected_provider": settings.provider,
        "credentials_present": settings.credentials.iter().map(|credential| serde_json::json!({
            "has_id": !credential.id.trim().is_empty(), "has_key": !credential.key.trim().is_empty()
        })).collect::<Vec<_>>(),
    });
    (settings, history, warning, report)
}

/// Loads configuration before creating a window, registering hotkeys, or taking the instance lock.
/// The report contains only paths, read/decrypt outcomes, byte counts, and presence booleans.
pub fn check_configuration() -> Result<PathBuf> {
    let (_, _, _, mut report) = load_from(&data_dir());
    report["process_id"] = std::process::id().into();
    report["time"] = chrono::Local::now().to_rfc3339().into();
    report["executable"] = serde_json::to_value(std::env::current_exe().ok())?;
    report["working_directory"] = serde_json::to_value(std::env::current_dir().ok())?;
    for name in ["LINGUA_DATA_DIR", "LOCALAPPDATA", "APPDATA", "USERPROFILE"] {
        report[name] = serde_json::to_value(std::env::var_os(name).map(PathBuf::from))?;
    }
    let filename = format!("configuration-check.{}.json", std::process::id());
    let bytes = serde_json::to_vec_pretty(&report)?;
    let executable_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    for dir in executable_dir
        .into_iter()
        .chain(std::iter::once(std::env::temp_dir()))
    {
        let path = dir.join(&filename);
        if fs::write(&path, &bytes).is_ok() {
            return Ok(path);
        }
    }
    anyhow::bail!("无法写入配置检查报告")
}
pub fn save_settings(settings: &Settings) -> Result<()> {
    let dir = data_dir();
    save_settings_to(&dir, settings)
}

fn save_settings_to(dir: &Path, settings: &Settings) -> Result<()> {
    if failed_credential_reads()
        .lock()
        .map(|failed| failed.contains(dir))
        .unwrap_or(true)
    {
        anyhow::bail!("API 凭据尚未成功读取，已保留原文件；请重新加载配置后再保存设置");
    }
    // A new failure between load and save must not replace the existing encrypted data either.
    match fs::read(dir.join("credentials.dpapi")) {
        Ok(encrypted) => {
            let plaintext = unprotect(&encrypted).context("无法验证现有 API 凭据，已保留原文件")?;
            let _: [Credential; 4] = serde_json::from_slice(&plaintext)
                .context("现有 API 凭据格式有误，已保留原文件")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("无法读取现有 API 凭据，已保留原文件"),
    }
    let protected = protect(&serde_json::to_vec(&settings.credentials)?)?;
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
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let Ok(_guard) = LOCK.lock() else {
        return;
    };
    value["time"] = chrono::Local::now().to_rfc3339().into();
    value["process_id"] = std::process::id().into();
    let dir = data_dir();
    value["data_directory"] = serde_json::to_value(&dir).unwrap_or_default();
    let result = fs::create_dir_all(&dir)
        .and_then(|_| append_diagnostic(&dir.join("translation-diagnostics.jsonl"), &value));
    if let Err(error) = result {
        value["diagnostic_write_os_error"] = error.raw_os_error().into();
        value["diagnostic_write_failed"] = true.into();
        let executable_dir = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf));
        for fallback in executable_dir
            .into_iter()
            .chain(std::iter::once(std::env::temp_dir()))
        {
            let path = fallback.join(format!(
                "wintranslater-bootstrap.{}.jsonl",
                std::process::id()
            ));
            if append_diagnostic(&path, &value).is_ok() {
                break;
            }
        }
    }
}

fn append_diagnostic(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    use std::io::Write;
    if fs::metadata(&path).is_ok_and(|m| m.len() > 1_048_576) {
        let previous = path.with_extension("previous.jsonl");
        let _ = fs::remove_file(&previous);
        let _ = fs::rename(&path, previous);
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{value}")
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
        if ok == 0 {
            return Err(std::io::Error::last_os_error()).context("Windows 密钥加解密失败");
        }
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

    struct Profile(PathBuf);
    impl Profile {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "wintranslater-storage-test-{}-{}-{}",
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Profile {
        fn drop(&mut self) {
            failed_credential_reads().lock().unwrap().remove(&self.0);
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn configuration_load_restores_providers_and_reports_only_presence() {
        let profile = Profile::new();
        let mut expected = Settings {
            provider: Provider::DeepL,
            ..Settings::default()
        };
        expected.credentials[Provider::Baidu.index()] = Credential {
            id: "fixture-account".into(),
            key: "fixture-baidu-secret".into(),
        };
        expected.credentials[Provider::DeepL.index()].key = "fixture-deepl-secret".into();
        save_settings_to(&profile.0, &expected).unwrap();
        let (loaded, _, warning, report) = load_from(&profile.0);
        assert!(warning.is_none());
        assert!(loaded.provider == Provider::DeepL);
        assert!(loaded.credentials == expected.credentials);
        assert_eq!(report["settings_file"]["status"], "loaded");
        assert_eq!(report["credentials_file"]["status"], "loaded");
        let rendered = report.to_string();
        assert!(!rendered.contains("fixture-account"));
        assert!(!rendered.contains("fixture-baidu-secret"));
        assert!(!rendered.contains("fixture-deepl-secret"));
        assert_eq!(
            report["credentials_present"][Provider::DeepL.index()]["has_key"],
            true
        );
    }

    #[test]
    fn credential_read_failure_blocks_saving_and_preserves_existing_settings() {
        let profile = Profile::new();
        let original_settings = b"{}";
        fs::write(profile.0.join("settings.json"), original_settings).unwrap();
        fs::create_dir(profile.0.join("credentials.dpapi")).unwrap();
        let (_, _, warning, report) = load_from(&profile.0);
        assert!(warning.is_some());
        assert_eq!(report["credentials_file"]["status"], "read_failed");
        assert!(save_settings_to(&profile.0, &Settings::default()).is_err());
        assert!(profile.0.join("credentials.dpapi").is_dir());
        assert_eq!(
            fs::read(profile.0.join("settings.json")).unwrap(),
            original_settings
        );
    }

    #[test]
    fn invalid_encrypted_payload_is_preserved_until_a_successful_reload() {
        let profile = Profile::new();
        let path = profile.0.join("credentials.dpapi");
        let original = protect(br#"{"unexpected":[]}"#).unwrap();
        fs::write(&path, &original).unwrap();
        let (_, _, warning, report) = load_from(&profile.0);
        assert!(warning.is_some());
        assert_eq!(report["credentials_file"]["status"], "invalid_json");
        assert!(save_settings_to(&profile.0, &Settings::default()).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        let valid =
            protect(&serde_json::to_vec(&Settings::default().credentials).unwrap()).unwrap();
        fs::write(&path, valid).unwrap();
        let (_, _, warning, _) = load_from(&profile.0);
        assert!(warning.is_none());
        assert!(save_settings_to(&profile.0, &Settings::default()).is_ok());
    }

    #[test]
    fn corruption_after_loading_is_not_overwritten_by_a_later_settings_save() {
        let profile = Profile::new();
        save_settings_to(&profile.0, &Settings::default()).unwrap();
        let _ = load_from(&profile.0);
        let path = profile.0.join("credentials.dpapi");
        let damaged = b"fixture-invalid-dpapi";
        fs::write(&path, damaged).unwrap();
        assert!(save_settings_to(&profile.0, &Settings::default()).is_err());
        assert_eq!(fs::read(path).unwrap(), damaged);
    }

    #[test]
    fn a_new_profile_has_no_credentials_and_no_load_warning() {
        let profile = Profile::new();
        let (settings, history, warning, report) = load_from(&profile.0);
        assert!(warning.is_none());
        assert!(history.is_empty());
        assert!(
            settings
                .credentials
                .iter()
                .all(|credential| credential.id.is_empty() && credential.key.is_empty())
        );
        assert_eq!(report["settings_file"]["status"], "missing");
        assert_eq!(report["credentials_file"]["status"], "missing");
    }

    #[test]
    fn credentials_are_encrypted_for_current_windows_user() {
        let original = b"test-only-secret";
        let encrypted = protect(original).unwrap();
        assert!(!encrypted.windows(original.len()).any(|x| x == original));
        assert_eq!(unprotect(&encrypted).unwrap(), original);
    }
}

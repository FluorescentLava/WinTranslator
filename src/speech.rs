use crate::storage;
use sha2::{Digest, Sha256};
use std::{
    ffi::c_void,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accent {
    British,
    American,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Phonetics {
    pub british: String,
    pub american: String,
}

const MAX_PHONETICS_RESPONSE: usize = 512 * 1024;
const MAX_PHONETICS_CACHE_FILES: usize = 200;
static PHONETICS_CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

fn dictionary_word(text: &str) -> Option<&str> {
    let text = text.trim();
    (text.len() <= 128
        && text.bytes().any(|c| c.is_ascii_alphabetic())
        && text
            .chars()
            .all(|c| c.is_ascii_alphabetic() || matches!(c, '\'' | '’' | '-')))
    .then_some(text)
}

fn phonetic_value(value: &serde_json::Value) -> String {
    let Some(value) = value.as_str() else {
        return String::new();
    };
    let value = value.trim().trim_matches('/').trim();
    if value.len() > 256
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>'))
    {
        String::new()
    } else {
        value.to_owned()
    }
}

fn parse_phonetics(value: &serde_json::Value) -> Phonetics {
    let field = |name| {
        let primary = phonetic_value(&value["ec"]["word"][0][name]);
        if primary.is_empty() {
            phonetic_value(&value["simple"]["word"][0][name])
        } else {
            primary
        }
    };
    // Each accent has its own source field. A missing accent remains missing,
    // even when the other field exists or the local dictionary has one IPA.
    Phonetics {
        british: field("ukphone"),
        american: field("usphone"),
    }
}

fn phonetics_cache_path(word: &str) -> PathBuf {
    let mut hash = Sha256::new();
    hash.update(b"youdao-jsonapi-phonetics-v1\0");
    hash.update(word.as_bytes());
    storage::data_dir()
        .join("pronunciation")
        .join("phonetics")
        .join(format!("{}.json", hex::encode(hash.finalize())))
}

fn cached_phonetics(path: &Path) -> Option<Phonetics> {
    let _guard = CACHE_LOCK.lock().ok()?;
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 2_048 {
        return None;
    }
    let cached: Phonetics = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let clean = |value: &str| {
        value.len() <= 256
            && !value
                .chars()
                .any(|c| c.is_control() || matches!(c, '<' | '>'))
    };
    (clean(&cached.british)
        && clean(&cached.american)
        && !(cached.british.is_empty() && cached.american.is_empty()))
    .then_some(cached)
}

fn save_phonetics(path: &Path, value: &Phonetics) -> Result<(), String> {
    if value.british.is_empty() && value.american.is_empty() {
        return Ok(());
    }
    let _guard = CACHE_LOCK.lock().map_err(|_| "音标缓存不可用".to_owned())?;
    let directory = path.parent().ok_or_else(|| "音标缓存路径无效".to_owned())?;
    fs::create_dir_all(directory).map_err(|_| "无法创建音标缓存".to_owned())?;
    let mut files = fs::read_dir(directory)
        .map_err(|_| "无法读取音标缓存".to_owned())?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            let stem = name.strip_suffix(".json")?;
            if stem.len() != 64
                || !stem.bytes().all(|c| c.is_ascii_hexdigit())
                || entry.path() == path
            {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| {
                (
                    metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    entry.path(),
                )
            })
        })
        .collect::<Vec<_>>();
    files.sort_by_key(|file| file.0);
    let mut count = files.len() + 1;
    for (_, old) in files {
        if count <= MAX_PHONETICS_CACHE_FILES {
            break;
        }
        if fs::remove_file(old).is_ok() {
            count -= 1;
        }
    }
    if count > MAX_PHONETICS_CACHE_FILES {
        return Err("音标缓存已满".into());
    }
    let bytes = serde_json::to_vec(value).map_err(|_| "无法保存音标缓存".to_owned())?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let result = fs::write(&temp, bytes).and_then(|_| fs::rename(&temp, path));
    let _ = fs::remove_file(temp);
    result.map_err(|_| "无法保存音标缓存".into())
}

/// Queries real UK/US dictionary fields for one English word. Sentences and
/// missing fields return empty strings; a cached entry also works offline.
pub fn phonetics(text: &str, cancelled: &dyn Fn() -> bool) -> Result<Phonetics, String> {
    if cancelled() {
        return Err("已取消".into());
    }
    let Some(word) = dictionary_word(text) else {
        return Ok(Phonetics::default());
    };
    let path = phonetics_cache_path(word);
    if let Some(cached) = cached_phonetics(&path) {
        return if cancelled() {
            Err("已取消".into())
        } else {
            Ok(cached)
        };
    }
    let client = PHONETICS_CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_millis(900))
            .timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent("TranslationTool/0.1")
            .build()
            .expect("phonetics HTTP client initialization failed")
    });
    if cancelled() {
        return Err("已取消".into());
    }
    let mut response = client
        .get("https://dict.youdao.com/jsonapi")
        .query(&[("q", word)])
        .send()
        .map_err(|_| "音标查询连接失败或超时".to_owned())?;
    if !response.status().is_success() {
        return Err(format!(
            "音标查询失败（HTTP {}）",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_PHONETICS_RESPONSE as u64)
    {
        return Err("音标服务响应过大".into());
    }
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 16_384];
    loop {
        if cancelled() {
            return Err("已取消".into());
        }
        let count = response
            .read(&mut buffer)
            .map_err(|_| "音标查询读取失败或超时".to_owned())?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > MAX_PHONETICS_RESPONSE {
            return Err("音标服务响应过大".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    if cancelled() {
        return Err("已取消".into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "音标服务返回了无效数据".to_owned())?;
    let result = parse_phonetics(&value);
    if cancelled() {
        return Err("已取消".into());
    }
    // Disk failure should not hide valid dictionary data already received.
    let _ = save_phonetics(&path, &result);
    if cancelled() {
        return Err("已取消".into());
    }
    Ok(result)
}

impl Accent {
    fn dictionary_type(self) -> &'static str {
        match self {
            Self::British => "1",
            Self::American => "2",
        }
    }
    fn locale(self) -> &'static str {
        match self {
            Self::British => "en-GB",
            Self::American => "en-US",
        }
    }
    fn language_filter(self) -> &'static str {
        match self {
            Self::British => "Language=809",
            Self::American => "Language=409",
        }
    }
}

const MAX_DICTIONARY_CHARACTERS: usize = 200;
const MAX_AUDIO_BYTES: usize = 2 * 1024 * 1024;
const MAX_CACHE_FILES: usize = 100;
const MAX_CACHE_BYTES: u64 = 30 * 1024 * 1024;
const MCI_ALIAS: &str = "wt_pronunciation";
static GENERATION: AtomicU64 = AtomicU64::new(0);
static PLAYBACK: Mutex<Option<u64>> = Mutex::new(None);
static CACHE_LOCK: Mutex<()> = Mutex::new(());
static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

#[link(name = "winmm")]
unsafe extern "system" {
    fn mciSendStringW(command: *const u16, result: *mut u16, count: u32, callback: usize) -> u32;
    fn mciGetErrorStringW(error: u32, text: *mut u16, count: u32) -> i32;
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn mci(command: &str) -> Result<String, String> {
    let command = wide(command);
    let mut output = [0u16; 256];
    // All commands and aliases are constructed here. Text being pronounced is
    // never interpreted as an MCI command or a filename.
    let result = unsafe {
        mciSendStringW(
            command.as_ptr(),
            output.as_mut_ptr(),
            output.len() as u32,
            0,
        )
    };
    if result != 0 {
        let mut description = [0u16; 256];
        unsafe {
            mciGetErrorStringW(result, description.as_mut_ptr(), description.len() as u32);
        }
        let end = description
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(description.len());
        return Err(format!(
            "Windows 音频播放失败：{}（{result}）",
            String::from_utf16_lossy(&description[..end])
        ));
    }
    let end = output.iter().position(|c| *c == 0).unwrap_or(output.len());
    Ok(String::from_utf16_lossy(&output[..end]))
}
fn check_current(generation: u64, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if cancelled() || GENERATION.load(Ordering::Acquire) != generation {
        Err("已取消".into())
    } else {
        Ok(())
    }
}

/// Stops active playback and invalidates audio that is still downloading.
pub fn stop() {
    GENERATION.fetch_add(1, Ordering::AcqRel);
    if let Ok(mut current) = PLAYBACK.lock() {
        if current.take().is_some() {
            let _ = mci(&format!("close {MCI_ALIAS}"));
        }
    }
}

struct PlaybackGuard(u64);
impl Drop for PlaybackGuard {
    fn drop(&mut self) {
        if let Ok(mut current) = PLAYBACK.lock() {
            if *current == Some(self.0) {
                let _ = mci(&format!("close {MCI_ALIAS}"));
                *current = None;
            }
        }
    }
}

fn play_mp3(path: &Path, generation: u64, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    let absolute = fs::canonicalize(path).map_err(|_| "无法打开缓存音频".to_owned())?;
    let path = absolute
        .to_str()
        .ok_or_else(|| "音频路径不是有效 Unicode".to_owned())?;
    let path = if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
    };
    if path.chars().any(|c| matches!(c, '"' | '\0' | '\r' | '\n')) {
        return Err("音频路径包含不支持的字符".into());
    }
    let guard;
    {
        let mut current = PLAYBACK
            .lock()
            .map_err(|_| "音频播放状态不可用".to_owned())?;
        check_current(generation, cancelled)?;
        if current.take().is_some() {
            let _ = mci(&format!("close {MCI_ALIAS}"));
        }
        mci(&format!("open \"{path}\" type mpegvideo alias {MCI_ALIAS}"))?;
        *current = Some(generation);
        guard = PlaybackGuard(generation);
        // The open operation can take time; check again immediately before play.
        if let Err(error) = check_current(generation, cancelled) {
            let _ = mci(&format!("close {MCI_ALIAS}"));
            *current = None;
            return Err(error);
        }
        if let Err(error) = mci(&format!("play {MCI_ALIAS} from 0")) {
            let _ = mci(&format!("close {MCI_ALIAS}"));
            *current = None;
            return Err(error);
        }
    }
    loop {
        check_current(generation, cancelled)?;
        {
            let current = PLAYBACK
                .lock()
                .map_err(|_| "音频播放状态不可用".to_owned())?;
            if *current != Some(generation) {
                return Err("已取消".into());
            }
            let mode = mci(&format!("status {MCI_ALIAS} mode"))?;
            if mode.trim() == "stopped" {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    drop(guard);
    Ok(())
}

fn cache_name(text: &str, accent: Accent) -> String {
    let mut hash = Sha256::new();
    hash.update(accent.locale().as_bytes());
    hash.update([0]);
    hash.update(text.as_bytes());
    format!("{}.mp3", hex::encode(hash.finalize()))
}
fn cache_filename(name: &str) -> bool {
    name.strip_suffix(".mp3")
        .is_some_and(|stem| stem.len() == 64 && stem.bytes().all(|c| c.is_ascii_hexdigit()))
}

// Require real, consecutive MPEG Layer III frames, not just an ID3 marker or an
// audio Content-Type. HTML error pages and truncated downloads are rejected.
fn mp3_frame_size(bytes: &[u8]) -> Option<usize> {
    let header = u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?);
    let version = (header >> 19) & 3;
    if header & 0xffe0_0000 != 0xffe0_0000 || version == 1 || (header >> 17) & 3 != 1 {
        return None;
    }
    let bitrate = ((header >> 12) & 15) as usize;
    let frequency = ((header >> 10) & 3) as usize;
    if bitrate == 0 || bitrate == 15 || frequency == 3 {
        return None;
    }
    let rates = if version == 3 {
        [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ]
    } else {
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160]
    };
    let sample_rate = [44_100, 48_000, 32_000][frequency]
        / match version {
            3 => 1,
            2 => 2,
            _ => 4,
        };
    Some(
        (if version == 3 { 144 } else { 72 }) * rates[bitrate] * 1_000 / sample_rate
            + ((header >> 9) & 1) as usize,
    )
}
fn valid_mp3(bytes: &[u8]) -> bool {
    if bytes.len() < 128 || bytes.len() > MAX_AUDIO_BYTES {
        return false;
    }
    let start = if bytes.starts_with(b"ID3") {
        if bytes.len() < 10 || bytes[6..10].iter().any(|byte| byte & 0x80 != 0) {
            return false;
        }
        let size = bytes[6..10]
            .iter()
            .fold(0usize, |size, byte| (size << 7) | *byte as usize);
        10 + size + if bytes[5] & 0x10 != 0 { 10 } else { 0 }
    } else {
        0
    };
    let Some(first) = bytes.get(start..).and_then(mp3_frame_size) else {
        return false;
    };
    let Some(second) = bytes.get(start + first..).and_then(mp3_frame_size) else {
        return false;
    };
    start + first + second <= bytes.len()
}

fn download_audio(
    text: &str,
    accent: Accent,
    generation: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<u8>, String> {
    check_current(generation, cancelled)?;
    let client = CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_millis(900))
            .timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent("TranslationTool/0.1")
            .build()
            .expect("pronunciation HTTP client initialization failed")
    });
    let mut response = client
        .get("https://dict.youdao.com/dictvoice")
        .query(&[("audio", text), ("type", accent.dictionary_type())])
        .send()
        .map_err(|_| "在线发音连接失败或超时".to_owned())?;
    if !response.status().is_success() {
        return Err(format!(
            "在线发音请求失败（HTTP {}）",
            response.status().as_u16()
        ));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !(content_type.starts_with("audio/") || content_type.starts_with("application/octet-stream"))
    {
        return Err("在线发音返回的不是音频".into());
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_AUDIO_BYTES as u64)
    {
        return Err("在线发音文件过大".into());
    }
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 16_384];
    loop {
        check_current(generation, cancelled)?;
        let count = response
            .read(&mut buffer)
            .map_err(|_| "在线发音下载失败或超时".to_owned())?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > MAX_AUDIO_BYTES {
            return Err("在线发音文件过大".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    check_current(generation, cancelled)?;
    if !valid_mp3(&bytes) {
        return Err("在线发音返回了无效或不完整的 MP3".into());
    }
    Ok(bytes)
}

fn prune_cache(directory: &Path, keep: &Path, incoming: u64) -> Result<(), String> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory)
        .map_err(|_| "无法读取发音缓存".to_owned())?
        .flatten()
    {
        if !cache_filename(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() || entry.path() == keep {
            continue;
        }
        entries.push((
            metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            metadata.len(),
            entry.path(),
        ));
    }
    entries.sort_by_key(|entry| entry.0);
    let mut count = entries.len() + 1;
    let mut bytes = entries.iter().map(|entry| entry.1).sum::<u64>() + incoming;
    for (_, size, path) in entries {
        if count <= MAX_CACHE_FILES && bytes <= MAX_CACHE_BYTES {
            break;
        }
        if fs::remove_file(path).is_ok() {
            count -= 1;
            bytes = bytes.saturating_sub(size);
        }
    }
    if count > MAX_CACHE_FILES || bytes > MAX_CACHE_BYTES {
        Err("发音缓存已满，无法清理旧音频".into())
    } else {
        Ok(())
    }
}
fn audio_file(
    text: &str,
    accent: Accent,
    generation: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<PathBuf, String> {
    let directory = storage::data_dir().join("pronunciation");
    let path = directory.join(cache_name(text, accent));
    {
        let _guard = CACHE_LOCK.lock().map_err(|_| "发音缓存不可用".to_owned())?;
        if let Ok(metadata) = fs::symlink_metadata(&path) {
            if metadata.is_file() && (128..=MAX_AUDIO_BYTES as u64).contains(&metadata.len()) {
                if let Ok(bytes) = fs::read(&path) {
                    if valid_mp3(&bytes) {
                        return Ok(path);
                    }
                }
            }
            let _ = fs::remove_file(&path);
        }
    }
    let bytes = download_audio(text, accent, generation, cancelled)?;
    check_current(generation, cancelled)?;
    let _guard = CACHE_LOCK.lock().map_err(|_| "发音缓存不可用".to_owned())?;
    fs::create_dir_all(&directory).map_err(|_| "无法创建发音缓存目录".to_owned())?;
    prune_cache(&directory, &path, bytes.len() as u64)?;
    let temp = directory.join(format!(
        "{}-{}-{generation}.tmp",
        cache_name(text, accent),
        std::process::id()
    ));
    let result = (|| {
        let mut file = File::create(&temp).map_err(|_| "无法写入发音缓存".to_owned())?;
        file.write_all(&bytes)
            .map_err(|_| "无法写入发音缓存".to_owned())?;
        drop(file);
        check_current(generation, cancelled)?;
        fs::rename(&temp, &path).map_err(|_| "无法保存发音缓存".to_owned())?;
        Ok(path)
    })();
    let _ = fs::remove_file(temp);
    result
}

// These small SAPI COM layouts follow the Windows SDK's sapi.h. Only the
// methods used below are typed; inherited/unused entries retain their slots.
#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}
impl Guid {
    const fn from_u128(value: u128) -> Self {
        Self {
            data1: (value >> 96) as u32,
            data2: (value >> 80) as u16,
            data3: (value >> 64) as u16,
            data4: (value as u64).to_be_bytes(),
        }
    }
}
const CLSID_VOICE: Guid = Guid::from_u128(0x96749377_3391_11d2_9ee3_00c04f797396);
const IID_VOICE: Guid = Guid::from_u128(0x6c44df74_72b9_4992_a1ec_ef996e0422d4);
const CLSID_TOKEN_CATEGORY: Guid = Guid::from_u128(0xa910187f_0c7a_45ac_92cc_59edafb77b53);
const IID_TOKEN_CATEGORY: Guid = Guid::from_u128(0x2d3d3845_39af_4850_bbf9_40b49780011d);

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
    fn CoUninitialize();
    fn CoCreateInstance(
        class: *const Guid,
        outer: *mut c_void,
        context: u32,
        interface: *const Guid,
        object: *mut *mut c_void,
    ) -> i32;
}
#[repr(C)]
struct UnknownVTable {
    query_interface: usize,
    add_ref: usize,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}
#[repr(C)]
struct CategoryVTable {
    inherited: [usize; 15],
    set_id: unsafe extern "system" fn(*mut c_void, *const u16, i32) -> i32,
    get_id: usize,
    get_data_key: usize,
    enum_tokens:
        unsafe extern "system" fn(*mut c_void, *const u16, *const u16, *mut *mut c_void) -> i32,
}
#[repr(C)]
struct TokensVTable {
    inherited: [usize; 7],
    item: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32,
    get_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> i32,
}
#[repr(C)]
struct VoiceVTable {
    inherited: [usize; 18],
    set_voice: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
    get_voice: usize,
    speak: unsafe extern "system" fn(*mut c_void, *const u16, u32, *mut u32) -> i32,
    unused: [usize; 11],
    wait_until_done: unsafe extern "system" fn(*mut c_void, u32) -> i32,
}
struct ComPtr<V> {
    raw: *mut c_void,
    marker: std::marker::PhantomData<V>,
}
impl<V> ComPtr<V> {
    fn from_raw(raw: *mut c_void) -> Result<Self, String> {
        if raw.is_null() {
            return Err("Windows 语音接口不可用".into());
        }
        Ok(Self {
            raw,
            marker: std::marker::PhantomData,
        })
    }
    fn vtable(&self) -> &V {
        // A non-null COM interface starts with a pointer to its SDK-defined vtable.
        unsafe { &**(self.raw as *const *const V) }
    }
}
impl<V> Drop for ComPtr<V> {
    fn drop(&mut self) {
        let vtable = unsafe { &**(self.raw as *const *const UnknownVTable) };
        unsafe {
            (vtable.release)(self.raw);
        }
    }
}
struct ComApartment(bool);
impl ComApartment {
    fn new() -> Result<Self, String> {
        // Use a background apartment without changing an already initialized one.
        let result = unsafe { CoInitializeEx(std::ptr::null_mut(), 0) };
        if result == 0x8001_0106u32 as i32 {
            return Ok(Self(false));
        }
        com_result(result, "无法初始化 Windows 语音")?;
        Ok(Self(true))
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe {
                CoUninitialize();
            }
        }
    }
}
fn com_result(result: i32, message: &str) -> Result<(), String> {
    if result < 0 {
        Err(format!("{message}（0x{:08X}）", result as u32))
    } else {
        Ok(())
    }
}
fn create_com<V>(class: &Guid, interface: &Guid) -> Result<ComPtr<V>, String> {
    let mut raw = std::ptr::null_mut();
    com_result(
        unsafe { CoCreateInstance(class, std::ptr::null_mut(), 1, interface, &mut raw) },
        "Windows 语音组件不可用",
    )?;
    ComPtr::from_raw(raw)
}
fn local_voice(accent: Accent) -> Result<ComPtr<UnknownVTable>, String> {
    let category: ComPtr<CategoryVTable> = create_com(&CLSID_TOKEN_CATEGORY, &IID_TOKEN_CATEGORY)?;
    let category_name = wide(r"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Speech\Voices");
    com_result(
        unsafe { (category.vtable().set_id)(category.raw, category_name.as_ptr(), 0) },
        "无法读取 Windows 已安装语音",
    )?;
    let filter = wide(accent.language_filter());
    let mut tokens = std::ptr::null_mut();
    com_result(
        unsafe {
            (category.vtable().enum_tokens)(
                category.raw,
                filter.as_ptr(),
                std::ptr::null(),
                &mut tokens,
            )
        },
        "无法列出 Windows 已安装语音",
    )?;
    let tokens = ComPtr::<TokensVTable>::from_raw(tokens)?;
    let mut count = 0;
    com_result(
        unsafe { (tokens.vtable().get_count)(tokens.raw, &mut count) },
        "无法列出 Windows 已安装语音",
    )?;
    if count == 0 {
        return Err(format!("本机未安装对应的 {} 语音", accent.locale()));
    }
    let mut token = std::ptr::null_mut();
    com_result(
        unsafe { (tokens.vtable().item)(tokens.raw, 0, &mut token) },
        "无法选择 Windows 语音",
    )?;
    ComPtr::from_raw(token)
}
struct SapiQueue<'a>(&'a ComPtr<VoiceVTable>);
impl Drop for SapiQueue<'_> {
    fn drop(&mut self) {
        // Purge this voice on its owning thread; never send a COM interface to UI.
        unsafe {
            (self.0.vtable().speak)(self.0.raw, std::ptr::null(), 3, std::ptr::null_mut());
        }
    }
}
fn pronounce_local(
    text: &str,
    accent: Accent,
    generation: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), String> {
    check_current(generation, cancelled)?;
    let _apartment = ComApartment::new()?;
    let token = local_voice(accent)?;
    let voice: ComPtr<VoiceVTable> = create_com(&CLSID_VOICE, &IID_VOICE)?;
    com_result(
        unsafe { (voice.vtable().set_voice)(voice.raw, token.raw) },
        "无法启用所选口音语音",
    )?;
    let text = wide(text);
    let guard;
    let queue;
    {
        let mut current = PLAYBACK
            .lock()
            .map_err(|_| "音频播放状态不可用".to_owned())?;
        check_current(generation, cancelled)?;
        *current = Some(generation);
        guard = PlaybackGuard(generation);
        queue = SapiQueue(&voice);
        // SPF_ASYNC | SPF_PURGEBEFORESPEAK | SPF_IS_NOT_XML: text is plain text.
        com_result(
            unsafe {
                (voice.vtable().speak)(voice.raw, text.as_ptr(), 1 | 2 | 16, std::ptr::null_mut())
            },
            "Windows 本地朗读失败",
        )?;
    }
    storage::diagnostic(
        serde_json::json!({"event":"pronunciation_source", "source":"windows_sapi", "accent":accent.locale()}),
    );
    loop {
        check_current(generation, cancelled)?;
        let result = unsafe { (voice.vtable().wait_until_done)(voice.raw, 25) };
        com_result(result, "Windows 本地朗读失败")?;
        if result == 0 {
            break;
        }
    }
    drop(queue);
    drop(guard);
    check_current(generation, cancelled)
}

/// Call on a background task. Returns when playback finishes or is cancelled.
/// Dictionary audio is requested by accent and never silently truncated.
pub fn pronounce(text: &str, accent: Accent, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        return Err("已取消".into());
    }
    let text = text.trim();
    if text.is_empty() {
        return Err("没有可朗读的英文文本".into());
    }
    if text.contains('\0') {
        return Err("发音文本包含不支持的字符".into());
    }
    let generation = {
        let mut current = PLAYBACK
            .lock()
            .map_err(|_| "音频播放状态不可用".to_owned())?;
        // Assign the module generation only after taking the lock and checking
        // UI cancellation, so a delayed old worker cannot invalidate a new one.
        if cancelled() {
            return Err("已取消".into());
        }
        let generation = GENERATION.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        if current.take().is_some() {
            let _ = mci(&format!("close {MCI_ALIAS}"));
        }
        generation
    };
    if text.chars().count() > MAX_DICTIONARY_CHARACTERS {
        // Long text is read completely with a matching installed local voice.
        return pronounce_local(text, accent, generation, cancelled);
    }
    let online = audio_file(text, accent, generation, cancelled).and_then(|path| {
        check_current(generation, cancelled)?;
        storage::diagnostic(serde_json::json!({"event":"pronunciation_source", "source":"dictionary_audio", "accent":accent.locale()}));
        play_mp3(&path, generation, cancelled)
    });
    match online {
        Ok(()) => Ok(()),
        Err(error) => {
            check_current(generation, cancelled)?;
            pronounce_local(text, accent, generation, cancelled).map_err(|local| {
                if local == "已取消" {
                    local
                } else {
                    format!("{error}；{local}")
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn two_frames() -> Vec<u8> {
        let mut bytes = vec![0; 417 * 2];
        bytes[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0x64]);
        bytes[417..421].copy_from_slice(&[0xff, 0xfb, 0x90, 0x64]);
        bytes
    }
    #[test]
    fn validates_mpeg_frames_and_rejects_html_or_truncation() {
        let frames = two_frames();
        assert!(valid_mp3(&frames));
        assert!(!valid_mp3(&frames[..500]));
        assert!(!valid_mp3(b"<html>error</html>"));
        let mut id3 = b"ID3\x04\0\0\0\0\0\0".to_vec();
        id3.extend_from_slice(&frames);
        assert!(valid_mp3(&id3));
        assert!(!valid_mp3(b"ID3\x04\0\0\x7f\x7f\x7f\x7f"));
    }
    #[test]
    fn cache_keys_distinguish_accents_and_keep_user_text_out_of_paths() {
        let british = cache_name("snake", Accent::British);
        let american = cache_name("snake", Accent::American);
        assert_ne!(british, american);
        assert!(cache_filename(&british));
        assert!(!british.contains("snake"));
        assert!(!cache_filename("../snake.mp3"));
    }
    #[test]
    fn already_cancelled_audio_makes_no_request_and_does_not_play() {
        assert_eq!(
            pronounce("snake", Accent::British, &|| true).err().unwrap(),
            "已取消"
        );
    }
    #[test]
    fn native_sapi_slots_and_accent_filters_match_the_windows_sdk() {
        let pointer = std::mem::size_of::<usize>();
        assert_eq!(std::mem::offset_of!(CategoryVTable, set_id) / pointer, 15);
        assert_eq!(
            std::mem::offset_of!(CategoryVTable, enum_tokens) / pointer,
            18
        );
        assert_eq!(std::mem::offset_of!(TokensVTable, item) / pointer, 7);
        assert_eq!(std::mem::offset_of!(VoiceVTable, set_voice) / pointer, 18);
        assert_eq!(std::mem::offset_of!(VoiceVTable, speak) / pointer, 20);
        assert_eq!(
            std::mem::offset_of!(VoiceVTable, wait_until_done) / pointer,
            32
        );
        assert_eq!(Accent::British.language_filter(), "Language=809");
        assert_eq!(Accent::American.language_filter(), "Language=409");
    }
    #[test]
    fn phonetics_preserve_real_accent_fields_and_leave_missing_accents_empty() {
        for (uk, us) in [
            ("təˈmɑːtəʊ", "təˈmeɪtoʊ"),
            ("ˈʃedjuːl", "ˈskedʒuːl"),
            ("sneɪk", "sneɪk"),
        ] {
            let result =
                parse_phonetics(&serde_json::json!({"ec":{"word":[{"ukphone":uk,"usphone":us}]}}));
            assert_eq!(result.british, uk);
            assert_eq!(result.american, us);
        }
        let uk_only =
            parse_phonetics(&serde_json::json!({"ec":{"word":[{"ukphone":"təˈmɑːtəʊ"}]}}));
        assert_eq!(uk_only.british, "təˈmɑːtəʊ");
        assert!(uk_only.american.is_empty());
        let simple = parse_phonetics(
            &serde_json::json!({"ec":{"word":[{"ukphone":"təˈmɑːtəʊ"}]}, "simple":{"word":[{"ukphone":"different","usphone":"təˈmeɪtoʊ"}]}}),
        );
        assert_eq!(simple.british, "təˈmɑːtəʊ");
        assert_eq!(simple.american, "təˈmeɪtoʊ");
        assert_eq!(
            parse_phonetics(&serde_json::json!({})),
            Phonetics::default()
        );
    }
    #[test]
    fn phonetics_only_query_single_english_words_and_respect_pre_cancellation() {
        for word in ["snake", "can't", "mother-in-law", "can’t"] {
            assert_eq!(dictionary_word(word), Some(word));
        }
        for sentence in [
            "snake in the grass",
            "中文",
            "https://example.com",
            "",
            "snake\nword",
        ] {
            assert!(dictionary_word(sentence).is_none());
            assert_eq!(
                phonetics(sentence, &|| false).unwrap(),
                Phonetics::default()
            );
        }
        assert_eq!(phonetics("snake", &|| true).err().unwrap(), "已取消");
    }
}

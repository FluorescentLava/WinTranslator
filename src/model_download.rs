//! The installer contains this small, pinned catalog; model weights are fetched
//! from their original Hugging Face repositories only when the user asks.

use anyhow::{Result, anyhow, bail, ensure};
use reqwest::{
    StatusCode,
    blocking::{Client, Response},
    header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_RANGE, RANGE},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const ORIGIN: &str = "https://huggingface.co";
const TOTAL_BYTES: u64 = 230_593_393;
const BUFFER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Downloading,
    Ready,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub phase: Phase,
    pub downloaded: u64,
    pub total: u64,
    pub error: Option<String>,
}

struct WorkerState {
    status: Status,
    running: bool,
    generation: u64,
    cancelled: Arc<AtomicBool>,
}

fn worker_state() -> &'static Mutex<WorkerState> {
    static STATE: OnceLock<Mutex<WorkerState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(WorkerState {
            status: Status {
                phase: Phase::Idle,
                downloaded: 0,
                total: TOTAL_BYTES,
                error: None,
            },
            running: false,
            generation: 0,
            cancelled: Arc::new(AtomicBool::new(false)),
        })
    })
}

/// This is a memory-only snapshot. No filesystem, hashing, network work, or
/// worker joins happen while a caller (including the UI) holds this lock.
pub fn status() -> Status {
    worker_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .status
        .clone()
}

pub fn total_bytes() -> u64 {
    TOTAL_BYTES
}

/// Start at most one worker in this process. Closing a settings window does not
/// own or cancel this worker. A cancelled worker acknowledges cancellation
/// before another start is accepted, so two workers cannot write the same part.
pub fn start() {
    let (generation, cancelled) = {
        let mut state = worker_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.running {
            return;
        }
        state.generation = state.generation.wrapping_add(1);
        state.cancelled = Arc::new(AtomicBool::new(false));
        state.running = true;
        state.status = Status {
            phase: Phase::Downloading,
            downloaded: 0,
            total: TOTAL_BYTES,
            error: None,
        };
        (state.generation, Arc::clone(&state.cancelled))
    };
    let worker_cancelled = Arc::clone(&cancelled);
    let spawned = std::thread::Builder::new()
        .name("offline-model-download".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                download(
                    &crate::offline::download_dir(),
                    &|| worker_cancelled.load(Ordering::Acquire),
                    &|downloaded, total| {
                        let mut state = worker_state()
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if state.running && state.generation == generation {
                            state.status.downloaded = downloaded;
                            state.status.total = total;
                        }
                    },
                )
            }))
            .unwrap_or_else(|_| Err(anyhow!("模型下载任务意外结束，请重试")));
            finish_worker(generation, &worker_cancelled, result);
        });
    if spawned.is_err() {
        finish_worker(
            generation,
            &cancelled,
            Err(anyhow!("无法启动模型下载，请重试")),
        );
    }
}

fn finish_worker(generation: u64, cancelled: &AtomicBool, result: Result<()>) {
    let mut state = worker_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !state.running || state.generation != generation {
        return;
    }
    state.running = false;
    // Cancellation wins a simultaneous finish. A later start gets its own
    // token, and an earlier completion cannot overwrite that newer run.
    if cancelled.load(Ordering::Acquire)
        || result.as_ref().is_err_and(|error| error.is::<Cancelled>())
    {
        state.status.phase = Phase::Cancelled;
        state.status.error = None;
    } else {
        match result {
            Ok(()) => {
                state.status.phase = Phase::Ready;
                state.status.downloaded = state.status.total;
                state.status.error = None;
            }
            Err(error) => {
                state.status.phase = Phase::Failed;
                state.status.error = Some(error.to_string());
            }
        }
    }
}

/// Acknowledgement can take up to the bounded connect/read timeout while a
/// network operation is pending. Keep the run active until it has stopped.
pub fn cancel() {
    let state = worker_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.running {
        state.cancelled.store(true, Ordering::Release);
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ModelFile {
    file: String,
    sha256: String,
    bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Manifest {
    repository: String,
    revision: String,
    files: Vec<ModelFile>,
}

struct ModelSpec {
    pair: &'static str,
    manifest: Manifest,
}

fn catalog() -> &'static [ModelSpec] {
    static CATALOG: OnceLock<Vec<ModelSpec>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        [
            ("en-zh", include_str!("../assets/models/en-zh.json")),
            ("zh-en", include_str!("../assets/models/zh-en.json")),
        ]
        .into_iter()
        .map(|(pair, source)| ModelSpec {
            pair,
            manifest: serde_json::from_str(source).expect("invalid embedded model catalog"),
        })
        .collect()
    })
}

/// Fast availability check for both complete directions. Full SHA-256 checks
/// run in the download worker, never in this UI-facing filesystem check.
pub fn ready_at(root: &Path) -> bool {
    ready_at_catalog(root, catalog())
}

fn ready_at_catalog(root: &Path, models: &[ModelSpec]) -> bool {
    models.iter().all(|model| {
        model.manifest.files.iter().all(|file| {
            fs::metadata(root.join(model.pair).join(&file.file))
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() == file.bytes)
        })
    })
}

/// Download into an explicit directory. This synchronous API is also usable by
/// isolated verification tools; application UI code should call `start`.
pub fn download(
    root: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(u64, u64),
) -> Result<()> {
    check_cancelled(cancelled)?;
    let client = Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        // reqwest's blocking timeout bounds each connect/read/write operation,
        // rather than putting a short deadline on an entire large model file.
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("WinTranslater/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| anyhow!("无法初始化模型下载，请重试"))?;
    download_catalog(root, catalog(), &client, ORIGIN, cancelled, progress)
}

fn download_catalog(
    root: &Path,
    models: &[ModelSpec],
    client: &Client,
    origin: &str,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(u64, u64),
) -> Result<()> {
    check_cancelled(cancelled)?;
    let total = models
        .iter()
        .flat_map(|model| &model.manifest.files)
        .map(|file| file.bytes)
        .sum();
    let mut completed = 0;
    progress(0, total);
    for model in models {
        check_cancelled(cancelled)?;
        let directory = root.join(model.pair);
        fs::create_dir_all(&directory)
            .map_err(|_| anyhow!("无法创建离线模型目录，请检查权限和磁盘空间"))?;
        for file in &model.manifest.files {
            check_cancelled(cancelled)?;
            let destination = directory.join(&file.file);
            // Reuse only a fully verified final file. A wrong-size or corrupt
            // existing file stays in place until its replacement is verified.
            let existing_valid = fs::metadata(&destination)
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() == file.bytes)
                && hash_matches(&destination, file, cancelled)?;
            if !existing_valid {
                let remote_path = if file.file.ends_with(".onnx") {
                    format!("onnx/{}", file.file)
                } else {
                    file.file.clone()
                };
                let url = format!(
                    "{}/{}/resolve/{}/{}",
                    origin.trim_end_matches('/'),
                    model.manifest.repository,
                    model.manifest.revision,
                    remote_path
                );
                download_file(client, &url, &destination, file, cancelled, &|received| {
                    progress(completed + received, total)
                })?;
            }
            check_cancelled(cancelled)?;
            completed += file.bytes;
            progress(completed, total);
        }
        check_cancelled(cancelled)?;
        write_manifest(&directory, &model.manifest, cancelled)?;
    }
    check_cancelled(cancelled)?;
    progress(total, total);
    Ok(())
}

#[derive(Debug)]
struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("已取消模型下载")
    }
}

impl std::error::Error for Cancelled {}

fn check_cancelled(cancelled: &dyn Fn() -> bool) -> Result<()> {
    if cancelled() {
        return Err(Cancelled.into());
    }
    Ok(())
}

fn hash_file(path: &Path, cancelled: &dyn Fn() -> bool) -> Result<Sha256> {
    let mut file = File::open(path).map_err(|_| anyhow!("无法读取离线模型文件，请检查文件权限"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; BUFFER_BYTES];
    loop {
        check_cancelled(cancelled)?;
        let count = file
            .read(&mut buffer)
            .map_err(|_| anyhow!("无法读取离线模型文件，请检查文件权限"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    check_cancelled(cancelled)?;
    Ok(hasher)
}

fn hash_matches(path: &Path, file: &ModelFile, cancelled: &dyn Fn() -> bool) -> Result<bool> {
    Ok(hex::encode(hash_file(path, cancelled)?.finalize()) == file.sha256)
}

fn part_path(destination: &Path) -> std::path::PathBuf {
    let mut name = destination.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    destination.with_file_name(name)
}

fn download_file(
    client: &Client,
    url: &str,
    destination: &Path,
    file: &ModelFile,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(u64),
) -> Result<()> {
    check_cancelled(cancelled)?;
    let partial = part_path(destination);
    let mut received = match fs::metadata(&partial) {
        Ok(metadata) if metadata.is_file() => metadata.len(),
        Ok(_) => bail!("离线模型临时文件不可写，请检查文件权限"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(_) => bail!("无法读取离线模型临时文件，请检查文件权限"),
    };
    if received == file.bytes && hash_matches(&partial, file, cancelled)? {
        check_cancelled(cancelled)?;
        promote(&partial, destination)?;
        progress(received);
        return Ok(());
    }
    if received >= file.bytes {
        // A full but bad part must be fetched afresh. Do not keep retrying a
        // 416 response or resume onto bytes that already failed verification.
        File::create(&partial)
            .map_err(|_| anyhow!("无法重建模型临时文件，请检查权限和磁盘空间"))?;
        received = 0;
    }
    progress(received);
    check_cancelled(cancelled)?;
    let mut request = client.get(url).header(ACCEPT_ENCODING, "identity");
    if received > 0 {
        request = request.header(RANGE, format!("bytes={received}-"));
    }
    let mut response = request.send().map_err(|error| {
        #[cfg(test)]
        if url.starts_with("http://127.0.0.1:") {
            eprintln!(
                "local model fixture request failed for {}: {error:?}",
                file.file
            );
        }
        if cancelled() {
            anyhow!(Cancelled)
        } else if error.is_timeout() {
            anyhow!("模型下载超时，请重试")
        } else if error.is_connect() {
            anyhow!("无法连接模型原仓库，请检查网络后重试")
        } else {
            // Never include reqwest's error text: redirected CDN URLs contain
            // signed query strings, which do not belong in logs or the UI.
            anyhow!("模型下载连接中断，请重试")
        }
    })?;
    check_cancelled(cancelled)?;
    let append = validate_response(&response, received, file.bytes)?;
    let mut hasher = if append {
        hash_file(&partial, cancelled)?
    } else {
        // A server is allowed to ignore Range and return the whole resource.
        // Reset both the file and its hasher before consuming a 200 response.
        received = 0;
        progress(0);
        Sha256::new()
    };
    check_cancelled(cancelled)?;
    let mut output = OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(&partial)
        .map_err(|_| anyhow!("无法写入离线模型，请检查权限和磁盘空间"))?;
    ensure!(
        output
            .metadata()
            .is_ok_and(|metadata| metadata.len() == received),
        "模型临时文件已变化，请重试"
    );
    let mut buffer = [0_u8; BUFFER_BYTES];
    loop {
        check_cancelled(cancelled)?;
        let count = match response.read(&mut buffer) {
            Ok(count) => count,
            Err(error) => {
                check_cancelled(cancelled)?;
                if error.kind() == std::io::ErrorKind::TimedOut {
                    bail!("模型下载超时，请重试");
                }
                bail!("模型下载连接中断，请重试以继续下载");
            }
        };
        check_cancelled(cancelled)?;
        if count == 0 {
            break;
        }
        if received + count as u64 > file.bytes {
            drop(output);
            let _ = fs::remove_file(&partial);
            bail!("模型文件大小不符，请重试");
        }
        output
            .write_all(&buffer[..count])
            .map_err(|_| anyhow!("无法写入离线模型，请检查权限和磁盘空间"))?;
        hasher.update(&buffer[..count]);
        received += count as u64;
        progress(received);
    }
    ensure!(received == file.bytes, "模型下载不完整，请重试以继续下载");
    if hex::encode(hasher.finalize()) != file.sha256 {
        drop(output);
        let _ = fs::remove_file(&partial);
        bail!("模型文件校验失败，请重试");
    }
    output
        .sync_all()
        .map_err(|_| anyhow!("无法保存离线模型，请检查权限和磁盘空间"))?;
    drop(output);
    check_cancelled(cancelled)?;
    promote(&partial, destination)
}

/// Return whether the response can safely append to an existing partial file.
fn validate_response(response: &Response, offset: u64, expected: u64) -> Result<bool> {
    let status = response.status();
    if status != StatusCode::OK && status != StatusCode::PARTIAL_CONTENT {
        let code = status.as_u16();
        match code {
            403 => bail!("模型原仓库拒绝访问（HTTP 403），请稍后重试或检查网络"),
            404 => bail!("模型原仓库暂时找不到文件（HTTP 404），请稍后重试"),
            429 => bail!("模型原仓库请求过于频繁，请稍后重试"),
            _ => bail!("模型原仓库返回 HTTP {code}，请稍后重试"),
        }
    }
    ensure!(
        response
            .headers()
            .get(CONTENT_ENCODING)
            .is_none_or(|encoding| encoding == "identity"),
        "模型服务器返回了不支持的编码，请重试"
    );
    if status == StatusCode::PARTIAL_CONTENT {
        let range = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|range| range.to_str().ok())
            .and_then(parse_content_range);
        ensure!(
            range == Some((offset, expected.saturating_sub(1), expected)),
            "模型服务器返回了无效的续传响应，请重试"
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|length| length == expected - offset),
            "模型续传文件大小不符，请重试"
        );
        Ok(offset > 0)
    } else {
        ensure!(
            response
                .content_length()
                .is_none_or(|length| length == expected),
            "模型文件大小不符，请重试"
        );
        Ok(false)
    }
}

fn parse_content_range(value: &str) -> Option<(u64, u64, u64)> {
    let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let start = start.parse().ok()?;
    let end = end.parse().ok()?;
    let total = total.parse().ok()?;
    (start <= end && end < total).then_some((start, end, total))
}

fn promote(partial: &Path, destination: &Path) -> Result<()> {
    // Both paths have the same parent directory. rename replaces a previous
    // file atomically, so readers never observe a half-written final filename.
    fs::rename(partial, destination)
        .map_err(|_| anyhow!("无法安装已校验的模型，请检查文件占用和目录权限"))
}

fn write_manifest(
    directory: &Path,
    manifest: &Manifest,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    let contents =
        serde_json::to_vec_pretty(manifest).map_err(|_| anyhow!("无法生成离线模型清单，请重试"))?;
    let destination = directory.join("manifest.json");
    let partial = part_path(&destination);
    check_cancelled(cancelled)?;
    let mut output =
        File::create(&partial).map_err(|_| anyhow!("无法写入模型清单，请检查目录权限"))?;
    output
        .write_all(&contents)
        .and_then(|_| output.sync_all())
        .map_err(|_| anyhow!("无法保存模型清单，请检查权限和磁盘空间"))?;
    drop(output);
    check_cancelled(cancelled)?;
    promote(&partial, &destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::RefCell,
        net::{TcpListener, TcpStream},
        path::PathBuf,
        sync::atomic::AtomicU64,
        thread::{self, JoinHandle},
        time::{Instant, SystemTime, UNIX_EPOCH},
    };

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "wintranslater-model-download-test-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            // Only remove our own unique, immediate child of the OS temp dir.
            if let (Ok(path), Ok(parent)) =
                (self.0.canonicalize(), std::env::temp_dir().canonicalize())
                && path.parent() == Some(parent.as_path())
                && path.file_name().is_some_and(|name| {
                    name.to_string_lossy()
                        .starts_with("wintranslater-model-download-test-")
                })
            {
                let _ = fs::remove_dir_all(path);
            }
        }
    }

    #[derive(Clone, Debug)]
    struct Request {
        path: String,
        range: Option<String>,
    }

    enum Reply {
        Full(Vec<u8>),
        Resume(Vec<u8>),
        Disconnect { body: Vec<u8>, after: usize },
        InvalidRange(Vec<u8>),
        Error(u16),
    }

    struct FixtureServer {
        origin: String,
        requests: Arc<Mutex<Vec<Request>>>,
        events: Arc<Mutex<Vec<String>>>,
        stopped: Arc<AtomicBool>,
        worker: Option<JoinHandle<()>>,
    }

    impl FixtureServer {
        fn new(replies: Vec<Reply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = Arc::clone(&requests);
            let events = Arc::new(Mutex::new(Vec::new()));
            let diagnostics = Arc::clone(&events);
            let stopped = Arc::new(AtomicBool::new(false));
            let stop = Arc::clone(&stopped);
            let worker = thread::spawn(move || {
                let started = Instant::now();
                for (index, reply) in replies.into_iter().enumerate() {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    let mut connection = loop {
                        if stop.load(Ordering::Acquire) {
                            return;
                        }
                        if Instant::now() > deadline {
                            diagnostics.lock().unwrap().push(format!(
                                "request {index}: accept deadline elapsed after {:?}",
                                started.elapsed()
                            ));
                            return;
                        }
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(5));
                            }
                            Err(error) => {
                                diagnostics
                                    .lock()
                                    .unwrap()
                                    .push(format!("request {index}: accept failed: {error}"));
                                return;
                            }
                        }
                    };
                    // Windows accept() inherits the listener's nonblocking
                    // mode. A TCP connection can be accepted before HTTP
                    // headers arrive, so blocking reads must be restored on
                    // this stream instead of treating WSAEWOULDBLOCK as EOF.
                    connection.set_nonblocking(false).unwrap();
                    connection
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    connection
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    diagnostics.lock().unwrap().push(format!(
                        "request {index}: accepted after {:?}",
                        started.elapsed()
                    ));
                    let request = match read_request(&mut connection) {
                        Ok(request) => request,
                        Err(error) => {
                            diagnostics.lock().unwrap().push(format!(
                                "request {index}: reading headers failed after {:?}: {error}",
                                started.elapsed()
                            ));
                            return;
                        }
                    };
                    seen.lock().unwrap().push(request.clone());
                    if let Err(error) = serve_reply(&mut connection, &request, reply) {
                        diagnostics.lock().unwrap().push(format!(
                            "request {index}: writing response failed after {:?}: {error}",
                            started.elapsed()
                        ));
                    }
                }
            });
            Self {
                origin,
                requests,
                events,
                stopped,
                worker: Some(worker),
            }
        }

        fn requests(&self) -> Vec<Request> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Drop for FixtureServer {
        fn drop(&mut self) {
            self.stopped.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }

    fn read_request(connection: &mut TcpStream) -> std::io::Result<Request> {
        let mut bytes = Vec::new();
        let mut byte = [0];
        while !bytes.ends_with(b"\r\n\r\n") && bytes.len() < 16 * 1024 {
            if connection.read(&mut byte)? == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection closed before request headers",
                ));
            }
            bytes.push(byte[0]);
        }
        let headers = String::from_utf8(bytes).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request headers are not UTF-8",
            )
        })?;
        Ok(Request {
            path: headers
                .lines()
                .next()
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "missing request line")
                })?
                .split_whitespace()
                .nth(1)
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "missing request path")
                })?
                .to_owned(),
            range: headers.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("range")
                    .then(|| value.trim().to_owned())
            }),
        })
    }

    fn serve_reply(
        connection: &mut TcpStream,
        request: &Request,
        reply: Reply,
    ) -> std::io::Result<()> {
        let (status, declared, extra_headers, body) = match reply {
            Reply::Full(body) => (200, body.len(), String::new(), body),
            Reply::Resume(body) => {
                if let Some(offset) = request.range.as_ref().and_then(|range| {
                    range
                        .strip_prefix("bytes=")?
                        .strip_suffix('-')?
                        .parse::<usize>()
                        .ok()
                }) {
                    let offset = offset.min(body.len());
                    let headers = format!(
                        "Content-Range: bytes {}-{}/{}\r\n",
                        offset,
                        body.len() - 1,
                        body.len()
                    );
                    let remaining = body[offset..].to_vec();
                    (206, remaining.len(), headers, remaining)
                } else {
                    (200, body.len(), String::new(), body)
                }
            }
            Reply::Disconnect { body, after } => {
                let declared = body.len();
                (200, declared, String::new(), body[..after].to_vec())
            }
            Reply::InvalidRange(body) => {
                let headers = format!(
                    "Content-Range: bytes 0-{}/{}\r\n",
                    body.len() - 1,
                    body.len()
                );
                (206, body.len(), headers, body)
            }
            Reply::Error(status) => {
                let body = b"https://cdn.example/model?signature=must-stay-private".to_vec();
                (status, body.len(), String::new(), body)
            }
        };
        let headers = format!(
            "HTTP/1.1 {status} fixture\r\nConnection: close\r\nContent-Length: {declared}\r\n{extra_headers}\r\n"
        );
        connection.write_all(headers.as_bytes())?;
        // A client can cancel before the fixture finishes writing its body.
        connection.write_all(&body)
    }

    fn body(bytes: usize) -> Vec<u8> {
        (0..bytes).map(|index| (index % 251) as u8).collect()
    }

    #[test]
    fn fixture_waits_for_request_bytes_after_accepting_a_connection() {
        let server = FixtureServer::new(vec![Reply::Full(b"ok".to_vec())]);
        let address = server.origin.strip_prefix("http://").unwrap();
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while server.events.lock().unwrap().is_empty() {
            assert!(
                Instant::now() < deadline,
                "fixture never accepted the connection"
            );
            thread::sleep(Duration::from_millis(1));
        }
        // TCP establishment can precede the first HTTP bytes. Waiting for the
        // accept above makes this transport boundary reproducible on Windows.
        thread::sleep(Duration::from_millis(20));
        let written = client.write_all(b"GET /delayed HTTP/1.1\r\nHost: localhost\r\n\r\n");
        let mut response = Vec::new();
        let read = client.read_to_end(&mut response);
        assert!(
            written.is_ok() && read.is_ok(),
            "write={written:?}, read={read:?}, fixture={:?}",
            server.events.lock().unwrap()
        );
        assert!(
            response.ends_with(b"\r\n\r\nok"),
            "fixture={:?}, response={response:?}",
            server.events.lock().unwrap()
        );
    }

    fn model(pair: &'static str, files: &[(&str, &[u8])]) -> ModelSpec {
        ModelSpec {
            pair,
            manifest: Manifest {
                repository: format!("fixture/{pair}"),
                revision: "0123456789abcdef0123456789abcdef01234567".into(),
                files: files
                    .iter()
                    .map(|(name, contents)| ModelFile {
                        file: (*name).into(),
                        sha256: hex::encode(Sha256::digest(contents)),
                        bytes: contents.len() as u64,
                    })
                    .collect(),
            },
        }
    }

    fn fixture_download(
        directory: &Path,
        models: &[ModelSpec],
        server: &FixtureServer,
        cancelled: &dyn Fn() -> bool,
        progress: &dyn Fn(u64, u64),
    ) -> Result<()> {
        let client = Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let result = download_catalog(
            directory,
            models,
            &client,
            &server.origin,
            cancelled,
            progress,
        );
        if result.is_err() {
            eprintln!(
                "local fixture requests: {:?}; events: {:?}",
                server.requests(),
                server.events.lock().unwrap()
            );
        }
        result
    }

    #[test]
    fn embedded_catalog_is_pinned_and_matches_the_displayed_total() {
        let models = catalog();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].manifest.repository, "Xenova/opus-mt-en-zh");
        assert_eq!(models[1].manifest.repository, "Xenova/opus-mt-zh-en");
        let mut bytes = 0;
        for model in models {
            assert_eq!(model.manifest.revision.len(), 40);
            assert!(
                model
                    .manifest
                    .revision
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            );
            assert_eq!(model.manifest.files.len(), 5);
            for file in &model.manifest.files {
                assert_eq!(file.sha256.len(), 64);
                assert!(file.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
                assert!(file.bytes > 0);
                bytes += file.bytes;
            }
        }
        assert_eq!(total_bytes(), bytes);
    }

    #[test]
    fn ready_requires_exact_lengths_for_both_directions_and_ignores_parts() {
        let directory = TestDirectory::new();
        assert!(!ready_at(&directory.0));
        for (index, model) in catalog().iter().enumerate() {
            let root = directory.0.join(model.pair);
            fs::create_dir_all(&root).unwrap();
            for file in &model.manifest.files {
                File::create(root.join(&file.file))
                    .unwrap()
                    .set_len(file.bytes)
                    .unwrap();
            }
            assert_eq!(ready_at(&directory.0), index == 1);
        }
        let model = &catalog()[1];
        let file = &model.manifest.files[0];
        let destination = directory.0.join(model.pair).join(&file.file);
        fs::rename(&destination, part_path(&destination)).unwrap();
        assert!(!ready_at(&directory.0));
        File::create(&destination)
            .unwrap()
            .set_len(file.bytes - 1)
            .unwrap();
        assert!(!ready_at(&directory.0));
    }

    #[test]
    fn downloads_both_complete_directions_and_writes_verified_manifests() {
        let directory = TestDirectory::new();
        let names = [
            "config.json",
            "source.spm",
            "vocab.json",
            "encoder_model_quantized.onnx",
            "decoder_model_quantized.onnx",
        ];
        let contents: Vec<_> = (1..=10).map(|index| body(index * 137)).collect();
        let files: Vec<_> = names
            .iter()
            .enumerate()
            .map(|(i, name)| (*name, &contents[i][..]))
            .collect();
        let others: Vec<_> = names
            .iter()
            .enumerate()
            .map(|(i, name)| (*name, &contents[i + 5][..]))
            .collect();
        let models = [model("en-zh", &files), model("zh-en", &others)];
        let server = FixtureServer::new(contents.iter().cloned().map(Reply::Full).collect());
        let updates = RefCell::new(Vec::new());
        fixture_download(&directory.0, &models, &server, &|| false, &|done, total| {
            updates.borrow_mut().push((done, total));
        })
        .unwrap();
        assert!(ready_at_catalog(&directory.0, &models));
        for model in &models {
            for file in &model.manifest.files {
                let path = directory.0.join(model.pair).join(&file.file);
                assert!(hash_matches(&path, file, &|| false).unwrap());
                assert!(!part_path(&path).exists());
            }
            let manifest: Manifest = serde_json::from_slice(
                &fs::read(directory.0.join(model.pair).join("manifest.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(manifest.revision, model.manifest.revision);
            assert_eq!(manifest.files.len(), 5);
        }
        let requests = server.requests();
        assert_eq!(requests.len(), 10);
        assert!(
            requests[3]
                .path
                .ends_with("/onnx/encoder_model_quantized.onnx")
        );
        assert!(
            requests[4]
                .path
                .ends_with("/onnx/decoder_model_quantized.onnx")
        );
        assert!(
            requests
                .iter()
                .all(|request| request.path.contains(&models[0].manifest.revision))
        );
        let updates = updates.borrow();
        let last = updates.last().unwrap();
        assert_eq!(last.0, last.1);
        assert!(updates.iter().all(|(done, total)| done <= total));

        // An already downloaded set remains usable without another request;
        // replacing its existing manifests must also work on Windows.
        let no_network = FixtureServer::new(Vec::new());
        fixture_download(&directory.0, &models, &no_network, &|| false, &|_, _| {}).unwrap();
        assert!(no_network.requests().is_empty());
    }

    #[test]
    fn cancellation_preserves_a_part_and_retry_resumes_before_publishing() {
        let directory = TestDirectory::new();
        let contents = body(BUFFER_BYTES * 4);
        let models = [model("en-zh", &[("config.json", &contents)])];
        let server = FixtureServer::new(vec![Reply::Full(contents.clone())]);
        let cancelled = AtomicBool::new(false);
        let error = fixture_download(
            &directory.0,
            &models,
            &server,
            &|| cancelled.load(Ordering::Acquire),
            &|done, _| {
                if done >= BUFFER_BYTES as u64 {
                    cancelled.store(true, Ordering::Release);
                }
            },
        )
        .unwrap_err();
        assert!(error.is::<Cancelled>());
        let destination = directory.0.join("en-zh/config.json");
        let partial = part_path(&destination);
        let offset = fs::metadata(&partial).unwrap().len();
        assert!(offset >= BUFFER_BYTES as u64 && offset < contents.len() as u64);
        assert!(!destination.exists());
        assert!(!directory.0.join("en-zh/manifest.json").exists());
        assert!(!ready_at_catalog(&directory.0, &models));

        let retry = FixtureServer::new(vec![Reply::Resume(contents.clone())]);
        fixture_download(&directory.0, &models, &retry, &|| false, &|_, _| {}).unwrap();
        assert_eq!(retry.requests()[0].range, Some(format!("bytes={offset}-")));
        assert_eq!(fs::read(destination).unwrap(), contents);
        assert!(ready_at_catalog(&directory.0, &models));
        assert!(!partial.exists());
    }

    #[test]
    fn disconnect_is_retryable_without_discarding_received_bytes() {
        let directory = TestDirectory::new();
        let contents = body(BUFFER_BYTES * 3);
        let models = [model("en-zh", &[("config.json", &contents)])];
        let server = FixtureServer::new(vec![Reply::Disconnect {
            body: contents.clone(),
            after: 50_000,
        }]);
        let error =
            fixture_download(&directory.0, &models, &server, &|| false, &|_, _| {}).unwrap_err();
        assert!(error.to_string().contains("中断") || error.to_string().contains("不完整"));
        let destination = directory.0.join("en-zh/config.json");
        let offset = fs::metadata(part_path(&destination)).unwrap().len();
        assert_eq!(offset, 50_000);
        assert!(!destination.exists());
        assert!(!ready_at_catalog(&directory.0, &models));
        let retry = FixtureServer::new(vec![Reply::Resume(contents.clone())]);
        fixture_download(&directory.0, &models, &retry, &|| false, &|_, _| {}).unwrap();
        assert_eq!(retry.requests()[0].range, Some(format!("bytes={offset}-")));
        assert_eq!(fs::read(destination).unwrap(), contents);
    }

    #[test]
    fn checksum_failure_never_publishes_and_a_fresh_retry_can_succeed() {
        let directory = TestDirectory::new();
        let contents = body(8192);
        let models = [model("en-zh", &[("config.json", &contents)])];
        let mut corrupt = contents.clone();
        corrupt[4096] ^= 0x55;
        let server = FixtureServer::new(vec![Reply::Full(corrupt)]);
        let error =
            fixture_download(&directory.0, &models, &server, &|| false, &|_, _| {}).unwrap_err();
        assert!(error.to_string().contains("校验失败"));
        let destination = directory.0.join("en-zh/config.json");
        assert!(!destination.exists());
        assert!(!part_path(&destination).exists());
        assert!(!ready_at_catalog(&directory.0, &models));
        let retry = FixtureServer::new(vec![Reply::Full(contents.clone())]);
        fixture_download(&directory.0, &models, &retry, &|| false, &|_, _| {}).unwrap();
        assert_eq!(retry.requests()[0].range, None);
        assert_eq!(fs::read(destination).unwrap(), contents);
    }

    #[test]
    fn ignored_range_restarts_the_file_and_hash_instead_of_appending() {
        let directory = TestDirectory::new();
        let contents = body(8192);
        let models = [model("en-zh", &[("config.json", &contents)])];
        let destination = directory.0.join("en-zh/config.json");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(part_path(&destination), b"wrong partial").unwrap();
        let server = FixtureServer::new(vec![Reply::Full(contents.clone())]);
        let updates = RefCell::new(Vec::new());
        fixture_download(&directory.0, &models, &server, &|| false, &|done, _| {
            updates.borrow_mut().push(done)
        })
        .unwrap();
        assert_eq!(server.requests()[0].range, Some("bytes=13-".into()));
        let updates = updates.borrow();
        let resumed = updates.iter().position(|done| *done == 13).unwrap();
        assert_eq!(updates[resumed + 1], 0);
        assert_eq!(fs::read(destination).unwrap(), contents);
    }

    #[test]
    fn invalid_content_range_keeps_the_existing_part_unchanged() {
        let directory = TestDirectory::new();
        let contents = body(8192);
        let models = [model("en-zh", &[("config.json", &contents)])];
        let destination = directory.0.join("en-zh/config.json");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(part_path(&destination), &contents[..32]).unwrap();
        let server = FixtureServer::new(vec![Reply::InvalidRange(contents.clone())]);
        let error =
            fixture_download(&directory.0, &models, &server, &|| false, &|_, _| {}).unwrap_err();
        assert!(error.to_string().contains("无效的续传"));
        assert_eq!(fs::read(part_path(&destination)).unwrap(), contents[..32]);
        assert!(!destination.exists());
        assert!(!ready_at_catalog(&directory.0, &models));
    }

    #[test]
    fn http_errors_are_short_and_do_not_expose_response_or_signed_urls() {
        for code in [403, 404, 416, 429, 500] {
            let directory = TestDirectory::new();
            let contents = body(128);
            let models = [model("en-zh", &[("config.json", &contents)])];
            let server = FixtureServer::new(vec![Reply::Error(code)]);
            let error = fixture_download(&directory.0, &models, &server, &|| false, &|_, _| {})
                .unwrap_err()
                .to_string();
            assert!(!error.contains("http://"), "{error}");
            assert!(!error.contains("https://"), "{error}");
            assert!(!error.contains("signature"), "{error}");
            assert!(error.contains("重试"), "{error}");
            assert!(!ready_at_catalog(&directory.0, &models));
            assert!(!directory.0.join("en-zh/manifest.json").exists());
        }
    }

    #[test]
    fn valid_files_are_reused_but_a_half_set_is_never_available() {
        let directory = TestDirectory::new();
        let first = body(128);
        let second = body(1024);
        let models = [model(
            "en-zh",
            &[
                ("config.json", &first),
                ("encoder_model_quantized.onnx", &second),
            ],
        )];
        let destination = directory.0.join("en-zh/config.json");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(&destination, &first).unwrap();
        let server = FixtureServer::new(vec![Reply::Error(403)]);
        fixture_download(&directory.0, &models, &server, &|| false, &|_, _| {}).unwrap_err();
        assert_eq!(server.requests().len(), 1);
        assert!(
            server.requests()[0]
                .path
                .ends_with("/onnx/encoder_model_quantized.onnx")
        );
        assert_eq!(fs::read(&destination).unwrap(), first);
        assert!(!ready_at_catalog(&directory.0, &models));
        assert!(!directory.0.join("en-zh/manifest.json").exists());
        let retry = FixtureServer::new(vec![Reply::Full(second)]);
        fixture_download(&directory.0, &models, &retry, &|| false, &|_, _| {}).unwrap();
        assert_eq!(retry.requests().len(), 1);
        assert_eq!(fs::read(destination).unwrap(), first);
        assert!(ready_at_catalog(&directory.0, &models));
    }

    #[test]
    fn corrupt_existing_file_is_replaced_only_after_a_verified_download() {
        let directory = TestDirectory::new();
        let contents = body(1024);
        let models = [model("en-zh", &[("config.json", &contents)])];
        let destination = directory.0.join("en-zh/config.json");
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        let old = vec![42; contents.len()];
        fs::write(&destination, &old).unwrap();
        let failed = FixtureServer::new(vec![Reply::Error(403)]);
        fixture_download(&directory.0, &models, &failed, &|| false, &|_, _| {}).unwrap_err();
        assert_eq!(fs::read(&destination).unwrap(), old);
        let retry = FixtureServer::new(vec![Reply::Full(contents.clone())]);
        fixture_download(&directory.0, &models, &retry, &|| false, &|_, _| {}).unwrap();
        assert_eq!(fs::read(destination).unwrap(), contents);
    }

    #[test]
    fn already_cancelled_download_does_not_create_a_directory_or_connect() {
        let directory = TestDirectory::new();
        let destination = directory.0.join("must-not-exist");
        let error = download(&destination, &|| true, &|_, _| {
            panic!("unexpected progress")
        })
        .unwrap_err();
        assert!(error.is::<Cancelled>());
        assert!(!destination.exists());
    }

    #[test]
    #[ignore = "downloads 230 MB from the original repositories; requires a new explicit LINGUA_TEST_MODEL_DOWNLOAD_DIR and preserves files"]
    fn real_origin_download_to_explicit_directory() {
        let directory = PathBuf::from(
            std::env::var_os("LINGUA_TEST_MODEL_DOWNLOAD_DIR")
                .expect("set LINGUA_TEST_MODEL_DOWNLOAD_DIR to a new absolute temporary directory"),
        );
        assert!(directory.is_absolute(), "test directory must be absolute");
        assert!(
            !directory.exists(),
            "refusing to touch an existing directory"
        );
        let last = std::cell::Cell::new(0);
        download(&directory, &|| false, &|done, total| {
            if done.saturating_sub(last.get()) >= 16 * 1024 * 1024 || done == total {
                println!("verified model download: {done}/{total} bytes");
                last.set(done);
            }
        })
        .unwrap();
        assert!(ready_at(&directory));
        println!("models preserved at {}", directory.display());
    }
}

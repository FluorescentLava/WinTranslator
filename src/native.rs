use anyhow::{Context, Result};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    rc::Rc,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
type Hwnd = *mut c_void;
type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;
#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
#[repr(C)]
struct MonitorInfo {
    size: u32,
    monitor: Rect,
    work: Rect,
    flags: u32,
}
#[repr(C)]
struct WindowPos {
    window: Hwnd,
    after: Hwnd,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    flags: u32,
}
#[repr(C)]
struct Cwp {
    lparam: isize,
    wparam: usize,
    message: u32,
    window: Hwnd,
}
#[repr(C)]
struct Msg {
    window: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point: Point,
    private: u32,
}
#[repr(C)]
struct WindowClass {
    style: u32,
    procedure: Option<WndProc>,
    class_extra: i32,
    window_extra: i32,
    instance: Hwnd,
    icon: Hwnd,
    cursor: Hwnd,
    background: Hwnd,
    menu_name: *const u16,
    class_name: *const u16,
}
#[repr(C)]
struct NotifyIcon {
    size: u32,
    window: Hwnd,
    id: u32,
    flags: u32,
    callback: u32,
    icon: Hwnd,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: [u8; 16],
    balloon_icon: Hwnd,
}
const HOTKEY_ID: i32 = 0x4854;
const TRAY_MESSAGE: u32 = 0x8000 + 42;
const SHOW_MESSAGE: u32 = 0x8000 + 43;
const REFRESH_MESSAGE: u32 = 0x8000 + 44;
const VERIFY_ACTIVATION_MESSAGE: u32 = 0x8000 + 45;
const TASKBAR_PROGRESS_MESSAGE: u32 = 0x8000 + 46;
const TASKBAR_PROGRESS_TIMER: usize = 0x5450_5247;
const TASKBAR_PROGRESS_INTERVAL_MS: u32 = 200;
const ACTIVATION_TIMER: usize = 0x5441_4354;
const ACTIVATION_DELAY_MS: u32 = 30;
const ACTIVATION_RETRY_WINDOW: Duration = Duration::from_millis(250);
const RESIZE_TIMER: usize = 0x5452_535a;
const FOCUS_TIMER: usize = 0x5446_4353;
const FOCUS_DELAY_MS: u32 = 60;
const RESIZE_DURATION_MS: f64 = 200.0;
// A 16 ms USER timer can be rounded up to two default clock ticks. Request
// the supported minimum and use elapsed time, rather than timer counts, below.
const RESIZE_INTERVAL_MS: u32 = 10;
// The visible card stays 16 DIPs from the work-area edges. The transparent
// host reserves the same gutter on all sides for the card's moving shadow.
pub const POPUP_SHADOW_INSET: u16 = 16;
const POPUP_MAX_HEIGHT: f64 = 480.0;
const CLASS_NAME: &str = "TranslationTool.Native.MessageWindow.v1";
const APP_NAME: &str = "WinTranslater";
const SETTINGS_TITLE: &str = "WinTranslater 设置";
const APP_ICON_ID: usize = 1;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskbarProgress {
    None,
    Indeterminate,
    Normal { completed: u64, total: u64 },
    Paused { completed: u64, total: u64 },
}
impl TaskbarProgress {
    fn from_bytes(completed: u64, total: u64) -> Self {
        if total == 0 {
            Self::Indeterminate
        } else {
            Self::Normal {
                completed: completed.min(total),
                total,
            }
        }
    }
    fn state(self) -> u32 {
        match self {
            Self::None => 0,          // TBPF_NOPROGRESS
            Self::Indeterminate => 1, // TBPF_INDETERMINATE
            Self::Normal { .. } => 2, // TBPF_NORMAL
            Self::Paused { .. } => 4, // TBPF_PAUSED
        }
    }
}
#[derive(Clone, Copy)]
enum TaskbarProgressSource {
    Manual(TaskbarProgress),
    ModelDownload,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct AppliedTaskbarProgress {
    window: Hwnd,
    progress: TaskbarProgress,
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct TaskbarProgressDiagnostic {
    window: Hwnd,
    state: u32,
    percent: Option<u64>,
    state_result: i32,
    value_result: Option<i32>,
}
// Worker threads only publish a request and post to the UI-owned sink. They
// never enumerate HWNDs, initialize COM, or use an apartment-bound interface.
static TASKBAR_PROGRESS_SOURCE: Mutex<TaskbarProgressSource> =
    Mutex::new(TaskbarProgressSource::Manual(TaskbarProgress::None));
static TASKBAR_PROGRESS_SINK: AtomicUsize = AtomicUsize::new(0);
static TASKBAR_PROGRESS_POSTED: AtomicBool = AtomicBool::new(false);
#[derive(Clone, Copy)]
struct ResizeAnimation {
    window: Hwnd,
    from: PopupViewport,
    to: PopupViewport,
    elapsed_ms: f64,
    last_sample: Instant,
    monitor: Hwnd,
    dpi: u32,
    revision: u64,
    transition_key: u64,
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct PopupViewport {
    width: f64,
    height: f64,
}
impl PopupViewport {
    fn from_frame(frame: PopupFrame) -> Self {
        Self {
            width: frame.width,
            height: frame.height,
        }
    }
    fn interpolate(self, target: Self, progress: f64) -> Self {
        Self {
            width: target.width,
            height: self.height + (target.height - self.height) * progress.clamp(0.0, 1.0),
        }
    }
}
#[derive(Clone, Copy, Default)]
struct PopupPresentation {
    revision: u64,
    show_requested: bool,
    hide_revision: Option<u64>,
}
impl PopupPresentation {
    fn show(&mut self) -> u64 {
        self.revision = self.revision.wrapping_add(1);
        self.show_requested = true;
        self.hide_revision = None;
        self.revision
    }
    fn hide(&mut self, animate: bool) -> Option<u64> {
        if self.hide_revision.is_some() {
            return None;
        }
        self.revision = self.revision.wrapping_add(1);
        self.show_requested = false;
        self.hide_revision = animate.then_some(self.revision);
        self.hide_revision
    }
    fn show_is_current(self, revision: u64) -> bool {
        self.show_requested && self.revision == revision && self.hide_revision.is_none()
    }
    fn hide_is_current(self, revision: u64) -> bool {
        !self.show_requested && self.hide_revision == Some(revision) && self.revision == revision
    }
    fn complete_hide(&mut self, revision: u64) -> bool {
        if !self.hide_is_current(revision) {
            return false;
        }
        self.hide_revision = None;
        true
    }
    fn hidden(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.show_requested = false;
        self.hide_revision = None;
    }
}
#[derive(Clone, Copy)]
struct ActivationVerification {
    revision: u64,
    source: Hwnd,
    started_at: Instant,
    retries: u8,
    checks: u8,
}
impl ActivationVerification {
    fn retry_denied_reason(
        self,
        presentation: PopupPresentation,
        foreground: Hwnd,
        sink: Hwnd,
        now: Instant,
    ) -> Option<&'static str> {
        if !presentation.show_is_current(self.revision) {
            Some("superseded")
        } else if self.retries >= 3 {
            Some("retry_limit")
        } else if now.saturating_duration_since(self.started_at) >= ACTIVATION_RETRY_WINDOW {
            Some("deadline")
        } else if !foreground.is_null() && foreground != self.source && foreground != sink {
            Some("foreground_changed")
        } else {
            None
        }
    }

    fn may_retry(
        self,
        presentation: PopupPresentation,
        foreground: Hwnd,
        sink: Hwnd,
        now: Instant,
    ) -> bool {
        self.retry_denied_reason(presentation, foreground, sink, now)
            .is_none()
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopupFrame {
    pub revision: u64,
    pub serial: u64,
    pub transition_key: u64,
    pub width: f64,
    pub height: f64,
    pub target_height: f64,
    pub progress: f64,
    pub finished: bool,
}
#[derive(Clone, Copy)]
struct PendingPopupFrame {
    frame: PopupFrame,
    window: Hwnd,
    placement: Option<(Rect, u32)>,
    transition_key: u64,
}
#[derive(Clone, Copy)]
pub enum Action {
    Show,
    PopupActivated(u64),
    PopupFocusReady(u64),
    PopupDismissed(u64),
    TranslateSelection,
    Settings,
    Quit,
    CompositionStarted,
    CompositionEnded,
}
thread_local! {
    static ACTION: RefCell<Option<Box<dyn Fn(Action)>>> = const { RefCell::new(None) };
    static ESCAPE: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    static INITIALIZED: Cell<bool> = const { Cell::new(false) };
    static COMPOSING: Cell<bool> = const { Cell::new(false) };
    static ORIGINAL_PROC: Cell<isize> = const { Cell::new(0) };
    static POPUP: Cell<Hwnd> = const { Cell::new(std::ptr::null_mut()) };
    static SINK: Cell<Hwnd> = const { Cell::new(std::ptr::null_mut()) };
    static ANCHOR: Cell<Hwnd> = const { Cell::new(std::ptr::null_mut()) };
    static SIZE: Cell<(f64, f64)> = const { Cell::new((480.0, 172.0)) };
    static TASKBAR_MESSAGE: Cell<u32> = const { Cell::new(0) };
    static TASKBAR_BUTTON_MESSAGE: Cell<u32> = const { Cell::new(0) };
    static TASKBAR_BUTTONS: RefCell<Vec<Hwnd>> = const { RefCell::new(Vec::new()) };
    static TASKBAR_INTERFACE: RefCell<Option<TaskbarInterface>> = const { RefCell::new(None) };
    static TASKBAR_INTERFACE_STALE: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_APPLIED: Cell<Option<AppliedTaskbarProgress>> = const { Cell::new(None) };
    static TASKBAR_ACTIVE_WINDOWS: RefCell<Vec<Hwnd>> = const { RefCell::new(Vec::new()) };
    static TASKBAR_FORCE_REFRESH: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_APPLYING: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_DIRTY: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_WAITING: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_DIAGNOSTIC: Cell<Option<TaskbarProgressDiagnostic>> = const { Cell::new(None) };
    static TASKBAR_COM_FAILURE: Cell<Option<(&'static str, i32)>> = const { Cell::new(None) };
    static STARTUP_WARNING: RefCell<Option<String>> = const { RefCell::new(None) };
    static PENDING_ACTION: Cell<Option<Action>> = const { Cell::new(None) };
    static RESIZE_ANIMATION: Cell<Option<ResizeAnimation>> = const { Cell::new(None) };
    static RESIZE_REVISION: Cell<u64> = const { Cell::new(0) };
    static FRAME_SERIAL: Cell<u64> = const { Cell::new(0) };
    static TRANSITION_KEY: Cell<u64> = const { Cell::new(0) };
    static PENDING_FRAME: Cell<Option<PendingPopupFrame>> = const { Cell::new(None) };
    static COMMITTED_FRAME: Cell<Option<PopupFrame>> = const { Cell::new(None) };
    static FRAME_HANDLER: RefCell<Option<Rc<dyn Fn(PopupFrame)>>> = const { RefCell::new(None) };
    static APPLYING_FRAME: Cell<bool> = const { Cell::new(false) };
    static MANUAL_MOVE: Cell<bool> = const { Cell::new(false) };
    static DEFER_EXTERNAL_GEOMETRY: Cell<bool> = const { Cell::new(false) };
    static SHOW_AFTER_FRAME: Cell<bool> = const { Cell::new(false) };
    static POPUP_PRESENTED: Cell<bool> = const { Cell::new(false) };
    static PRESENTATION: Cell<PopupPresentation> = const { Cell::new(PopupPresentation {
        revision: 0,
        show_requested: false,
        hide_revision: None,
    }) };
    static REFRESH_PENDING: Cell<bool> = const { Cell::new(false) };
    static FOCUS_RECHECKS: Cell<u8> = const { Cell::new(0) };
    static ACTIVATION_CHECK: Cell<Option<ActivationVerification>> = const { Cell::new(None) };
    static INPUT_ACTIVATION_RECOVERY: Cell<Option<u64>> = const { Cell::new(None) };
    static ACTIVATION_SOURCE: Cell<Hwnd> = const { Cell::new(std::ptr::null_mut()) };
}
#[link(name = "user32")]
unsafe extern "system" {
    fn EnumThreadWindows(
        thread: u32,
        callback: unsafe extern "system" fn(Hwnd, isize) -> i32,
        param: isize,
    ) -> i32;
    fn GetWindowTextW(window: Hwnd, text: *mut u16, count: i32) -> i32;
    fn GetClassNameW(window: Hwnd, text: *mut u16, count: i32) -> i32;
    fn GetWindowLongPtrW(window: Hwnd, index: i32) -> isize;
    fn SetWindowLongPtrW(window: Hwnd, index: i32, value: isize) -> isize;
    fn CallWindowProcW(
        procedure: isize,
        window: Hwnd,
        message: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize;
    fn SetWindowPos(
        window: Hwnd,
        after: Hwnd,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
    fn GetDpiForWindow(window: Hwnd) -> u32;
    fn GetWindowRect(window: Hwnd, rect: *mut Rect) -> i32;
    fn GetClientRect(window: Hwnd, rect: *mut Rect) -> i32;
    fn SetWindowRgn(window: Hwnd, region: Hwnd, redraw: i32) -> i32;
    fn IsWindowVisible(window: Hwnd) -> i32;
    fn IsWindow(window: Hwnd) -> i32;
    fn RedrawWindow(window: Hwnd, rectangle: *const Rect, region: Hwnd, flags: u32) -> i32;
    fn SetTimer(
        window: Hwnd,
        id: usize,
        milliseconds: u32,
        callback: Option<unsafe extern "system" fn(Hwnd, u32, usize, u32)>,
    ) -> usize;
    fn KillTimer(window: Hwnd, id: usize) -> i32;
    fn ReleaseCapture() -> i32;
    fn SendMessageW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> isize;
    fn PostMessageW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> i32;
    fn SetForegroundWindow(window: Hwnd) -> i32;
    fn SetActiveWindow(window: Hwnd) -> Hwnd;
    fn GetForegroundWindow() -> Hwnd;
    fn AttachThreadInput(from: u32, to: u32, attach: i32) -> i32;
    fn GetWindowThreadProcessId(window: Hwnd, process: *mut u32) -> u32;
    fn AllowSetForegroundWindow(process: u32) -> i32;
    fn SetWindowsHookExW(
        id: i32,
        callback: unsafe extern "system" fn(i32, usize, isize) -> isize,
        module: Hwnd,
        thread: u32,
    ) -> Hwnd;
    fn UnhookWindowsHookEx(hook: Hwnd) -> i32;
    fn CallNextHookEx(hook: Hwnd, code: i32, wparam: usize, lparam: isize) -> isize;
    fn GetAncestor(window: Hwnd, flags: u32) -> Hwnd;
    fn RegisterHotKey(window: Hwnd, id: i32, modifiers: u32, key: u32) -> i32;
    fn UnregisterHotKey(window: Hwnd, id: i32) -> i32;
    fn RegisterClassW(class: *const WindowClass) -> u16;
    fn CreateWindowExW(
        extended: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: Hwnd,
        instance: Hwnd,
        parameter: Hwnd,
    ) -> Hwnd;
    fn DestroyWindow(window: Hwnd) -> i32;
    fn DefWindowProcW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> isize;
    fn FindWindowExW(parent: Hwnd, after: Hwnd, class: *const u16, title: *const u16) -> Hwnd;
    fn ShowWindow(window: Hwnd, command: i32) -> i32;
    fn GetCursorPos(point: *mut Point) -> i32;
    fn MonitorFromPoint(point: Point, flags: u32) -> Hwnd;
    fn MonitorFromWindow(window: Hwnd, flags: u32) -> Hwnd;
    fn GetMonitorInfoW(monitor: Hwnd, info: *mut MonitorInfo) -> i32;
    fn CreatePopupMenu() -> Hwnd;
    fn AppendMenuW(menu: Hwnd, flags: u32, id: usize, text: *const u16) -> i32;
    fn TrackPopupMenu(
        menu: Hwnd,
        flags: u32,
        x: i32,
        y: i32,
        reserved: i32,
        owner: Hwnd,
        rect: *const Rect,
    ) -> i32;
    fn DestroyMenu(menu: Hwnd) -> i32;
    fn LoadImageW(
        instance: Hwnd,
        name: *const u16,
        kind: u32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> Hwnd;
    fn GetSystemMetricsForDpi(index: i32, dpi: u32) -> i32;
    fn GetDpiForSystem() -> u32;
    fn RegisterWindowMessageW(name: *const u16) -> u32;
    fn GetFocus() -> Hwnd;
    fn GetMessageW(message: *mut Msg, window: Hwnd, minimum: u32, maximum: u32) -> i32;
    fn TranslateMessage(message: *const Msg) -> i32;
    fn DispatchMessageW(message: *const Msg) -> isize;
}
#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateRectRgn(left: i32, top: i32, right: i32, bottom: i32) -> Hwnd;
    fn DeleteObject(object: Hwnd) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
    fn GetModuleHandleW(name: *const u16) -> Hwnd;
    fn CreateMutexW(attributes: Hwnd, owner: i32, name: *const u16) -> Hwnd;
    fn GetLastError() -> u32;
    fn CloseHandle(handle: Hwnd) -> i32;
    fn LoadLibraryExW(name: *const u16, file: Hwnd, flags: u32) -> Hwnd;
    fn GetProcAddress(module: Hwnd, name: *const u8) -> Hwnd;
}
#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(window: Hwnd, attribute: u32, value: *const c_void, size: u32) -> i32;
}
#[link(name = "shcore")]
unsafe extern "system" {
    fn GetDpiForMonitor(monitor: Hwnd, kind: i32, x: *mut u32, y: *mut u32) -> i32;
}
#[link(name = "shell32")]
unsafe extern "system" {
    fn Shell_NotifyIconW(message: u32, data: *const NotifyIcon) -> i32;
}
#[link(name = "imm32")]
unsafe extern "system" {
    fn ImmGetContext(window: Hwnd) -> Hwnd;
    fn ImmGetCompositionStringW(context: Hwnd, index: u32, buffer: Hwnd, size: u32) -> i32;
    fn ImmReleaseContext(window: Hwnd, context: Hwnd) -> i32;
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn app_icon(size: i32) -> Hwnd {
    unsafe {
        // Load the matching embedded ICO frame instead of scaling the default
        // large icon down for both the tray and the window's small-icon slot.
        LoadImageW(
            GetModuleHandleW(std::ptr::null()),
            APP_ICON_ID as *const u16,
            1, // IMAGE_ICON
            size,
            size,
            0x8000, // LR_SHARED: the module owns the cached resource handle.
        )
    }
}
fn set_window_icon(window: Hwnd) {
    let dpi = unsafe { GetDpiForWindow(window) }.max(96);
    for (slot, metric) in [(0, 49), (1, 11)] {
        let icon = app_icon(unsafe { GetSystemMetricsForDpi(metric, dpi) }.max(1));
        if !icon.is_null() {
            unsafe {
                SendMessageW(window, 0x0080, slot, icon as isize); // WM_SETICON
            }
        }
    }
}
fn emit(action: Action) {
    ACTION.with(|f| {
        if let Some(f) = f.borrow().as_ref() {
            f(action);
        } else {
            PENDING_ACTION.with(|v| v.set(Some(action)));
        }
    });
}
fn title(window: Hwnd) -> String {
    let mut value = [0_u16; 128];
    let len = unsafe { GetWindowTextW(window, value.as_mut_ptr(), value.len() as i32) };
    String::from_utf16_lossy(&value[..len.max(0) as usize])
}
struct Search<'a> {
    title: &'a str,
    window: Hwnd,
}
unsafe extern "system" fn find(window: Hwnd, context: isize) -> i32 {
    let search = unsafe { &mut *(context as *mut Search<'_>) };
    if title(window) == search.title {
        search.window = window;
        0
    } else {
        1
    }
}
fn window(name: &str) -> Option<Hwnd> {
    let mut search = Search {
        title: name,
        window: std::ptr::null_mut(),
    };
    unsafe {
        EnumThreadWindows(GetCurrentThreadId(), find, &mut search as *mut _ as isize);
    }
    (!search.window.is_null()).then_some(search.window)
}
fn request_taskbar_progress_refresh() {
    let sink = TASKBAR_PROGRESS_SINK.load(Ordering::Acquire) as Hwnd;
    if !sink.is_null()
        && !TASKBAR_PROGRESS_POSTED.swap(true, Ordering::AcqRel)
        && unsafe { PostMessageW(sink, TASKBAR_PROGRESS_MESSAGE, 0, 0) } == 0
    {
        TASKBAR_PROGRESS_POSTED.store(false, Ordering::Release);
    }
}
fn publish_taskbar_progress(source: TaskbarProgressSource) {
    *TASKBAR_PROGRESS_SOURCE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = source;
    request_taskbar_progress_refresh();
}
/// May be called from any thread. A zero total displays indeterminate progress;
/// all HWND and ITaskbarList3 work is dispatched to the native UI thread.
pub fn set_taskbar_progress(completed: u64, total: u64) {
    publish_taskbar_progress(TaskbarProgressSource::Manual(TaskbarProgress::from_bytes(
        completed, total,
    )));
}
/// Stop automatic download tracking and remove progress from the last target.
pub fn clear_taskbar_progress() {
    publish_taskbar_progress(TaskbarProgressSource::Manual(TaskbarProgress::None));
}
/// Call after starting a model download or opening its settings. The native
/// timer follows the memory-only worker snapshot even after settings closes,
/// preserves paused progress, and clears it when downloading is cancelled,
/// removed, completed, or fails.
pub fn sync_model_download_taskbar_progress() {
    publish_taskbar_progress(TaskbarProgressSource::ModelDownload);
}
fn requested_taskbar_progress() -> TaskbarProgress {
    let source = *TASKBAR_PROGRESS_SOURCE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match source {
        TaskbarProgressSource::Manual(progress) => progress,
        TaskbarProgressSource::ModelDownload => {
            let status = crate::model_download::status();
            match status.phase {
                crate::model_download::Phase::Downloading => {
                    TaskbarProgress::from_bytes(status.downloaded, status.total)
                }
                crate::model_download::Phase::Pausing | crate::model_download::Phase::Paused => {
                    TaskbarProgress::Paused {
                        completed: status.downloaded.min(status.total),
                        total: status.total.max(1),
                    }
                }
                _ => TaskbarProgress::None,
            }
        }
    }
}
fn taskbar_progress_window() -> Hwnd {
    // A hidden popup has no taskbar button. Settings remains the download's
    // visible owner when that popup is dismissed; minimized windows count too.
    for name in [SETTINGS_TITLE, APP_NAME] {
        if let Some(window) = window(name)
            && unsafe { IsWindowVisible(window) } != 0
            && TASKBAR_BUTTONS.with(|buttons| buttons.borrow().contains(&window))
        {
            return window;
        }
    }
    std::ptr::null_mut()
}
fn taskbar_progress_diagnostic(
    window: Hwnd,
    progress: TaskbarProgress,
    state_result: i32,
    value_result: Option<i32>,
) {
    let (completed, total, percent) = match progress {
        TaskbarProgress::Normal { completed, total }
        | TaskbarProgress::Paused { completed, total } => (
            Some(completed),
            Some(total),
            Some((u128::from(completed) * 100 / u128::from(total)) as u64),
        ),
        _ => (None, None, None),
    };
    let diagnostic = TaskbarProgressDiagnostic {
        window,
        state: progress.state(),
        percent,
        state_result,
        value_result,
    };
    if TASKBAR_DIAGNOSTIC.with(|last| last.replace(Some(diagnostic))) == Some(diagnostic) {
        return;
    }
    crate::storage::diagnostic(serde_json::json!({
        "event": "taskbar_progress_applied",
        "thread": unsafe { GetCurrentThreadId() },
        "window": format!("{:p}", window),
        "title": title(window),
        "visible": unsafe { IsWindowVisible(window) } != 0,
        "state": match progress {
            TaskbarProgress::None => "none",
            TaskbarProgress::Indeterminate => "indeterminate",
            TaskbarProgress::Normal { .. } => "normal",
            TaskbarProgress::Paused { .. } => "paused",
        },
        "completed": completed,
        "total": total,
        "percent": percent,
        "state_hresult": format!("0x{:08X}", state_result as u32),
        "value_hresult": value_result.map(|result| format!("0x{:08X}", result as u32)),
    }));
}
fn apply_taskbar_progress(progress: TaskbarProgress, force: bool) -> bool {
    let target = if progress == TaskbarProgress::None {
        std::ptr::null_mut()
    } else {
        taskbar_progress_window()
    };
    let waiting = progress != TaskbarProgress::None && target.is_null();
    if TASKBAR_WAITING.with(|value| value.replace(waiting)) != waiting && waiting {
        crate::storage::diagnostic(serde_json::json!({
            "event": "taskbar_progress_waiting_for_visible_button",
            "thread": unsafe { GetCurrentThreadId() },
        }));
    }
    let desired = AppliedTaskbarProgress {
        window: target,
        progress,
    };
    let previous = TASKBAR_APPLIED.with(Cell::get);
    if !force && previous == Some(desired) {
        return false;
    }
    let clear_windows = TASKBAR_ACTIVE_WINDOWS.with(|windows| {
        let mut windows = windows.borrow_mut();
        windows.retain(|window| unsafe { IsWindow(*window) } != 0);
        windows
            .iter()
            .copied()
            .filter(|window| *window != target)
            .collect::<Vec<_>>()
    });
    if target.is_null() && clear_windows.is_empty() {
        TASKBAR_APPLIED.with(|value| value.set(Some(desired)));
        return false;
    }
    // Release RefCell borrows before COM calls, which can pump native messages.
    let mut interface = TASKBAR_INTERFACE.with(|value| value.borrow_mut().take());
    if TASKBAR_INTERFACE_STALE.with(|value| value.replace(false)) {
        interface = None;
    }
    if interface.is_none() {
        interface = TaskbarInterface::new();
    }
    let Some(interface) = interface else {
        TASKBAR_APPLIED.with(|value| value.set(None));
        return true;
    };
    let table = unsafe { interface.object.table::<TaskbarList3Table>() };
    let mut succeeded = true;
    for window in clear_windows {
        let result = unsafe { (table.set_progress_state)(interface.object.0, window, 0) };
        taskbar_progress_diagnostic(window, TaskbarProgress::None, result, None);
        succeeded &= result >= 0;
        if result >= 0 {
            TASKBAR_ACTIVE_WINDOWS.with(|windows| {
                windows.borrow_mut().retain(|active| *active != window);
            });
        }
    }
    if !target.is_null() {
        // Remember every touched button, including partial COM failures, so a
        // later cancel/retarget can still clear all progress we may have set.
        TASKBAR_ACTIVE_WINDOWS.with(|windows| {
            let mut windows = windows.borrow_mut();
            if !windows.contains(&target) {
                windows.push(target);
            }
        });
        let value_result = if let TaskbarProgress::Normal { completed, total }
        | TaskbarProgress::Paused { completed, total } = progress
        {
            Some(unsafe {
                (table.set_progress_value)(interface.object.0, target, completed, total)
            })
        } else {
            None
        };
        // Apply the final state after updating the value so pause/resume keeps
        // the desired color even when the button has just been recreated.
        let state_result =
            unsafe { (table.set_progress_state)(interface.object.0, target, progress.state()) };
        taskbar_progress_diagnostic(target, progress, state_result, value_result);
        succeeded &= state_result >= 0 && value_result.is_none_or(|result| result >= 0);
    }
    if succeeded {
        TASKBAR_APPLIED.with(|value| value.set(Some(desired)));
        TASKBAR_INTERFACE.with(|value| *value.borrow_mut() = Some(interface));
    } else {
        TASKBAR_APPLIED.with(|value| value.set(None));
    }
    !succeeded
}
fn update_taskbar_progress(sink: Hwnd) {
    if TASKBAR_APPLYING.with(|value| value.replace(true)) {
        TASKBAR_DIRTY.with(|value| value.set(true));
        return;
    }
    let progress = requested_taskbar_progress();
    let force = TASKBAR_FORCE_REFRESH.with(|value| value.replace(false));
    let retry = apply_taskbar_progress(progress, force);
    unsafe {
        if progress != TaskbarProgress::None || retry {
            SetTimer(
                sink,
                TASKBAR_PROGRESS_TIMER,
                TASKBAR_PROGRESS_INTERVAL_MS,
                None,
            );
        } else {
            KillTimer(sink, TASKBAR_PROGRESS_TIMER);
        }
    }
    TASKBAR_APPLYING.with(|value| value.set(false));
    if TASKBAR_DIRTY.with(|value| value.replace(false)) {
        request_taskbar_progress_refresh();
    }
}
fn mouse_monitor() -> Hwnd {
    let mut point = Point { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut point);
        MonitorFromPoint(point, 2)
    }
}
fn monitor_dpi(window: Hwnd, monitor: Hwnd) -> u32 {
    if unsafe { MonitorFromWindow(window, 2) } == monitor {
        return unsafe { GetDpiForWindow(window) }.max(96);
    }
    let (mut x, mut y) = (96, 96);
    if unsafe { GetDpiForMonitor(monitor, 0, &mut x, &mut y) } == 0 {
        x.max(96)
    } else {
        unsafe { GetDpiForWindow(window) }.max(96)
    }
}
fn placement(window: Hwnd) -> (i32, i32, i32, i32) {
    let monitor = ANCHOR.with(|value| {
        if value.get().is_null() {
            value.set(mouse_monitor());
        }
        value.get()
    });
    let mut info: MonitorInfo = unsafe { std::mem::zeroed() };
    info.size = std::mem::size_of::<MonitorInfo>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        info.work = Rect {
            left: 0,
            top: 0,
            right: 1280,
            bottom: 720,
        };
    }
    let scale = f64::from(monitor_dpi(window, monitor)) / 96.0;
    let (width, _) = SIZE.with(Cell::get);
    let bounds = popup_bounds(info.work, scale, width);
    (
        bounds.left,
        bounds.top,
        bounds.right - bounds.left,
        bounds.bottom - bounds.top,
    )
}
fn popup_bounds(work: Rect, scale: f64, width: f64) -> Rect {
    let inset = (f64::from(POPUP_SHADOW_INSET) * scale).round() as i32;
    let width = ((width * scale).round() as i32).min((work.right - work.left - 2 * inset).max(1));
    let height = ((POPUP_MAX_HEIGHT * scale).round() as i32)
        .min((work.bottom - work.top - 2 * inset).max(1));
    Rect {
        left: work.right - 2 * inset - width,
        top: work.bottom - 2 * inset - height,
        right: work.right,
        bottom: work.bottom,
    }
}
fn popup_viewport_extent(physical_extent: i32, scale: f64) -> f64 {
    let inset = (f64::from(POPUP_SHADOW_INSET) * scale).round() as i32;
    f64::from((physical_extent - 2 * inset).max(1)) / scale
}
fn limited_viewport(host: Rect, scale: f64, requested: (f64, f64)) -> PopupViewport {
    let available_width = popup_viewport_extent(host.right - host.left, scale);
    let available_height = popup_viewport_extent(host.bottom - host.top, scale);
    PopupViewport {
        width: available_width.min(requested.0).max(1.0),
        height: ((requested.1 * scale).round() / scale).clamp(1.0, available_height),
    }
}
fn update_popup_region(window: Hwnd, card_height: f64) {
    let mut client: Rect = unsafe { std::mem::zeroed() };
    if unsafe { GetClientRect(window, &mut client) } == 0 {
        return;
    }
    let scale = f64::from(unsafe { GetDpiForWindow(window) }.max(96)) / 96.0;
    let visible_height =
        ((card_height + 2.0 * f64::from(POPUP_SHADOW_INSET)) * scale).ceil() as i32;
    let region = unsafe {
        CreateRectRgn(
            0,
            (client.bottom - visible_height).max(0),
            client.right,
            client.bottom,
        )
    };
    if region.is_null() {
        return;
    }
    // Success transfers ownership to USER. The excluded upper host area is
    // outside the native window region, so other processes receive its clicks.
    if unsafe { SetWindowRgn(window, region, 1) } == 0 {
        unsafe {
            DeleteObject(region);
        }
    }
}
/// Distance needed to move the entire card and shadow below the taskbar edge.
/// Read the committed viewport rather than a pending translation result size.
pub fn popup_slide_distance() -> f32 {
    let height = COMMITTED_FRAME
        .with(Cell::get)
        .map_or_else(|| SIZE.with(Cell::get).1, |frame| frame.height);
    (height + 2.0 * f64::from(POPUP_SHADOW_INSET)).max(1.0) as f32
}
fn finish_resize(window: Hwnd) {
    unsafe {
        KillTimer(window, RESIZE_TIMER);
    }
    RESIZE_ANIMATION.with(|value| value.set(None));
}
fn stop_resize(window: Hwnd) {
    finish_resize(window);
    PENDING_FRAME.with(|value| value.set(None));
    RESIZE_REVISION.with(|value| value.set(value.get().wrapping_add(1)));
}
fn request_refresh(window: Hwnd) {
    if unsafe { IsWindowVisible(window) } != 0
        && !REFRESH_PENDING.with(|value| value.replace(true))
        && unsafe { PostMessageW(window, REFRESH_MESSAGE, 0, 0) } == 0
    {
        REFRESH_PENDING.with(|value| value.set(false));
    }
}
fn set_rectangle(window: Hwnd, rectangle: Rect, flags: u32) {
    let mut current = rectangle;
    if flags == 0 && unsafe { GetWindowRect(window, &mut current) } != 0 && current == rectangle {
        return;
    }
    let was_applying = APPLYING_FRAME.with(|value| value.replace(true));
    unsafe {
        SetWindowPos(
            window,
            std::ptr::null_mut(),
            rectangle.left,
            rectangle.top,
            rectangle.right - rectangle.left,
            rectangle.bottom - rectangle.top,
            // The caller has already committed the corresponding XAML viewport.
            flags | 0x0004 | 0x0010 | 0x2000,
        );
    }
    APPLYING_FRAME.with(|value| value.set(was_applying));
    if RESIZE_ANIMATION.with(Cell::get).is_none() {
        request_refresh(window);
    }
}
fn target_rectangle(window: Hwnd) -> Rect {
    let (x, y, width, height) = placement(window);
    Rect {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    }
}

fn update_presentation<T>(update: impl FnOnce(&mut PopupPresentation) -> T) -> T {
    PRESENTATION.with(|value| {
        let mut presentation = value.get();
        let result = update(&mut presentation);
        value.set(presentation);
        result
    })
}
fn foreground_belongs_to_app(foreground: Hwnd, popup: Hwnd) -> bool {
    if foreground.is_null() || popup.is_null() {
        return false;
    }
    if foreground == popup || unsafe { GetAncestor(foreground, 3) } == popup {
        return true;
    }
    let (mut foreground_process, mut popup_process) = (0, 0);
    unsafe {
        GetWindowThreadProcessId(foreground, &mut foreground_process);
        GetWindowThreadProcessId(popup, &mut popup_process);
    }
    foreground_process != 0 && foreground_process == popup_process
}
fn cancel_focus_check(window: Hwnd) {
    unsafe {
        KillTimer(window, FOCUS_TIMER);
    }
    FOCUS_RECHECKS.with(|value| value.set(0));
}
fn schedule_focus_check() {
    if ACTIVATION_CHECK.with(Cell::get).is_some() {
        return;
    }
    let popup = POPUP.with(Cell::get);
    if !popup.is_null()
        && INITIALIZED.with(Cell::get)
        && POPUP_PRESENTED.with(Cell::get)
        && unsafe { IsWindowVisible(popup) } != 0
    {
        FOCUS_RECHECKS.with(|value| value.set(0));
        unsafe {
            SetTimer(popup, FOCUS_TIMER, FOCUS_DELAY_MS, None);
        }
    }
}
fn focus_check_tick(window: Hwnd) {
    unsafe {
        KillTimer(window, FOCUS_TIMER);
    }
    if SHOW_AFTER_FRAME.with(Cell::get)
        || ACTIVATION_CHECK.with(Cell::get).is_some()
        || !POPUP_PRESENTED.with(Cell::get)
        || !PRESENTATION.with(Cell::get).show_requested
        || unsafe { IsWindowVisible(window) } == 0
    {
        return;
    }
    let foreground = unsafe { GetForegroundWindow() };
    if foreground_belongs_to_app(foreground, window) {
        if foreground != window {
            release_popup_layer(window);
        }
        return;
    }
    // XAML can briefly have no foreground HWND while opening or closing a
    // native Flyout. Recheck after that transient focus handoff has settled.
    if foreground.is_null() && FOCUS_RECHECKS.with(Cell::get) < 2 {
        FOCUS_RECHECKS.with(|value| value.set(value.get() + 1));
        unsafe {
            SetTimer(window, FOCUS_TIMER, FOCUS_DELAY_MS, None);
        }
        return;
    }
    hide_popup();
}

/// Installs the popup's single animation-frame consumer on its UI thread.
/// The consumer commits its fixed/cropped viewport before acknowledging a frame.
pub fn install_popup_frame_handler(handler: impl Fn(PopupFrame) + 'static) {
    FRAME_HANDLER.with(|value| *value.borrow_mut() = Some(Rc::new(handler)));
    if let Some(pending) = PENDING_FRAME.with(Cell::get) {
        publish_popup_frame(pending);
    }
}

pub fn popup_frame_is_current(frame: PopupFrame) -> bool {
    PENDING_FRAME.with(Cell::get).is_some_and(|pending| {
        pending.frame == frame && RESIZE_REVISION.with(Cell::get) == frame.revision
    })
}

fn publish_popup_frame(pending: PendingPopupFrame) {
    PENDING_FRAME.with(|value| value.set(Some(pending)));
    let handler = FRAME_HANDLER.with(|value| value.borrow().clone());
    if let Some(handler) = handler {
        handler(pending.frame);
    } else {
        // During bootstrap there may be no component yet. There is no second
        // independently running HWND animation in this fallback.
        commit_popup_frame(pending.frame);
    }
}

fn queue_popup_frame(
    window: Hwnd,
    viewport: PopupViewport,
    target: PopupViewport,
    revision: u64,
    progress: f64,
    finished: bool,
    placement: Option<(Rect, u32)>,
) {
    let serial = FRAME_SERIAL.with(|value| {
        let next = value.get().wrapping_add(1);
        value.set(next);
        next
    });
    publish_popup_frame(PendingPopupFrame {
        window,
        placement,
        transition_key: TRANSITION_KEY.with(Cell::get),
        frame: PopupFrame {
            revision,
            serial,
            transition_key: TRANSITION_KEY.with(Cell::get),
            width: viewport.width,
            height: viewport.height,
            target_height: target.height,
            progress,
            finished,
        },
    });
}

/// Applies only the currently outstanding frame, after the UI view has committed.
/// Retargeting invalidates an older acknowledgement, including its physical rect.
pub fn commit_popup_frame(frame: PopupFrame) {
    if !popup_frame_is_current(frame) {
        return;
    }
    let Some(pending) = PENDING_FRAME.with(Cell::take) else {
        return;
    };
    // Only a first presentation or a monitor/DPI change places the fixed host.
    // Ordinary animation frames update XAML alone; HWND top and client size
    // cannot race the footer's bottom anchor on a separate rendering clock.
    if let Some((rectangle, flags)) = pending.placement {
        set_rectangle(pending.window, rectangle, flags);
    }
    // Moving between monitors can synchronously raise WM_DPICHANGED and replace
    // the request while SetWindowPos is on the stack. Its new frame owns the HWND.
    if RESIZE_REVISION.with(Cell::get) != frame.revision {
        return;
    }
    COMMITTED_FRAME.with(|value| value.set(Some(frame)));
    update_popup_region(pending.window, frame.height);
    if frame.finished {
        finish_resize(pending.window);
        if SHOW_AFTER_FRAME.with(Cell::take) {
            activate_popup(pending.window);
        }
        request_refresh(pending.window);
    } else {
        RESIZE_ANIMATION.with(|value| {
            if let Some(mut animation) = value.get()
                && animation.revision == frame.revision
            {
                // UI commit time is backpressure, not elapsed animation time.
                // An overloaded message queue cannot skip directly to the end.
                animation.last_sample = Instant::now();
                value.set(Some(animation));
            }
        });
    }
}

fn resize_tick(window: Hwnd) {
    let Some(mut animation) = RESIZE_ANIMATION.with(Cell::get) else {
        return;
    };
    if animation.window != window {
        return;
    }
    if unsafe { IsWindowVisible(window) } == 0 {
        stop_resize(window);
        return;
    }
    // At most one XAML height frame may be in flight. The HWND remains fixed.
    if PENDING_FRAME.with(Cell::get).is_some() {
        return;
    }
    let monitor = unsafe { MonitorFromWindow(window, 2) };
    if monitor != animation.monitor || unsafe { GetDpiForWindow(window) }.max(96) != animation.dpi {
        stop_resize(window);
        ANCHOR.with(|value| value.set(monitor));
        resize_popup(window, true);
        return;
    }
    animation.elapsed_ms += (animation.last_sample.elapsed().as_secs_f64() * 1000.0).min(32.0);
    animation.last_sample = Instant::now();
    let progress = (animation.elapsed_ms / RESIZE_DURATION_MS).clamp(0.0, 1.0);
    // Zero velocity at both ends avoids cubic-out's large first-frame movement.
    let eased = progress * progress * (3.0 - 2.0 * progress);
    RESIZE_ANIMATION.with(|value| value.set(Some(animation)));
    queue_popup_frame(
        window,
        animation.from.interpolate(animation.to, eased),
        animation.to,
        animation.revision,
        eased,
        progress >= 1.0,
        None,
    );
}
fn resize_popup(window: Hwnd, first: bool) {
    // Keep the committed viewport steady while its WinUI visual slides out.
    // SIZE still records newer results, and reopening consumes that target.
    if PRESENTATION.with(Cell::get).hide_revision.is_some() {
        return;
    }
    let host = target_rectangle(window);
    let mut current = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let visible = unsafe { IsWindowVisible(window) } != 0;
    let monitor = unsafe { MonitorFromWindow(window, 2) };
    let anchor = ANCHOR.with(Cell::get);
    let dpi = monitor_dpi(window, anchor);
    let scale = f64::from(dpi.max(96)) / 96.0;
    let target = limited_viewport(host, scale, SIZE.with(Cell::get));
    let transition_key = TRANSITION_KEY.with(Cell::get);
    let has_rectangle = unsafe { GetWindowRect(window, &mut current) } != 0;
    if !first
        && (RESIZE_ANIMATION.with(Cell::get).is_some_and(|animation| {
            animation.window == window
                && animation.to == target
                && animation.dpi == dpi
                && animation.transition_key == transition_key
        }) || PENDING_FRAME.with(Cell::get).is_some_and(|pending| {
            pending.window == window
                && pending.frame.finished
                && PopupViewport::from_frame(pending.frame) == target
                && pending
                    .placement
                    .map_or(has_rectangle && current == host, |(bounds, _)| {
                        bounds == host
                    })
                && pending.transition_key == transition_key
        }))
    {
        return;
    }
    let committed = COMMITTED_FRAME.with(Cell::get);
    if !first
        && has_rectangle
        && current == host
        && committed.is_some_and(|frame| {
            PopupViewport::from_frame(frame) == target
                && frame.transition_key == transition_key
                && frame.finished
        })
    {
        return;
    }
    let animate = !first
        && FRAME_HANDLER.with(|value| value.borrow().is_some())
        && visible
        && has_rectangle
        && monitor == anchor
        && current == host
        && committed.is_some_and(|frame| frame.width == target.width);
    stop_resize(window);
    let revision = RESIZE_REVISION.with(Cell::get);
    if !animate {
        queue_popup_frame(
            window,
            target,
            target,
            revision,
            1.0,
            true,
            (first || !has_rectangle || current != host)
                .then_some((host, if first { 0x0020 } else { 0 })),
        );
        return;
    }
    // A reversal starts from the last XAML viewport actually acknowledged,
    // independently of the fixed maximum-height native client rectangle.
    let from = PopupViewport::from_frame(committed.unwrap());
    RESIZE_ANIMATION.with(|value| {
        value.set(Some(ResizeAnimation {
            window,
            from,
            to: target,
            elapsed_ms: 0.0,
            last_sample: Instant::now(),
            monitor,
            dpi,
            revision,
            transition_key,
        }))
    });
    if unsafe { SetTimer(window, RESIZE_TIMER, RESIZE_INTERVAL_MS, None) } == 0 {
        stop_resize(window);
        queue_popup_frame(
            window,
            target,
            target,
            RESIZE_REVISION.with(Cell::get),
            1.0,
            true,
            None,
        );
    } else {
        queue_popup_frame(window, from, target, revision, 0.0, false, None);
    }
}
unsafe extern "system" fn popup_proc(
    window: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if INITIALIZED.with(Cell::get) && POPUP.with(Cell::get) == window {
        if message == 0x0083 && wparam != 0 {
            // This popup has no caption or resize frame. Keep its client area
            // equal to the whole HWND, rather than letting the overlapped
            // presenter reintroduce top/bottom frame insets during a resize.
            // The rounded Mica card is inside the transparent WinUI host.
            return 0;
        }
    }
    if message == REFRESH_MESSAGE {
        REFRESH_PENDING.with(|value| value.set(false));
        if unsafe { IsWindowVisible(window) } != 0 && RESIZE_ANIMATION.with(Cell::get).is_none() {
            unsafe {
                // Invalidate once at the settled size. Let the ordinary XAML
                // dispatcher/compositor paint; RDW_UPDATENOW | ALLCHILDREN used
                // to synchronously repaint every intermediate client surface.
                RedrawWindow(
                    window,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    0x0001 | 0x0020,
                );
            }
        }
        return 0;
    }
    if message == 0x0113 && wparam == RESIZE_TIMER {
        resize_tick(window);
        return 0;
    }
    if message == 0x0113 && wparam == FOCUS_TIMER {
        focus_check_tick(window);
        return 0;
    }
    if message == VERIFY_ACTIVATION_MESSAGE {
        verify_popup_activation(window, wparam as u64);
        return 0;
    }
    if message == 0x0113 && wparam == ACTIVATION_TIMER {
        if let Some(check) = ACTIVATION_CHECK.with(Cell::get) {
            verify_popup_activation(window, check.revision);
        }
        return 0;
    }
    if message == 0x0231 {
        MANUAL_MOVE.with(|value| value.set(true));
        stop_resize(window);
    }
    if message == 0x0232 {
        MANUAL_MOVE.with(|value| value.set(false));
    }
    if (message == 0x0018 && wparam == 0) || message == 0x0082 {
        cancel_activation_check(window);
        SHOW_AFTER_FRAME.with(|value| value.set(false));
        stop_resize(window);
        cancel_focus_check(window);
        POPUP_PRESENTED.with(|value| value.set(false));
        update_presentation(PopupPresentation::hidden);
    }
    if message == 0x0046 {
        let position = unsafe { &mut *(lparam as *mut WindowPos) };
        // Position the first intentionally opened WinUI frame before it is shown.
        if !INITIALIZED.with(Cell::get) {
            let (x, y, width, height) = placement(window);
            position.x = x;
            position.y = y;
            position.width = width;
            position.height = height;
            position.flags &= !(0x0001 | 0x0002);
        } else if DEFER_EXTERNAL_GEOMETRY.with(Cell::get)
            || (!APPLYING_FRAME.with(Cell::get) && !MANUAL_MOVE.with(Cell::get))
        {
            // Only an acknowledged popup frame owns programmatic geometry;
            // AppWindow and default DPI handling cannot override that frame.
            position.flags |= 0x0001 | 0x0002;
        }
    }
    if message == 0x0010 || (message == 0x0112 && wparam & 0xfff0 == 0xf060) {
        hide_popup();
        return 0;
    }
    if message == 0x02e0 && INITIALIZED.with(Cell::get) {
        stop_resize(window);
        let previous = DEFER_EXTERNAL_GEOMETRY.with(|value| value.replace(true));
        let result = unsafe {
            CallWindowProcW(
                ORIGINAL_PROC.with(Cell::get),
                window,
                message,
                wparam,
                lparam,
            )
        };
        DEFER_EXTERNAL_GEOMETRY.with(|value| value.set(previous));
        ANCHOR.with(|v| v.set(unsafe { MonitorFromWindow(window, 2) }));
        resize_popup(window, true);
        return result;
    }
    if INITIALIZED.with(Cell::get) && (message == 0x007e || (message == 0x001a && wparam == 0x002f))
    {
        stop_resize(window);
        let previous = DEFER_EXTERNAL_GEOMETRY.with(|value| value.replace(true));
        let result = unsafe {
            CallWindowProcW(
                ORIGINAL_PROC.with(Cell::get),
                window,
                message,
                wparam,
                lparam,
            )
        };
        DEFER_EXTERNAL_GEOMETRY.with(|value| value.set(previous));
        ANCHOR.with(|value| value.set(unsafe { MonitorFromWindow(window, 2) }));
        resize_popup(window, true);
        return result;
    }
    let original = ORIGINAL_PROC.with(Cell::get);
    unsafe { CallWindowProcW(original, window, message, wparam, lparam) }
}
fn attach_popup(window: Hwnd) {
    if ORIGINAL_PROC.with(Cell::get) == 0 {
        POPUP.with(|v| v.set(window));
        ORIGINAL_PROC.with(|v| {
            v.set(unsafe { SetWindowLongPtrW(window, -4, popup_proc as *const () as isize) })
        });
    }
}
fn observe_taskbar_button_created(window: Hwnd, message: u32) {
    let taskbar_button_message = TASKBAR_BUTTON_MESSAGE.with(Cell::get);
    if taskbar_button_message == 0 || message != taskbar_button_message || window.is_null() {
        return;
    }
    // Explorer explicitly announces when this HWND's button is ready.
    // Calling SetProgress* before this message can silently do nothing.
    TASKBAR_BUTTONS.with(|buttons| {
        let mut buttons = buttons.borrow_mut();
        if !buttons.contains(&window) {
            buttons.push(window);
        }
    });
    TASKBAR_FORCE_REFRESH.with(|value| value.set(true));
    request_taskbar_progress_refresh();
}
unsafe extern "system" fn call_hook(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 {
        let event = unsafe { &*(lparam as *const Cwp) };
        observe_taskbar_button_created(event.window, event.message);
        if event.message == 0x0082 {
            TASKBAR_BUTTONS.with(|buttons| {
                buttons
                    .borrow_mut()
                    .retain(|window| *window != event.window);
            });
            TASKBAR_ACTIVE_WINDOWS.with(|windows| {
                windows
                    .borrow_mut()
                    .retain(|window| *window != event.window);
            });
        }
        if matches!(event.message, 0x0018 | 0x0082)
            && unsafe { GetAncestor(event.window, 2) } == event.window
        {
            request_taskbar_progress_refresh();
        }
        // Observe our settings and native popup windows too: main-window blur
        // alone misses settings -> another-app activation and internal flyouts.
        if (event.message == 0x0006 && event.wparam & 0xffff == 0)
            || (event.message == 0x001c && event.wparam == 0)
            || event.message == 0x0008
        {
            schedule_focus_check();
        }
        if event.message == 0x000c && event.lparam != 0 {
            for (name, popup) in [(APP_NAME, true), (SETTINGS_TITLE, false)] {
                let expected = wide(name);
                let matches = expected
                    .iter()
                    .enumerate()
                    .all(|(i, value)| unsafe { *((event.lparam as *const u16).add(i)) == *value });
                if matches {
                    if popup {
                        attach_popup(event.window);
                    }
                    set_window_icon(event.window);
                    request_taskbar_progress_refresh();
                    break;
                }
            }
        }
        if matches!(event.message, 0x010d | 0x010e | 0x010f)
            && unsafe { GetAncestor(event.window, 2) } == POPUP.with(Cell::get)
        {
            if event.message == 0x010d {
                COMPOSING.with(|v| v.set(true));
                emit(Action::CompositionStarted);
            } else if event.message == 0x010e {
                COMPOSING.with(|v| v.set(false));
                emit(Action::CompositionEnded);
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}
unsafe extern "system" fn cbt_hook(code: i32, wparam: usize, lparam: isize) -> isize {
    if code == 3 && POPUP.with(Cell::get).is_null() {
        let mut class = [0_u16; 128];
        let len = unsafe { GetClassNameW(wparam as Hwnd, class.as_mut_ptr(), class.len() as i32) };
        if String::from_utf16_lossy(&class[..len.max(0) as usize]) == "WinUIDesktopWin32WindowClass"
        {
            attach_popup(wparam as Hwnd);
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}
unsafe extern "system" fn key_hook(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 && wparam == 1 {
        let message = unsafe { &mut *(lparam as *mut Msg) };
        // TaskbarButtonCreated can be posted. WH_CALLWNDPROC observes sent
        // messages; consume queued notifications through WH_GETMESSAGE too,
        // once PM_REMOVE confirms the UI thread is taking the message.
        observe_taskbar_button_created(message.window, message.message);
        if message.message == 0x0100
            && message.wparam == 0x1b
            && !is_composing()
            && let Some(popup) = window(APP_NAME)
            && unsafe { GetAncestor(message.window, 2) } == popup
        {
            message.message = 0;
            ESCAPE.with(|handler| {
                if let Some(handler) = handler.borrow().as_ref() {
                    handler();
                }
            });
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}
fn tray(window: Hwnd, operation: u32) -> bool {
    let mut data: NotifyIcon = unsafe { std::mem::zeroed() };
    data.size = std::mem::size_of::<NotifyIcon>() as u32;
    data.window = window;
    data.id = 1;
    data.flags = 1 | 2 | 4;
    data.callback = TRAY_MESSAGE;
    data.icon = app_icon(unsafe { GetSystemMetricsForDpi(49, GetDpiForSystem()) }.max(1));
    for (to, from) in data
        .tip
        .iter_mut()
        .zip("WinTranslater · Ctrl+Tab".encode_utf16())
    {
        *to = from;
    }
    unsafe { Shell_NotifyIconW(operation, &data) != 0 }
}
unsafe extern "system" fn sink_proc(
    window: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if message == TASKBAR_PROGRESS_MESSAGE {
        TASKBAR_PROGRESS_POSTED.store(false, Ordering::Release);
        update_taskbar_progress(window);
        return 0;
    }
    if message == 0x0113 && wparam == TASKBAR_PROGRESS_TIMER {
        update_taskbar_progress(window);
        return 0;
    }
    if message == 0x0312 && wparam == HOTKEY_ID as usize {
        remember_activation_source();
        crate::desktop::remember_foreground();
        emit(Action::TranslateSelection);
        return 0;
    }
    if message == SHOW_MESSAGE {
        remember_activation_source();
        crate::desktop::remember_foreground();
        emit(if wparam == 1 {
            Action::Settings
        } else {
            Action::Show
        });
        return 0;
    }
    if message == TRAY_MESSAGE {
        if lparam as u32 == 0x0202 {
            remember_activation_source();
            crate::desktop::remember_foreground();
            emit(Action::Show);
        } else if matches!(lparam as u32, 0x0205 | 0x007b) {
            remember_activation_source();
            crate::desktop::remember_foreground();
            let mut point = Point { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&mut point);
                let menu = CreatePopupMenu();
                AppendMenuW(menu, 0, 1, wide("打开 · Ctrl+Tab").as_ptr());
                AppendMenuW(menu, 0, 2, wide("设置").as_ptr());
                AppendMenuW(menu, 0x0800, 0, std::ptr::null());
                AppendMenuW(menu, 0, 3, wide("退出").as_ptr());
                SetForegroundWindow(window);
                let selected = TrackPopupMenu(
                    menu,
                    0x0100 | 0x0002,
                    point.x,
                    point.y,
                    0,
                    window,
                    std::ptr::null(),
                );
                DestroyMenu(menu);
                match selected {
                    1 => emit(Action::Show),
                    2 => emit(Action::Settings),
                    3 => emit(Action::Quit),
                    _ => {}
                }
                PostMessageW(window, 0, 0, 0);
            }
        }
        return 0;
    }
    if message != 0 && message == TASKBAR_MESSAGE.with(Cell::get) {
        tray(window, 0);
        TASKBAR_BUTTONS.with(|buttons| buttons.borrow_mut().clear());
        TASKBAR_INTERFACE_STALE.with(|value| value.set(true));
        TASKBAR_FORCE_REFRESH.with(|value| value.set(true));
        request_taskbar_progress_refresh();
        return 0;
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}
pub struct NativeSession {
    mutex: Hwnd,
    sink: Hwnd,
    hooks: Vec<Hwnd>,
}
impl Drop for NativeSession {
    fn drop(&mut self) {
        TASKBAR_PROGRESS_SINK.store(0, Ordering::Release);
        clear_taskbar_progress();
        if !self.sink.is_null() {
            update_taskbar_progress(self.sink);
            unsafe { KillTimer(self.sink, TASKBAR_PROGRESS_TIMER) };
        }
        // Balance COM initialization on the same thread that owns the sink.
        TASKBAR_INTERFACE.with(|value| value.borrow_mut().take());
        TASKBAR_BUTTONS.with(|buttons| buttons.borrow_mut().clear());
        TASKBAR_ACTIVE_WINDOWS.with(|windows| windows.borrow_mut().clear());
        TASKBAR_APPLIED.with(|value| value.set(None));
        TASKBAR_PROGRESS_POSTED.store(false, Ordering::Release);
        unsafe {
            if !self.sink.is_null() {
                tray(self.sink, 2);
                UnregisterHotKey(self.sink, HOTKEY_ID);
                DestroyWindow(self.sink);
            }
            for hook in &self.hooks {
                UnhookWindowsHookEx(*hook);
            }
            if !self.mutex.is_null() {
                CloseHandle(self.mutex);
            }
        }
    }
}
pub fn initialize(background: bool) -> Result<Option<NativeSession>> {
    remember_activation_source();
    let mutex = unsafe {
        CreateMutexW(
            std::ptr::null_mut(),
            0,
            wide("Local\\TranslationTool.Native.CurrentUser.v1").as_ptr(),
        )
    };
    anyhow::ensure!(
        !mutex.is_null(),
        "无法初始化单实例：{}",
        std::io::Error::last_os_error()
    );
    if unsafe { GetLastError() } == 183 {
        if !background {
            for _ in 0..60 {
                let existing = unsafe {
                    FindWindowExW(
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        wide(CLASS_NAME).as_ptr(),
                        std::ptr::null(),
                    )
                };
                if !existing.is_null() {
                    unsafe {
                        let mut process = 0;
                        GetWindowThreadProcessId(existing, &mut process);
                        AllowSetForegroundWindow(process);
                        PostMessageW(
                            existing,
                            SHOW_MESSAGE,
                            usize::from(std::env::args().any(|argument| argument == "--settings")),
                            0,
                        );
                    }
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        unsafe {
            CloseHandle(mutex);
        }
        return Ok(None);
    }
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class_name = wide(CLASS_NAME);
    let class = WindowClass {
        style: 0,
        procedure: Some(sink_proc),
        class_extra: 0,
        window_extra: 0,
        instance,
        icon: app_icon(unsafe { GetSystemMetricsForDpi(11, GetDpiForSystem()) }.max(1)),
        cursor: std::ptr::null_mut(),
        background: std::ptr::null_mut(),
        menu_name: std::ptr::null(),
        class_name: class_name.as_ptr(),
    };
    let mut session = NativeSession {
        mutex,
        sink: std::ptr::null_mut(),
        hooks: Vec::new(),
    };
    anyhow::ensure!(
        unsafe { RegisterClassW(&class) } != 0,
        "无法创建后台窗口类：{}",
        std::io::Error::last_os_error()
    );
    // An invisible tool window also receives TaskbarCreated broadcasts after Explorer restarts.
    session.sink = unsafe {
        CreateWindowExW(
            0x00000080,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null_mut(),
        )
    };
    anyhow::ensure!(
        !session.sink.is_null(),
        "无法创建后台消息窗口：{}",
        std::io::Error::last_os_error()
    );
    SINK.with(|value| value.set(session.sink));
    TASKBAR_PROGRESS_SINK.store(session.sink as usize, Ordering::Release);
    if unsafe { RegisterHotKey(session.sink, HOTKEY_ID, 0x0002 | 0x4000, 0x09) } == 0 {
        STARTUP_WARNING.with(|v| {
            *v.borrow_mut() = Some(format!(
                "Ctrl+Tab 快捷键注册失败，可能已被其他程序占用。可从托盘打开 WinTranslater。{}",
                std::io::Error::last_os_error()
            ))
        });
    }
    for (kind, callback) in [
        (
            4,
            call_hook as unsafe extern "system" fn(i32, usize, isize) -> isize,
        ),
        (5, cbt_hook),
        (3, key_hook),
    ] {
        let hook = unsafe {
            SetWindowsHookExW(kind, callback, std::ptr::null_mut(), GetCurrentThreadId())
        };
        anyhow::ensure!(
            !hook.is_null(),
            "无法初始化窗口消息：{}",
            std::io::Error::last_os_error()
        );
        session.hooks.push(hook);
    }
    TASKBAR_MESSAGE
        .with(|v| v.set(unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) }));
    TASKBAR_BUTTON_MESSAGE
        .with(|v| v.set(unsafe { RegisterWindowMessageW(wide("TaskbarButtonCreated").as_ptr()) }));
    anyhow::ensure!(
        tray(session.sink, 0),
        "无法创建通知区域图标：{}",
        std::io::Error::last_os_error()
    );
    STARTUP_WARNING.with(|v| { if let Some(warning) = v.borrow().as_ref() {
        crate::storage::diagnostic(serde_json::json!({"event":"hotkey_registration_failed", "hotkey":"Ctrl+Tab", "message":warning}));
        let mut data: NotifyIcon = unsafe { std::mem::zeroed() }; data.size = std::mem::size_of::<NotifyIcon>() as u32; data.window = session.sink; data.id = 1; data.flags = 0x10; data.info_flags = 2;
        for (to, from) in data.info.iter_mut().take(255).zip(warning.encode_utf16()) { *to = from; }
        for (to, from) in data.info_title.iter_mut().zip("WinTranslater：快捷键不可用".encode_utf16()) { *to = from; }
        unsafe { Shell_NotifyIconW(1, &data); }
    } });
    if std::env::args().any(|argument| argument == "--settings") {
        emit(Action::Settings);
    }
    Ok(Some(session))
}
pub fn take_startup_warning() -> Option<String> {
    STARTUP_WARNING.with(|v| v.borrow_mut().take())
}
pub fn install_handler(handler: impl Fn(Action) + 'static) {
    ACTION.with(|v| *v.borrow_mut() = Some(Box::new(handler)));
    if let Some(action) = PENDING_ACTION.with(Cell::take) {
        emit(action);
    }
}
// No WinUI Application, Window, or Activate exists on the silent startup path.
// The first explicit open action is what starts the WinUI application in main().
pub fn wait_for_activation() -> Result<bool> {
    loop {
        if let Some(action) = PENDING_ACTION.with(Cell::take) {
            match action {
                Action::Quit => return Ok(false),
                Action::Show => return Ok(true),
                Action::TranslateSelection => {
                    // main captures the current foreground selection before
                    // creating/activating the first WinUI window.
                    PENDING_ACTION.with(|value| value.set(Some(Action::TranslateSelection)));
                    return Ok(true);
                }
                Action::Settings => {
                    PENDING_ACTION.with(|v| v.set(Some(Action::Settings)));
                    return Ok(true);
                }
                _ => {}
            }
        }
        let mut message: Msg = unsafe { std::mem::zeroed() };
        let result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
        if result == 0 {
            return Ok(false);
        }
        anyhow::ensure!(
            result > 0,
            "后台消息循环失败：{}",
            std::io::Error::last_os_error()
        );
        unsafe {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
pub fn take_startup_selection_request() -> bool {
    PENDING_ACTION.with(|value| match value.take() {
        Some(Action::TranslateSelection) => true,
        other => {
            value.set(other);
            false
        }
    })
}
pub fn configure_popup_with_key(width: f64, height: f64, transition_key: u64) {
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return;
    }
    SIZE.with(|v| v.set((width, height)));
    TRANSITION_KEY.with(|value| value.set(transition_key));
    if let Some(window) = window(APP_NAME) {
        unsafe {
            attach_popup(window);
            set_window_icon(window);
            let style = GetWindowLongPtrW(window, -16);
            let popup_style =
                (style & !(0x00C00000 | 0x00040000 | 0x00080000 | 0x00030000)) | 0x02000000;
            let frame_changed = style != popup_style;
            if frame_changed {
                SetWindowLongPtrW(window, -16, popup_style);
            }
            let first = !INITIALIZED.with(Cell::get);
            if first {
                let round: u32 = 1; // DWMWCP_DONOTROUND: round the animated card in XAML.
                DwmSetWindowAttribute(window, 33, &round as *const _ as _, 4);
                let border: u32 = 0xffff_fffe; // DWMWA_COLOR_NONE: no stationary host edge.
                DwmSetWindowAttribute(window, 34, &border as *const _ as _, 4);
            }
            // Activate the frameless client calculation before FRAMECHANGED
            // sends WM_NCCALCSIZE for the first configured rectangle.
            INITIALIZED.with(|v| v.set(true));
            // If the presenter ever restores a frame style, clear its cached
            // non-client metrics too; SetWindowLongPtr alone does not do this.
            resize_popup(window, first || frame_changed);
        }
    }
}
fn prepare_activation(window: Hwnd) -> bool {
    let ready = crate::desktop::move_to_remembered_desktop(window as usize);
    if !ready {
        crate::storage::diagnostic(serde_json::json!({
            "event": "desktop_activation_deferred",
            "reason": "window_not_confirmed_on_current_desktop"
        }));
    }
    ready
}

/// Share the current foreground queue only during the requested activation.
/// UIA selection capture and a committed XAML frame can outlive the hotkey's
/// foreground permission. Attaching here makes the one SetForegroundWindow
/// synchronous with that queue; it does not inject input or change desktops.
struct InputQueueAttachment {
    foreground_thread: u32,
    attached: bool,
}
impl InputQueueAttachment {
    fn new() -> Self {
        let foreground = unsafe { GetForegroundWindow() };
        let foreground_thread =
            unsafe { GetWindowThreadProcessId(foreground, std::ptr::null_mut()) };
        let current_thread = unsafe { GetCurrentThreadId() };
        Self {
            foreground_thread,
            attached: foreground_thread != 0
                && foreground_thread != current_thread
                && unsafe { AttachThreadInput(current_thread, foreground_thread, 1) } != 0,
        }
    }
}
impl Drop for InputQueueAttachment {
    fn drop(&mut self) {
        if self.attached {
            unsafe { AttachThreadInput(GetCurrentThreadId(), self.foreground_thread, 0) };
        }
    }
}

fn activate_window(window: Hwnd) -> bool {
    activate_window_with_intent(window, None)
}

fn activate_window_with_intent(window: Hwnd, intent: Option<ActivationVerification>) -> bool {
    let permitted = || {
        intent.is_none_or(|check| {
            let presentation = PRESENTATION.with(Cell::get);
            let foreground = unsafe { GetForegroundWindow() };
            presentation.show_is_current(check.revision)
                && (foreground == window
                    || check.may_retry(
                        presentation,
                        foreground,
                        SINK.with(Cell::get),
                        Instant::now(),
                    ))
        })
    };
    if !permitted() {
        return false;
    }
    if !prepare_activation(window) {
        return false;
    }
    set_window_icon(window);
    // Desktop preparation can dispatch work. Recheck the explicit open before
    // changing visibility or foreground.
    if !permitted() {
        return false;
    }
    unsafe {
        // Explicit visibility is independent of WinUI's initial Activate and
        // STARTUPINFO's initial show command. The desktop and final viewport
        // have both been committed before this nonactivating show.
        SetWindowPos(
            window,
            if window == POPUP.with(Cell::get) {
                -1_isize as Hwnd // HWND_TOPMOST, only while this popup is active.
            } else {
                std::ptr::null_mut() // HWND_TOP
            },
            0,
            0,
            0,
            0,
            0x0001 | 0x0002 | 0x0010 | 0x0040,
        );
        // The hotkey or a launching instance normally grants foreground rights.
        // Let USER deliver the activation to WinUI on its own input queue first.
        SetForegroundWindow(window);
    }
    if unsafe { GetForegroundWindow() } != window && permitted() {
        let attachment = InputQueueAttachment::new();
        if permitted() {
            unsafe { SetForegroundWindow(window) };
        }
        drop(attachment);
    }
    (unsafe { IsWindowVisible(window) }) != 0 && unsafe { GetForegroundWindow() } == window
}

fn activate_popup(window: Hwnd) {
    cancel_activation_check(window);
    let revision = PRESENTATION.with(Cell::get).revision;
    // Hold off blur hiding until the foreground handoff crosses the UI message
    // boundary. Key releases and pointer motion belong to the same explicit
    // open; a short deadline and source HWND bound its activation permission.
    let check = ActivationVerification {
        revision,
        source: ACTIVATION_SOURCE.with(Cell::get),
        started_at: Instant::now(),
        retries: 0,
        checks: 0,
    };
    ACTIVATION_CHECK.with(|value| value.set(Some(check)));
    let activated = activate_window_with_intent(window, Some(check));
    let foreground = unsafe { GetForegroundWindow() };
    POPUP_PRESENTED.with(|value| value.set(unsafe { IsWindowVisible(window) } != 0));
    crate::storage::diagnostic(serde_json::json!({
        "event": "popup_activation",
        "visible": unsafe { IsWindowVisible(window) } != 0,
        "foreground": activated,
        "source_is_null": check.source.is_null(),
        "foreground_is_source": foreground == check.source,
        "foreground_is_sink": foreground == SINK.with(Cell::get),
        "retry_denied": check.retry_denied_reason(
            PRESENTATION.with(Cell::get), foreground, SINK.with(Cell::get), Instant::now(),
        ).filter(|_| foreground != window),
    }));
    if POPUP_PRESENTED.with(Cell::get) {
        // Reveal visible content even if Windows rejects foreground activation.
        // The UI requests TextBox focus only after confirming foreground.
        let presentation = PRESENTATION.with(Cell::get);
        if presentation.show_requested {
            emit(Action::PopupActivated(presentation.revision));
        }
    }
    if unsafe { PostMessageW(window, VERIFY_ACTIVATION_MESSAGE, revision as usize, 0) } == 0 {
        schedule_activation_verification(window);
    }
}

fn remember_activation_source() {
    let foreground = unsafe { GetForegroundWindow() };
    // Settings is a legitimate source too. Preserve the external source only
    // when the popup itself receives another hotkey while already foreground.
    if foreground != POPUP.with(Cell::get) || foreground.is_null() {
        ACTIVATION_SOURCE.with(|value| value.set(foreground));
    }
}

fn schedule_activation_verification(window: Hwnd) {
    if unsafe { SetTimer(window, ACTIVATION_TIMER, ACTIVATION_DELAY_MS, None) } == 0 {
        cancel_activation_check(window);
        release_popup_layer(window);
        schedule_focus_check();
    }
}

fn cancel_activation_check(window: Hwnd) {
    ACTIVATION_CHECK.with(|value| value.set(None));
    unsafe { KillTimer(window, ACTIVATION_TIMER) };
}

fn release_popup_layer(window: Hwnd) {
    unsafe {
        SetWindowPos(
            window,
            -2_isize as Hwnd,
            0,
            0,
            0,
            0,
            0x0001 | 0x0002 | 0x0010,
        );
    }
}

fn verify_popup_activation(window: Hwnd, revision: u64) {
    let Some(mut check) = ACTIVATION_CHECK.with(Cell::get) else {
        return;
    };
    if check.revision != revision {
        return;
    }
    unsafe { KillTimer(window, ACTIVATION_TIMER) };
    let presentation = PRESENTATION.with(Cell::get);
    if !presentation.show_is_current(revision) {
        cancel_activation_check(window);
        return;
    }
    let foreground = unsafe { GetForegroundWindow() };
    if foreground == window && unsafe { IsWindowVisible(window) } != 0 {
        if check.checks == 0 {
            check.checks = 1;
            ACTIVATION_CHECK.with(|value| value.set(Some(check)));
            schedule_activation_verification(window);
            return;
        }
        cancel_activation_check(window);
        crate::storage::diagnostic(serde_json::json!({
            "event":"popup_activation_verified", "foreground":true, "retries":check.retries,
        }));
        emit(Action::PopupFocusReady(revision));
        return;
    }
    let now = Instant::now();
    let denied = check.retry_denied_reason(presentation, foreground, SINK.with(Cell::get), now);
    if denied.is_none() {
        let attempt = check;
        check.retries += 1;
        // A previous foreground observation cannot count toward a stable
        // handoff after it has returned to the source or the tray menu sink.
        check.checks = 0;
        ACTIVATION_CHECK.with(|value| value.set(Some(check)));
        let activated = activate_window_with_intent(window, Some(attempt));
        crate::storage::diagnostic(serde_json::json!({
            "event":"popup_activation_retry", "foreground":activated, "attempt":check.retries,
            "elapsed_ms":now.saturating_duration_since(check.started_at).as_millis(),
            "foreground_is_source":foreground == check.source,
            "foreground_is_sink":foreground == SINK.with(Cell::get),
        }));
        let visible = unsafe { IsWindowVisible(window) } != 0;
        let was_presented = POPUP_PRESENTED.with(|value| value.replace(visible));
        if visible && !was_presented {
            emit(Action::PopupActivated(revision));
        }
        schedule_activation_verification(window);
    } else {
        cancel_activation_check(window);
        crate::storage::diagnostic(serde_json::json!({
            "event":"popup_activation_verified", "foreground":false, "retries":check.retries,
            "reason":denied,
            "elapsed_ms":now.saturating_duration_since(check.started_at).as_millis(),
            "foreground_is_source":foreground == check.source,
            "foreground_is_sink":foreground == SINK.with(Cell::get),
        }));
        release_popup_layer(window);
        schedule_focus_check();
    }
}

/// A native Composition show request must still own this presentation when its
/// UI message is consumed. A later hide/reopen makes an older request obsolete.
pub fn popup_show_animation_is_current(revision: u64) -> bool {
    PRESENTATION.with(Cell::get).show_is_current(revision)
        && POPUP_PRESENTED.with(Cell::get)
        && !POPUP.with(Cell::get).is_null()
}

pub fn popup_hide_animation_is_current(revision: u64) -> bool {
    PRESENTATION.with(Cell::get).hide_is_current(revision)
}

pub fn popup_is_foreground() -> bool {
    let popup = POPUP.with(Cell::get);
    !popup.is_null()
        && unsafe { IsWindowVisible(popup) } != 0
        && unsafe { GetForegroundWindow() } == popup
}

/// Confirm native activation before letting ContentIsland focus the TextBox.
pub fn prepare_popup_input_focus() -> bool {
    if !popup_is_foreground() {
        return false;
    }
    let popup = POPUP.with(Cell::get);
    // ContentIsland owns the keyboard target. Focusing the top-level HWND here
    // would take focus back from the input site initialized by WinUI.
    unsafe { SetActiveWindow(popup) };
    popup_is_foreground()
}

/// Reconcile a foreground handoff that happened while USER input queues were
/// attached. Such a handoff can leave ContentIsland's activation listener in
/// Deactivated even though both GetForegroundWindow and GetFocus look correct.
/// A real, synchronous activation through an owned zero-sized tool window delivers
/// the missing native transition. No other application's window is activated.
/// This runs after the first loaded focus attempt and before revealing the card.
pub fn recover_popup_input_activation(revision: u64) -> bool {
    if !popup_show_animation_is_current(revision)
        || !popup_is_foreground()
        || INPUT_ACTIVATION_RECOVERY.with(Cell::get) == Some(revision)
    {
        return false;
    }
    let popup = POPUP.with(Cell::get);
    // Create this relay on the current desktop, owned by the already-positioned
    // popup. Reusing the background tray HWND could activate its older desktop.
    let relay = unsafe {
        CreateWindowExW(
            0x80, // WS_EX_TOOLWINDOW: no taskbar or Alt+Tab entry.
            wide("STATIC").as_ptr(),
            std::ptr::null(),
            0x80000000, // WS_POPUP, with no caption, border or visible pixels.
            -32000,
            -32000,
            0,
            0,
            popup,
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null_mut(),
        )
    };
    if relay.is_null() {
        return false;
    }
    INPUT_ACTIVATION_RECOVERY.with(|value| value.set(Some(revision)));
    unsafe {
        SetWindowPos(
            relay,
            std::ptr::null_mut(),
            -32000,
            -32000,
            0,
            0,
            0x0010 | 0x0004 | 0x0040,
        );
        if popup_is_foreground() && popup_show_animation_is_current(revision) {
            SetForegroundWindow(relay);
            if GetForegroundWindow() == relay && popup_show_animation_is_current(revision) {
                SetForegroundWindow(popup);
            }
        }
        DestroyWindow(relay);
    }
    let restored = popup_is_foreground();
    crate::storage::diagnostic(serde_json::json!({
        "event":"popup_input_activation_recovered", "foreground":restored,
    }));
    restored
}

pub fn popup_has_keyboard_focus() -> bool {
    let popup = POPUP.with(Cell::get);
    let focused = unsafe { GetFocus() };
    !popup.is_null() && !focused.is_null() && unsafe { GetAncestor(focused, 2) } == popup
}

pub fn show_popup() {
    ANCHOR.with(|v| v.set(mouse_monitor()));
    let reversing_hide = PRESENTATION.with(Cell::get).hide_revision.is_some();
    // Invalidate old Completion callbacks before activating or publishing a new
    // frame. A rapid hotkey can safely reverse a native Composition hide.
    update_presentation(PopupPresentation::show);
    if let Some(window) = window(APP_NAME) {
        cancel_activation_check(window);
        cancel_focus_check(window);
        if POPUP_PRESENTED.with(Cell::get)
            && unsafe { IsWindowVisible(window) } != 0
            && unsafe { MonitorFromWindow(window, 2) } == ANCHOR.with(Cell::get)
        {
            if reversing_hide {
                resize_popup(window, false);
            }
            // Re-activating an already visible popup must not snap an active
            // expansion to its destination. Its pending UI frame remains valid.
            activate_popup(window);
            return;
        }
        // Hidden/first/cross-monitor presentation is a single final frame, but
        // ShowWindow still waits for the matching UI viewport to be committed.
        SHOW_AFTER_FRAME.with(|value| value.set(true));
        stop_resize(window);
        resize_popup(window, true);
    }
}
pub fn hide_popup() {
    SHOW_AFTER_FRAME.with(|value| value.set(false));
    if let Some(window) = window(APP_NAME) {
        cancel_activation_check(window);
        release_popup_layer(window);
        cancel_focus_check(window);
        stop_resize(window);
        let visible = unsafe { IsWindowVisible(window) } != 0;
        if let Some(revision) = update_presentation(|state| state.hide(visible)) {
            // The UI starts a WinUI Composition visual animation; the HWND and
            // its backdrop stay alive until that animation's completion arrives.
            emit(Action::PopupDismissed(revision));
        } else if !visible {
            POPUP_PRESENTED.with(|value| value.set(false));
        }
    } else {
        update_presentation(|state| state.hide(false));
    }
}

/// Called on the UI thread by the native visual animation's completion callback.
/// Only the matching hide request can remove the HWND; reopening cancels it.
pub fn complete_popup_hide(revision: u64) {
    if !update_presentation(|state| state.complete_hide(revision)) {
        return;
    }
    SHOW_AFTER_FRAME.with(|value| value.set(false));
    if let Some(window) = window(APP_NAME) {
        cancel_activation_check(window);
        release_popup_layer(window);
        cancel_focus_check(window);
        stop_resize(window);
        unsafe {
            ShowWindow(window, 0);
        }
        POPUP_PRESENTED.with(|value| value.set(false));
    }
}
pub fn drag_popup() {
    if let Some(window) = window(APP_NAME) {
        stop_resize(window);
        unsafe {
            ReleaseCapture();
            SendMessageW(window, 0x00A1, 2, 0);
        }
    }
}
pub fn activate_settings() -> bool {
    if let Some(window) = window(SETTINGS_TITLE) {
        activate_window(window);
        true
    } else {
        false
    }
}
pub fn escape_handler(handler: impl Fn() + 'static) {
    ESCAPE.with(|value| *value.borrow_mut() = Some(Box::new(handler)));
}
pub fn is_composing() -> bool {
    let focused = unsafe { GetFocus() };
    if focused.is_null() || unsafe { GetAncestor(focused, 2) } != POPUP.with(Cell::get) {
        return false;
    }
    if COMPOSING.with(Cell::get) {
        return true;
    }
    // WinUI uses TSF; IMM alone does not cover modern Microsoft Pinyin composition.
    if tsf_is_composing().unwrap_or(false) {
        return true;
    }
    unsafe {
        let window = GetFocus();
        if window.is_null() {
            return false;
        }
        let context = ImmGetContext(window);
        if context.is_null() {
            return false;
        }
        let active = ImmGetCompositionStringW(context, 0x0008, std::ptr::null_mut(), 0) > 0;
        ImmReleaseContext(window, context);
        active
    }
}

#[repr(C)]
struct Guid {
    first: u32,
    second: u16,
    third: u16,
    last: [u8; 8],
}
type ComOut = unsafe extern "system" fn(Hwnd, *mut Hwnd) -> i32;
#[repr(C)]
struct UnknownTable {
    query: unsafe extern "system" fn(Hwnd, *const Guid, *mut Hwnd) -> i32,
    add_ref: unsafe extern "system" fn(Hwnd) -> u32,
    release: unsafe extern "system" fn(Hwnd) -> u32,
}
#[repr(C)]
struct TaskbarList3Table {
    unknown: UnknownTable,
    hr_init: unsafe extern "system" fn(Hwnd) -> i32,
    add_tab: usize,
    delete_tab: usize,
    activate_tab: usize,
    set_active_alt: usize,
    mark_fullscreen_window: usize,
    set_progress_value: unsafe extern "system" fn(Hwnd, Hwnd, u64, u64) -> i32,
    set_progress_state: unsafe extern "system" fn(Hwnd, Hwnd, u32) -> i32,
}
#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: Hwnd, flags: u32) -> i32;
    fn CoUninitialize();
    fn CoCreateInstance(
        class: *const Guid,
        outer: Hwnd,
        context: u32,
        iid: *const Guid,
        object: *mut Hwnd,
    ) -> i32;
}
struct TaskbarApartment {
    initialized: bool,
}
impl Drop for TaskbarApartment {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { CoUninitialize() };
        }
    }
}
struct TaskbarInterface {
    // Fields drop in declaration order: release the COM object before balancing
    // our successful CoInitializeEx (including S_FALSE on an existing STA).
    object: Com,
    _apartment: TaskbarApartment,
}
fn taskbar_com_failure(stage: &'static str, result: i32) {
    if TASKBAR_COM_FAILURE.with(|value| value.replace(Some((stage, result))))
        != Some((stage, result))
    {
        crate::storage::diagnostic(serde_json::json!({
            "event": "taskbar_progress_com_failed",
            "stage": stage,
            "hresult": format!("0x{:08X}", result as u32),
            "thread": unsafe { GetCurrentThreadId() },
        }));
    }
}
impl TaskbarInterface {
    fn new() -> Option<Self> {
        const RPC_E_CHANGED_MODE: i32 = 0x80010106_u32 as i32;
        let apartment_result = unsafe { CoInitializeEx(std::ptr::null_mut(), 2) };
        if apartment_result < 0 && apartment_result != RPC_E_CHANGED_MODE {
            taskbar_com_failure("CoInitializeEx", apartment_result);
            return None;
        }
        let apartment = TaskbarApartment {
            initialized: apartment_result >= 0,
        };
        let class = Guid {
            first: 0x56fdf344,
            second: 0xfd6d,
            third: 0x11d0,
            last: [0x95, 0x8a, 0x00, 0x60, 0x97, 0xc9, 0xa0, 0x90],
        };
        let iid = Guid {
            first: 0xea1afb91,
            second: 0x9e28,
            third: 0x4b86,
            last: [0x90, 0xe9, 0x9e, 0x9f, 0x8a, 0x5e, 0xef, 0xaf],
        };
        let mut create_result = 0;
        let object = Com::out(|out| {
            create_result = unsafe { CoCreateInstance(&class, std::ptr::null_mut(), 1, &iid, out) };
            create_result
        });
        let Some(object) = object else {
            taskbar_com_failure("CoCreateInstance", create_result);
            return None;
        };
        let init_result = unsafe { (object.table::<TaskbarList3Table>().hr_init)(object.0) };
        if init_result < 0 {
            taskbar_com_failure("ITaskbarList3::HrInit", init_result);
            return None;
        }
        TASKBAR_COM_FAILURE.with(|value| value.set(None));
        crate::storage::diagnostic(serde_json::json!({
            "event": "taskbar_progress_initialized",
            "thread": unsafe { GetCurrentThreadId() },
            "apartment_hresult": format!("0x{:08X}", apartment_result as u32),
            "create_hresult": format!("0x{:08X}", create_result as u32),
            "init_hresult": format!("0x{:08X}", init_result as u32),
        }));
        Some(Self {
            object,
            _apartment: apartment,
        })
    }
}
#[repr(C)]
struct ThreadTable {
    unknown: UnknownTable,
    activate: usize,
    deactivate: usize,
    create_document: usize,
    enum_documents: usize,
    get_focus: ComOut,
}
#[repr(C)]
struct DocumentTable {
    unknown: UnknownTable,
    create_context: usize,
    push: usize,
    pop: usize,
    get_top: ComOut,
}
#[repr(C)]
struct CompositionTable {
    unknown: UnknownTable,
    start: usize,
    enumerate: ComOut,
}
#[repr(C)]
struct EnumerationTable {
    unknown: UnknownTable,
    clone: usize,
    next: unsafe extern "system" fn(Hwnd, u32, *mut Hwnd, *mut u32) -> i32,
}
struct Com(Hwnd);
impl Com {
    fn out(call: impl FnOnce(*mut Hwnd) -> i32) -> Option<Self> {
        let mut object = std::ptr::null_mut();
        if call(&mut object) >= 0 && !object.is_null() {
            Some(Self(object))
        } else {
            None
        }
    }
    unsafe fn table<T>(&self) -> &T {
        unsafe { &**(self.0 as *const *const T) }
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            (self.table::<UnknownTable>().release)(self.0);
        }
    }
}
fn tsf_is_composing() -> Option<bool> {
    unsafe {
        static GET_THREAD: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let address = *GET_THREAD.get_or_init(|| {
            let module =
                LoadLibraryExW(wide("msctf.dll").as_ptr(), std::ptr::null_mut(), 0x00000800);
            if module.is_null() {
                0
            } else {
                GetProcAddress(module, c"TF_GetThreadMgr".as_ptr().cast()) as usize
            }
        });
        if address == 0 {
            return None;
        }
        let get_thread: unsafe extern "system" fn(*mut Hwnd) -> i32 = std::mem::transmute(address);
        let thread = Com::out(|out| get_thread(out))?;
        let document = Com::out(|out| (thread.table::<ThreadTable>().get_focus)(thread.0, out))?;
        let context = Com::out(|out| (document.table::<DocumentTable>().get_top)(document.0, out))?;
        let iid = Guid {
            first: 0xd40c8aae,
            second: 0xac92,
            third: 0x4fc7,
            last: [0x9a, 0x11, 0x0e, 0xe0, 0xe2, 0x3a, 0xa3, 0x9b],
        };
        let composition =
            Com::out(|out| (context.table::<UnknownTable>().query)(context.0, &iid, out))?;
        let enumeration = Com::out(|out| {
            (composition.table::<CompositionTable>().enumerate)(composition.0, out)
        })?;
        let mut count = 0;
        let view = Com::out(|out| {
            (enumeration.table::<EnumerationTable>().next)(enumeration.0, 1, out, &mut count)
        });
        Some(count > 0 && view.is_some())
    }
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegCreateKeyExW(
        key: Hwnd,
        subkey: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        security: Hwnd,
        result: *mut Hwnd,
        disposition: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        key: Hwnd,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        size: u32,
    ) -> i32;
    fn RegDeleteValueW(key: Hwnd, name: *const u16) -> i32;
    fn RegCloseKey(key: Hwnd) -> i32;
}
pub fn set_autostart(enabled: bool) -> Result<()> {
    let command = if enabled {
        Some(wide(&format!(
            "\"{}\" --background",
            std::env::current_exe()
                .context("无法读取程序路径")?
                .display()
        )))
    } else {
        None
    };
    let mut key = std::ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            0x80000001_u32 as i32 as isize as Hwnd,
            wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run").as_ptr(),
            0,
            std::ptr::null(),
            0,
            0x0002,
            std::ptr::null_mut(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    anyhow::ensure!(status == 0, "{}", std::io::Error::from_raw_os_error(status));
    let name = wide("TranslationTool");
    let result = if let Some(command) = command {
        unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                1,
                command.as_ptr().cast(),
                (command.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteValueW(key, name.as_ptr()) }
    };
    unsafe {
        RegCloseKey(key);
    }
    anyhow::ensure!(
        result == 0 || (!enabled && result == 2),
        "{}",
        std::io::Error::from_raw_os_error(result)
    );
    Ok(())
}

#[cfg(test)]
mod presentation_tests {
    use super::*;

    #[test]
    fn posted_taskbar_button_notification_reaches_progress_tracking() {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn PeekMessageW(
                message: *mut Msg,
                window: Hwnd,
                minimum: u32,
                maximum: u32,
                remove: u32,
            ) -> i32;
        }
        struct NotificationTest {
            window: Hwnd,
            hook: Hwnd,
            old_message: u32,
            old_buttons: Vec<Hwnd>,
            old_force: bool,
        }
        impl Drop for NotificationTest {
            fn drop(&mut self) {
                unsafe {
                    if !self.hook.is_null() {
                        UnhookWindowsHookEx(self.hook);
                    }
                    if !self.window.is_null() {
                        DestroyWindow(self.window);
                    }
                }
                TASKBAR_BUTTON_MESSAGE.with(|value| value.set(self.old_message));
                TASKBAR_BUTTONS.with(|buttons| {
                    buttons.replace(std::mem::take(&mut self.old_buttons));
                });
                TASKBAR_FORCE_REFRESH.with(|value| value.set(self.old_force));
            }
        }
        let foreground = unsafe { GetForegroundWindow() };
        let notification = unsafe { RegisterWindowMessageW(wide("TaskbarButtonCreated").as_ptr()) };
        assert_ne!(notification, 0);
        let mut test = NotificationTest {
            window: std::ptr::null_mut(),
            hook: std::ptr::null_mut(),
            old_message: TASKBAR_BUTTON_MESSAGE.with(|value| value.replace(notification)),
            old_buttons: TASKBAR_BUTTONS.with(|buttons| buttons.replace(Vec::new())),
            old_force: TASKBAR_FORCE_REFRESH.with(|value| value.replace(false)),
        };
        test.window = unsafe {
            CreateWindowExW(
                0x80,
                wide("STATIC").as_ptr(),
                std::ptr::null(),
                0x80000000,
                0,
                0,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null_mut(),
            )
        };
        assert!(!test.window.is_null());
        test.hook =
            unsafe { SetWindowsHookExW(3, key_hook, std::ptr::null_mut(), GetCurrentThreadId()) };
        assert!(!test.hook.is_null());
        assert_ne!(unsafe { PostMessageW(test.window, notification, 0, 0) }, 0);
        let mut message: Msg = unsafe { std::mem::zeroed() };
        // Merely peeking must not consume the button's readiness notification.
        assert_ne!(
            unsafe { PeekMessageW(&mut message, test.window, notification, notification, 0) },
            0
        );
        assert!(TASKBAR_BUTTONS.with(|buttons| buttons.borrow().is_empty()));
        assert_ne!(
            unsafe { PeekMessageW(&mut message, test.window, notification, notification, 1) },
            0
        );
        assert!(TASKBAR_BUTTONS.with(|buttons| buttons.borrow().contains(&test.window)));
        assert!(TASKBAR_FORCE_REFRESH.with(Cell::get));
        assert_eq!(message.message, notification);
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
    }

    #[test]
    fn transparent_slide_track_reaches_taskbar_without_changing_card_size() {
        let work = Rect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1040,
        };
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let bounds = popup_bounds(work, scale, 480.0);
            let margin = (16.0 * scale).round() as i32;
            assert_eq!(bounds.bottom, work.bottom);
            assert_eq!(bounds.right, work.right);
            assert_eq!(
                bounds.right - bounds.left,
                (480.0 * scale).round() as i32 + 2 * margin
            );
            assert_eq!(
                bounds.top,
                work.bottom - 2 * margin - (POPUP_MAX_HEIGHT * scale).round() as i32
            );
            assert_eq!(
                popup_viewport_extent(bounds.right - bounds.left, scale),
                480.0
            );
            assert_eq!(
                popup_viewport_extent(bounds.bottom - bounds.top, scale),
                POPUP_MAX_HEIGHT
            );
            let card = limited_viewport(bounds, scale, (480.0, 320.0));
            assert_eq!(card.height, 320.0);
            // The card still ends one gutter above the work-area bottom.
            assert_eq!(
                bounds.bottom - margin - (card.height * scale).round() as i32,
                work.bottom - margin - (320.0 * scale).round() as i32
            );
        }
    }

    #[test]
    fn large_popup_fits_work_area_and_excludes_track_from_content_viewport() {
        let work = Rect {
            left: 100,
            top: -900,
            right: 1380,
            bottom: -180,
        };
        let bounds = popup_bounds(work, 1.5, 1600.0);
        assert_eq!(bounds.left, work.left);
        assert_eq!(bounds.top, work.top);
        assert_eq!(bounds.bottom, work.bottom);
        let viewport = popup_viewport_extent(bounds.bottom - bounds.top, 1.5);
        assert_eq!(viewport, (720.0 - 48.0) / 1.5);
        assert_eq!(
            limited_viewport(bounds, 1.5, (1600.0, 1200.0)).height,
            viewport
        );
    }

    #[test]
    fn logical_height_reversal_preserves_the_committed_viewport() {
        let from = PopupViewport {
            width: 480.0,
            height: 172.0,
        };
        let to = PopupViewport {
            width: 480.0,
            height: 480.0,
        };
        let halfway = from.interpolate(to, 0.5);
        assert_eq!(halfway.height, 326.0);
        assert_eq!(halfway.interpolate(from, 0.0), halfway);
        assert_eq!(halfway.interpolate(from, 1.0), from);
        let work = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let host = popup_bounds(work, 1.0, 480.0);
        for height in [172.0, 200.0, 326.0, 480.0, 320.0, 172.0] {
            assert_eq!(limited_viewport(host, 1.0, (480.0, height)).height, height);
            assert_eq!(popup_bounds(work, 1.0, 480.0), host);
        }
    }

    #[test]
    fn slide_distance_uses_committed_card_height_and_both_shadow_gutters() {
        let old_size = SIZE.with(|value| value.replace((480.0, 480.0)));
        let old_frame = COMMITTED_FRAME.with(|value| {
            value.replace(Some(PopupFrame {
                revision: 1,
                serial: 1,
                transition_key: 0,
                width: 480.0,
                height: 172.0,
                target_height: 480.0,
                progress: 0.0,
                finished: false,
            }))
        });
        assert_eq!(popup_slide_distance(), 204.0);
        SIZE.with(|value| value.set(old_size));
        COMMITTED_FRAME.with(|value| value.set(old_frame));
    }

    #[test]
    fn hidden_host_region_excludes_upper_blank_area_without_moving_the_window() {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetWindowRgn(window: Hwnd, region: Hwnd) -> i32;
        }
        #[link(name = "gdi32")]
        unsafe extern "system" {
            fn GetRgnBox(region: Hwnd, bounds: *mut Rect) -> i32;
        }
        struct HiddenWindow(Hwnd);
        impl Drop for HiddenWindow {
            fn drop(&mut self) {
                unsafe {
                    DestroyWindow(self.0);
                }
            }
        }
        let foreground = unsafe { GetForegroundWindow() };
        let class = wide("STATIC");
        let window = HiddenWindow(unsafe {
            CreateWindowExW(
                0x80,
                class.as_ptr(),
                std::ptr::null(),
                0x80000000,
                100,
                100,
                512,
                512,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null_mut(),
            )
        });
        assert!(!window.0.is_null());
        let mut original: Rect = unsafe { std::mem::zeroed() };
        assert_ne!(unsafe { GetWindowRect(window.0, &mut original) }, 0);
        let scale = f64::from(unsafe { GetDpiForWindow(window.0) }.max(96)) / 96.0;
        for height in [172.0, 320.0, 480.0, 320.0, 172.0] {
            update_popup_region(window.0, height);
            let region = unsafe { CreateRectRgn(0, 0, 0, 0) };
            assert!(!region.is_null());
            let mut actual: Rect = unsafe { std::mem::zeroed() };
            assert_ne!(unsafe { GetWindowRgn(window.0, region) }, 0);
            assert_ne!(unsafe { GetRgnBox(region, &mut actual) }, 0);
            unsafe {
                DeleteObject(region);
            }
            assert_eq!(
                actual,
                Rect {
                    left: 0,
                    top: (512 - ((height + 32.0) * scale).ceil() as i32).max(0),
                    right: 512,
                    bottom: 512
                }
            );
            let mut current: Rect = unsafe { std::mem::zeroed() };
            assert_ne!(unsafe { GetWindowRect(window.0, &mut current) }, 0);
            assert_eq!(current, original);
            assert_eq!(unsafe { IsWindowVisible(window.0) }, 0);
        }
        assert_eq!(unsafe { GetForegroundWindow() }, foreground);
    }

    #[test]
    fn an_old_hide_completion_cannot_hide_a_reopened_popup() {
        let mut state = PopupPresentation::default();
        let first_show = state.show();
        assert!(state.show_is_current(first_show));
        let hide = state.hide(true).unwrap();
        assert!(!state.show_is_current(first_show));
        assert!(state.hide_is_current(hide));
        let reopened = state.show();
        assert!(!state.hide_is_current(hide));
        assert!(!state.complete_hide(hide));
        assert!(state.show_is_current(reopened));
    }

    #[test]
    fn repeated_hides_share_one_animation_and_complete_once() {
        let mut state = PopupPresentation::default();
        state.show();
        let hide = state.hide(true).unwrap();
        assert_eq!(state.hide(true), None);
        assert!(state.hide_is_current(hide));
        assert!(state.complete_hide(hide));
        assert!(!state.complete_hide(hide));
        assert!(!state.show_requested);
    }

    #[test]
    fn external_hiding_invalidates_queued_show_and_hide_callbacks() {
        let mut state = PopupPresentation::default();
        let show = state.show();
        let hide = state.hide(true).unwrap();
        state.hidden();
        assert!(!state.show_is_current(show));
        assert!(!state.hide_is_current(hide));
        assert!(!state.complete_hide(hide));
        assert_eq!(state.hide(false), None);
        assert!(!state.show_requested);
    }

    fn pending_activation(state: &mut PopupPresentation) -> ActivationVerification {
        ActivationVerification {
            revision: state.show(),
            source: 1_usize as Hwnd,
            started_at: Instant::now(),
            retries: 0,
            checks: 0,
        }
    }

    fn can_retry(
        check: ActivationVerification,
        state: PopupPresentation,
        foreground: Hwnd,
    ) -> bool {
        check.may_retry(state, foreground, 2_usize as Hwnd, check.started_at)
    }

    #[test]
    fn explicit_open_can_retry_source_null_and_its_tray_menu_sink() {
        let mut state = PopupPresentation::default();
        let check = pending_activation(&mut state);

        // Releasing Ctrl/Tab or moving the pointer does not revoke this open.
        // GetLastInputInfo cannot distinguish those events from a new intent.
        assert!(can_retry(check, state, check.source));
        assert!(can_retry(check, state, std::ptr::null_mut()));
        assert!(can_retry(check, state, 2_usize as Hwnd));
    }

    #[test]
    fn hiding_cancels_activation_retries_before_the_hide_animation_completes() {
        for animate in [false, true] {
            let mut state = PopupPresentation::default();
            let check = pending_activation(&mut state);
            state.hide(animate);

            assert!(!can_retry(check, state, check.source));
            assert!(!can_retry(check, state, std::ptr::null_mut()));
        }
    }

    #[test]
    fn reopening_cannot_revive_an_old_activation_verification() {
        let mut state = PopupPresentation::default();
        let old_check = pending_activation(&mut state);
        let hide = state.hide(true).unwrap();
        let new_check = pending_activation(&mut state);

        assert!(!can_retry(old_check, state, old_check.source));
        assert!(!state.complete_hide(hide));
        assert!(can_retry(new_check, state, new_check.source));
    }

    #[test]
    fn external_hiding_cancels_a_pending_activation_verification() {
        let mut state = PopupPresentation::default();
        let check = pending_activation(&mut state);
        state.hidden();

        assert!(!can_retry(check, state, check.source));
        assert!(!can_retry(check, state, std::ptr::null_mut()));
    }

    #[test]
    fn activation_retry_window_has_an_absolute_deadline() {
        let mut state = PopupPresentation::default();
        let check = pending_activation(&mut state);
        for foreground in [check.source, std::ptr::null_mut(), 2_usize as Hwnd] {
            assert!(check.may_retry(
                state,
                foreground,
                2_usize as Hwnd,
                check.started_at + ACTIVATION_RETRY_WINDOW - Duration::from_millis(1),
            ));
            assert!(!check.may_retry(
                state,
                foreground,
                2_usize as Hwnd,
                check.started_at + ACTIVATION_RETRY_WINDOW,
            ));
        }
    }

    #[test]
    fn another_foreground_window_cannot_be_displaced_by_an_activation_retry() {
        let mut state = PopupPresentation::default();
        let check = pending_activation(&mut state);
        let another_window = 3_usize as Hwnd;

        assert!(!can_retry(check, state, another_window));
        assert_eq!(
            check.retry_denied_reason(state, another_window, 2_usize as Hwnd, check.started_at),
            Some("foreground_changed"),
        );
    }

    #[test]
    fn a_missing_source_still_cannot_displace_an_external_foreground() {
        let mut state = PopupPresentation::default();
        let check = ActivationVerification {
            source: std::ptr::null_mut(),
            ..pending_activation(&mut state)
        };

        assert!(can_retry(check, state, std::ptr::null_mut()));
        assert!(can_retry(check, state, 2_usize as Hwnd));
        assert!(!can_retry(check, state, 3_usize as Hwnd));
    }

    #[test]
    fn activation_retries_are_bounded_per_show_request() {
        let mut state = PopupPresentation::default();
        let mut check = pending_activation(&mut state);

        for _ in 0..3 {
            assert!(can_retry(check, state, check.source));
            check.retries += 1;
        }
        assert!(!can_retry(check, state, check.source));
        assert!(!can_retry(check, state, std::ptr::null_mut()));

        let next_check = pending_activation(&mut state);
        assert!(!can_retry(check, state, check.source));
        assert!(can_retry(next_check, state, next_check.source));
    }
}

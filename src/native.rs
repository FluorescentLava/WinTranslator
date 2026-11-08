use anyhow::{Context, Result};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    rc::Rc,
    time::Instant,
};
type Hwnd = *mut c_void;
type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;
#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
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
const RESIZE_TIMER: usize = 0x5452_535a;
const FOCUS_TIMER: usize = 0x5446_4353;
const FOCUS_DELAY_MS: u32 = 60;
const RESIZE_DURATION_MS: f64 = 200.0;
// A 16 ms USER timer can be rounded up to two default clock ticks. Request
// the supported minimum and use elapsed time, rather than timer counts, below.
const RESIZE_INTERVAL_MS: u32 = 10;
const CLASS_NAME: &str = "TranslationTool.Native.MessageWindow.v1";
const APP_NAME: &str = "WinTranslater";
const SETTINGS_TITLE: &str = "WinTranslater 设置";
const APP_ICON_ID: usize = 1;
#[derive(Clone, Copy)]
struct ResizeAnimation {
    window: Hwnd,
    from: Rect,
    to: Rect,
    elapsed_ms: f64,
    last_sample: Instant,
    monitor: Hwnd,
    dpi: u32,
    revision: u64,
    transition_key: u64,
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
    rectangle: Rect,
    flags: u32,
    transition_key: u64,
}
#[derive(Clone, Copy)]
pub enum Action {
    Show,
    PopupActivated(u64),
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
    static ANCHOR: Cell<Hwnd> = const { Cell::new(std::ptr::null_mut()) };
    static SIZE: Cell<(f64, f64)> = const { Cell::new((480.0, 172.0)) };
    static TASKBAR_MESSAGE: Cell<u32> = const { Cell::new(0) };
    static STARTUP_WARNING: RefCell<Option<String>> = const { RefCell::new(None) };
    static PENDING_ACTION: Cell<Option<Action>> = const { Cell::new(None) };
    static RESIZE_ANIMATION: Cell<Option<ResizeAnimation>> = const { Cell::new(None) };
    static RESIZE_REVISION: Cell<u64> = const { Cell::new(0) };
    static FRAME_SERIAL: Cell<u64> = const { Cell::new(0) };
    static TRANSITION_KEY: Cell<u64> = const { Cell::new(0) };
    static PENDING_FRAME: Cell<Option<PendingPopupFrame>> = const { Cell::new(None) };
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
    fn IsWindowVisible(window: Hwnd) -> i32;
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
    fn LoadIconW(instance: Hwnd, name: *const u16) -> Hwnd;
    fn RegisterWindowMessageW(name: *const u16) -> u32;
    fn GetFocus() -> Hwnd;
    fn PostQuitMessage(code: i32);
    fn GetMessageW(message: *mut Msg, window: Hwnd, minimum: u32, maximum: u32) -> i32;
    fn TranslateMessage(message: *const Msg) -> i32;
    fn DispatchMessageW(message: *const Msg) -> isize;
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
fn app_icon() -> Hwnd {
    unsafe {
        LoadIconW(
            GetModuleHandleW(std::ptr::null()),
            APP_ICON_ID as *const u16,
        )
    }
}
fn set_window_icon(window: Hwnd) {
    let icon = app_icon();
    if !icon.is_null() {
        unsafe {
            SendMessageW(window, 0x0080, 0, icon as isize); // WM_SETICON / ICON_SMALL
            SendMessageW(window, 0x0080, 1, icon as isize); // WM_SETICON / ICON_BIG
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
    let margin = (16.0 * scale).round() as i32;
    let (width, height) = SIZE.with(Cell::get);
    let width = ((width * scale).round() as i32)
        .min((info.work.right - info.work.left - 2 * margin).max(1));
    let height = ((height * scale).round() as i32)
        .min((info.work.bottom - info.work.top - 2 * margin).max(1));
    (
        info.work.right - margin - width,
        info.work.bottom - margin - height,
        width,
        height,
    )
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
        || !POPUP_PRESENTED.with(Cell::get)
        || !PRESENTATION.with(Cell::get).show_requested
        || unsafe { IsWindowVisible(window) } == 0
    {
        return;
    }
    let foreground = unsafe { GetForegroundWindow() };
    if foreground_belongs_to_app(foreground, window) {
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
    rectangle: Rect,
    target: Rect,
    dpi: u32,
    revision: u64,
    progress: f64,
    finished: bool,
    flags: u32,
) {
    let serial = FRAME_SERIAL.with(|value| {
        let next = value.get().wrapping_add(1);
        value.set(next);
        next
    });
    let scale = f64::from(dpi.max(96)) / 96.0;
    publish_popup_frame(PendingPopupFrame {
        window,
        rectangle,
        flags,
        transition_key: TRANSITION_KEY.with(Cell::get),
        frame: PopupFrame {
            revision,
            serial,
            transition_key: TRANSITION_KEY.with(Cell::get),
            width: f64::from(rectangle.right - rectangle.left) / scale,
            height: f64::from(rectangle.bottom - rectangle.top) / scale,
            target_height: f64::from(target.bottom - target.top) / scale,
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
    set_rectangle(pending.window, pending.rectangle, pending.flags);
    // Moving between monitors can synchronously raise WM_DPICHANGED and replace
    // the request while SetWindowPos is on the stack. Its new frame owns the HWND.
    if RESIZE_REVISION.with(Cell::get) != frame.revision {
        return;
    }
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
    // At most one UI frame may be in flight. HWND geometry never runs ahead of
    // the fixed/cropped XAML viewport rendered by the frame consumer.
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
    let start_height = animation.from.bottom - animation.from.top;
    let end_height = animation.to.bottom - animation.to.top;
    let height =
        (f64::from(start_height) + f64::from(end_height - start_height) * eased).round() as i32;
    RESIZE_ANIMATION.with(|value| value.set(Some(animation)));
    queue_popup_frame(
        window,
        Rect {
            top: animation.to.bottom - height,
            ..animation.to
        },
        animation.to,
        animation.dpi,
        animation.revision,
        eased,
        progress >= 1.0,
        0,
    );
}
fn resize_popup(window: Hwnd, first: bool) {
    // Keep the committed viewport steady while its WinUI visual slides out.
    // SIZE still records newer results, and reopening consumes that target.
    if PRESENTATION.with(Cell::get).hide_revision.is_some() {
        return;
    }
    let target = target_rectangle(window);
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
                && pending.rectangle == target
                && pending.transition_key == transition_key
        }))
    {
        return;
    }
    let animate = !first
        && FRAME_HANDLER.with(|value| value.borrow().is_some())
        && visible
        && has_rectangle
        && monitor == anchor
        && current.left == target.left
        && current.right == target.right
        && current.bottom == target.bottom;
    stop_resize(window);
    let revision = RESIZE_REVISION.with(Cell::get);
    if !animate {
        queue_popup_frame(
            window,
            target,
            target,
            dpi,
            revision,
            1.0,
            true,
            if first { 0x0020 } else { 0 },
        );
        return;
    }
    // Sample committed client pixels, never an unacknowledged requested frame.
    let mut client: Rect = unsafe { std::mem::zeroed() };
    if unsafe { GetClientRect(window, &mut client) } != 0 {
        current.top = current.bottom - (client.bottom - client.top);
    }
    RESIZE_ANIMATION.with(|value| {
        value.set(Some(ResizeAnimation {
            window,
            from: current,
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
            dpi,
            RESIZE_REVISION.with(Cell::get),
            1.0,
            true,
            0,
        );
    } else {
        // Publish the exact starting viewport first. New content is measured
        // at its fixed layout size before any HWND height is changed.
        queue_popup_frame(window, current, target, dpi, revision, 0.0, false, 0);
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
            // DWM continues to own the rounded edge and shadow.
            return 0;
        }
        if message == 0x0014 {
            // The configured popup is covered by the native Mica/composition
            // surface. A class-background GDI erase would flash through the
            // newly exposed band while XAML is preparing its next frame.
            return 1;
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
    if message == 0x0231 {
        MANUAL_MOVE.with(|value| value.set(true));
        stop_resize(window);
    }
    if message == 0x0232 {
        MANUAL_MOVE.with(|value| value.set(false));
    }
    if (message == 0x0018 && wparam == 0) || message == 0x0082 {
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
            // Only an acknowledged popup frame owns programmatic geometry.
            // In particular, a cached AppWindow client size or the default DPI
            // handler must not first resize one way and then get corrected back.
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
unsafe extern "system" fn call_hook(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 {
        let event = unsafe { &*(lparam as *const Cwp) };
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
    data.icon = app_icon();
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
    if message == 0x0312 && wparam == HOTKEY_ID as usize {
        crate::desktop::remember_foreground();
        emit(Action::TranslateSelection);
        return 0;
    }
    if message == SHOW_MESSAGE {
        crate::desktop::remember_foreground();
        emit(Action::Show);
        return 0;
    }
    if message == TRAY_MESSAGE {
        crate::desktop::remember_foreground();
        if lparam as u32 == 0x0202 {
            emit(Action::Show);
        } else if matches!(lparam as u32, 0x0205 | 0x007b) {
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
                        PostMessageW(existing, SHOW_MESSAGE, 0, 0);
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
        icon: app_icon(),
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
                let round: u32 = 2;
                DwmSetWindowAttribute(window, 33, &round as *const _ as _, 4);
                let border: u32 = 0xffff_fffe; // DWMWA_COLOR_NONE: Mica keeps its rounded edge.
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
    if !prepare_activation(window) {
        return false;
    }
    set_window_icon(window);
    unsafe {
        // Explicit visibility is independent of WinUI's initial Activate and
        // STARTUPINFO's initial show command. The desktop and final viewport
        // have both been committed before this nonactivating show.
        SetWindowPos(
            window,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            0x0001 | 0x0002 | 0x0004 | 0x0010 | 0x0040,
        );
    }
    let attachment = InputQueueAttachment::new();
    if unsafe { GetForegroundWindow() } != window {
        unsafe { SetForegroundWindow(window) };
    }
    drop(attachment);
    (unsafe { IsWindowVisible(window) }) != 0 && unsafe { GetForegroundWindow() } == window
}

fn activate_popup(window: Hwnd) {
    let activated = activate_window(window);
    POPUP_PRESENTED.with(|value| value.set(unsafe { IsWindowVisible(window) } != 0));
    crate::storage::diagnostic(serde_json::json!({
        "event": "popup_activation",
        "visible": unsafe { IsWindowVisible(window) } != 0,
        "foreground": activated,
    }));
    if POPUP_PRESENTED.with(Cell::get) {
        // Reveal visible content even if Windows rejects foreground activation.
        // The UI requests TextBox focus only after confirming foreground.
        let presentation = PRESENTATION.with(Cell::get);
        if presentation.show_requested {
            emit(Action::PopupActivated(presentation.revision));
        }
    }
    if !activated {
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

pub fn show_popup() {
    ANCHOR.with(|v| v.set(mouse_monitor()));
    let reversing_hide = PRESENTATION.with(Cell::get).hide_revision.is_some();
    // Invalidate old Completion callbacks before activating or publishing a new
    // frame. A rapid hotkey can safely reverse a native Composition hide.
    update_presentation(PopupPresentation::show);
    if let Some(window) = window(APP_NAME) {
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
        cancel_focus_check(window);
        stop_resize(window);
        unsafe {
            ShowWindow(window, 0);
        }
        POPUP_PRESENTED.with(|value| value.set(false));
    }
}
pub fn quit() {
    unsafe {
        PostQuitMessage(0);
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
}

//! Keep our popup and settings HWNDs on the user's current virtual desktop.
//!
//! Remember the external foreground before constructing or activating WinUI.
//! Immediately before ShowWindow/SetForegroundWindow, move the hidden HWND with
//! `move_to_remembered_desktop`. Later normal shows can use `follow_foreground`.
//! A false move result means that activating the HWND could switch the user to
//! its old desktop, so the caller should leave that window unactivated.
//!
//! Only the public IVirtualDesktopManager interface is used. No desktop switch,
//! input injection, foreign window move, window title or document text is used.
use std::{
    ffi::c_void,
    marker::PhantomData,
    ptr::NonNull,
    rc::Rc,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

type Hwnd = *mut c_void;
type HResult = i32;
const GA_ROOT: u32 = 2;
const GWL_STYLE: i32 = -16;
const GWL_EXSTYLE: i32 = -20;
const WS_CHILD: u32 = 0x4000_0000;
const WS_POPUP: u32 = 0x8000_0000;
const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
const WS_EX_TRANSPARENT: u32 = 0x0000_0020;
const WS_EX_LAYERED: u32 = 0x0008_0000;
const WS_EX_NOACTIVATE: u32 = 0x0800_0000;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_SHOWWINDOW: u32 = 0x0040;
const SWP_NOZORDER: u32 = 0x0004;
const LWA_ALPHA: u32 = 2;
const DWMWA_CLOAKED: u32 = 14;
const RPC_E_CHANGED_MODE: HResult = 0x8001_0106_u32 as i32;
const FALLBACK_BUDGET: Duration = Duration::from_millis(60);
const PROBE_REGISTRATION_BUDGET: Duration = Duration::from_millis(80);
const MAX_ENUM_WINDOWS: usize = 256;
static PROBE_SERIAL: AtomicU64 = AtomicU64::new(0);

static REMEMBERED: Mutex<Remembered> = Mutex::new(Remembered {
    desktop: None,
    revision: 0,
});

/// Call at the hotkey/tray boundary, before any of our windows take foreground.
/// An already active window in this process never overwrites the saved target.
/// An unresolvable external foreground clears an older target instead of using
/// a desktop saved by a previous invocation.
pub fn remember_foreground() -> bool {
    let source = Foreground::capture();
    let revision = {
        let mut memory = remembered();
        let Some(revision) = memory.begin(source.is_ours()) else {
            return memory.desktop.is_some();
        };
        revision
    };
    let desktop = Manager::new().and_then(|manager| {
        let desktop = current_desktop(&manager, source.window as Hwnd);
        (Foreground::capture() == source)
            .then_some(desktop)
            .flatten()
    });
    let mut memory = remembered();
    memory.finish(revision, desktop);
    memory.desktop.is_some()
}

/// Move only a top-level HWND belonging to this process, without activating it.
/// Returns true when the HWND is confirmed to be on the current virtual desktop
/// after the operation (including when it was already there).
pub fn move_to_remembered_desktop(window: usize) -> bool {
    let window = window as Hwnd;
    if !our_top_level_window(window) {
        return false;
    }
    let desktop = remembered().desktop;
    let Some(manager) = Manager::new() else {
        return false;
    };
    if let Some(desktop) = desktop {
        if manager.desktop_id(window) != Some(desktop) {
            move_owned_window(&manager, window, &desktop);
        }
    }
    // In particular, an unknown/empty current desktop must not turn a stale
    // remembered GUID into permission to activate a window on another desktop.
    our_top_level_window(window) && manager.is_current(window) == Some(true)
}

fn move_owned_window(manager: &Manager, window: Hwnd, desktop: &Guid) -> bool {
    // Recheck ownership immediately before the only call in this module which
    // can move a window. The live harness uses this same gate for a same-desktop
    // move and rejection of another process's HWND.
    if desktop.is_null() || !our_top_level_window(window) {
        return false;
    }
    unsafe { (manager.table().move_window)(manager.raw(), window, desktop) >= 0 }
}

/// Convenience for an existing hidden window, before showing or focusing it.
/// For initial WinUI construction use remember_foreground first, then the
/// explicit move function, so automatic activation cannot replace the target.
pub fn follow_foreground(window: usize) -> bool {
    if window == 0 {
        return false;
    }
    remember_foreground();
    move_to_remembered_desktop(window)
}

#[derive(Default)]
struct Remembered {
    desktop: Option<Guid>,
    revision: u64,
}
impl Remembered {
    fn begin(&mut self, ours: bool) -> Option<u64> {
        if ours {
            return None;
        }
        self.revision = self.revision.wrapping_add(1);
        self.desktop = None;
        Some(self.revision)
    }

    fn finish(&mut self, revision: u64, desktop: Option<Guid>) {
        if revision == self.revision {
            self.desktop = desktop.filter(|value| !value.is_null());
        }
    }
}
fn remembered() -> MutexGuard<'static, Remembered> {
    REMEMBERED.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Foreground {
    window: usize,
    process: u32,
}
impl Foreground {
    fn capture() -> Self {
        let window = unsafe { GetForegroundWindow() };
        Self {
            window: window as usize,
            process: process_id(window),
        }
    }

    fn is_ours(self) -> bool {
        self.window != 0 && self.process == unsafe { GetCurrentProcessId() }
    }
}

fn current_desktop(manager: &Manager, foreground: Hwnd) -> Option<Guid> {
    if ordinary_external_window(foreground)
        && manager.is_current(foreground) == Some(true)
        && let Some(desktop) = manager.desktop_id(foreground)
    {
        return Some(desktop);
    }
    // The taskbar, desktop background and a window pinned to every desktop do
    // not identify the current virtual desktop. Look only at visible, ordinary
    // external top-level windows which the public API confirms are current.
    let mut search = Search {
        manager,
        deadline: Instant::now() + FALLBACK_BUDGET,
        visited: 0,
        desktop: None,
    };
    unsafe { EnumWindows(find_current_desktop, &mut search as *mut _ as isize) };
    search.desktop.or_else(|| probe_current_desktop(manager))
}

fn probe_current_desktop(manager: &Manager) -> Option<Guid> {
    // With no ordinary application open on this desktop, use a newly created
    // window rather than an existing translator HWND assigned to an old one.
    // It is alpha-zero before it is shown and never activates. Showing lets the
    // shell assign it to the current virtual desktop without a forced taskbar tab.
    let foreground = Foreground::capture();
    let probe = DesktopProbe::new()?;
    let desktop = registered_probe_desktop(manager, &probe, foreground);
    drop(probe);
    if Foreground::capture() == foreground {
        desktop
    } else {
        // Never restore focus: a user's deliberate change wins.
        None
    }
}

fn registered_probe_desktop(
    manager: &Manager,
    probe: &DesktopProbe,
    foreground: Foreground,
) -> Option<Guid> {
    let deadline = Instant::now() + PROBE_REGISTRATION_BUDGET;
    loop {
        if Foreground::capture() != foreground
            || Instant::now() >= deadline
            || !service_probe_messages(probe.window())
        {
            return None;
        }
        if let Some(desktop) = manager.desktop_id(probe.window())
            && manager.is_current(probe.window()) == Some(true)
        {
            return Some(desktop);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}
#[repr(C)]
struct WindowMessage {
    window: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    point: Point,
    private: u32,
}

fn service_probe_messages(window: Hwnd) -> bool {
    // The shell may synchronously request metadata while creating its view.
    // Serve only this probe's queued messages, and never consume WM_QUIT.
    for _ in 0..16 {
        let mut message: WindowMessage = unsafe { std::mem::zeroed() };
        if unsafe { PeekMessageW(&mut message, window, 0, 0, 0) } == 0 {
            return true;
        }
        if message.message == 0x0012 {
            return false;
        }
        if unsafe { PeekMessageW(&mut message, window, 0, 0, 1) } != 0 {
            unsafe { DispatchMessageW(&message) };
        }
    }
    true
}

type WindowProcedure = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

unsafe extern "system" fn probe_procedure(
    window: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    match message {
        0x0021 => 3,  // WM_MOUSEACTIVATE: MA_NOACTIVATE
        0x0084 => -1, // WM_NCHITTEST: HTTRANSPARENT
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

// SDK WinUser.h WNDCLASSW. The class has no icon, cursor or background brush.
#[repr(C)]
struct WindowClass {
    style: u32,
    procedure: Option<WindowProcedure>,
    class_extra: i32,
    window_extra: i32,
    instance: Hwnd,
    icon: Hwnd,
    cursor: Hwnd,
    background: Hwnd,
    menu_name: *const u16,
    class_name: *const u16,
}

struct ProbeClass {
    name: Vec<u16>,
    instance: Hwnd,
}
impl ProbeClass {
    fn new() -> Option<Self> {
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        if instance.is_null() {
            return None;
        }
        let serial = PROBE_SERIAL.fetch_add(1, Ordering::Relaxed);
        let name: Vec<_> = format!(
            "TranslationTool.DesktopProbe.{}.{}",
            unsafe { GetCurrentProcessId() },
            serial
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let class = WindowClass {
            style: 0,
            procedure: Some(probe_procedure),
            class_extra: 0,
            window_extra: 0,
            instance,
            icon: std::ptr::null_mut(),
            cursor: std::ptr::null_mut(),
            background: std::ptr::null_mut(),
            menu_name: std::ptr::null(),
            class_name: name.as_ptr(),
        };
        if unsafe { RegisterClassW(&class) } == 0 {
            return None;
        }
        Some(Self { name, instance })
    }
}
impl Drop for ProbeClass {
    fn drop(&mut self) {
        unsafe { UnregisterClassW(self.name.as_ptr(), self.instance) };
    }
}

struct DesktopProbe {
    pointer: NonNull<c_void>,
    _class: ProbeClass,
    _thread_bound: PhantomData<Rc<()>>,
}
impl DesktopProbe {
    fn new() -> Option<Self> {
        let class = ProbeClass::new()?;
        let pointer = unsafe {
            CreateWindowExW(
                // TOOLWINDOW and NOACTIVATE windows have no desktop GUID on
                // supported shells. Use nonactivating positioning instead;
                // do not force a taskbar tab with WS_EX_APPWINDOW.
                WS_EX_LAYERED | WS_EX_TRANSPARENT,
                class.name.as_ptr(),
                std::ptr::null(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                class.instance,
                std::ptr::null_mut(),
            )
        };
        let probe = Self {
            pointer: NonNull::new(pointer)?,
            _class: class,
            _thread_bound: PhantomData,
        };
        // Never show a nontransparent probe, even if layering initialization
        // fails. All subsequent failure paths destroy HWND and unregister class.
        if unsafe { SetLayeredWindowAttributes(probe.window(), 0, 0, LWA_ALPHA) } == 0 {
            return None;
        }
        // Unlike the process's first ShowWindow call, SWP_NOACTIVATE cannot be
        // overridden by STARTUPINFO's initial show command.
        if unsafe {
            SetWindowPos(
                probe.window(),
                std::ptr::null_mut(),
                0,
                0,
                1,
                1,
                SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOZORDER,
            )
        } == 0
        {
            return None;
        }
        (unsafe { IsWindowVisible(probe.window()) } != 0).then_some(probe)
    }

    fn window(&self) -> Hwnd {
        self.pointer.as_ptr()
    }
}
impl Drop for DesktopProbe {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.window()) };
    }
}

struct Search<'a> {
    manager: &'a Manager,
    deadline: Instant,
    visited: usize,
    desktop: Option<Guid>,
}
unsafe extern "system" fn find_current_desktop(window: Hwnd, parameter: isize) -> i32 {
    let search = unsafe { &mut *(parameter as *mut Search<'_>) };
    search.visited += 1;
    if search.visited > MAX_ENUM_WINDOWS || Instant::now() >= search.deadline {
        return 0;
    }
    if ordinary_external_window(window)
        && search.manager.is_current(window) == Some(true)
        && let Some(desktop) = search.manager.desktop_id(window)
        && search.manager.is_current(window) == Some(true)
    {
        search.desktop = Some(desktop);
        return 0;
    }
    1
}

fn process_id(window: Hwnd) -> u32 {
    if window.is_null() {
        return 0;
    }
    let mut process = 0;
    unsafe { GetWindowThreadProcessId(window, &mut process) };
    process
}

fn our_top_level_window(window: Hwnd) -> bool {
    !window.is_null()
        && unsafe { IsWindow(window) } != 0
        && process_id(window) == unsafe { GetCurrentProcessId() }
        && unsafe { GetAncestor(window, GA_ROOT) } == window
}

fn ordinary_external_window(window: Hwnd) -> bool {
    let process = process_id(window);
    if window.is_null()
        || process == 0
        || process == unsafe { GetCurrentProcessId() }
        || unsafe { IsWindowVisible(window) } == 0
        || unsafe { IsIconic(window) } != 0
        || unsafe { GetAncestor(window, GA_ROOT) } != window
        || window == unsafe { GetShellWindow() }
        || window == unsafe { GetDesktopWindow() }
    {
        return false;
    }
    let style = unsafe { GetWindowLongW(window, GWL_STYLE) } as u32;
    let extended = unsafe { GetWindowLongW(window, GWL_EXSTYLE) } as u32;
    if style & WS_CHILD != 0 || extended & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) != 0 {
        return false;
    }
    let mut class = [0_u16; 128];
    let length = unsafe { GetClassNameW(window, class.as_mut_ptr(), class.len() as i32) };
    if length <= 0 || shell_class(&class[..length as usize]) {
        return false;
    }
    let mut cloaked = 0_u32;
    // Fail closed if visibility cannot be confirmed. IsWindowVisible alone is
    // also true for some windows whose virtual desktop is hidden by the shell.
    (unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut c_void,
            std::mem::size_of::<u32>() as u32,
        )
    }) >= 0
        && cloaked == 0
}

fn shell_class(class: &[u16]) -> bool {
    [
        "Progman",
        "WorkerW",
        "Shell_TrayWnd",
        "Shell_SecondaryTrayWnd",
        "Shell_InputSwitchTopLevelWindow",
        "XamlExplorerHostIslandWindow",
    ]
    .iter()
    .any(|name| class.iter().copied().eq(name.encode_utf16()))
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Guid {
    first: u32,
    second: u16,
    third: u16,
    last: [u8; 8],
}
impl Guid {
    fn is_null(self) -> bool {
        self == Self::default()
    }
}
const CLSID_MANAGER: Guid = Guid {
    first: 0xaa509086,
    second: 0x5ca9,
    third: 0x4c25,
    last: [0x8f, 0x95, 0x58, 0x9d, 0x3c, 0x07, 0xb4, 0x8a],
};
const IID_MANAGER: Guid = Guid {
    first: 0xa5cd92ff,
    second: 0x29be,
    third: 0x454c,
    last: [0x8d, 0x04, 0xd8, 0x28, 0x79, 0xfb, 0x3f, 0x1b],
};

// SDK ShObjIdl_core.h, IVirtualDesktopManager: IUnknown followed by slots 3-5.
#[repr(C)]
struct ManagerTable {
    _query: unsafe extern "system" fn(Hwnd, *const Guid, *mut Hwnd) -> HResult,
    _add_ref: unsafe extern "system" fn(Hwnd) -> u32,
    release: unsafe extern "system" fn(Hwnd) -> u32,
    is_current: unsafe extern "system" fn(Hwnd, Hwnd, *mut i32) -> HResult,
    desktop_id: unsafe extern "system" fn(Hwnd, Hwnd, *mut Guid) -> HResult,
    move_window: unsafe extern "system" fn(Hwnd, Hwnd, *const Guid) -> HResult,
}
#[repr(C)]
struct ManagerInterface {
    table: *const ManagerTable,
}
struct Manager {
    pointer: NonNull<ManagerInterface>,
    _apartment: Apartment,
    _apartment_bound: PhantomData<Rc<()>>,
}
impl Manager {
    fn new() -> Option<Self> {
        let apartment = Apartment::new()?;
        let mut pointer = std::ptr::null_mut();
        let status = unsafe {
            CoCreateInstance(
                &CLSID_MANAGER,
                std::ptr::null_mut(),
                1, // CLSCTX_INPROC_SERVER
                &IID_MANAGER,
                &mut pointer,
            )
        };
        if status < 0 {
            return None;
        }
        Some(Self {
            pointer: NonNull::new(pointer.cast::<ManagerInterface>())?,
            _apartment: apartment,
            _apartment_bound: PhantomData,
        })
    }

    fn raw(&self) -> Hwnd {
        self.pointer.as_ptr().cast()
    }

    fn table(&self) -> &ManagerTable {
        unsafe { &*self.pointer.as_ref().table }
    }

    fn desktop_id(&self, window: Hwnd) -> Option<Guid> {
        let mut desktop = Guid::default();
        let status = unsafe { (self.table().desktop_id)(self.raw(), window, &mut desktop) };
        (status >= 0 && !desktop.is_null()).then_some(desktop)
    }

    fn is_current(&self, window: Hwnd) -> Option<bool> {
        let mut current = 0;
        let status = unsafe { (self.table().is_current)(self.raw(), window, &mut current) };
        (status >= 0).then_some(current != 0)
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        unsafe { (self.table().release)(self.raw()) };
    }
}

struct Apartment(bool);
impl Apartment {
    fn new() -> Option<Self> {
        let status = unsafe { CoInitializeEx(std::ptr::null_mut(), 2) };
        if status >= 0 {
            Some(Self(true))
        } else if status == RPC_E_CHANGED_MODE {
            // WinUI or a worker may have already chosen another COM apartment.
            // Use it as-is, and do not balance an initialization we did not do.
            Some(Self(false))
        } else {
            None
        }
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetForegroundWindow() -> Hwnd;
    fn GetWindowThreadProcessId(window: Hwnd, process: *mut u32) -> u32;
    fn IsWindow(window: Hwnd) -> i32;
    fn IsWindowVisible(window: Hwnd) -> i32;
    fn IsIconic(window: Hwnd) -> i32;
    fn GetAncestor(window: Hwnd, flags: u32) -> Hwnd;
    fn GetShellWindow() -> Hwnd;
    fn GetDesktopWindow() -> Hwnd;
    fn GetWindowLongW(window: Hwnd, index: i32) -> i32;
    fn GetClassNameW(window: Hwnd, class: *mut u16, count: i32) -> i32;
    fn EnumWindows(
        callback: unsafe extern "system" fn(Hwnd, isize) -> i32,
        parameter: isize,
    ) -> i32;
    fn RegisterClassW(class: *const WindowClass) -> u16;
    fn UnregisterClassW(class: *const u16, instance: Hwnd) -> i32;
    fn CreateWindowExW(
        extended: u32,
        class: *const u16,
        name: *const u16,
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
    fn SetLayeredWindowAttributes(window: Hwnd, color: u32, alpha: u8, flags: u32) -> i32;
    fn SetWindowPos(
        window: Hwnd,
        after: Hwnd,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
    fn DestroyWindow(window: Hwnd) -> i32;
    fn DefWindowProcW(window: Hwnd, message: u32, wparam: usize, lparam: isize) -> isize;
    fn PeekMessageW(
        message: *mut WindowMessage,
        window: Hwnd,
        minimum: u32,
        maximum: u32,
        remove: u32,
    ) -> i32;
    fn DispatchMessageW(message: *const WindowMessage) -> isize;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcessId() -> u32;
    fn GetModuleHandleW(name: *const u16) -> Hwnd;
}
#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmGetWindowAttribute(window: Hwnd, attribute: u32, value: Hwnd, size: u32) -> HResult;
}
#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: Hwnd, flags: u32) -> HResult;
    fn CoUninitialize();
    fn CoCreateInstance(
        class: *const Guid,
        outer: Hwnd,
        context: u32,
        iid: *const Guid,
        object: *mut Hwnd,
    ) -> HResult;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn manager_abi_matches_the_windows_sdk() {
        let slot = size_of::<usize>();
        assert_eq!(size_of::<Guid>(), 16);
        assert_eq!(align_of::<Guid>(), 4);
        assert_eq!(size_of::<ManagerInterface>(), slot);
        assert_eq!(size_of::<ManagerTable>(), 6 * slot);
        assert_eq!(offset_of!(ManagerTable, release), 2 * slot);
        assert_eq!(offset_of!(ManagerTable, is_current), 3 * slot);
        assert_eq!(offset_of!(ManagerTable, desktop_id), 4 * slot);
        assert_eq!(offset_of!(ManagerTable, move_window), 5 * slot);
        assert_eq!(size_of::<WindowClass>(), if slot == 8 { 72 } else { 40 });
        assert_eq!(size_of::<WindowMessage>(), if slot == 8 { 48 } else { 32 });
        assert_eq!(offset_of!(WindowClass, procedure), slot);
        assert_eq!(
            offset_of!(WindowClass, instance),
            if slot == 8 { 24 } else { 16 }
        );
        assert_eq!(
            offset_of!(WindowClass, class_name),
            if slot == 8 { 64 } else { 36 }
        );
    }

    #[test]
    fn our_foreground_never_replaces_the_external_target() {
        let mut memory = Remembered::default();
        let revision = memory.begin(false).unwrap();
        memory.finish(revision, Some(IID_MANAGER));
        assert_eq!(memory.begin(true), None);
        assert_eq!(memory.desktop, Some(IID_MANAGER));
    }

    #[test]
    fn unknown_external_desktop_clears_an_older_target() {
        let mut memory = Remembered {
            desktop: Some(IID_MANAGER),
            revision: 0,
        };
        let revision = memory.begin(false).unwrap();
        memory.finish(revision, Some(Guid::default()));
        assert_eq!(memory.desktop, None);
    }

    #[test]
    fn older_foreground_result_cannot_overwrite_a_newer_request() {
        let mut memory = Remembered::default();
        let older = memory.begin(false).unwrap();
        let newer = memory.begin(false).unwrap();
        memory.finish(newer, Some(IID_MANAGER));
        memory.finish(older, Some(CLSID_MANAGER));
        assert_eq!(memory.desktop, Some(IID_MANAGER));
    }

    #[test]
    fn shell_classes_do_not_supply_a_desktop() {
        assert!(shell_class(
            &"Shell_TrayWnd".encode_utf16().collect::<Vec<_>>()
        ));
        assert!(shell_class(&"WorkerW".encode_utf16().collect::<Vec<_>>()));
        assert!(!shell_class(&"Notepad".encode_utf16().collect::<Vec<_>>()));
    }

    #[test]
    fn null_window_does_not_initialize_com_or_inspect_the_desktop() {
        assert!(!move_to_remembered_desktop(0));
        assert!(!follow_foreground(0));
    }

    // Compile/run independently with rustc --test; it only creates alpha-zero
    // noactivate windows and moves one to its existing desktop. Never switches
    // a virtual desktop, focuses a window or moves another process's window.
    #[test]
    #[ignore = "live Win32 harness; run explicitly, without switching desktops"]
    fn live_probe_and_owned_move_preserve_foreground() {
        let foreground = Foreground::capture();
        let manager = Manager::new().expect("public virtual desktop manager");
        let probe = DesktopProbe::new().expect("transparent noactivate probe");
        let window = probe.window();
        assert!(our_top_level_window(window));
        assert_eq!(manager.is_current(window), Some(true));
        let desktop = registered_probe_desktop(&manager, &probe, foreground)
            .expect("probe desktop GUID within 80 ms");
        assert!(move_owned_window(&manager, window, &desktop));
        assert_eq!(manager.desktop_id(window), Some(desktop));
        assert_eq!(manager.is_current(window), Some(true));

        let foreign = unsafe { GetShellWindow() };
        assert!(!foreign.is_null());
        assert_ne!(process_id(foreign), unsafe { GetCurrentProcessId() });
        assert!(!our_top_level_window(foreign));
        assert!(!move_owned_window(&manager, foreign, &desktop));
        assert!(!move_owned_window(&manager, std::ptr::null_mut(), &desktop));
        assert!(
            Foreground::capture() == foreground,
            "probe must not take focus"
        );

        drop(probe);
        assert_eq!(
            unsafe { IsWindow(window) },
            0,
            "probe HWND must be destroyed"
        );
        assert_eq!(probe_current_desktop(&manager), Some(desktop));
        assert!(
            Foreground::capture() == foreground,
            "focus must stay unchanged"
        );
        eprintln!(
            "Live probe: current GUID found; own same-desktop move passed; foreign/NULL rejected; HWND cleaned; foreground unchanged."
        );
    }
}

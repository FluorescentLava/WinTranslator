//! Read the current application's explicit text selection without input injection.
//!
//! Call `CaptureRequest::new()` at the hotkey boundary, and run `request.capture()`
//! before activating the translator window. `capture()` is also a convenience for
//! callers which have not changed the foreground window. The caller waits at most
//! `CAPTURE_TIMEOUT`; UI Automation itself runs on a separate MTA thread.
//!
//! Only TextPattern.GetSelection ranges are read. A caret, unsupported provider,
//! password control, changed foreground window, cancellation or timeout yields
//! None. There is deliberately no Ctrl+C or clipboard fallback: preserving just
//! clipboard text would destroy images, file lists and application-owned formats.
use std::{
    ffi::c_void,
    marker::PhantomData,
    ptr::NonNull,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub const CAPTURE_TIMEOUT: Duration = Duration::from_millis(240);
const PROVIDER_TIMEOUT_MS: u32 = 80;
const POLL_INTERVAL: Duration = Duration::from_millis(8);
const MAX_SELECTION_UNITS: usize = 1_048_576;
const MAX_SELECTION_RANGES: i32 = 16;
const MAX_ANCESTORS: usize = 6;
// A provider which ignores RPC cancellation may outlive our deadline. Keep only
// one such worker instead of leaking another thread on every hotkey press.
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

type Raw = *mut c_void;
type HResult = i32;
type ObjectGetter = unsafe extern "system" fn(Raw, *mut Raw) -> HResult;

#[derive(Clone, Copy, Debug)]
pub struct CaptureRequest {
    window: usize,
    process: u32,
    created_at: Instant,
}

impl CaptureRequest {
    /// Snapshot the source before the popup takes focus. Ignore our own windows.
    pub fn new() -> Option<Self> {
        let window = unsafe { GetForegroundWindow() };
        if window.is_null() {
            return None;
        }
        let mut process = 0;
        unsafe { GetWindowThreadProcessId(window, &mut process) };
        if process == 0 || process == unsafe { GetCurrentProcessId() } {
            return None;
        }
        Some(Self {
            window: window as usize,
            process,
            created_at: Instant::now(),
        })
    }

    /// Run on a background task when a WinUI window already exists. Cancellation
    /// is polled on the calling thread; the callback need not be Send or 'static.
    /// Do not activate the popup until this method has returned.
    pub fn capture(&self, cancelled: &dyn Fn() -> bool) -> Option<String> {
        let deadline = self.created_at + CAPTURE_TIMEOUT;
        if cancelled() || Instant::now() >= deadline || !self.is_current() {
            return None;
        }
        if IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let thread_id = Arc::new(AtomicU32::new(0));
        let (sender, receiver) = mpsc::sync_channel(1);
        let context = Context {
            request: *self,
            deadline,
            stop: Arc::clone(&stop),
        };
        let worker_id = Arc::clone(&thread_id);
        let worker = std::thread::Builder::new()
            .name("selected-text-uia".into())
            .spawn(move || {
                let _flight = Flight;
                let result = read_on_mta(&context, &worker_id);
                let _ = sender.send(result);
            });
        if worker.is_err() {
            IN_FLIGHT.store(false, Ordering::Release);
            return None;
        }
        // Dropping the join handle detaches the worker. Never join a third-party
        // accessibility provider on the UI thread or after the deadline.
        drop(worker);
        loop {
            let now = Instant::now();
            if cancelled() || now >= deadline || !self.is_current() {
                cancel_worker(&stop, &thread_id);
                return None;
            }
            match receiver.recv_timeout(POLL_INTERVAL.min(deadline - now)) {
                Ok(result) => {
                    return if cancelled() || Instant::now() >= deadline || !self.is_current() {
                        None
                    } else {
                        result
                    };
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
    }

    fn is_current(&self) -> bool {
        let window = self.window as Raw;
        if unsafe { GetForegroundWindow() } != window || unsafe { IsWindow(window) } == 0 {
            return false;
        }
        let mut process = 0;
        unsafe { GetWindowThreadProcessId(window, &mut process) };
        process == self.process && process != unsafe { GetCurrentProcessId() }
    }
}

/// Capture before showing the popup; None leaves the existing input untouched.
pub fn capture(cancelled: &dyn Fn() -> bool) -> Option<String> {
    if cancelled() {
        return None;
    }
    CaptureRequest::new()?.capture(cancelled)
}

struct Context {
    request: CaptureRequest,
    deadline: Instant,
    stop: Arc<AtomicBool>,
}
impl Context {
    fn active(&self) -> bool {
        !self.stop.load(Ordering::Acquire)
            && Instant::now() < self.deadline
            && self.request.is_current()
    }
}
struct Flight;
impl Drop for Flight {
    fn drop(&mut self) {
        IN_FLIGHT.store(false, Ordering::Release);
    }
}
fn cancel_worker(stop: &AtomicBool, thread_id: &AtomicU32) {
    stop.store(true, Ordering::Release);
    let id = thread_id.load(Ordering::Acquire);
    if id != 0 {
        // Best-effort cancellation of the worker's pending outbound COM call.
        // The public deadline does not depend on the provider honoring it.
        unsafe { CoCancelCall(id, 0) };
    }
}

fn read_on_mta(context: &Context, thread_id: &AtomicU32) -> Option<String> {
    if !context.active() {
        return None;
    }
    let _apartment = Apartment::new()?;
    let _cancellation = CallCancellation::new();
    thread_id.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    let result = read_selection(context);
    thread_id.store(0, Ordering::Release);
    result
}

fn read_selection(context: &Context) -> Option<String> {
    let automation = create_automation()?;
    if !context.active() {
        return None;
    }
    let mut element: Com<ElementVtable> =
        owned(|out| unsafe { (automation.vtable().focused)(automation.raw(), out) })?;
    let mut walker: Option<Com<TreeWalkerVtable>> = None;
    for _ in 0..MAX_ANCESTORS {
        if !context.active() || !safe_element(&element, context) {
            return None;
        }
        match selected_ranges(&element, context) {
            Selection::Text(value) => return context.active().then_some(value),
            Selection::Empty | Selection::Failed => return None,
            Selection::Unsupported => {}
        }
        // Walk only ancestors of the focused element, never document children,
        // siblings or a desktop-wide search. Stop at the source native window.
        let mut native_window = std::ptr::null_mut();
        let status = unsafe { (element.vtable().native_window)(element.raw(), &mut native_window) };
        if status >= 0 && native_window == context.request.window as Raw {
            return None;
        }
        if !context.active() {
            return None;
        }
        if walker.is_none() {
            walker = owned(|out| unsafe { (automation.vtable().walker)(automation.raw(), out) });
        }
        let parent = walker.as_ref()?;
        element =
            owned(|out| unsafe { (parent.vtable().parent)(parent.raw(), element.raw(), out) })?;
    }
    None
}

fn safe_element(element: &Com<ElementVtable>, context: &Context) -> bool {
    let mut process = 0;
    if unsafe { (element.vtable().process_id)(element.raw(), &mut process) } < 0
        || process as u32 == unsafe { GetCurrentProcessId() }
        || !context.active()
    {
        return false;
    }
    let mut password = 1;
    if unsafe { (element.vtable().password)(element.raw(), &mut password) } < 0
        || password != 0
        || !context.active()
    {
        return false;
    }
    let mut window = std::ptr::null_mut();
    if unsafe { (element.vtable().native_window)(element.raw(), &mut window) } < 0 {
        return false;
    }
    // Browser descendants can live in a renderer process and have no HWND. The
    // focused element plus unchanged foreground scope is authoritative there.
    window.is_null()
        || window == context.request.window as Raw
        || unsafe { GetAncestor(window, 2) } == context.request.window as Raw
}

enum Selection {
    Unsupported,
    Empty,
    Failed,
    Text(String),
}
fn selected_ranges(element: &Com<ElementVtable>, context: &Context) -> Selection {
    let mut pattern: Option<Com<TextPatternVtable>> = None;
    for pattern_id in [10014, 10024] {
        if !context.active() {
            return Selection::Failed;
        }
        pattern = owned(|out| unsafe {
            (element.vtable().pattern)(element.raw(), pattern_id, &IID_TEXT_PATTERN, out)
        });
        if pattern.is_some() {
            break;
        }
    }
    let Some(pattern) = pattern else {
        return Selection::Unsupported;
    };
    let Some(ranges): Option<Com<TextRangeArrayVtable>> =
        owned(|out| unsafe { (pattern.vtable().selection)(pattern.raw(), out) })
    else {
        return Selection::Failed;
    };
    let mut count = 0;
    if unsafe { (ranges.vtable().length)(ranges.raw(), &mut count) } < 0
        || !(0..=MAX_SELECTION_RANGES).contains(&count)
    {
        return Selection::Failed;
    }
    let mut text = String::new();
    let mut units = 0;
    for index in 0..count {
        if !context.active() {
            return Selection::Failed;
        }
        let Some(range): Option<Com<TextRangeVtable>> =
            owned(|out| unsafe { (ranges.vtable().element)(ranges.raw(), index, out) })
        else {
            return Selection::Failed;
        };
        // A collapsed caret is often returned as a one-element selection array.
        // Never expand it to a word, paragraph or the entire document.
        let mut comparison = 0;
        if unsafe {
            (range.vtable().compare_endpoints)(range.raw(), 0, range.raw(), 1, &mut comparison)
        } < 0
        {
            return Selection::Failed;
        }
        if comparison == 0 {
            continue;
        }
        if !context.active() {
            return Selection::Failed;
        }
        let remaining = MAX_SELECTION_UNITS.saturating_sub(units);
        let mut value = BStr(std::ptr::null_mut());
        if unsafe { (range.vtable().text)(range.raw(), (remaining + 1) as i32, &mut value.0) } < 0 {
            return Selection::Failed;
        }
        let length = value.len();
        if length > remaining {
            // Do not silently import a truncated selection.
            return Selection::Failed;
        }
        if length != 0 {
            if !text.is_empty() {
                text.push('\n');
                units += 1;
            }
            if units + length > MAX_SELECTION_UNITS {
                return Selection::Failed;
            }
            text.push_str(&value.to_string());
            units += length;
        }
    }
    if text.trim().is_empty() {
        Selection::Empty
    } else {
        Selection::Text(text)
    }
}

fn create_automation() -> Option<Com<AutomationVtable>> {
    let automation: Com<AutomationVtable> = owned(|out| unsafe {
        CoCreateInstance(
            &CLSID_AUTOMATION_8,
            std::ptr::null_mut(),
            1,
            &IID_AUTOMATION,
            out,
        )
    })
    .or_else(|| {
        owned(|out| unsafe {
            CoCreateInstance(
                &CLSID_AUTOMATION,
                std::ptr::null_mut(),
                1,
                &IID_AUTOMATION,
                out,
            )
        })
    })?;
    if let Some(options) = automation.query::<Automation2Vtable>(&IID_AUTOMATION_2) {
        unsafe {
            (options.vtable().auto_focus)(options.raw(), 0);
            (options.vtable().connection_timeout)(options.raw(), PROVIDER_TIMEOUT_MS);
            (options.vtable().transaction_timeout)(options.raw(), PROVIDER_TIMEOUT_MS);
        }
    }
    Some(automation)
}

struct Apartment;
impl Apartment {
    fn new() -> Option<Self> {
        if unsafe { CoInitializeEx(std::ptr::null_mut(), 0) } >= 0 {
            Some(Self)
        } else {
            None
        }
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}
struct CallCancellation(bool);
impl CallCancellation {
    fn new() -> Self {
        Self(unsafe { CoEnableCallCancellation(std::ptr::null_mut()) } >= 0)
    }
}
impl Drop for CallCancellation {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoDisableCallCancellation(std::ptr::null_mut()) };
        }
    }
}

// Narrow, typed COM ABI definitions from Windows SDK UIAutomationClient.h.
// Unused slots remain pointer-sized so offsets match the inherited interfaces.
#[repr(C)]
struct Guid {
    a: u32,
    b: u16,
    c: u16,
    d: [u8; 8],
}
const IID_AUTOMATION: Guid = Guid {
    a: 0x30cbe57d,
    b: 0xd9d0,
    c: 0x452a,
    d: [0xab, 0x13, 0x7a, 0xc5, 0xac, 0x48, 0x25, 0xee],
};
const IID_AUTOMATION_2: Guid = Guid {
    a: 0x34723aff,
    b: 0x0c9d,
    c: 0x49d0,
    d: [0x98, 0x96, 0x7a, 0xb5, 0x2d, 0xf8, 0xcd, 0x8a],
};
const IID_TEXT_PATTERN: Guid = Guid {
    a: 0x32eba289,
    b: 0x3583,
    c: 0x42c9,
    d: [0x9c, 0x59, 0x3b, 0x6d, 0x9a, 0x1e, 0x9b, 0x6a],
};
const CLSID_AUTOMATION: Guid = Guid {
    a: 0xff48dba4,
    b: 0x60ef,
    c: 0x4201,
    d: [0xaa, 0x87, 0x54, 0x10, 0x3e, 0xef, 0x59, 0x4e],
};
const CLSID_AUTOMATION_8: Guid = Guid {
    a: 0xe22ad333,
    b: 0xb25f,
    c: 0x460c,
    d: [0x83, 0xd0, 0x05, 0x81, 0x10, 0x73, 0x95, 0xc9],
};
#[repr(C)]
struct UnknownVtable {
    query: unsafe extern "system" fn(Raw, *const Guid, *mut Raw) -> HResult,
    _add_ref: usize,
    release: unsafe extern "system" fn(Raw) -> u32,
}
#[repr(C)]
struct AutomationVtable {
    _unknown: UnknownVtable,
    _slots_3_7: [usize; 5],
    focused: ObjectGetter,
    _slots_9_13: [usize; 5],
    walker: ObjectGetter,
}
#[repr(C)]
struct Automation2Vtable {
    _slots_0_58: [usize; 59],
    auto_focus: unsafe extern "system" fn(Raw, i32) -> HResult,
    _slot_60: usize,
    connection_timeout: unsafe extern "system" fn(Raw, u32) -> HResult,
    _slot_62: usize,
    transaction_timeout: unsafe extern "system" fn(Raw, u32) -> HResult,
}
#[repr(C)]
struct ElementVtable {
    _unknown: UnknownVtable,
    _slots_3_13: [usize; 11],
    pattern: unsafe extern "system" fn(Raw, i32, *const Guid, *mut Raw) -> HResult,
    _slots_15_19: [usize; 5],
    process_id: unsafe extern "system" fn(Raw, *mut i32) -> HResult,
    _slots_21_34: [usize; 14],
    password: unsafe extern "system" fn(Raw, *mut i32) -> HResult,
    native_window: unsafe extern "system" fn(Raw, *mut Raw) -> HResult,
}
#[repr(C)]
struct TextPatternVtable {
    _unknown: UnknownVtable,
    _slots_3_4: [usize; 2],
    selection: ObjectGetter,
}
#[repr(C)]
struct TextRangeArrayVtable {
    _unknown: UnknownVtable,
    length: unsafe extern "system" fn(Raw, *mut i32) -> HResult,
    element: unsafe extern "system" fn(Raw, i32, *mut Raw) -> HResult,
}
#[repr(C)]
struct TextRangeVtable {
    _unknown: UnknownVtable,
    _slots_3_4: [usize; 2],
    compare_endpoints: unsafe extern "system" fn(Raw, i32, Raw, i32, *mut i32) -> HResult,
    _slots_6_11: [usize; 6],
    text: unsafe extern "system" fn(Raw, i32, *mut *mut u16) -> HResult,
}
#[repr(C)]
struct TreeWalkerVtable {
    _unknown: UnknownVtable,
    parent: unsafe extern "system" fn(Raw, Raw, *mut Raw) -> HResult,
}
#[repr(C)]
struct Interface<V> {
    vtable: *const V,
}
struct Com<V> {
    pointer: NonNull<Interface<V>>,
    _apartment_bound: PhantomData<Rc<()>>,
}
impl<V> Com<V> {
    fn raw(&self) -> Raw {
        self.pointer.as_ptr().cast()
    }
    fn vtable(&self) -> &V {
        unsafe { &*self.pointer.as_ref().vtable }
    }
    fn unknown(&self) -> &UnknownVtable {
        unsafe { &**self.raw().cast::<*const UnknownVtable>() }
    }
    fn query<U>(&self, iid: &Guid) -> Option<Com<U>> {
        owned(|out| unsafe { (self.unknown().query)(self.raw(), iid, out) })
    }
}
impl<V> Drop for Com<V> {
    fn drop(&mut self) {
        unsafe { (self.unknown().release)(self.raw()) };
    }
}
fn owned<V>(call: impl FnOnce(*mut Raw) -> HResult) -> Option<Com<V>> {
    let mut raw = std::ptr::null_mut();
    let status = call(&mut raw);
    let object = Com {
        pointer: NonNull::new(raw.cast::<Interface<V>>())?,
        _apartment_bound: PhantomData,
    };
    (status >= 0).then_some(object)
}
struct BStr(*mut u16);
impl BStr {
    fn len(&self) -> usize {
        unsafe { SysStringLen(self.0) as usize }
    }
    fn to_string(&self) -> String {
        if self.0.is_null() {
            String::new()
        } else {
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(self.0, self.len()) })
        }
    }
}
impl Drop for BStr {
    fn drop(&mut self) {
        unsafe { SysFreeString(self.0) }
    }
}
#[link(name = "user32")]
unsafe extern "system" {
    fn GetForegroundWindow() -> Raw;
    fn GetWindowThreadProcessId(window: Raw, process: *mut u32) -> u32;
    fn GetAncestor(window: Raw, flags: u32) -> Raw;
    fn IsWindow(window: Raw) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcessId() -> u32;
    fn GetCurrentThreadId() -> u32;
}
#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: Raw, flags: u32) -> HResult;
    fn CoUninitialize();
    fn CoCreateInstance(
        class: *const Guid,
        outer: Raw,
        context: u32,
        iid: *const Guid,
        out: *mut Raw,
    ) -> HResult;
    fn CoEnableCallCancellation(reserved: Raw) -> HResult;
    fn CoDisableCallCancellation(reserved: Raw) -> HResult;
    fn CoCancelCall(thread: u32, timeout: u32) -> HResult;
}
#[link(name = "oleaut32")]
unsafe extern "system" {
    fn SysStringLen(value: *const u16) -> u32;
    fn SysFreeString(value: *const u16);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn com_slots_match_the_windows_sdk() {
        let slot = size_of::<usize>();
        assert_eq!(size_of::<Guid>(), 16);
        assert_eq!(align_of::<Guid>(), 4);
        assert_eq!(size_of::<UnknownVtable>(), 3 * slot);
        assert_eq!(offset_of!(UnknownVtable, query), 0);
        assert_eq!(offset_of!(UnknownVtable, release), 2 * slot);
        assert_eq!(size_of::<AutomationVtable>(), 15 * slot);
        assert_eq!(offset_of!(AutomationVtable, focused), 8 * slot);
        assert_eq!(offset_of!(AutomationVtable, walker), 14 * slot);
        assert_eq!(offset_of!(Automation2Vtable, auto_focus), 59 * slot);
        assert_eq!(offset_of!(Automation2Vtable, connection_timeout), 61 * slot);
        assert_eq!(
            offset_of!(Automation2Vtable, transaction_timeout),
            63 * slot
        );
        assert_eq!(size_of::<Automation2Vtable>(), 64 * slot);
        assert_eq!(offset_of!(ElementVtable, pattern), 14 * slot);
        assert_eq!(offset_of!(ElementVtable, process_id), 20 * slot);
        assert_eq!(offset_of!(ElementVtable, password), 35 * slot);
        assert_eq!(offset_of!(ElementVtable, native_window), 36 * slot);
        assert_eq!(size_of::<ElementVtable>(), 37 * slot);
        assert_eq!(offset_of!(TextPatternVtable, selection), 5 * slot);
        assert_eq!(size_of::<TextPatternVtable>(), 6 * slot);
        assert_eq!(offset_of!(TextRangeArrayVtable, length), 3 * slot);
        assert_eq!(offset_of!(TextRangeArrayVtable, element), 4 * slot);
        assert_eq!(size_of::<TextRangeArrayVtable>(), 5 * slot);
        assert_eq!(offset_of!(TextRangeVtable, compare_endpoints), 5 * slot);
        assert_eq!(offset_of!(TextRangeVtable, text), 12 * slot);
        assert_eq!(size_of::<TextRangeVtable>(), 13 * slot);
        assert_eq!(offset_of!(TreeWalkerVtable, parent), 3 * slot);
        assert_eq!(size_of::<TreeWalkerVtable>(), 4 * slot);
    }

    #[test]
    fn already_cancelled_capture_does_not_query_the_desktop() {
        assert!(capture(&|| true).is_none());
    }

    #[test]
    fn expired_request_does_not_query_the_desktop() {
        let request = CaptureRequest {
            window: 0,
            process: 0,
            created_at: Instant::now() - CAPTURE_TIMEOUT,
        };
        assert!(request.capture(&|| false).is_none());
    }
}

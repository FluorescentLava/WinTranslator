//! Focus the actual ContentIsland, then verify the target in its XamlRoot.
//! These public interface IDs and ABI prefixes are from Windows App SDK 2.4
//! Microsoft.UI[.Xaml].winmd. A successful UIElement::Focus alone can leave a
//! logical focus target in an inactive island that cannot receive keyboard input.
use super::{FocusState, IWindowNative, UIElement, Window, XamlRoot};
use core::ffi::c_void;
use std::cell::Cell;
use windows_core::{HRESULT, IInspectable_Vtbl, IUnknown, Interface, Result, RuntimeName};

type Raw = *mut c_void;
type ObjectGetter = unsafe extern "system" fn(Raw, *mut Raw) -> HRESULT;
type IslandFactory = unsafe extern "system" fn(Raw, Raw, *mut Raw) -> HRESULT;

windows_core::imp::define_interface!(
    XamlRoot4,
    XamlRoot4Vtable,
    0x377bec22_632b_52be_b26f_5edf7838e5ca
);
#[repr(C)]
pub struct XamlRoot4Vtable {
    base: IInspectable_Vtbl,
    content_island: ObjectGetter,
}

windows_core::imp::define_interface!(
    InputFocusControllerStatics,
    InputFocusControllerStaticsVtable,
    0xaeb311da_da9b_5a1b_92f4_83ddde933e00
);
#[repr(C)]
pub struct InputFocusControllerStaticsVtable {
    base: IInspectable_Vtbl,
    get_for_island: IslandFactory,
}
struct InputFocusControllerClass;
impl RuntimeName for InputFocusControllerClass {
    const NAME: &'static str = "Microsoft.UI.Input.InputFocusController";
}
windows_core::imp::define_interface!(
    InputFocusController,
    InputFocusControllerVtable,
    0x8dfdc26c_8b8d_515d_8ddd_4685b3a540e9
);
#[repr(C)]
pub struct InputFocusControllerVtable {
    base: IInspectable_Vtbl,
    has_focus: unsafe extern "system" fn(Raw, *mut bool) -> HRESULT,
    try_set_focus: unsafe extern "system" fn(Raw, *mut bool) -> HRESULT,
}

windows_core::imp::define_interface!(
    InputActivationListenerStatics,
    InputActivationListenerStaticsVtable,
    0x7ea26120_9636_5292_a7b1_56544ac51a22
);
#[repr(C)]
pub struct InputActivationListenerStaticsVtable {
    base: IInspectable_Vtbl,
    get_for_island: IslandFactory,
}
struct InputActivationListenerClass;
impl RuntimeName for InputActivationListenerClass {
    const NAME: &'static str = "Microsoft.UI.Input.InputActivationListener";
}
windows_core::imp::define_interface!(
    InputActivationListener,
    InputActivationListenerVtable,
    0x3b818627_6ce7_5e0d_a0f5_6684fd1aec78
);
#[repr(C)]
pub struct InputActivationListenerVtable {
    base: IInspectable_Vtbl,
    state: unsafe extern "system" fn(Raw, *mut i32) -> HRESULT,
}
// InputActivationState: None = 0, Deactivated = 1, Activated = 2.
const INPUT_ACTIVATED: i32 = 2;

windows_core::imp::define_interface!(
    FocusManagerStatics,
    FocusManagerStaticsVtable,
    0xe73dce04_e23a_5fb3_96ab_7df04c51dff2
);
#[repr(C)]
pub struct FocusManagerStaticsVtable {
    base: IInspectable_Vtbl,
    // Slot 19 is the overload without a root; slot 20 is root-scoped.
    preceding_methods: [usize; 20],
    get_focused_element_for_root: IslandFactory,
}
struct FocusManagerClass;
impl RuntimeName for FocusManagerClass {
    const NAME: &'static str = "Microsoft.UI.Xaml.Input.FocusManager";
}

windows_core::link!("user32.dll" "system" fn GetForegroundWindow() -> Raw);
windows_core::link!("user32.dll" "system" fn IsWindowVisible(hwnd: Raw) -> i32);
windows_core::link!("user32.dll" "system" fn GetActiveWindow() -> Raw);
windows_core::link!("user32.dll" "system" fn SetActiveWindow(hwnd: Raw) -> Raw);

#[derive(Default)]
pub(super) struct InitialActivation {
    restarted: Cell<bool>,
}

impl InitialActivation {
    fn repair(&self, window: &Window, hwnd: Raw, snapshot: FocusSnapshot) -> Result<()> {
        if snapshot.input_active()
            || self.restarted.get()
            || !owns_foreground(hwnd)
            || unsafe { GetActiveWindow() } != hwnd
        {
            return Ok(());
        }
        self.restarted.set(true);
        // The first HWND activation can precede ContentIsland readiness. Once
        // its listener exists, another Activate on the already-active HWND is
        // a no-op. Re-enter native activation once so WinUI receives a genuine
        // WM_ACTIVATE with the ready island, before revealing the Mica card.
        // Deactivate only our own window; never activate another application.
        unsafe { SetActiveWindow(std::ptr::null_mut()) };
        let foreground = unsafe { GetForegroundWindow() };
        if unsafe { IsWindowVisible(hwnd) } != 0 && (foreground == hwnd || foreground.is_null()) {
            window.Activate()?;
        }
        Ok(())
    }
}

fn owns_foreground(hwnd: Raw) -> bool {
    !hwnd.is_null()
        && unsafe { IsWindowVisible(hwnd) } != 0
        && unsafe { GetForegroundWindow() } == hwnd
}

struct InputSite {
    root: XamlRoot,
    controller: InputFocusController,
    activation: InputActivationListener,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct FocusSnapshot {
    pub island_has_focus: bool,
    pub activation_state: i32,
    pub element_is_focused: bool,
}
impl FocusSnapshot {
    pub fn input_active(self) -> bool {
        self.activation_state == INPUT_ACTIVATED
    }
    pub fn verified(self) -> bool {
        self.island_has_focus && self.input_active() && self.element_is_focused
    }
}

impl InputSite {
    fn for_element(element: &UIElement) -> Result<Self> {
        let root = element.XamlRoot()?;
        let root4 = root.cast::<XamlRoot4>()?;
        let controller_factory = windows_core::imp::load_factory::<
            InputFocusControllerClass,
            InputFocusControllerStatics,
        >()?;
        let activation_factory = windows_core::imp::load_factory::<
            InputActivationListenerClass,
            InputActivationListenerStatics,
        >()?;
        unsafe {
            let mut island = std::ptr::null_mut();
            (root4.vtable().content_island)(root4.as_raw(), &mut island).ok()?;
            let island: IUnknown = windows_core::imp::Type::from_abi(island)?;
            let mut controller = std::ptr::null_mut();
            (controller_factory.vtable().get_for_island)(
                controller_factory.as_raw(),
                island.as_raw(),
                &mut controller,
            )
            .ok()?;
            let controller: InputFocusController = windows_core::imp::Type::from_abi(controller)?;
            let mut activation = std::ptr::null_mut();
            (activation_factory.vtable().get_for_island)(
                activation_factory.as_raw(),
                island.as_raw(),
                &mut activation,
            )
            .ok()?;
            let activation: InputActivationListener =
                windows_core::imp::Type::from_abi(activation)?;
            Ok(Self {
                root,
                controller,
                activation,
            })
        }
    }

    fn snapshot(&self, element: &UIElement) -> Result<FocusSnapshot> {
        let manager = windows_core::imp::load_factory::<FocusManagerClass, FocusManagerStatics>()?;
        unsafe {
            let mut island_has_focus = false;
            (self.controller.vtable().has_focus)(self.controller.as_raw(), &mut island_has_focus)
                .ok()?;
            let mut activation_state = 0;
            (self.activation.vtable().state)(self.activation.as_raw(), &mut activation_state)
                .ok()?;
            let mut focused = std::ptr::null_mut();
            (manager.vtable().get_focused_element_for_root)(
                manager.as_raw(),
                self.root.as_raw(),
                &mut focused,
            )
            .ok()?;
            // Compare canonical IUnknown identities, not different projections
            // of the same TextBox returned through UIElement and IInspectable.
            let element_is_focused = if focused.is_null() {
                false
            } else {
                let focused = IUnknown::from_raw(focused).cast::<IUnknown>()?;
                focused.as_raw() == element.cast::<IUnknown>()?.as_raw()
            };
            Ok(FocusSnapshot {
                island_has_focus,
                activation_state,
                element_is_focused,
            })
        }
    }
}

/// Read-only observation, also used by the hidden-window regression probe.
#[allow(dead_code)]
pub(super) fn inspect(element: &UIElement) -> Result<FocusSnapshot> {
    InputSite::for_element(element)?.snapshot(element)
}

/// Never activate an unrelated or hidden window as a side effect of focusing a
/// control. Every operation that can dispatch input rechecks this native gate.
pub(super) fn focus(
    element: &UIElement,
    window: &Window,
    initial_activation: &InitialActivation,
) -> Result<bool> {
    let mut hwnd = std::ptr::null_mut();
    unsafe {
        window
            .cast::<IWindowNative>()?
            .WindowHandle(&mut hwnd)
            .ok()?;
    }
    let foreground = || owns_foreground(hwnd);
    if !foreground() {
        return Ok(false);
    }
    let site = InputSite::for_element(element)?;
    let before = site.snapshot(element)?;
    if !before.input_active() {
        if !foreground() {
            return Ok(false);
        }
        // Re-enter WinUI's activation path so the XamlRoot and Mica receive the
        // real activation event. This does not synthesize or override backdrop policy.
        window.Activate()?;
    }
    if !foreground() {
        return Ok(false);
    }
    let mut activated = site.snapshot(element)?;
    if !activated.input_active() {
        initial_activation.repair(window, hwnd, activated)?;
        if !foreground() {
            return Ok(false);
        }
        activated = site.snapshot(element)?;
    }
    if !activated.island_has_focus || !activated.input_active() {
        let mut accepted = false;
        unsafe {
            (site.controller.vtable().try_set_focus)(site.controller.as_raw(), &mut accepted)
                .ok()?;
        }
    }
    if !foreground() {
        return Ok(false);
    }
    let _ = element.Focus(FocusState::Programmatic)?;
    Ok(foreground() && site.snapshot(element)?.verified())
}

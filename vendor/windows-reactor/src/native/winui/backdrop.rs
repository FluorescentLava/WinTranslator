//! Stable WinUI/Windows App SDK 2.x card and an aggregated transparent window backdrop.
//! Interface IDs and method order are verified against Windows App Runtime 2.4 metadata;
//! the OS compositor ABI is verified against the Windows SDK 10.0.26100 headers.
use super::*;
use ::core;
use ::core::ffi::c_void;
use windows_core::{
    Compose, Error, GUID, HRESULT, IInspectable_Vtbl, IUnknown, IUnknownImpl, Ref, Result,
    RuntimeName, RuntimeType, implement_decl,
};

type Raw = *mut c_void;
type CreateInstance = unsafe extern "system" fn(Raw, Raw, *mut Raw, *mut Raw) -> HRESULT;

windows_core::imp::define_interface!(
    BackdropFactory,
    BackdropFactoryVtable,
    0x1e07656b_fad2_5b29_913f_b6748bc45942
);
#[repr(C)]
pub struct BackdropFactoryVtable {
    base: IInspectable_Vtbl,
    create_instance: CreateInstance,
}
windows_core::imp::define_interface!(
    ElementFactory,
    ElementFactoryVtable,
    0x7937bec0_7c3c_5ba9_9f3c_33cbc7d72fe0
);
#[repr(C)]
pub struct ElementFactoryVtable {
    base: IInspectable_Vtbl,
    create_instance: CreateInstance,
}
windows_core::imp::define_interface!(
    BackdropElement,
    BackdropElementVtable,
    0x6e741848_7dc4_5fe1_9bfb_942249e62ae1
);
#[repr(C)]
pub struct BackdropElementVtable {
    base: IInspectable_Vtbl,
    get_backdrop: unsafe extern "system" fn(Raw, *mut Raw) -> HRESULT,
    set_backdrop: unsafe extern "system" fn(Raw, Raw) -> HRESULT,
    get_corners: unsafe extern "system" fn(Raw, *mut bindings::CornerRadius) -> HRESULT,
    set_corners: unsafe extern "system" fn(Raw, bindings::CornerRadius) -> HRESULT,
}
struct BackdropElementClass;
impl RuntimeName for BackdropElementClass {
    const NAME: &'static str = "Microsoft.UI.Xaml.Controls.SystemBackdropElement";
}

windows_core::imp::define_interface!(
    BackdropTarget,
    BackdropTargetVtable,
    0x397dafe4_b6c2_5bb9_951d_f5707de8b7bc
);
#[repr(C)]
pub struct BackdropTargetVtable {
    base: IInspectable_Vtbl,
    get_brush: unsafe extern "system" fn(Raw, *mut Raw) -> HRESULT,
    set_brush: unsafe extern "system" fn(Raw, Raw) -> HRESULT,
}
impl RuntimeType for BackdropTarget {
    const SIGNATURE: windows_core::imp::ConstBuffer =
        windows_core::imp::ConstBuffer::for_interface::<Self>();
}
impl BackdropTarget {
    fn set_brush(&self, brush: Option<&IUnknown>) -> Result<()> {
        unsafe {
            (self.vtable().set_brush)(
                self.as_raw(),
                brush.map_or(std::ptr::null_mut(), Interface::as_raw),
            )
            .ok()
        }
    }
}

// Windows.UI.Composition is the OS compositor used by ICompositionSupportsSystemBackdrop.
// It is intentionally distinct from the lifted Microsoft.UI.Composition visual tree.
windows_core::imp::define_interface!(
    OsCompositor,
    OsCompositorVtable,
    0xb403ca50_7f8c_4e83_985f_cc45060036d8
);
#[repr(C)]
pub struct OsCompositorVtable {
    base: IInspectable_Vtbl,
    create_color_frames: usize,
    create_color_brush: usize,
    create_color_brush_with_color:
        unsafe extern "system" fn(Raw, bindings::Color, *mut Raw) -> HRESULT,
}
struct OsCompositorClass;
impl RuntimeName for OsCompositorClass {
    const NAME: &'static str = "Windows.UI.Composition.Compositor";
}
windows_core::imp::define_interface!(
    OsBrush,
    IInspectable_Vtbl,
    0xab0d7608_30c0_40e9_b568_b60a6bd1fb46
);
windows_core::imp::define_interface!(
    SystemDispatcherQueue,
    SystemDispatcherQueueVtable,
    0x14a7a175_5c27_5a35_b079_21960cf764a8
);
#[repr(C)]
pub struct SystemDispatcherQueueVtable {
    base: IInspectable_Vtbl,
    enqueue_event_loop_exit: usize,
    ensure_system_dispatcher_queue: unsafe extern "system" fn(Raw) -> HRESULT,
}

windows_core::imp::define_interface!(
    BackdropOverrides,
    BackdropOverridesVtable,
    0xeb1f5399_cad7_5611_b637_09d76a07e708
);
impl RuntimeType for BackdropOverrides {
    const SIGNATURE: windows_core::imp::ConstBuffer =
        windows_core::imp::ConstBuffer::for_interface::<Self>();
    const NAME: windows_core::imp::ConstBuffer = windows_core::imp::ConstBuffer::from_slice(
        b"Microsoft.UI.Xaml.Media.ISystemBackdropOverrides",
    );
}
impl RuntimeName for BackdropOverrides {
    const NAME: &'static str = "Microsoft.UI.Xaml.Media.ISystemBackdropOverrides";
}
pub(super) trait BackdropOverridesImpl: IUnknownImpl {
    fn connected(&self, target: Ref<BackdropTarget>, root: Ref<XamlRoot>) -> Result<()>;
    fn disconnected(&self, target: Ref<BackdropTarget>) -> Result<()>;
    fn changed(&self, target: Ref<BackdropTarget>, root: Ref<XamlRoot>) -> Result<()>;
}
#[repr(C)]
pub struct BackdropOverridesVtable {
    base: IInspectable_Vtbl,
    connected: unsafe extern "system" fn(Raw, Raw, Raw) -> HRESULT,
    disconnected: unsafe extern "system" fn(Raw, Raw) -> HRESULT,
    changed: unsafe extern "system" fn(Raw, Raw, Raw) -> HRESULT,
}
impl BackdropOverridesVtable {
    pub const fn new<Identity: BackdropOverridesImpl, const OFFSET: isize>() -> Self {
        unsafe extern "system" fn connected<
            Identity: BackdropOverridesImpl,
            const OFFSET: isize,
        >(
            this: Raw,
            target: Raw,
            root: Raw,
        ) -> HRESULT {
            unsafe {
                let this = &*((this as *const *const ()).offset(OFFSET) as *const Identity);
                this.connected(
                    std::mem::transmute_copy(&target),
                    std::mem::transmute_copy(&root),
                )
                .into()
            }
        }
        unsafe extern "system" fn disconnected<
            Identity: BackdropOverridesImpl,
            const OFFSET: isize,
        >(
            this: Raw,
            target: Raw,
        ) -> HRESULT {
            unsafe {
                let this = &*((this as *const *const ()).offset(OFFSET) as *const Identity);
                this.disconnected(std::mem::transmute_copy(&target)).into()
            }
        }
        unsafe extern "system" fn changed<Identity: BackdropOverridesImpl, const OFFSET: isize>(
            this: Raw,
            target: Raw,
            root: Raw,
        ) -> HRESULT {
            unsafe {
                let this = &*((this as *const *const ()).offset(OFFSET) as *const Identity);
                this.changed(
                    std::mem::transmute_copy(&target),
                    std::mem::transmute_copy(&root),
                )
                .into()
            }
        }
        Self {
            base: IInspectable_Vtbl::new::<Identity, BackdropOverrides, OFFSET>(),
            connected: connected::<Identity, OFFSET>,
            disconnected: disconnected::<Identity, OFFSET>,
            changed: changed::<Identity, OFFSET>,
        }
    }
    pub fn matches(iid: &GUID) -> bool {
        iid == &BackdropOverrides::IID
    }
}

implement_decl! {
    impl TransparentOverrides as TransparentOverrides_Impl: [BackdropOverrides]
}
struct TransparentOverrides {
    hwnd: isize,
    compositor: OsCompositor,
    brush: RefCell<Option<IUnknown>>,
    connections: Rc<Cell<u32>>,
    disconnections: Rc<Cell<u32>>,
}
impl Drop for TransparentOverrides {
    fn drop(&mut self) {
        // Also covers a failed attach before WinUI can issue OnTargetDisconnected.
        unsafe {
            RemoveWindowSubclass(self.hwnd as Raw, Some(transparent_window_proc), SUBCLASS_ID);
        }
    }
}
impl TransparentOverrides_Impl {
    fn base_overrides(&self) -> Result<BackdropOverrides> {
        self.base
            .as_option()
            .as_ref()
            .ok_or_else(Error::empty)?
            .cast()
    }
}
impl BackdropOverridesImpl for TransparentOverrides_Impl {
    fn connected(&self, target: Ref<BackdropTarget>, root: Ref<XamlRoot>) -> Result<()> {
        let target = target.as_ref().ok_or_else(Error::empty)?;
        configure_dwm(self.hwnd)?;
        unsafe {
            if SetWindowSubclass(
                self.hwnd as Raw,
                Some(transparent_window_proc),
                SUBCLASS_ID,
                0,
            ) == 0
            {
                return Err(Error::from_thread());
            }
            let dc = GetDC(self.hwnd as Raw);
            if !dc.is_null() {
                clear_background(self.hwnd as Raw, dc);
                ReleaseDC(self.hwnd as Raw, dc);
            }
        }
        let mut raw = std::ptr::null_mut();
        unsafe {
            (self.compositor.vtable().create_color_brush_with_color)(
                self.compositor.as_raw(),
                bindings::Color::default(),
                &mut raw,
            )
            .ok()?;
        }
        let brush = unsafe { IUnknown::from_raw(raw) }
            .cast::<OsBrush>()?
            .cast::<IUnknown>()?;
        target.set_brush(Some(&brush))?;
        *self.brush.borrow_mut() = Some(brush);
        let base = self.base_overrides()?;
        unsafe {
            (base.vtable().connected)(
                base.as_raw(),
                target.as_raw(),
                root.as_ref()
                    .map_or(std::ptr::null_mut(), Interface::as_raw),
            )
            .ok()?;
        }
        self.connections.set(self.connections.get() + 1);
        Ok(())
    }
    fn disconnected(&self, target: Ref<BackdropTarget>) -> Result<()> {
        if let Some(target) = target.as_ref() {
            target.set_brush(None)?;
        }
        self.brush.borrow_mut().take();
        unsafe {
            RemoveWindowSubclass(self.hwnd as Raw, Some(transparent_window_proc), SUBCLASS_ID);
        }
        let base = self.base_overrides()?;
        unsafe {
            (base.vtable().disconnected)(
                base.as_raw(),
                target
                    .as_ref()
                    .map_or(std::ptr::null_mut(), Interface::as_raw),
            )
            .ok()?;
        }
        self.disconnections.set(self.disconnections.get() + 1);
        Ok(())
    }
    fn changed(&self, target: Ref<BackdropTarget>, root: Ref<XamlRoot>) -> Result<()> {
        let base = self.base_overrides()?;
        unsafe {
            (base.vtable().changed)(
                base.as_raw(),
                target
                    .as_ref()
                    .map_or(std::ptr::null_mut(), Interface::as_raw),
                root.as_ref()
                    .map_or(std::ptr::null_mut(), Interface::as_raw),
            )
            .ok()
        }
    }
}

struct TransparentBackdrop {
    value: SystemBackdrop,
    _connections: Rc<Cell<u32>>,
    _disconnections: Rc<Cell<u32>>,
}
impl TransparentBackdrop {
    fn new(hwnd: isize) -> Result<Self> {
        let compositor = windows_core::imp::load_factory::<
            OsCompositorClass,
            windows_core::imp::IGenericFactory,
        >()?
        .ActivateInstance::<OsCompositor>()?;
        let connections = Rc::new(Cell::new(0));
        let disconnections = Rc::new(Cell::new(0));
        let implementation = TransparentOverrides {
            hwnd,
            compositor,
            brush: RefCell::new(None),
            connections: connections.clone(),
            disconnections: disconnections.clone(),
        };
        let factory = windows_core::imp::load_factory::<SystemBackdrop, BackdropFactory>()?;
        let value = unsafe {
            let (outer, inner) = Compose::compose(implementation);
            let mut raw = std::ptr::null_mut();
            (factory.vtable().create_instance)(
                factory.as_raw(),
                outer.as_raw(),
                inner as *mut _ as _,
                &mut raw,
            )
            .ok()?;
            let value = IUnknown::from_raw(raw).cast::<SystemBackdrop>()?;
            let _ = &outer;
            value
        };
        Ok(Self {
            value,
            _connections: connections,
            _disconnections: disconnections,
        })
    }
}

// Public WinRT interfaces not included in this runtime's generated subset.
// Method indices below exclude IInspectable's six slots and are verified
// against the installed Microsoft.UI[.Xaml].winmd, version 2.4.
windows_core::imp::define_interface!(
    CardThemeShadow,
    IInspectable_Vtbl,
    0xc264208a_d1f4_58ae_8a88_fc59148bee69
);
windows_core::imp::define_interface!(
    CardCompositionObject,
    IInspectable_Vtbl,
    0x0e583d49_fb5e_5481_a426_d3c41e059a5a
);
windows_core::imp::define_interface!(
    CardVisual,
    IInspectable_Vtbl,
    0xc0eeab6c_c897_5ac6_a1c9_63abd5055b9b
);
windows_core::imp::define_interface!(
    CardCompositor5,
    IInspectable_Vtbl,
    0xbb52d644_a030_5c19_b883_577ded739ae7
);
windows_core::imp::define_interface!(
    CardCompositor6,
    IInspectable_Vtbl,
    0x5fd20200_c188_5737_b567_5e61741af19c
);
windows_core::imp::define_interface!(
    CardGeometry,
    IInspectable_Vtbl,
    0x4e40bdb2_450b_5a81_9e9b_149417980cc4
);
windows_core::imp::define_interface!(
    CardClip,
    IInspectable_Vtbl,
    0xb66b55cb_b5a5_5bee_8972_ae78233cb34c
);
windows_core::imp::define_interface!(
    CardRoundedRectangle,
    IInspectable_Vtbl,
    0x02eafc87_8d1f_5445_a416_d81baee8a750
);
unsafe fn native_method<T: Copy>(object: &impl Interface, index: usize) -> T {
    unsafe {
        let table = *(object.as_raw() as *const *const usize);
        std::mem::transmute_copy(&*table.add(6 + index))
    }
}
fn native_object(object: &impl Interface, index: usize) -> Result<IUnknown> {
    let mut result = std::ptr::null_mut();
    unsafe {
        native_method::<unsafe extern "system" fn(Raw, *mut Raw) -> HRESULT>(object, index)(
            object.as_raw(),
            &mut result,
        )
        .ok()?;
        if result.is_null() {
            return Err(Error::empty());
        }
        Ok(IUnknown::from_raw(result))
    }
}
fn set_clip_size(geometry: &CardRoundedRectangle, width: f64, height: f64) -> Result<()> {
    unsafe {
        native_method::<unsafe extern "system" fn(Raw, windows_numerics::Vector2) -> HRESULT>(
            geometry, 5,
        )(
            geometry.as_raw(),
            windows_numerics::Vector2 {
                x: width.max(0.0) as f32,
                y: height.max(0.0) as f32,
            },
        )
        .ok()
    }
}
fn overlay_corners(caster: &bindings::Border) -> Result<bindings::CornerRadius> {
    let mut corners = bindings::CornerRadius::default();
    // Read the resolved WinUI OverlayCornerRadius from the native Border.
    // Its public IBorder getter is slot 8, immediately before SetCornerRadius.
    unsafe {
        native_method::<unsafe extern "system" fn(Raw, *mut bindings::CornerRadius) -> HRESULT>(
            caster, 8,
        )(caster.as_raw(), &mut corners)
        .ok()?;
    }
    Ok(corners)
}
fn round_content(drawing: &bindings::Grid, radius: f64) -> Result<windows_core::EventRevoker> {
    // Clip the material/content/stroke, while keeping the caster and its native
    // ThemeShadow outside the clip. Composition owns a real rounded geometry;
    // only layout size changes update its bounds, never an animation timer.
    let visual = ElementCompositionPreview::GetElementVisual(&drawing.cast::<UIElement>()?)?;
    let compositor = native_object(&visual.cast::<CardCompositionObject>()?, 0)?;
    let geometry = native_object(&compositor.cast::<CardCompositor5>()?, 14)?
        .cast::<CardRoundedRectangle>()?;
    unsafe {
        native_method::<unsafe extern "system" fn(Raw, windows_numerics::Vector2) -> HRESULT>(
            &geometry, 1,
        )(
            geometry.as_raw(),
            windows_numerics::Vector2 {
                x: radius as f32,
                y: radius as f32,
            },
        )
        .ok()?;
    }
    let element = drawing.cast::<FrameworkElement>()?;
    set_clip_size(&geometry, element.ActualWidth()?, element.ActualHeight()?)?;
    let clip_factory = compositor.cast::<CardCompositor6>()?;
    let mut raw = std::ptr::null_mut();
    unsafe {
        native_method::<unsafe extern "system" fn(Raw, Raw, *mut Raw) -> HRESULT>(&clip_factory, 1)(
            clip_factory.as_raw(), geometry.cast::<CardGeometry>()?.as_raw(), &mut raw,
        ).ok()?;
        let clip = IUnknown::from_raw(raw).cast::<CardClip>()?;
        let visual = visual.cast::<CardVisual>()?;
        native_method::<unsafe extern "system" fn(Raw, Raw) -> HRESULT>(&visual, 9)(
            visual.as_raw(),
            clip.as_raw(),
        )
        .ok()?;
    }
    element.SizeChanged(move |_, args| {
        if let Some(args) = args.as_ref() {
            if let Ok(size) = args.NewSize() {
                _ = set_clip_size(&geometry, f64::from(size.width), f64::from(size.height));
            }
        }
    })
}

pub(super) struct WindowHost {
    pub root: Option<NodeId>,
    pub host: bindings::Grid,
    pub initial_activation: input_activation::InitialActivation,
    card: Option<MicaCard>,
}
struct MicaCard {
    surface: bindings::Grid,
    caster: bindings::Border,
    content: bindings::Grid,
    element: BackdropElement,
    mica: SystemBackdrop,
    _clip_size_changed: windows_core::EventRevoker,
    _frame: presenter_frame::PresenterFrame,
    transparent: TransparentBackdrop,
}
impl WindowHost {
    pub fn new() -> Result<Self> {
        Ok(Self {
            root: None,
            host: bindings::Grid::new()?,
            initial_activation: input_activation::InitialActivation::default(),
            card: None,
        })
    }
    pub fn content(&self) -> &bindings::Grid {
        self.card.as_ref().map_or(&self.host, |card| &card.content)
    }
    pub fn animated_surface(&self) -> Option<&bindings::Grid> {
        self.card.as_ref().map(|card| &card.surface)
    }
    pub fn prepare_slide(&self, window: &Window, entering: bool) -> Result<bool> {
        let mut hwnd = std::ptr::null_mut();
        unsafe {
            window
                .cast::<IWindowNative>()?
                .WindowHandle(&mut hwnd)
                .ok()?;
        }
        // Win32 foreground alone does not initialize WinUI's input site after
        // the final frameless viewport has been committed. Re-enter WinUI's
        // activation path only while this visible window still owns foreground.
        // Its XamlRoot input-activation event drives the built-in Mica policy.
        let activate = entering
            && unsafe { IsWindowVisible(hwnd) } != 0
            && unsafe { GetForegroundWindow() } == hwnd;
        if activate {
            window.Activate()?;
        }
        let element = self.host.cast::<IUIElement>()?;
        unsafe {
            native_method::<unsafe extern "system" fn(Raw) -> HRESULT>(&element, 214)(
                element.as_raw(),
            )
            .ok()?;
        }
        Ok(activate)
    }
    pub fn slide_layout(&self, window: &Window) -> Result<Option<slide::LayoutGate>> {
        let Some(card) = &self.card else {
            return Ok(None);
        };
        let mut hwnd = std::ptr::null_mut();
        unsafe {
            window
                .cast::<IWindowNative>()?
                .WindowHandle(&mut hwnd)
                .ok()?;
        }
        let mut client: Rect = unsafe { std::mem::zeroed() };
        if unsafe { GetClientRect(hwnd, &mut client) } == 0 {
            return Err(Error::from_thread());
        }
        let scale = f64::from(unsafe { GetDpiForWindow(hwnd) }.max(96)) / 96.0;
        let children = card.content.cast::<IPanel>()?.Children()?;
        let card_root = if children.Size()? > 0 {
            Some(children.GetAt(0)?.cast::<FrameworkElement>()?)
        } else {
            None
        };
        Ok(Some(slide::LayoutGate {
            host: self.host.cast::<FrameworkElement>()?,
            content: card.content.cast::<FrameworkElement>()?,
            host_width: f64::from(client.right - client.left) / scale,
            host_height: f64::from(client.bottom - client.top) / scale,
            card_root,
        }))
    }
    #[cfg(feature = "test")]
    pub fn lifecycle_counts(&self) -> Option<(u32, u32)> {
        self.card.as_ref().map(|card| {
            (
                card.transparent._connections.get(),
                card.transparent._disconnections.get(),
            )
        })
    }
    pub fn enable_card(&mut self, window: &Window, inset: u16) -> Result<()> {
        if let Some(card) = &self.card {
            set_card_inset(&card.caster, inset)?;
            // disconnect() releases both targets; reconnect the moving Mica
            // element as well as the transparent window target on reuse.
            unsafe {
                (card.element.vtable().set_backdrop)(card.element.as_raw(), card.mica.as_raw())
                    .ok()?;
            }
            return window
                .cast::<IWindow2>()?
                .SetSystemBackdrop(&card.transparent.value);
        }
        let dispatcher = DispatcherQueue::GetForCurrentThread()?.cast::<SystemDispatcherQueue>()?;
        unsafe {
            (dispatcher.vtable().ensure_system_dispatcher_queue)(dispatcher.as_raw()).ok()?;
        }
        let mut hwnd = std::ptr::null_mut();
        unsafe {
            window
                .cast::<IWindowNative>()?
                .WindowHandle(&mut hwnd)
                .ok()?;
        }
        let frame = presenter_frame::PresenterFrame::hide(window)?;
        let surface = bindings::Grid::new()?;
        surface.cast::<UIElement>()?.SetOpacity(0.0)?;
        // The animated root includes the transparent shadow gutter. A receiver
        // confined to the card bounds would cut off the projected shadow.
        let receiver = XamlReader::Load(
            r#"<Grid xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" Background="Transparent" IsHitTestVisible="False"/>"#,
        )?.cast::<bindings::Grid>()?;
        let caster = XamlReader::Load(
            r#"<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" Background="Transparent" CornerRadius="{ThemeResource OverlayCornerRadius}" Translation="0,0,32"><Border.Shadow><ThemeShadow/></Border.Shadow></Border>"#,
        )?.cast::<bindings::Border>()?;
        set_card_inset(&caster, inset)?;
        // The native host has a stable maximum height. The app root supplies
        // the current card height, and this caster grows upward from its bottom.
        caster
            .cast::<FrameworkElement>()?
            .SetVerticalAlignment(bindings::VerticalAlignment::Bottom)?;
        let corners = overlay_corners(&caster)?;
        let shadow =
            native_object(&caster.cast::<IUIElement>()?, 105)?.cast::<CardThemeShadow>()?;
        native_object(&shadow, 0)?
            .cast::<windows_collections::IVector<UIElement>>()?
            .Append(&receiver.cast::<UIElement>()?)?;
        let drawing = bindings::Grid::new()?;
        let content = bindings::Grid::new()?;
        let (element, mica) = create_backdrop_element(corners)?;
        // Draw the one-DIP theme stroke as an overlay, so adding it does not
        // reduce the application's content viewport or add internal padding.
        let stroke = XamlReader::Load(
            r#"<Border xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" BorderBrush="{ThemeResource SurfaceStrokeColorFlyoutBrush}" BorderThickness="1" CornerRadius="{ThemeResource OverlayCornerRadius}" IsHitTestVisible="False"/>"#,
        )?.cast::<UIElement>()?;
        let drawing_children = drawing.cast::<IPanel>()?.Children()?;
        drawing_children.Append(&element.cast::<UIElement>()?)?;
        drawing_children.Append(&content.cast::<UIElement>()?)?;
        drawing_children.Append(&stroke)?;
        caster.SetChild(&drawing.cast::<UIElement>()?)?;
        let clip_size_changed = round_content(&drawing, corners.top_left)?;
        let surface_children = surface.cast::<IPanel>()?.Children()?;
        surface_children.Append(&receiver.cast::<UIElement>()?)?;
        surface_children.Append(&caster.cast::<UIElement>()?)?;
        let transparent = TransparentBackdrop::new(hwnd as isize)?;
        move_children(&self.host, &content)?;
        self.host
            .cast::<IPanel>()?
            .Children()?
            .Append(&surface.cast::<UIElement>()?)?;
        self.card = Some(MicaCard {
            surface,
            caster,
            content,
            element,
            mica,
            _clip_size_changed: clip_size_changed,
            _frame: frame,
            transparent,
        });
        let card = self.card.as_ref().unwrap();
        if let Err(error) = window
            .cast::<IWindow2>()?
            .SetSystemBackdrop(&card.transparent.value)
        {
            self.disable_card(window)?;
            return Err(error);
        }
        Ok(())
    }
    pub fn disable_card(&mut self, window: &Window) -> Result<()> {
        let Some(card) = self.card.take() else {
            return Ok(());
        };
        window
            .cast::<IWindow2>()?
            .SetSystemBackdrop(None::<&SystemBackdrop>)?;
        unsafe {
            (card.element.vtable().set_backdrop)(card.element.as_raw(), std::ptr::null_mut())
                .ok()?;
        }
        self.host.cast::<IPanel>()?.Children()?.Clear()?;
        move_children(&card.content, &self.host)
    }
    pub fn disconnect(&mut self, window: &Window) {
        if let Some(card) = self.card.as_ref() {
            _ = window
                .cast::<IWindow2>()
                .and_then(|window| window.SetSystemBackdrop(None::<&SystemBackdrop>));
            unsafe {
                _ = (card.element.vtable().set_backdrop)(
                    card.element.as_raw(),
                    std::ptr::null_mut(),
                );
            }
        }
    }
}
fn set_card_inset(caster: &bindings::Border, inset: u16) -> Result<()> {
    caster
        .cast::<FrameworkElement>()?
        .SetMargin(bindings::Thickness {
            left: f64::from(inset),
            top: f64::from(inset),
            right: f64::from(inset),
            bottom: f64::from(inset),
        })
}
fn move_children(from: &bindings::Grid, to: &bindings::Grid) -> Result<()> {
    let source = from.cast::<IPanel>()?.Children()?;
    let target = to.cast::<IPanel>()?.Children()?;
    while source.Size()? > 0 {
        let child = source.GetAt(0)?;
        source.RemoveAt(0)?;
        target.Append(&child)?;
    }
    Ok(())
}
fn create_backdrop_element(
    corners: bindings::CornerRadius,
) -> Result<(BackdropElement, SystemBackdrop)> {
    let factory = windows_core::imp::load_factory::<BackdropElementClass, ElementFactory>()?;
    let mut inner = std::ptr::null_mut();
    let mut raw = std::ptr::null_mut();
    unsafe {
        (factory.vtable().create_instance)(
            factory.as_raw(),
            std::ptr::null_mut(),
            &mut inner,
            &mut raw,
        )
        .ok()?;
        if !inner.is_null() {
            drop(IUnknown::from_raw(inner));
        }
    }
    let element = unsafe { IUnknown::from_raw(raw) }.cast::<BackdropElement>()?;
    let mica = MicaBackdrop::new()?.cast::<SystemBackdrop>()?;
    unsafe {
        (element.vtable().set_corners)(element.as_raw(), corners).ok()?;
        (element.vtable().set_backdrop)(element.as_raw(), mica.as_raw()).ok()?;
    }
    Ok((element, mica))
}

// Transparent host setup follows the public DWM/WinUI composition-brush backdrop pattern.
// WM_ERASEBKGND must clear to black (zero alpha in the DWM redirection surface).
const SUBCLASS_ID: usize = 0x5752_4d43;
#[repr(C)]
struct Margins {
    left: i32,
    right: i32,
    top: i32,
    bottom: i32,
}
#[repr(C)]
struct BlurBehind {
    flags: u32,
    enable: i32,
    region: Raw,
    transition: i32,
}
#[repr(C)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}
type SubclassProc = unsafe extern "system" fn(Raw, u32, usize, isize, usize, usize) -> isize;
windows_core::link!("dwmapi.dll" "system" fn DwmExtendFrameIntoClientArea(hwnd: Raw, margins: *const Margins) -> HRESULT);
windows_core::link!("dwmapi.dll" "system" fn DwmEnableBlurBehindWindow(hwnd: Raw, blur: *const BlurBehind) -> HRESULT);
windows_core::link!("dwmapi.dll" "system" fn DwmSetWindowAttribute(hwnd: Raw, attribute: u32, value: *const c_void, size: u32) -> HRESULT);
windows_core::link!("gdi32.dll" "system" fn CreateRectRgn(left: i32, top: i32, right: i32, bottom: i32) -> Raw);
windows_core::link!("gdi32.dll" "system" fn DeleteObject(object: Raw) -> i32);
windows_core::link!("gdi32.dll" "system" fn GetStockObject(index: i32) -> Raw);
windows_core::link!("user32.dll" "system" fn GetClientRect(hwnd: Raw, rect: *mut Rect) -> i32);
windows_core::link!("user32.dll" "system" fn IsWindowVisible(hwnd: Raw) -> i32);
windows_core::link!("user32.dll" "system" fn GetForegroundWindow() -> Raw);
windows_core::link!("user32.dll" "system" fn FillRect(dc: Raw, rect: *const Rect, brush: Raw) -> i32);
windows_core::link!("user32.dll" "system" fn GetDC(hwnd: Raw) -> Raw);
windows_core::link!("user32.dll" "system" fn ReleaseDC(hwnd: Raw, dc: Raw) -> i32);
windows_core::link!("comctl32.dll" "system" fn SetWindowSubclass(hwnd: Raw, callback: Option<SubclassProc>, id: usize, data: usize) -> i32);
windows_core::link!("comctl32.dll" "system" fn RemoveWindowSubclass(hwnd: Raw, callback: Option<SubclassProc>, id: usize) -> i32);
windows_core::link!("comctl32.dll" "system" fn DefSubclassProc(hwnd: Raw, message: u32, wparam: usize, lparam: isize) -> isize);
fn configure_dwm(hwnd: isize) -> Result<()> {
    unsafe {
        let hwnd = hwnd as Raw;
        DwmExtendFrameIntoClientArea(
            hwnd,
            &Margins {
                left: 0,
                right: 0,
                top: 0,
                bottom: 0,
            },
        )
        .ok()?;
        let region = CreateRectRgn(-2, -2, -1, -1);
        let result = DwmEnableBlurBehindWindow(
            hwnd,
            &BlurBehind {
                flags: 3,
                enable: 1,
                region,
                transition: 0,
            },
        );
        if !region.is_null() {
            DeleteObject(region);
        }
        result.ok()?;
        let corners: u32 = 1; // DWMWCP_DONOTROUND; the moving card owns its corners.
        DwmSetWindowAttribute(hwnd, 33, &corners as *const _ as _, 4).ok()?;
        let border: u32 = 0xffff_fffe;
        DwmSetWindowAttribute(hwnd, 34, &border as *const _ as _, 4).ok()
    }
}
unsafe fn clear_background(hwnd: Raw, dc: Raw) {
    let mut rect = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe {
        if GetClientRect(hwnd, &mut rect) != 0 {
            FillRect(dc, &rect, GetStockObject(4));
        }
    }
}
unsafe extern "system" fn transparent_window_proc(
    hwnd: Raw,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    _data: usize,
) -> isize {
    unsafe {
        match message {
            0x0014 => {
                clear_background(hwnd, wparam as Raw);
                return 1;
            }
            0x031e => {
                _ = configure_dwm(hwnd as isize);
            }
            0x0082 => {
                RemoveWindowSubclass(hwnd, Some(transparent_window_proc), SUBCLASS_ID);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

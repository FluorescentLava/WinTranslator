//! Optional layout for the native NavigationView left-menu repeater.
//!
//! The NavigationViewItems and NavigationView selection model remain native.
//! Only the template's MenuItemsHost Layout is replaced, so selection indicators,
//! keyboard focus, invocation, and accessibility stay owned by NavigationView.

use super::bindings::{
    Control, DependencyObject, DependencyProperty, DispatcherQueue, DispatcherQueueTimer,
    FrameworkElement, IControl, IFrameworkElement, IItemsRepeater, NavigationView,
    ScrollBarVisibility, ScrollViewer, Thickness, XamlReader,
};
use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::{Rc, Weak};
use windows_core::{IInspectable, IInspectable_Vtbl, Interface};

const MAX_VISITED: usize = 512;
const RETRY_ATTEMPTS: u8 = 20;

#[derive(Default)]
pub(super) struct NavigationMenuLayout {
    shared: Rc<RefCell<LayoutState>>,
    subscriptions: Vec<windows_core::EventRevoker>,
}

#[derive(Default)]
struct LayoutState {
    requested_layout: Option<IInspectable>,
    repeater: Option<IItemsRepeater>,
    original_layout: Option<IInspectable>,
    applied: bool,
    timer: Option<DispatcherQueueTimer>,
    attempts_remaining: u8,
    compact: bool,
    compact_properties: Vec<(IDependencyValues, DependencyProperty, IInspectable)>,
    compact_grid_padding: Vec<(IGridPadding, Thickness)>,
}

impl Drop for NavigationMenuLayout {
    fn drop(&mut self) {
        if let Some(timer) = self.shared.borrow_mut().timer.take() {
            let _ = timer.Stop();
        }
    }
}

impl NavigationMenuLayout {
    pub(super) fn restore(&self) -> windows_core::Result<()> {
        let mut state = self.shared.borrow_mut();
        restore_compact(&mut state)?;
        if let (Some(repeater), Some(layout)) = (&state.repeater, &state.original_layout) {
            repeater.SetLayout(layout)?;
        }
        Ok(())
    }

    fn ensure_subscriptions(&mut self, control: &NavigationView) -> windows_core::Result<()> {
        if !self.subscriptions.is_empty() {
            return Ok(());
        }
        let timer = DispatcherQueue::GetForCurrentThread()?.CreateTimer()?;
        timer.SetInterval(windows_time::TimeSpan { duration: 500_000 })?;
        timer.SetIsRepeating(true)?;
        let weak = Rc::downgrade(&self.shared);
        let tick_control = control.clone();
        let tick = timer.Tick(move |sender, _| {
            let Some(shared) = weak.upgrade() else {
                if let Some(timer) = sender.as_ref() {
                    let _ = timer.Stop();
                }
                return;
            };
            let Ok(mut state) = shared.try_borrow_mut() else {
                return;
            };
            state.attempts_remaining = state.attempts_remaining.saturating_sub(1);
            let done = matches!(apply(&tick_control, &mut state), Ok(true));
            if done || state.attempts_remaining == 0 {
                if let Some(timer) = &state.timer {
                    let _ = timer.Stop();
                }
            }
        })?;
        let weak = Rc::downgrade(&self.shared);
        let loaded_control = control.clone();
        let loaded = control.cast::<IFrameworkElement>()?.Loaded(move |_, _| {
            let _ = request_apply(&weak, &loaded_control);
        })?;
        let weak = Rc::downgrade(&self.shared);
        let sized_control = control.clone();
        let sized = control
            .cast::<IFrameworkElement>()?
            .SizeChanged(move |_, _| {
                let _ = request_apply(&weak, &sized_control);
            })?;
        self.shared.borrow_mut().timer = Some(timer);
        self.subscriptions = vec![tick, loaded, sized];
        Ok(())
    }
}

pub(super) fn set_columns(
    control: &NavigationView,
    columns: i32,
    state: &mut NavigationMenuLayout,
) -> windows_core::Result<()> {
    // A donor repeater creates native layout objects without another Layout projection.
    // Compact menu items keep their own native 36-DIP height and selection visuals.
    let layout_xaml = if columns == 1 {
        "<StackLayout Orientation=\"Vertical\" Spacing=\"0\"/>".to_owned()
    } else {
        format!(
            r#"<UniformGridLayout Orientation="Horizontal" MaximumRowsOrColumns="{columns}"
            MinItemWidth="116" MinItemHeight="56" MinColumnSpacing="6"
            MinRowSpacing="4" ItemsStretch="Fill"/>"#
        )
    };
    let xaml = format!(
        r#"<ItemsRepeater xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation">
            <ItemsRepeater.Layout>{layout_xaml}</ItemsRepeater.Layout>
        </ItemsRepeater>"#
    );
    let donor = XamlReader::Load(&xaml)?.cast::<IItemsRepeater>()?;
    let layout = donor.Layout()?;
    state.ensure_subscriptions(control)?;
    {
        let mut shared = state.shared.borrow_mut();
        if shared.compact && columns != 1 {
            restore_compact(&mut shared)?;
        }
        shared.compact = columns == 1;
        shared.requested_layout = Some(layout);
        shared.applied = false;
    }
    request_apply(&Rc::downgrade(&state.shared), control)
}

fn request_apply(
    weak: &Weak<RefCell<LayoutState>>,
    control: &NavigationView,
) -> windows_core::Result<()> {
    let Some(shared) = weak.upgrade() else {
        return Ok(());
    };
    let Ok(mut state) = shared.try_borrow_mut() else {
        return Ok(());
    };
    state.attempts_remaining = RETRY_ATTEMPTS;
    let applied = apply(control, &mut state)?;
    if let Some(timer) = &state.timer {
        if applied {
            timer.Stop()?;
        } else {
            timer.Start()?;
        }
    }
    Ok(())
}

fn apply(control: &NavigationView, state: &mut LayoutState) -> windows_core::Result<bool> {
    VISUAL_TREE.call(|tree| {
        let mut pending = vec![(control.cast::<DependencyObject>()?, Vec::new())];
        let mut visited = 0;
        let mut found = None;
        let mut chrome = Vec::new();
        while let Some((element, mut ancestors)) = pending.pop() {
            visited += 1;
            if visited > MAX_VISITED {
                return Ok(false);
            }
            let name = element
                .cast::<IFrameworkElement>()
                .and_then(|element| element.Name())
                .map(|name| name.to_string_lossy())
                .unwrap_or_default();
            if state.compact
                && matches!(
                    name.as_str(),
                    "PaneToggleButtonGrid"
                        | "TogglePaneTopPadding"
                        | "ContentPaneTopPadding"
                        | "PaneHeaderContentBorder"
                        | "AutoSuggestArea"
                        | "PaneCustomContentBorder"
                        | "FooterContentBorder"
                        | "FooterItemsScrollViewer"
                        | "VisualItemsSeparator"
                        | "HeaderContent"
                )
            {
                chrome.push(element.clone());
            }
            if name == "MenuItemsHost"
                && let Ok(repeater) = element.cast::<IItemsRepeater>()
            {
                ancestors.push(element);
                found = Some((repeater, ancestors));
                // Item templates contain the native indicator and focus visuals; leave them alone.
                continue;
            }
            if state.compact && matches!(name.as_str(), "TopNavArea" | "FooterItemsScrollViewer") {
                continue;
            }
            let count = tree.count(&element)?;
            if count < 0 || count as usize + pending.len() + visited > MAX_VISITED {
                return Ok(false);
            }
            for index in 0..count {
                let mut path = ancestors.clone();
                path.push(element.clone());
                pending.push((tree.child(&element, index)?, path));
            }
        }
        let Some((repeater, ancestors)) = found else {
            return Ok(false);
        };
        let same = state
            .repeater
            .as_ref()
            .is_some_and(|previous| previous.as_raw() == repeater.as_raw());
        if !same {
            state.original_layout = Some(repeater.Layout()?);
            state.repeater = Some(repeater.clone());
            state.applied = false;
        }
        if !state.applied
            && let Some(layout) = &state.requested_layout
        {
            repeater.SetLayout(layout)?;
            state.applied = true;
        }
        if state.compact {
            for element in chrome {
                set_compact_property(state, &element, FrameworkElement::MinHeightProperty()?, 0.0)?;
                set_compact_property(state, &element, FrameworkElement::HeightProperty()?, 0.0)?;
                set_compact_property(
                    state,
                    &element,
                    FrameworkElement::MarginProperty()?,
                    Thickness::default(),
                )?;
            }
            for element in ancestors {
                let framework = element.cast::<IFrameworkElement>()?;
                let name = framework.Name()?.to_string_lossy();
                let margin = if name == "PaneContentGrid" {
                    Thickness {
                        top: 4.0,
                        bottom: 4.0,
                        ..Default::default()
                    }
                } else {
                    Thickness::default()
                };
                set_compact_property(state, &element, FrameworkElement::MarginProperty()?, margin)?;
                set_compact_property(state, &element, FrameworkElement::MinHeightProperty()?, 0.0)?;
                if element.cast::<IControl>().is_ok() {
                    set_compact_property(
                        state,
                        &element,
                        Control::PaddingProperty()?,
                        Thickness::default(),
                    )?;
                }
                if let Ok(grid) = element.cast::<IGridPadding>() {
                    if !state
                        .compact_grid_padding
                        .iter()
                        .any(|(previous, _)| previous.as_raw() == grid.as_raw())
                    {
                        state
                            .compact_grid_padding
                            .push((grid.clone(), grid.padding()?));
                    }
                    grid.set_padding(Thickness::default())?;
                }
                if name == "MenuItemsScrollViewer" {
                    set_compact_property(
                        state,
                        &element,
                        ScrollViewer::HorizontalScrollBarVisibilityProperty()?,
                        ScrollBarVisibility::Disabled,
                    )?;
                    set_compact_property(
                        state,
                        &element,
                        ScrollViewer::VerticalScrollBarVisibilityProperty()?,
                        ScrollBarVisibility::Disabled,
                    )?;
                }
            }
        }
        Ok(state.applied)
    })
}

fn set_compact_property<T: windows_core::RuntimeType + 'static>(
    state: &mut LayoutState,
    element: &DependencyObject,
    property: DependencyProperty,
    value: T,
) -> windows_core::Result<()>
where
    windows_reference::IReference<T>: From<T>,
{
    let object = element.cast::<IDependencyValues>()?;
    if !state
        .compact_properties
        .iter()
        .any(|(previous, key, _)| previous.as_raw() == object.as_raw() && key == &property)
    {
        state
            .compact_properties
            .push((object.clone(), property.clone(), object.get(&property)?));
    }
    object.set(
        &property,
        &windows_reference::IReference::<T>::from(value).cast::<IInspectable>()?,
    )
}

fn restore_compact(state: &mut LayoutState) -> windows_core::Result<()> {
    for (object, property, value) in state.compact_properties.drain(..) {
        object.set(&property, &value)?;
    }
    for (grid, padding) in state.compact_grid_padding.drain(..) {
        grid.set_padding(padding)?;
    }
    Ok(())
}

// Public interfaces and member order verified from the local Microsoft.UI.Xaml.winmd.
windows_core::imp::define_interface!(
    IDependencyValues,
    IDependencyValuesVtbl,
    0xe7beaee7_160e_50f7_8789_d63463f979fa
);
#[repr(C)]
pub struct IDependencyValuesVtbl {
    base: IInspectable_Vtbl,
    get: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *mut *mut c_void,
    ) -> windows_core::HRESULT,
    set: unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> windows_core::HRESULT,
}
impl IDependencyValues {
    fn get(&self, property: &DependencyProperty) -> windows_core::Result<IInspectable> {
        unsafe {
            let mut value = std::ptr::null_mut();
            (self.vtable().get)(self.as_raw(), property.as_raw(), &mut value)
                .and_then(|| windows_core::imp::Type::from_abi(value))
        }
    }
    fn set(&self, property: &DependencyProperty, value: &IInspectable) -> windows_core::Result<()> {
        unsafe { (self.vtable().set)(self.as_raw(), property.as_raw(), value.as_raw()).ok() }
    }
}
windows_core::imp::define_interface!(
    IGridPadding,
    IGridPaddingVtbl,
    0xc4496219_9014_58a1_b4ad_c5044913a5bb
);
#[repr(C)]
pub struct IGridPaddingVtbl {
    base: IInspectable_Vtbl,
    before_padding: [usize; 10],
    get: unsafe extern "system" fn(*mut c_void, *mut Thickness) -> windows_core::HRESULT,
    set: unsafe extern "system" fn(*mut c_void, Thickness) -> windows_core::HRESULT,
}
impl IGridPadding {
    fn padding(&self) -> windows_core::Result<Thickness> {
        unsafe {
            let mut value = Thickness::default();
            (self.vtable().get)(self.as_raw(), &mut value).map(|| value)
        }
    }
    fn set_padding(&self, value: Thickness) -> windows_core::Result<()> {
        unsafe { (self.vtable().set)(self.as_raw(), value).ok() }
    }
}

struct VisualTreeHelper;
impl windows_core::RuntimeName for VisualTreeHelper {
    const NAME: &'static str = "Microsoft.UI.Xaml.Media.VisualTreeHelper";
}
static VISUAL_TREE: windows_core::imp::FactoryCache<VisualTreeHelper, IVisualTreeHelperStatics> =
    windows_core::imp::FactoryCache::new();

// Public WinUI ABI; the named repeater is verified before changing its layout.
windows_core::imp::define_interface!(
    IVisualTreeHelperStatics,
    IVisualTreeHelperStaticsVtbl,
    0x5aece43c_7651_5bb5_855c_2198496e455e
);
#[repr(C)]
pub struct IVisualTreeHelperStaticsVtbl {
    base: IInspectable_Vtbl,
    find_point: usize,
    find_rect: usize,
    find_all_point: usize,
    find_all_rect: usize,
    get_child: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        i32,
        *mut *mut c_void,
    ) -> windows_core::HRESULT,
    get_children_count:
        unsafe extern "system" fn(*mut c_void, *mut c_void, *mut i32) -> windows_core::HRESULT,
}
impl IVisualTreeHelperStatics {
    fn child(
        &self,
        reference: &DependencyObject,
        index: i32,
    ) -> windows_core::Result<DependencyObject> {
        unsafe {
            let mut result = std::ptr::null_mut();
            (self.vtable().get_child)(self.as_raw(), reference.as_raw(), index, &mut result)
                .and_then(|| windows_core::imp::Type::from_abi(result))
        }
    }
    fn count(&self, reference: &DependencyObject) -> windows_core::Result<i32> {
        unsafe {
            let mut count = 0;
            (self.vtable().get_children_count)(self.as_raw(), reference.as_raw(), &mut count)
                .map(|| count)
        }
    }
}

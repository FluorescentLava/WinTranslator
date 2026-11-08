//! Optional layout for the native NavigationView left-menu repeater.
//!
//! The NavigationViewItems and NavigationView selection model remain native.
//! Only the template's MenuItemsHost Layout is replaced, so selection indicators,
//! keyboard focus, invocation, and accessibility stay owned by NavigationView.

use super::bindings::{
    DependencyObject, DispatcherQueue, DispatcherQueueTimer, IFrameworkElement, IItemsRepeater,
    NavigationView, XamlReader,
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
        let state = self.shared.borrow();
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
    // A donor repeater lets XAML create a native UniformGridLayout without a
    // second projection of WinUI's Layout/UniformGridLayout interfaces.
    let xaml = format!(
        r#"<ItemsRepeater xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation">
            <ItemsRepeater.Layout>
                <UniformGridLayout Orientation="Horizontal" MaximumRowsOrColumns="{columns}"
                    MinItemWidth="116" MinItemHeight="56" MinColumnSpacing="6"
                    MinRowSpacing="4" ItemsStretch="Fill"/>
            </ItemsRepeater.Layout>
        </ItemsRepeater>"#
    );
    let donor = XamlReader::Load(&xaml)?.cast::<IItemsRepeater>()?;
    let layout = donor.Layout()?;
    state.ensure_subscriptions(control)?;
    {
        let mut shared = state.shared.borrow_mut();
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
        let mut pending = vec![control.cast::<DependencyObject>()?];
        let mut visited = 0;
        while let Some(element) = pending.pop() {
            visited += 1;
            if visited > MAX_VISITED {
                return Ok(false);
            }
            if let Ok(repeater) = element.cast::<IItemsRepeater>() {
                if element
                    .cast::<IFrameworkElement>()?
                    .Name()?
                    .to_string_lossy()
                    == "MenuItemsHost"
                {
                    let same = state
                        .repeater
                        .as_ref()
                        .is_some_and(|previous| previous.as_raw() == repeater.as_raw());
                    if !same {
                        state.original_layout = Some(repeater.Layout()?);
                        state.repeater = Some(repeater.clone());
                        state.applied = false;
                    }
                    if !state.applied {
                        if let Some(layout) = &state.requested_layout {
                            repeater.SetLayout(layout)?;
                            state.applied = true;
                        }
                    }
                    return Ok(state.applied);
                }
            }
            let count = tree.count(&element)?;
            if count < 0 || count as usize + pending.len() + visited > MAX_VISITED {
                return Ok(false);
            }
            for index in 0..count {
                pending.push(tree.child(&element, index)?);
            }
        }
        Ok(false)
    })
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

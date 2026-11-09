//! Native DestInvert caret rendering over a transparent system backdrop.
//!
//! The identifying invariants come from Microsoft's TextBoxView.cpp, EnsureCaretElement and
//! CreateCaretShape. VisualTreeHelper returns the ordinary child collection, including this shape.
//! TextBoxViewGenerated::QueryInterfaceImpl exposes the internal class identity used below.
//! https://github.com/microsoft/microsoft-ui-xaml/blob/main/dxaml/xcp/core/native/text/Controls/TextBoxView.cpp
//! https://github.com/microsoft/microsoft-ui-xaml/blob/main/dxaml/xcp/dxaml/lib/winrtgeneratedclasses/TextBoxView.g.cpp

use super::bindings::{
    DependencyObject, DispatcherQueue, DispatcherQueueTimer, IFrameworkElement, IShape,
    ISolidColorBrush, IUIElement, SolidColorBrush, TextBox,
};
use crate::core::NodeId;
use std::cell::RefCell;
use std::ffi::c_void;
use std::io::Write;
use std::rc::{Rc, Weak};
use windows_core::{IInspectable, IInspectable_Vtbl, Interface};

const SOURCE_OVER: i32 = 1;
const DEST_INVERT: i32 = 3;
const MAX_VISITED: usize = 128;
const RETRY_ATTEMPTS: u8 = 40;
const RETRY_INTERVAL_100NS: i64 = 500_000;

#[derive(Default)]
pub(super) struct CaretState {
    shared: Rc<RefCell<CaretVisualState>>,
    subscriptions: Vec<windows_core::EventRevoker>,
}

#[derive(Default)]
struct CaretVisualState {
    caret: Option<DependencyObject>,
    color: Option<crate::Color>,
    requested_color: Option<crate::Color>,
    timer: Option<DispatcherQueueTimer>,
    attempts_remaining: u8,
    last_report: String,
}

impl Drop for CaretState {
    fn drop(&mut self) {
        // The runtime removes this state when its node retires. Event handlers
        // only hold Weak state; stopping here also prevents a pending timer
        // from retaining a removed TextBox through its Tick delegate.
        if let Some(timer) = self.shared.borrow_mut().timer.take() {
            let _ = timer.Stop();
        }
    }
}

impl CaretVisualState {
    fn report(&mut self, node: NodeId, message: String) {
        if self.last_report == message {
            return;
        }
        self.last_report = message;
        // Opt-in diagnostics contain visual metadata only, never the TextBox's text or selection.
        if let Some(path) = std::env::var_os("WINDOWS_REACTOR_CARET_LOG")
            && let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
        {
            let _ = writeln!(file, "caret {node:?}: {}", self.last_report);
        }
    }
}

impl CaretState {
    fn ensure_subscriptions(
        &mut self,
        control: &TextBox,
        node: NodeId,
    ) -> windows_core::Result<()> {
        if !self.subscriptions.is_empty() {
            return Ok(());
        }
        let timer = DispatcherQueue::GetForCurrentThread()?.CreateTimer()?;
        timer.SetInterval(windows_time::TimeSpan {
            duration: RETRY_INTERVAL_100NS,
        })?;
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
            if state.attempts_remaining == 0 {
                if let Some(timer) = &state.timer {
                    let _ = timer.Stop();
                }
                return;
            }
            state.attempts_remaining -= 1;
            if let Some(color) = state.requested_color {
                let _ = apply_color(&tick_control, color, node, &mut state);
            }
            if state.attempts_remaining == 0 {
                if let Some(timer) = &state.timer {
                    let _ = timer.Stop();
                }
            }
        })?;
        let weak = Rc::downgrade(&self.shared);
        let loaded = control.cast::<IFrameworkElement>()?.Loaded(move |_, _| {
            let _ = arm_retries(&weak, node, "Loaded");
        })?;
        let weak = Rc::downgrade(&self.shared);
        let focused = control.cast::<IUIElement>()?.GotFocus(move |_, _| {
            let _ = arm_retries(&weak, node, "GotFocus");
        })?;
        self.shared.borrow_mut().timer = Some(timer);
        self.subscriptions = vec![tick, loaded, focused];
        Ok(())
    }
}

fn arm_retries(
    weak: &Weak<RefCell<CaretVisualState>>,
    node: NodeId,
    source: &str,
) -> windows_core::Result<()> {
    let Some(shared) = weak.upgrade() else {
        return Ok(());
    };
    let Ok(mut state) = shared.try_borrow_mut() else {
        return Ok(());
    };
    state.attempts_remaining = RETRY_ATTEMPTS;
    state.report(
        node,
        format!("watch source={source} attempts={RETRY_ATTEMPTS} interval-ms=50"),
    );
    if let Some(timer) = &state.timer {
        timer.Start()?;
    }
    Ok(())
}

pub(super) fn set_color(
    control: &TextBox,
    color: crate::Color,
    node: NodeId,
    state: &mut CaretState,
) -> windows_core::Result<bool> {
    state.ensure_subscriptions(control, node)?;
    state.shared.borrow_mut().requested_color = Some(color);
    let result = apply_color(control, color, node, &mut state.shared.borrow_mut());
    // Keep checking briefly even after a successful early match: focus/layout
    // can replace the caret or restore its native Fill after this request.
    // Each request/Loaded/GotFocus arms at most two seconds of UI-thread work.
    arm_retries(&Rc::downgrade(&state.shared), node, "request")?;
    result
}

fn apply_color(
    control: &TextBox,
    color: crate::Color,
    node: NodeId,
    state: &mut CaretVisualState,
) -> windows_core::Result<bool> {
    let result = VISUAL_TREE.call(|tree| set_color_inner(tree, control, color, node, state));
    if let Err(error) = &result {
        state.report(
            node,
            format!("native-error HRESULT={:#010x}", error.code().0),
        );
    }
    result
}

fn set_color_inner(
    tree: &IVisualTreeHelperStatics,
    control: &TextBox,
    color: crate::Color,
    node: NodeId,
    state: &mut CaretVisualState,
) -> windows_core::Result<bool> {
    let mut pending = vec![control.cast::<DependencyObject>()?];
    let mut visited = 0;
    let mut view = None;
    while let Some(element) = pending.pop() {
        visited += 1;
        if visited > MAX_VISITED {
            state.report(node, format!("skipped visual-tree-limit visited={visited}"));
            return Ok(false);
        }
        if element.cast::<ITextBoxViewIdentity>().is_ok() {
            if view.is_some() {
                state.report(
                    node,
                    format!("skipped ambiguous-TextBoxView visited={visited}"),
                );
                return Ok(false);
            }
            view = Some(element);
            continue;
        }
        let count = tree.count(&element)?;
        if count < 0 || count as usize + pending.len() + visited > MAX_VISITED {
            state.report(
                node,
                format!("skipped visual-tree-limit children={count} visited={visited}"),
            );
            return Ok(false);
        }
        for index in 0..count {
            pending.push(tree.child(&element, index)?);
        }
    }
    let Some(view) = view else {
        state.report(
            node,
            format!("pending TextBoxView-not-found visited={visited}"),
        );
        return Ok(false);
    };
    let count = tree.count(&view)?;
    // Current TextBoxView adds only the caret as a child. Do not select an arbitrary thin shape.
    if count != 1 {
        state.report(
            node,
            format!("pending TextBoxView-children={count} visited={visited}"),
        );
        return Ok(false);
    }
    let candidate = tree.child(&view, 0)?;
    let class = candidate
        .cast::<IInspectable>()?
        .GetRuntimeClassName()?
        .to_string_lossy();
    if class != "Microsoft.UI.Xaml.Shapes.Rectangle" && class != "Microsoft.UI.Xaml.Shapes.Path" {
        state.report(node, format!("skipped first-child-class={class}"));
        return Ok(false);
    }
    let shape = candidate.cast::<IShape>()?;
    let element = candidate.cast::<IUIElement>()?;
    let framework = candidate.cast::<IFrameworkElement>()?;
    let hit_test = element.IsHitTestVisible()?;
    let mode = element.CompositeMode()?;
    let width = framework.Width()?;
    let height = framework.Height()?;
    let previously_applied = state.caret.as_ref() == Some(&candidate);
    if hit_test
        || (mode != DEST_INVERT && !(previously_applied && mode == SOURCE_OVER))
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
    {
        state.report(node, format!(
            "skipped caret-flags class={class} hit-test={hit_test} mode={mode} size={width}x{height} owned={previously_applied}"
        ));
        return Ok(false);
    }
    let desired_color = super::bindings::Color {
        a: color.a,
        r: color.r,
        g: color.g,
        b: color.b,
    };
    let fill_matches = shape
        .Fill()
        .and_then(|brush| brush.cast::<ISolidColorBrush>())
        .and_then(|brush| brush.Color())
        .is_ok_and(|actual| actual == desired_color);
    if !previously_applied || state.color != Some(color) || mode != SOURCE_OVER || !fill_matches {
        // A dedicated brush avoids mutating a shared/theme brush used by text or selection.
        let brush = SolidColorBrush::new()?;
        brush.SetColor(desired_color)?;
        element.SetCompositeMode(SOURCE_OVER)?;
        if let Err(error) = shape.SetFill(&brush) {
            let _ = element.SetCompositeMode(mode);
            return Err(error);
        }
        state.caret = Some(candidate);
        state.color = Some(color);
    }
    state.report(node, format!(
        "applied class={class} visited={visited} size={width}x{height} argb={:02x}{:02x}{:02x}{:02x} mode=SourceOver",
        color.a, color.r, color.g, color.b
    ));
    Ok(true)
}

// This IID is a type identity, not a callable private editing interface. We only QI and release it.
// The identity has been stable since the original TextBoxView peer and is checked defensively.
windows_core::imp::define_interface!(
    ITextBoxViewIdentity,
    IInspectable_Vtbl,
    0x053f2498_8d9c_4a3f_b52b_d130903fba13
);

struct VisualTreeHelper;
impl windows_core::RuntimeName for VisualTreeHelper {
    const NAME: &'static str = "Microsoft.UI.Xaml.Media.VisualTreeHelper";
}
static VISUAL_TREE: windows_core::imp::FactoryCache<VisualTreeHelper, IVisualTreeHelperStatics> =
    windows_core::imp::FactoryCache::new();

// Public WinUI ABI, verified against Microsoft.WindowsAppSDK.WinUI metadata.
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

//! Reads back a text box's native layout.
//!
//! A text box without an explicit height sizes itself to its wrapped text. Callers that
//! predict that size for surrounding content use this to confirm the prediction against
//! what WinUI really laid out.

use super::bindings::{
    DependencyObject, DependencyProperty, DependencyPropertyChangedCallback, IControl,
    IFrameworkElement, IScrollViewer, IUIElement, ScrollBarVisibility, TextBox, Thickness, Viewbox,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use windows_core::Interface;

struct PropertyWatch {
    object: DependencyObject,
    property: DependencyProperty,
    token: i64,
}
impl PropertyWatch {
    fn new(
        object: DependencyObject,
        property: DependencyProperty,
        handler: impl Fn() + 'static,
    ) -> windows_core::Result<Self> {
        let callback = DependencyPropertyChangedCallback::new(move |_, _| handler());
        let token = object.RegisterPropertyChangedCallback(&property, &callback)?;
        Ok(Self {
            object,
            property,
            token,
        })
    }
}
impl Drop for PropertyWatch {
    fn drop(&mut self) {
        _ = self
            .object
            .UnregisterPropertyChangedCallback(&self.property, self.token);
    }
}

/// Owns the editor's scale at the native text-property boundary. TextChanged is
/// queued by WinUI, so waiting for its component callback can paint new text at
/// the old size. Property notifications run before that text is drawn.
pub(super) struct AdaptiveTextScale {
    _text: PropertyWatch,
    _state: Rc<ScaleState>,
}
struct ScaleState {
    control: TextBox,
    font: f64,
    large_font: Cell<f64>,
    padding: Thickness,
    updating: Cell<bool>,
    parent: RefCell<Option<DependencyObject>>,
    parent_watches: RefCell<Vec<PropertyWatch>>,
}
impl AdaptiveTextScale {
    pub fn new(control: &TextBox, large_font: f64) -> windows_core::Result<Self> {
        let native = control.cast::<IControl>()?;
        let state = Rc::new(ScaleState {
            control: control.clone(),
            font: native.FontSize()?,
            large_font: Cell::new(large_font),
            padding: native.Padding()?,
            updating: Cell::new(false),
            parent: RefCell::new(None),
            parent_watches: RefCell::new(Vec::new()),
        });
        let weak = Rc::downgrade(&state);
        let text = PropertyWatch::new(control.cast()?, TextBox::TextProperty()?, move || {
            if let Some(state) = weak.upgrade() {
                _ = state.sync();
            }
        })?;
        Ok(Self {
            _text: text,
            _state: state,
        })
    }
    pub fn set_large_font(&self, font: f64) -> windows_core::Result<()> {
        self._state.large_font.set(font);
        self._state.sync()
    }
    pub fn attach(&self, parent: &Viewbox) -> windows_core::Result<()> {
        self._state.attach(parent)
    }
}
impl ScaleState {
    fn attach(self: &Rc<Self>, parent: &Viewbox) -> windows_core::Result<()> {
        let parent = parent.cast::<DependencyObject>()?;
        if self.parent.borrow().as_ref() != Some(&parent) {
            let mut watches = Vec::new();
            for property in [
                super::bindings::FrameworkElement::WidthProperty()?,
                super::bindings::FrameworkElement::HeightProperty()?,
            ] {
                let weak = Rc::downgrade(self);
                watches.push(PropertyWatch::new(parent.clone(), property, move || {
                    if let Some(state) = weak.upgrade() {
                        _ = state.sync();
                    }
                })?);
            }
            *self.parent_watches.borrow_mut() = watches;
            *self.parent.borrow_mut() = Some(parent.clone());
        }
        self.sync()
    }

    fn sync(self: &Rc<Self>) -> windows_core::Result<()> {
        if self.updating.replace(true) {
            return Ok(());
        }
        let result = self.apply();
        self.updating.set(false);
        result
    }

    fn apply(self: &Rc<Self>) -> windows_core::Result<()> {
        let element = self.control.cast::<IFrameworkElement>()?;
        let Some(parent) = self.parent.borrow().clone() else {
            return Ok(());
        };
        let parent = parent.cast::<IFrameworkElement>()?;
        let width = parent.Width()?;
        let height = parent.Height()?;
        if !width.is_finite() || width <= 0.0 || !height.is_finite() || height <= 0.0 {
            return Ok(());
        }
        let large_scale = (self.large_font.get() / self.font).max(1.0);
        let content_width = (width - self.padding.left - self.padding.right) / large_scale;
        let text = self.control.Text()?;
        let compact = compact_text(&text, self.font, content_width);
        let scale = if compact { 1.0 } else { large_scale };
        // RichEdit keeps a fixed FontSize throughout typing and IME composition.
        // The Viewbox measures this child and scales its entire visual atomically.
        self.control.cast::<IControl>()?.SetPadding(Thickness {
            left: self.padding.left / scale,
            top: self.padding.top / scale,
            right: self.padding.right / scale,
            bottom: self.padding.bottom / scale,
        })?;
        element.SetWidth(width / scale)?;
        element.SetHeight(height / scale)
    }
}

fn compact_text(value: &str, font: f64, width: f64) -> bool {
    if value.is_empty() {
        return false;
    }
    if value.chars().nth(1200).is_some() {
        return true;
    }
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    if normalized.split('\n').count() > 2 {
        return true;
    }
    let measurable = if normalized.ends_with('\n') {
        format!("{normalized}A")
    } else {
        normalized
    };
    let line = super::text_metrics::measure_wrapped_text_height("A", font, width);
    let text = super::text_metrics::measure_wrapped_text_height(&measurable, font, width);
    match (line, text) {
        (Some(line), Some(text)) if line > 0.0 => ((text / line) + 0.25).floor() > 2.0,
        _ => false,
    }
}

#[cfg(feature = "test")]
pub(super) fn verify_synchronous_scale() -> windows_core::Result<()> {
    use super::bindings::{TextWrapping, UIElement};
    let control = TextBox::new()?;
    let native = control.cast::<IControl>()?;
    native.SetFontSize(15.0)?;
    native.SetPadding(Thickness {
        left: 10.0,
        top: 5.0,
        right: 6.0,
        bottom: 6.0,
    })?;
    control.SetAcceptsReturn(true)?;
    control.SetTextWrapping(TextWrapping::Wrap)?;
    let parent = Viewbox::new()?;
    let parent_size = parent.cast::<IFrameworkElement>()?;
    parent_size.SetWidth(448.0)?;
    parent_size.SetHeight(74.0)?;
    parent.SetChild(&control.cast::<UIElement>()?)?;
    let layout = AdaptiveTextScale::new(&control, 23.0)?;
    layout.attach(&parent)?;
    let editor = control.cast::<IFrameworkElement>()?;
    let verify = |compact: bool| -> windows_core::Result<()> {
        let scale = if compact { 1.0 } else { 23.0 / 15.0 };
        if (editor.Width()? - 448.0 / scale).abs() > 0.01
            || (editor.Height()? - parent_size.Height()? / scale).abs() > 0.01
            || native.FontSize()? != 15.0
            || (native.Padding()?.left * scale - 10.0).abs() > 0.01
        {
            return Err(windows_core::Error::new(
                windows_core::HRESULT(0x80004005u32 as _),
                format!(
                    "editor scale did not update synchronously: compact={compact}, width={}, height={}, font={}, padding={}, text={:?}",
                    editor.Width()?,
                    editor.Height()?,
                    native.FontSize()?,
                    native.Padding()?.left,
                    control.Text()?,
                ),
            ));
        }
        Ok(())
    };
    // No dispatcher/rendering turn is allowed between SetText and inspection.
    // Both wrapping directions, paste, Enter and empty placeholder are covered.
    for (text, compact) in [
        ("", false),
        ("one", false),
        ("one\rtwo", false),
        ("one\rtwo\rthree", true),
        ("one\rtwo", false),
        ("", false),
    ] {
        control.SetText(text)?;
        verify(compact)?;
    }
    let mut text = String::new();
    for _ in 0..120 {
        text.push('字');
        control.SetText(&text)?;
        verify(compact_text(&text, 15.0, 432.0 / (23.0 / 15.0)))?;
    }
    while text.pop().is_some() {
        control.SetText(&text)?;
        verify(compact_text(&text, 15.0, 432.0 / (23.0 / 15.0)))?;
    }
    control.SetText("one\rtwo\rthree")?;
    parent_size.SetHeight(171.0)?;
    verify(true)?;
    control.SetText("one")?;
    verify(false)?;
    // Dropping the policy revokes the callbacks and releases their native refs.
    let state = Rc::downgrade(&layout._state);
    drop(layout);
    if state.upgrade().is_some() {
        return Err(windows_core::Error::new(
            windows_core::HRESULT(0x80004005u32 as _),
            "editor scale subscriptions retain their owner",
        ));
    }
    Ok(())
}

/// Runs pending layout and returns the control's laid-out height, in DIPs.
pub(super) fn laid_out_height(control: &TextBox) -> windows_core::Result<f64> {
    control.cast::<IUIElement>()?.UpdateLayout()?;
    // The template's scroll viewer hides its vertical bar by default, which leaves text
    // beyond the maximum height with no sign that it scrolls. The bar overlays the text,
    // so showing it does not change where lines wrap. Best effort: an unfamiliar template
    // is left alone.
    if let Ok(Some(viewer)) =
        super::caret::find_descendant::<IScrollViewer>(&control.cast::<DependencyObject>()?)
    {
        let _ = viewer.SetVerticalScrollBarVisibility(ScrollBarVisibility::Auto);
    }
    control.cast::<IFrameworkElement>()?.ActualHeight()
}

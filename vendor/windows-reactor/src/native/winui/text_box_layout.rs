//! Reads back a text box's native layout.
//!
//! A text box without an explicit height sizes itself to its wrapped text. Callers that
//! predict that size for surrounding content use this to confirm the prediction against
//! what WinUI really laid out.

use super::bindings::{
    DependencyObject, IFrameworkElement, IScrollViewer, IUIElement, ScrollBarVisibility, TextBox,
};
use windows_core::Interface;

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

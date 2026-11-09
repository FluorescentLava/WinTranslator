use super::bindings::{ITextBlock, IUIElement, Size, TextBlock, TextWrapping};
use std::cell::RefCell;
use std::collections::VecDeque;
use windows_core::Interface;

/// Intrinsic unwrapped text dimensions, in DIPs, using WinUI's default font.
#[derive(Clone, Copy, Debug)]
pub struct TextMetrics {
    pub width: f64,
    pub height: f64,
    pub baseline: f64,
}

thread_local! {
    static CACHE: RefCell<VecDeque<(String, u64, TextMetrics)>> = RefCell::default();
}

/// Measure text on an initialized WinUI UI thread. Returns `None` when the
/// native control is unavailable. Recent measurements are cached per thread.
pub fn measure_text(value: &str, font_size: f64) -> Option<TextMetrics> {
    if !font_size.is_finite() || font_size <= 0.0 {
        return None;
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((_, _, metrics)) = cache
            .iter()
            .find(|(text, size, _)| text == value && *size == font_size.to_bits())
        {
            return Some(*metrics);
        }
        let block = TextBlock::new().ok()?;
        block.SetText(value).ok()?;
        block.SetFontSize(font_size).ok()?;
        block.SetTextWrapping(TextWrapping::NoWrap).ok()?;
        let element = block.cast::<IUIElement>().ok()?;
        let text = block.cast::<ITextBlock>().ok()?;
        let mut size = Size::default();
        let mut baseline = 0.0;
        unsafe {
            (element.vtable().Measure)(
                element.as_raw(),
                Size {
                    width: f32::INFINITY,
                    height: f32::INFINITY,
                },
            )
            .ok()
            .ok()?;
            (element.vtable().DesiredSize)(element.as_raw(), &mut size)
                .ok()
                .ok()?;
            (text.vtable().BaselineOffset)(text.as_raw(), &mut baseline)
                .ok()
                .ok()?;
        }
        let metrics = TextMetrics {
            width: f64::from(size.width),
            height: f64::from(size.height),
            baseline,
        };
        if !metrics.width.is_finite() || !metrics.height.is_finite() || !baseline.is_finite() {
            return None;
        }
        if cache.len() == 64 {
            cache.pop_front();
        }
        cache.push_back((value.to_owned(), font_size.to_bits(), metrics));
        Some(metrics)
    })
}

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

thread_local! {
    static WRAPPED_CACHE: RefCell<VecDeque<(String, u64, u64, f64)>> = RefCell::default();
}

/// Height, in DIPs, of text wrapped at `width` in WinUI's default font: what a
/// wrapping text control of that content width lays out. Line breaks in `value`
/// start new lines. Returns `None` when the native control is unavailable.
/// Recent measurements are cached per thread.
pub fn measure_wrapped_text_height(value: &str, font_size: f64, width: f64) -> Option<f64> {
    if !font_size.is_finite() || font_size <= 0.0 || !width.is_finite() || width <= 0.0 {
        return None;
    }
    WRAPPED_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((_, _, _, height)) = cache.iter().find(|(text, size, extent, _)| {
            text == value && *size == font_size.to_bits() && *extent == width.to_bits()
        }) {
            return Some(*height);
        }
        let block = TextBlock::new().ok()?;
        block.SetText(value).ok()?;
        block.SetFontSize(font_size).ok()?;
        block.SetTextWrapping(TextWrapping::Wrap).ok()?;
        let element = block.cast::<IUIElement>().ok()?;
        let mut size = Size::default();
        unsafe {
            (element.vtable().Measure)(
                element.as_raw(),
                Size {
                    width: width as f32,
                    height: f32::INFINITY,
                },
            )
            .ok()
            .ok()?;
            (element.vtable().DesiredSize)(element.as_raw(), &mut size)
                .ok()
                .ok()?;
        }
        let height = f64::from(size.height);
        if !height.is_finite() {
            return None;
        }
        if cache.len() == 16 {
            cache.pop_front();
        }
        cache.push_back((value.to_owned(), font_size.to_bits(), width.to_bits(), height));
        Some(height)
    })
}

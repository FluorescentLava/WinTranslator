//! Sizing rules for the popup's text input.
//!
//! Native text measurement determines the editor's height on every edit. The
//! viewport reveals that height with the card and native window animation.
//!
//! Text that fits two lines at the large size stays large. Longer text uses
//! the compact size, which the native editor applies before drawing new text.

use std::borrow::Cow;

/// WinUI's default `TextControlThemePadding`, in DIPs.
pub const PADDING_LEFT: f64 = 10.0;
pub const PADDING_RIGHT: f64 = 6.0;
const PADDING_VERTICAL: f64 = 5.0 + 6.0;

pub const LARGE_FONT: f64 = 23.0;
pub const COMPACT_FONT: f64 = 15.0;
/// Line pitch inside the text box: 1.33 em snapped to device pixels. The large
/// value is the upper bound across display scales, so it never underestimates.
const LARGE_LINE: f64 = 31.0;
const COMPACT_LINE: f64 = 20.0;

/// Fits two large lines or three compact ones across display scales. Crossing
/// the third large line therefore does not shrink the editor's height.
pub const MIN_HEIGHT: f64 = 74.0;
/// Beyond this many compact lines the box stops growing and scrolls.
pub const MAX_VISIBLE_LINES: usize = 8;
pub const MAX_HEIGHT: f64 = MAX_VISIBLE_LINES as f64 * COMPACT_LINE + PADDING_VERTICAL;

/// Longer text cannot fit in `MAX_VISIBLE_LINES` at any glyph width, so its
/// exact line count is never needed.
const MEASURE_LIMIT: usize = 1200;

/// Splits on every line-break style. A WinUI text box reports Enter as a bare
/// carriage return, while pasted and captured text uses `\n` or `\r\n`.
pub fn lines(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = Some(text);
    std::iter::from_fn(move || {
        let current = rest?;
        match current.find(['\r', '\n']) {
            Some(index) => {
                let bytes = current.as_bytes();
                let width = if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                rest = Some(&current[index + width..]);
                Some(&current[..index])
            }
            None => {
                rest = None;
                Some(current)
            }
        }
    })
}

/// The same text with `\n` as its only line break, which is what translation
/// services and the paragraph splitter expect.
pub fn unix_newlines(text: &str) -> Cow<'_, str> {
    if text.contains('\r') {
        Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(text)
    }
}

fn advance(c: char, font_size: f64) -> f64 {
    if c == '\t' {
        font_size * 2.0
    } else if c.is_whitespace() {
        font_size * 0.35
    } else if c >= '\u{1100}' {
        font_size
    } else {
        font_size * 0.6
    }
}

/// Wrapped line count from average glyph widths. Used for text the native
/// measurement does not cover (results, notices) and when it is unavailable.
pub fn estimate_lines(text: &str, font_size: f64, width: f64) -> usize {
    lines(text)
        .map(|line| {
            let mut count = 1;
            let mut used = 0.0;
            for c in line.chars() {
                let advance = advance(c, font_size);
                if used + advance > width && used > 0.0 {
                    count += 1;
                    used = 0.0;
                }
                used += advance;
            }
            count
        })
        .sum::<usize>()
        .max(1)
}

/// Text handed to a measuring text block: `\n` breaks, and a visible last line
/// where the text box would show an empty one for the caret.
pub fn measurable(text: &str) -> Cow<'_, str> {
    let text = unix_newlines(text);
    if text.is_empty() || text.ends_with('\n') {
        Cow::Owned(format!("{text}A"))
    } else {
        text
    }
}

pub fn font_size(compact: bool) -> f64 {
    if compact { COMPACT_FONT } else { LARGE_FONT }
}

/// Always measure at the large size, so compact text cannot flip back merely
/// because its smaller glyphs fit fewer lines.
pub fn wants_compact(text: &str, wrapped_lines: &dyn Fn(&str, f64) -> usize) -> bool {
    if text.is_empty() {
        return false;
    }
    if lines(text).count() > 2 || text.chars().nth(MEASURE_LIMIT).is_some() {
        return true;
    }
    wrapped_lines(text, LARGE_FONT) > 2
}

/// Height of the text box for `text` at the given size.
/// `wrapped_lines(text, font_size)` is the number of lines the text occupies in
/// the text box; it is only asked about text that might fit.
pub fn height(text: &str, compact: bool, wrapped_lines: &dyn Fn(&str, f64) -> usize) -> f64 {
    let pitch = if compact { COMPACT_LINE } else { LARGE_LINE };
    let fits = (MAX_HEIGHT - PADDING_VERTICAL) / pitch;
    let lines = if lines(text).count() as f64 >= fits || text.chars().nth(MEASURE_LIMIT).is_some() {
        fits
    } else {
        wrapped_lines(text, font_size(compact)) as f64
    };
    (lines * pitch + PADDING_VERTICAL).clamp(MIN_HEIGHT, MAX_HEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDTH: f64 = 432.0;

    fn compact_after(text: &str, _compact: bool) -> bool {
        wants_compact(text, &|value, font| estimate_lines(value, font, WIDTH))
    }

    fn estimated_height(text: &str, compact: bool) -> f64 {
        height(text, compact, &|text, font_size| {
            estimate_lines(text, font_size, WIDTH)
        })
    }

    /// Types `text` one character at a time, as the popup sees it.
    fn typed_heights(text: &str) -> Vec<(bool, f64)> {
        let mut typed = String::new();
        let mut compact = false;
        text.chars()
            .map(|c| {
                typed.push(c);
                compact = compact_after(&typed, compact);
                (compact, estimated_height(&typed, compact))
            })
            .collect()
    }

    #[test]
    fn every_line_break_style_separates_lines() {
        assert_eq!(
            lines("a\rb\nc\r\nd").collect::<Vec<_>>(),
            ["a", "b", "c", "d"]
        );
        assert_eq!(lines("").collect::<Vec<_>>(), [""]);
        assert_eq!(lines("a\r").collect::<Vec<_>>(), ["a", ""]);
        assert_eq!(lines("\r\n\r\n").collect::<Vec<_>>(), ["", "", ""]);
        assert_eq!(lines("一\r二").collect::<Vec<_>>(), ["一", "二"]);
    }

    #[test]
    fn text_box_line_breaks_become_unix_newlines() {
        assert_eq!(unix_newlines("a\rb\r\nc\nd"), "a\nb\nc\nd");
        assert!(matches!(unix_newlines("plain\ntext"), Cow::Borrowed(_)));
    }

    #[test]
    fn carriage_returns_count_as_lines() {
        assert_eq!(
            estimate_lines("first\rsecond\rthird", COMPACT_FONT, WIDTH),
            3
        );
        assert_eq!(estimate_lines("first\r\nsecond", COMPACT_FONT, WIDTH), 2);
    }

    #[test]
    fn words_and_sentences_that_fit_two_lines_stay_large() {
        assert!(!compact_after("", true));
        assert!(!compact_after("hello", false));
        assert!(!compact_after("你好世界", false));
        assert!(!compact_after(
            "the quick brown fox jumps over the lazy dog",
            false
        ));
        assert!(!compact_after(
            "离线翻译使用量化模型，支持简体中文与英语互译",
            false
        ));
        assert!(!wants_compact(&"i".repeat(100), &|_, _| 2));
    }

    #[test]
    fn two_explicit_lines_stay_large_and_the_third_becomes_compact() {
        assert!(!compact_after("hello\r", false));
        assert!(!compact_after("hello\rworld", false));
        for text in [
            "hello\rworld\ragain",
            "hello\nworld\n",
            "hello\r\nworld\r\n",
        ] {
            assert!(compact_after(text, false));
        }
        assert_eq!(estimated_height("hello", false), MIN_HEIGHT);
        // The frame WinUI draws before the size can change: two large lines.
        assert_eq!(estimated_height("hello\r", false), MIN_HEIGHT);
        assert_eq!(estimated_height("hello\r", true), MIN_HEIGHT);
        assert_eq!(estimated_height("hello\rworld", true), MIN_HEIGHT);
        assert_eq!(estimated_height("hello\rworld\ragain", true), MIN_HEIGHT);
    }

    #[test]
    fn compact_threshold_uses_large_wrapped_lines_in_both_directions() {
        assert!(!compact_after(&"文".repeat(36), false));
        assert!(!compact_after(&"文".repeat(36), true));
        assert!(compact_after(&"文".repeat(37), false));
        assert!(compact_after(&"文".repeat(37), true));
    }

    #[test]
    fn typing_never_shrinks_the_box() {
        for text in [
            "The quick brown fox jumps over the lazy dog while the afternoon sun settles behind \
             the hills, and nobody in the village seems to notice that the old clock tower has \
             stopped again.\rA second paragraph follows it.\r\rAnd a third.",
            "今天的会议记录\r一、离线模型下载流程需要重新检查，尤其是暂停和继续的状态\r二、长文本\
             输入时窗口高度的估算与实际换行不一致，导致输入框抖动\r三、发布",
        ] {
            let mut previous = (false, MIN_HEIGHT);
            for (index, state) in typed_heights(text).into_iter().enumerate() {
                assert!(state.1 >= previous.1, "shrank at character {index}");
                assert!(
                    state.0 || !previous.0,
                    "became large again at character {index}"
                );
                previous = state;
            }
            assert!(previous.0 && previous.1 > MIN_HEIGHT);
        }
    }

    #[test]
    fn heights_move_in_whole_lines_up_to_the_cap() {
        let heights: Vec<f64> = (1..=10)
            .map(|count| estimated_height(&vec!["line"; count].join("\r"), true))
            .collect();
        assert_eq!(
            heights,
            [
                74.0, 74.0, 74.0, 91.0, 111.0, 131.0, 151.0, 171.0, 171.0, 171.0
            ]
        );
        assert_eq!(MAX_HEIGHT, 171.0);
    }

    #[test]
    fn a_composition_that_wraps_large_text_gets_whole_large_lines() {
        // The size is held during a composition, so large text can wrap.
        assert_eq!(estimated_height(&"文".repeat(30), false), MIN_HEIGHT);
        assert_eq!(estimated_height(&"文".repeat(50), false), 3.0 * 31.0 + 11.0);
        assert_eq!(estimated_height(&"文".repeat(400), false), MAX_HEIGHT);
    }

    #[test]
    fn text_that_cannot_fit_is_not_measured() {
        let many_lines = vec!["x"; MAX_VISIBLE_LINES].join("\n");
        let long = "字".repeat(MEASURE_LIMIT + 1);
        for text in [many_lines.as_str(), long.as_str()] {
            let height = height(text, true, &|_, _| panic!("measured text that cannot fit"));
            assert_eq!(height, MAX_HEIGHT);
        }
        assert!(wants_compact(&long, &|_, _| panic!("measured long text")));
    }

    #[test]
    fn measuring_text_keeps_the_caret_line() {
        assert_eq!(measurable(""), "A");
        assert_eq!(measurable("a\r"), "a\nA");
        assert_eq!(measurable("a\rb"), "a\nb");
    }
}

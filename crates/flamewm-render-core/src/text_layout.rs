use std::borrow::Cow;

use crate::glyphs;
use crate::model::TextWrap;

/// One laid-out line: collapsed text plus its advance width in px.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextLine {
    pub text: String,
    pub width: f32,
}

/// Owned result of [`layout_text`]: lines plus the measured block size.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextLayout {
    pub lines: Vec<TextLine>,
    pub width: f32,
    pub height: f32,
    pub line_height: f32,
}

#[must_use]
pub fn line_height(font_size: f32) -> f32 {
    font_size.max(0.0) * 1.30
}

/// Estimated advance per char, in em. 0.49em matches IBM Plex Sans Regular
/// (installed at /usr/share/fonts/truetype/ibm-plex): "readme.txt" measures
/// 63.6px at 13px via PIL. The earlier 0.58em overestimated by ~18%.
/// This is a font-measured estimate, not a shaped advance; native checks
/// still govern any visual claim.
#[must_use]
pub fn char_width(font_size: f32) -> f32 {
    font_size.max(0.0) * 0.49
}

/// Layout width of `text`: the same fractional shaped advance that paint and caret use. Falls
/// back to the per-char estimate only when no font is installed.
fn text_width(text: &str, font_size: f32) -> f32 {
    glyphs::advance_width(text, font_size)
        .unwrap_or_else(|| text.chars().count() as f32 * char_width(font_size))
}

/// Lay out text to an available width.
///
/// Rules: `\n` always breaks; runs of ASCII whitespace collapse to one
/// space and trim at paragraph edges; `Wrap` fills greedily at word
/// boundaries and falls back to char-splitting an overlong token;
/// `break_anywhere` fills greedily at char level; no emitted line exceeds
/// `avail_width` except the degenerate case where one char is wider.
#[must_use]
pub fn layout_text(
    text: &str,
    font_size: f32,
    avail_width: f32,
    wrap: TextWrap,
    break_anywhere: bool,
) -> TextLayout {
    let size = font_size.max(0.0);
    let line_h = line_height(size);
    let avail_ok = avail_width.is_finite() && avail_width > 0.0;
    let normalized: Cow<str> = if text.contains('\r') {
        Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(text)
    };
    let mut lines: Vec<TextLine> = Vec::new();

    for paragraph in normalized.split('\n') {
        let collapsed = collapse_whitespace(paragraph);
        if collapsed.is_empty() {
            lines.push(TextLine::default());
        } else if wrap == TextWrap::NoWrap && avail_ok {
            push_measured(&mut lines, collapsed, size);
        } else if break_anywhere {
            push_chunks(&mut lines, &collapsed, size, avail_width);
        } else {
            // Greedy word fill; overlong or degenerate-avail tokens char-split.
            let mut current = String::new();
            let mut current_w = 0.0f32;
            let space_w = text_width(" ", size);
            let flush = |current: &mut String, current_w: &mut f32, lines: &mut Vec<TextLine>| {
                if !current.is_empty() {
                    lines.push(TextLine {
                        width: *current_w,
                        text: std::mem::take(current),
                    });
                }
                *current_w = 0.0;
            };
            for word in collapsed.split(' ') {
                let word_w = text_width(word, size);
                if !avail_ok || word_w > avail_width {
                    flush(&mut current, &mut current_w, &mut lines);
                    push_chunks(&mut lines, word, size, avail_width);
                    continue;
                }
                let candidate = if current.is_empty() {
                    word_w
                } else {
                    current_w + space_w + word_w
                };
                if candidate <= avail_width {
                    if !current.is_empty() {
                        current.push(' ');
                    }
                    current.push_str(word);
                    current_w = candidate;
                } else {
                    flush(&mut current, &mut current_w, &mut lines);
                    current.push_str(word);
                    current_w = word_w;
                }
            }
            flush(&mut current, &mut current_w, &mut lines);
        }
    }

    if lines.is_empty() {
        lines.push(TextLine::default());
    }
    let width = lines.iter().map(|line| line.width).fold(0.0f32, f32::max);
    let height = line_h * lines.len() as f32;
    TextLayout {
        lines,
        width,
        height,
        line_height: line_h,
    }
}

fn push_measured(lines: &mut Vec<TextLine>, text: String, font_size: f32) {
    lines.push(TextLine {
        width: text_width(&text, font_size),
        text,
    });
}

fn push_chunks(lines: &mut Vec<TextLine>, text: &str, font_size: f32, avail_width: f32) {
    for chunk in char_chunks(text, font_size, avail_width) {
        push_measured(lines, chunk, font_size);
    }
}

fn collapse_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pending_space = false;
    for ch in input.chars() {
        if ch.is_ascii_whitespace() {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(ch);
        }
    }
    out
}

/// Greedy char-level chunks by measured caret position; degenerate avail yields one char per chunk.
fn char_chunks(text: &str, font_size: f32, avail_width: f32) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let boundaries = glyphs::caret_positions(text, font_size).unwrap_or_else(|| {
        (0..=chars.len())
            .map(|i| i as f32 * char_width(font_size))
            .collect()
    });
    let usable = avail_width.is_finite() && avail_width > 0.0;
    let mut chunks = Vec::new();
    let mut start = 0usize;
    for end in 1..=chars.len() {
        let fits = usable && boundaries[end] - boundaries[start] <= avail_width;
        if !fits && end - start > 1 {
            chunks.push(chars[start..end - 1].iter().collect());
            start = end - 1;
        } else if !fits {
            // One char wider than the available width still gets its own line.
            chunks.push(chars[start..end].iter().collect());
            start = end;
        }
    }
    if start < chars.len() {
        chunks.push(chars[start..].iter().collect());
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widths(layout: &TextLayout) -> Vec<f32> {
        layout.lines.iter().map(|line| line.width).collect()
    }

    fn texts(layout: &TextLayout) -> Vec<&str> {
        layout.lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn word_wrap_prefers_boundaries() {
        let layout = layout_text("aa bb cc", 10.0, 5.0 * 10.0 * 0.58, TextWrap::Wrap, false);
        assert_eq!(texts(&layout), vec!["aa bb", "cc"]);
        assert!((layout.line_height - 13.0).abs() < 0.01);
        assert!((layout.height - 26.0).abs() < 0.01);
        assert!(
            widths(&layout)
                .iter()
                .all(|&w| w <= 5.0 * 10.0 * 0.58 + 0.01)
        );
    }

    #[test]
    fn overlong_token_char_splits() {
        let layout = layout_text("abcdefgh", 10.0, 3.0 * 10.0 * 0.58, TextWrap::Wrap, false);
        assert!(layout.lines.len() > 1);
        assert_eq!(layout.lines.iter().map(|l| l.text.len()).sum::<usize>(), 8);
        assert!(
            widths(&layout)
                .iter()
                .all(|&w| w <= 3.0 * 10.0 * 0.58 + 0.01)
        );
    }

    #[test]
    fn newlines_preserved_and_whitespace_collapsed() {
        let layout = layout_text("a   b\n\nc\td", 10.0, 1000.0, TextWrap::Wrap, false);
        assert_eq!(texts(&layout), vec!["a b", "", "c d"]);
    }

    #[test]
    fn narrow_width_degrades_to_char_lines() {
        let layout = layout_text("ab cd", 10.0, 0.0, TextWrap::Wrap, false);
        assert!(layout.lines.len() >= 4);
        assert!(widths(&layout).iter().all(|&w| w <= 10.0 * 0.58 + 0.01));
    }

    #[test]
    fn degenerate_single_char_wider_than_avail_is_allowed() {
        let layout = layout_text("a", 10.0, 1.0, TextWrap::Wrap, false);
        assert_eq!(layout.lines.len(), 1);
        assert_eq!(layout.lines[0].text, "a");
    }

    #[test]
    fn nowrap_keeps_single_line_per_paragraph() {
        let layout = layout_text("a b c\nd e", 10.0, 1.0, TextWrap::NoWrap, false);
        assert_eq!(texts(&layout), vec!["a b c", "d e"]);
    }

    #[test]
    fn laid_out_width_equals_caret_end_for_a_long_mixed_string() {
        // Caret bars (`.caret` spans, Finder query, assistant input) are flex siblings placed at the
        // laid-out width of the text before them. That width must equal the shared caret model's end.
        let text = "Wi AVAST, Tofu: ilm 日本 Wa ff W./ VAT To ilm AVocado Search everything";
        assert!(text.chars().count() >= 40);
        for size in [13.0f32, 15.0] {
            let layout = layout_text(text, size, 10_000.0, TextWrap::NoWrap, false);
            let carets = glyphs::caret_positions(text, size);
            let Some(carets) = carets else {
                return; // no font installed on this machine
            };
            let end = *carets.last().expect("end");
            assert!(
                (layout.lines[0].width - end).abs() < 0.5,
                "{size}px: layout width {} vs caret end {end}",
                layout.lines[0].width
            );
        }
    }

    #[test]
    fn readme_txt_width_tracks_installed_plex_measurement() {
        // Measured: IBM Plex Sans Regular "readme.txt" at 13px = 63.6px (PIL).
        // Desktop label line box is 76px wide; estimate must fit with 4px slack.
        let layout = layout_text("readme.txt", 13.0, 76.0, TextWrap::NoWrap, false);
        let width = layout.lines[0].width;
        let measured = 63.6f32;
        let expected = 10.0 * 13.0 * 0.49;
        assert!((expected - measured).abs() <= 2.0);
        assert!(
            (width - measured).abs() <= 2.0,
            "estimated width {width} not within 2px of measured {measured}"
        );
        assert!(
            width <= 76.0 - 4.0,
            "estimated width {width} exceeds label box minus 4px"
        );
    }

    #[test]
    fn break_anywhere_fills_chars() {
        let layout = layout_text("aa bb", 10.0, 3.0 * 10.0 * 0.58, TextWrap::Wrap, true);
        let joined: String = layout.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(joined, "aa bb");
        assert!(layout.lines.len() > 1);
    }
}

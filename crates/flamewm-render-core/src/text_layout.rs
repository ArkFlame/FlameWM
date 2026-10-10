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

fn text_width(text: &str, font_size: f32) -> f32 {
    text.chars().count() as f32 * char_width(font_size)
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
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<TextLine> = Vec::new();

    for paragraph in normalized.split('\n') {
        let collapsed = collapse_whitespace(paragraph);
        if collapsed.is_empty() {
            lines.push(TextLine::default());
            continue;
        }
        if wrap == TextWrap::NoWrap && avail_width > 0.0 && avail_width.is_finite() {
            lines.push(TextLine {
                width: text_width(&collapsed, size),
                text: collapsed,
            });
            continue;
        }
        if break_anywhere {
            for chunk in char_chunks(&collapsed, size, avail_width) {
                lines.push(TextLine {
                    width: text_width(&chunk, size),
                    text: chunk,
                });
            }
            continue;
        }
        // Greedy word fill; overlong tokens char-split onto own lines.
        let mut current = String::new();
        let mut current_w = 0.0f32;
        let space_w = char_width(size);
        let flush = |current: &mut String, current_w: &mut f32, lines: &mut Vec<TextLine>| {
            lines.push(TextLine {
                width: *current_w,
                text: std::mem::take(current),
            });
            *current_w = 0.0;
        };
        for word in collapsed.split(' ') {
            let word_w = text_width(word, size);
            if word_w > avail_width && avail_width.is_finite() && avail_width > 0.0 {
                if !current.is_empty() {
                    flush(&mut current, &mut current_w, &mut lines);
                }
                for chunk in char_chunks(word, size, avail_width) {
                    lines.push(TextLine {
                        width: text_width(&chunk, size),
                        text: chunk,
                    });
                }
                continue;
            }
            // Narrow/degenerate avail: char-split the word.
            if !(avail_width.is_finite() && avail_width > 0.0) {
                // Narrow avail: one char-split stream per word.
                if !current.is_empty() {
                    flush(&mut current, &mut current_w, &mut lines);
                }
                for chunk in char_chunks(word, size, avail_width) {
                    lines.push(TextLine {
                        width: text_width(&chunk, size),
                        text: chunk,
                    });
                }
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
                if !current.is_empty() {
                    flush(&mut current, &mut current_w, &mut lines);
                }
                current.push_str(word);
                current_w = word_w;
            }
        }
        if !current.is_empty() {
            lines.push(TextLine {
                width: current_w,
                text: current,
            });
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

/// Greedy char-level chunks; degenerate avail yields one char per chunk.
fn char_chunks(text: &str, font_size: f32, avail_width: f32) -> Vec<String> {
    let char_w = char_width(font_size);
    let per_line = if !avail_width.is_finite() || avail_width <= 0.0 || char_w <= 0.0 {
        1
    } else {
        ((avail_width / char_w).floor() as usize).max(1)
    };
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(per_line)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widths(layout: &TextLayout) -> Vec<f32> {
        layout.lines.iter().map(|line| line.width).collect()
    }

    #[test]
    fn word_wrap_prefers_boundaries() {
        let layout = layout_text("aa bb cc", 10.0, 5.0 * 10.0 * 0.58, TextWrap::Wrap, false);
        let texts: Vec<&str> = layout.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["aa bb", "cc"]);
        assert!((layout.line_height - 13.0).abs() < 0.01);
        assert!((layout.height - 26.0).abs() < 0.01);
        for w in widths(&layout) {
            assert!(w <= 5.0 * 10.0 * 0.58 + 0.01);
        }
    }

    #[test]
    fn overlong_token_char_splits() {
        let layout = layout_text("abcdefgh", 10.0, 3.0 * 10.0 * 0.58, TextWrap::Wrap, false);
        assert!(layout.lines.len() > 1);
        assert_eq!(layout.lines.iter().map(|l| l.text.len()).sum::<usize>(), 8);
        for w in widths(&layout) {
            assert!(w <= 3.0 * 10.0 * 0.58 + 0.01);
        }
    }

    #[test]
    fn newlines_preserved_and_whitespace_collapsed() {
        let layout = layout_text("a   b\n\nc\td", 10.0, 1000.0, TextWrap::Wrap, false);
        let texts: Vec<&str> = layout.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["a b", "", "c d"]);
    }

    #[test]
    fn narrow_width_degrades_to_char_lines() {
        let layout = layout_text("ab cd", 10.0, 0.0, TextWrap::Wrap, false);
        assert!(layout.lines.len() >= 4);
        for w in widths(&layout) {
            assert!(w <= 10.0 * 0.58 + 0.01);
        }
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
        let texts: Vec<&str> = layout.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["a b c", "d e"]);
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
        let joined: String = layout.lines.iter().map(|l| l.text.clone()).collect();
        assert_eq!(joined, "aa bb");
        assert!(layout.lines.len() > 1);
    }
}

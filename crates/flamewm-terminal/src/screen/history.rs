//! Scrollback line storage. A history line keeps its characters as UTF-8 text plus one run per stretch of equal
//! style, which is far smaller than a `Cell` per column for ordinary output. A one-style line stores no run list at
//! all; lines whose style changes too often to pay for the runs stay as plain cells. Every form round-trips exactly.
use std::borrow::Cow;
use std::mem::size_of;

use super::cell::{Cell, Color, Line};

/// `len` consecutive cells that share one style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct StyleRun {
    len: u32,
    fg: Color,
    bg: Color,
    attrs: u16,
}

#[derive(Debug)]
pub(super) enum History {
    Plain(Line),
    /// One style across the line: shell and build output, where a `runs` boxed slice would be pure overhead.
    Uniform {
        text: Box<str>,
        fg: Color,
        bg: Color,
        attrs: u16,
    },
    Packed {
        text: Box<str>,
        runs: Box<[StyleRun]>,
    },
}

impl History {
    /// Packs `line` (already trimmed, see `history_len`) into the smaller of the three forms. Borrowed so the live row's buffer is reused.
    pub(super) fn pack(line: &[Cell]) -> Self {
        let mut text = String::with_capacity(line.len());
        // The run list is built only when a second style appears: ordinary output is one style per line, so the
        // common case allocates no `Vec` at all, and the trailing run stays in `style`/`len` until the end.
        let mut runs: Vec<StyleRun> = Vec::new();
        let mut style: Option<(Color, Color, u16)> = None;
        let mut len = 0_u32;
        for cell in line {
            text.push(cell.ch);
            let next = (cell.fg, cell.bg, cell.attrs);
            match style {
                Some(current) if current == next => len += 1,
                Some(current) => {
                    if runs.is_empty() {
                        runs.reserve(8);
                    }
                    runs.push(StyleRun {
                        len,
                        fg: current.0,
                        bg: current.1,
                        attrs: current.2,
                    });
                    style = Some(next);
                    len = 1;
                }
                None => {
                    style = Some(next);
                    len = 1;
                }
            }
        }
        // One style across the whole line: shell and build output, stored as text with no run list.
        if runs.is_empty() {
            if let Some(current) = style {
                let packed = text.len() + size_of::<StyleRun>();
                if packed < std::mem::size_of_val(line) {
                    return Self::Uniform {
                        text: text.into_boxed_str(),
                        fg: current.0,
                        bg: current.1,
                        attrs: current.2,
                    };
                }
            }
            return Self::Plain(line.to_vec());
        }
        if let Some(current) = style {
            runs.push(StyleRun {
                len,
                fg: current.0,
                bg: current.1,
                attrs: current.2,
            });
        }
        let packed = text.len() + runs.len() * size_of::<StyleRun>();
        if packed < std::mem::size_of_val(line) {
            Self::Packed {
                text: text.into_boxed_str(),
                runs: runs.into_boxed_slice(),
            }
        } else {
            Self::Plain(line.to_vec())
        }
    }

    /// The line's cells, borrowed when stored plain and decoded otherwise.
    pub(super) fn cells(&self) -> Cow<'_, Line> {
        if let Self::Plain(line) = self {
            Cow::Borrowed(line)
        } else {
            let mut out = Line::new();
            self.cells_into(&mut out);
            Cow::Owned(out)
        }
    }

    /// Writes the line's cells into `out`, which is cleared first. A scan over the scrollback reuses one buffer
    /// instead of allocating a decoded line per entry.
    pub(super) fn cells_into(&self, out: &mut Line) {
        out.clear();
        match self {
            Self::Plain(line) => out.extend_from_slice(line),
            Self::Uniform {
                text,
                fg,
                bg,
                attrs,
            } => out.extend(text.chars().map(|ch| Cell {
                ch,
                fg: *fg,
                bg: *bg,
                attrs: *attrs,
            })),
            Self::Packed { text, runs } => {
                let mut chars = text.chars();
                out.reserve(runs.iter().map(|run| run.len as usize).sum());
                for run in runs {
                    for _ in 0..run.len {
                        out.push(Cell {
                            ch: chars.next().unwrap_or(' '),
                            fg: run.fg,
                            bg: run.bg,
                            attrs: run.attrs,
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::attr;

    fn cell(ch: char, fg: Color, attrs: u16) -> Cell {
        Cell {
            ch,
            fg,
            bg: Color::Default,
            attrs,
        }
    }

    #[test]
    fn uniform_text_stores_no_run_list_and_round_trips() {
        let line: Line = "hello wörld 世界"
            .chars()
            .map(|c| cell(c, Color::Default, 0))
            .collect();
        let history = History::pack(&line);
        assert!(matches!(history, History::Uniform { .. }));
        assert_eq!(*history.cells(), line);
    }

    #[test]
    fn wide_pair_inside_packed_line_round_trips() {
        let mut line: Line = (0..60).map(|_| cell('a', Color::Default, 0)).collect();
        line[10] = cell('世', Color::Rgb(1, 2, 3), attr::WIDE);
        line[11] = cell(' ', Color::Rgb(1, 2, 3), attr::SPACER);
        let history = History::pack(&line);
        assert!(matches!(history, History::Packed { .. }));
        assert_eq!(*history.cells(), line);
    }

    #[test]
    fn style_churn_falls_back_to_plain_cells() {
        let line: Line = (0..40)
            .map(|i| cell('x', Color::Indexed((i % 2) as u8), 0))
            .collect();
        let history = History::pack(&line);
        assert!(matches!(history, History::Plain(_)));
        assert_eq!(*history.cells(), line);
    }
}

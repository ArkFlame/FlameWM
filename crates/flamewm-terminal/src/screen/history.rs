//! Scrollback line storage. A history line keeps its characters as UTF-8 text plus one run per stretch of equal
//! style, which is far smaller than a `Cell` per column for ordinary output. Lines whose style changes too often
//! to pay for the runs stay as plain cells. Either form round-trips every cell exactly.
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
    Packed {
        text: Box<str>,
        runs: Box<[StyleRun]>,
    },
}

impl History {
    /// Packs `line` (already compacted, see `compact_history`) into the smaller of the two forms.
    pub(super) fn pack(line: Line) -> Self {
        let mut text = String::with_capacity(line.len());
        let mut runs: Vec<StyleRun> = Vec::new();
        for cell in &line {
            text.push(cell.ch);
            match runs.last_mut() {
                Some(run) if run.fg == cell.fg && run.bg == cell.bg && run.attrs == cell.attrs => {
                    run.len += 1;
                }
                _ => runs.push(StyleRun {
                    len: 1,
                    fg: cell.fg,
                    bg: cell.bg,
                    attrs: cell.attrs,
                }),
            }
        }
        let packed = text.len() + runs.len() * size_of::<StyleRun>();
        if packed < line.len() * size_of::<Cell>() {
            Self::Packed {
                text: text.into_boxed_str(),
                runs: runs.into_boxed_slice(),
            }
        } else {
            Self::Plain(line)
        }
    }

    /// The line's cells, borrowed when stored plain and decoded otherwise.
    pub(super) fn cells(&self) -> Cow<'_, Line> {
        match self {
            Self::Plain(line) => Cow::Borrowed(line),
            Self::Packed { text, runs } => {
                let mut chars = text.chars();
                let mut line = Vec::with_capacity(runs.iter().map(|run| run.len as usize).sum());
                for run in runs {
                    for _ in 0..run.len {
                        line.push(Cell {
                            ch: chars.next().unwrap_or(' '),
                            fg: run.fg,
                            bg: run.bg,
                            attrs: run.attrs,
                        });
                    }
                }
                Cow::Owned(line)
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
    fn uniform_text_packs_and_round_trips() {
        let line: Line = "hello wörld 世界"
            .chars()
            .map(|c| cell(c, Color::Default, 0))
            .collect();
        let history = History::pack(line.clone());
        assert!(matches!(history, History::Packed { .. }));
        assert_eq!(*history.cells(), line);
    }

    #[test]
    fn wide_pair_inside_packed_line_round_trips() {
        let mut line: Line = (0..60).map(|_| cell('a', Color::Default, 0)).collect();
        line[10] = cell('世', Color::Rgb(1, 2, 3), attr::WIDE);
        line[11] = cell(' ', Color::Rgb(1, 2, 3), attr::SPACER);
        let history = History::pack(line.clone());
        assert!(matches!(history, History::Packed { .. }));
        assert_eq!(*history.cells(), line);
    }

    #[test]
    fn style_churn_falls_back_to_plain_cells() {
        let line: Line = (0..40)
            .map(|i| cell('x', Color::Indexed((i % 2) as u8), 0))
            .collect();
        let history = History::pack(line.clone());
        assert!(matches!(history, History::Plain(_)));
        assert_eq!(*history.cells(), line);
    }
}

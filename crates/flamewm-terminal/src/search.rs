//! Find in scrollback: case-insensitive plain-text search over absolute screen lines.
//!
//! The matcher works on one `char` per cell, so a match column is a cell column. Wide-character
//! spacer cells read as NUL and never match. Matches never overlap within a line.

use crate::screen::{Cell, Screen, attr};

/// One hit: absolute line index (scrollback first), starting cell column and length in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub line: usize,
    pub col: usize,
    pub len: usize,
}

/// Cell text of one line: one char per cell, NUL for a wide-character spacer.
#[must_use]
pub fn line_chars(line: &[Cell]) -> Vec<char> {
    line.iter()
        .map(|cell| {
            if cell.attrs & attr::SPACER != 0 {
                '\0'
            } else {
                cell.ch
            }
        })
        .collect()
}

/// Case fold of one char. Single-char folding keeps columns aligned with the cells. `match_case` keeps the char as is.
fn fold(c: char, match_case: bool) -> char {
    if match_case {
        c
    } else {
        c.to_lowercase().next().unwrap_or(c)
    }
}

/// Every non-overlapping occurrence of `query` in `lines`, in reading order. Case-insensitive unless `match_case`.
/// An empty query matches nothing.
#[must_use]
pub fn find_in(lines: &[Vec<char>], query: &str, match_case: bool) -> Vec<Match> {
    let needle: Vec<char> = query.chars().map(|c| fold(c, match_case)).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    for (line_index, line) in lines.iter().enumerate() {
        let folded: Vec<char> = line.iter().map(|&c| fold(c, match_case)).collect();
        let mut col = 0;
        while col + needle.len() <= folded.len() {
            if folded[col..col + needle.len()] == needle[..] {
                matches.push(Match {
                    line: line_index,
                    col,
                    len: needle.len(),
                });
                col += needle.len();
            } else {
                col += 1;
            }
        }
    }
    matches
}

/// Searches every line of `screen`, scrollback included. The alt screen has no scrollback.
#[must_use]
pub fn find(screen: &Screen, query: &str, match_case: bool) -> Vec<Match> {
    let lines: Vec<Vec<char>> = (0..screen.total_lines())
        .filter_map(|index| screen.line_abs(index))
        .map(|line| line_chars(&line))
        .collect();
    find_in(&lines, query, match_case)
}

/// Find bar state of one pane. `current` indexes `matches` and is kept in range by `step`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Search {
    pub query: String,
    pub matches: Vec<Match>,
    pub current: usize,
    /// Exact-case matching when set; the find bar starts case-insensitive.
    pub match_case: bool,
}

impl Search {
    #[must_use]
    pub fn current_match(&self) -> Option<Match> {
        self.matches.get(self.current).copied()
    }
}

/// Scroll offset that puts absolute `line` near the middle of the view. Zero is the live bottom.
#[must_use]
pub fn view_for_line(screen: &Screen, line: usize) -> usize {
    let bottom = screen.total_lines().saturating_sub(screen.rows());
    let top = line.saturating_sub(screen.rows() / 2).min(bottom);
    (bottom - top).min(screen.scrollback_len())
}

/// Index of the next (or previous) match, wrapping at both ends. `None` when there are no matches.
#[must_use]
pub fn step(count: usize, current: usize, forward: bool) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let current = current.min(count - 1);
    Some(if forward {
        (current + 1) % count
    } else {
        (current + count - 1) % count
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    #[test]
    fn finds_case_insensitive_matches_in_reading_order() {
        let lines = vec![
            chars("Hello World"),
            chars("no hit here"),
            chars("WORLD of worlds"),
        ];
        let found = find_in(&lines, "world", false);
        assert_eq!(
            found,
            vec![
                Match {
                    line: 0,
                    col: 6,
                    len: 5
                },
                Match {
                    line: 2,
                    col: 0,
                    len: 5
                },
                Match {
                    line: 2,
                    col: 9,
                    len: 5
                },
            ]
        );
    }

    #[test]
    fn matches_do_not_overlap_within_a_line() {
        let lines = vec![chars("aaaa")];
        let found = find_in(&lines, "aa", false);
        assert_eq!(found.iter().map(|m| m.col).collect::<Vec<_>>(), vec![0, 2]);
    }

    #[test]
    fn empty_query_and_missing_text_match_nothing() {
        let lines = vec![chars("abc")];
        assert!(find_in(&lines, "", false).is_empty());
        assert!(find_in(&lines, "zz", false).is_empty());
        assert!(find_in(&lines, "abcd", false).is_empty());
    }

    #[test]
    fn non_ascii_folds_by_char() {
        let lines = vec![chars("Ünïcode ÉCOLE")];
        let found = find_in(&lines, "école", false);
        assert_eq!(
            found,
            vec![Match {
                line: 0,
                col: 8,
                len: 5
            }]
        );
    }

    #[test]
    fn match_case_mode_only_matches_exact_case() {
        let lines = vec![chars("Hello hello HELLO")];
        let insensitive = find_in(&lines, "hello", false);
        assert_eq!(
            insensitive.iter().map(|m| m.col).collect::<Vec<_>>(),
            vec![0, 6, 12]
        );
        let exact = find_in(&lines, "hello", true);
        assert_eq!(exact.iter().map(|m| m.col).collect::<Vec<_>>(), vec![6]);
        assert_eq!(find_in(&lines, "Hello", true)[0].col, 0);
        assert!(find_in(&lines, "HeLLo", true).is_empty());
    }

    #[test]
    fn wide_spacer_cells_never_match_the_query() {
        let line = vec![
            Cell {
                ch: '漢',
                fg: crate::screen::Color::Default,
                bg: crate::screen::Color::Default,
                attrs: attr::WIDE,
            },
            Cell {
                ch: ' ',
                fg: crate::screen::Color::Default,
                bg: crate::screen::Color::Default,
                attrs: attr::SPACER,
            },
        ];
        assert_eq!(line_chars(&line), vec!['漢', '\0']);
        assert!(find_in(&[line_chars(&line)], " ", false).is_empty());
    }

    #[test]
    fn step_wraps_both_ways_and_handles_no_matches() {
        assert_eq!(step(0, 0, true), None);
        assert_eq!(step(3, 2, true), Some(0));
        assert_eq!(step(3, 0, false), Some(2));
        assert_eq!(step(3, 0, true), Some(1));
        assert_eq!(step(1, 0, false), Some(0));
    }

    #[test]
    fn view_offset_brings_a_scrollback_line_into_view() {
        let mut screen = Screen::new(8, 2, 10);
        Parser::new().advance(b"one\r\ntwo\r\nthree\r\nfour", &mut screen);
        assert_eq!(
            view_for_line(&screen, screen.total_lines() - 1),
            0,
            "live line stays at bottom"
        );
        let offset = view_for_line(&screen, 0);
        assert!(offset > 0 && offset <= screen.scrollback_len());
        let top = screen.total_lines() - (screen.rows() + offset);
        assert!(
            (top..top + screen.rows()).contains(&0),
            "line 0 inside view"
        );
    }

    #[test]
    fn search_state_reports_the_current_match() {
        let mut state = Search {
            query: "x".into(),
            matches: vec![Match {
                line: 1,
                col: 0,
                len: 1,
            }],
            current: 0,
            match_case: false,
        };
        assert_eq!(
            state.current_match(),
            Some(Match {
                line: 1,
                col: 0,
                len: 1
            })
        );
        state.matches.clear();
        assert_eq!(state.current_match(), None);
    }

    #[test]
    fn finds_text_in_scrollback_and_live_grid() {
        // Two rows of width 8 with 10 lines of scrollback: the first line scrolls into history.
        let mut screen = Screen::new(8, 2, 10);
        Parser::new().advance(b"needle1\r\nmid\r\nneedle2", &mut screen);
        let found = find(&screen, "NEEDLE", false);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].line, 0, "first hit is in scrollback");
        assert!(
            found[1].line >= screen.scrollback_len(),
            "second hit is on the live grid"
        );
    }
}

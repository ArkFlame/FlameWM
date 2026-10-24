//! SGR extended colour parsing (`38/48` in both semicolon and colon forms).

use super::cell::Color;

/// Parses `38/48` extended colours in both forms: `38;5;n` / `38;2;r;g;b` (separate groups) and `38:5:n` /
/// `38:2:cs:r:g:b` (one group). Returns the colour and how many extra groups it consumed.
pub(super) fn extended_color(group: &[u16], params: &[Vec<u16>], index: usize) -> (Option<Color>, usize) {
    if group.len() > 1 {
        return match group[1] {
            5 => (group.get(2).map(|n| Color::Indexed(*n as u8)), 0),
            2 if group.len() >= 6 => (Some(Color::Rgb(group[3] as u8, group[4] as u8, group[5] as u8)), 0),
            2 if group.len() >= 5 => (Some(Color::Rgb(group[2] as u8, group[3] as u8, group[4] as u8)), 0),
            _ => (None, 0),
        };
    }
    let first = |i: usize| params.get(index + i).and_then(|g| g.first()).copied().unwrap_or(0);
    match params.get(index + 1).and_then(|g| g.first()).copied() {
        Some(5) => (Some(Color::Indexed(first(2) as u8)), 2),
        Some(2) => (Some(Color::Rgb(first(2) as u8, first(3) as u8, first(4) as u8)), 4),
        _ => (None, 0),
    }
}

//! Cell, colour and pen value types, the `attr` bit flags and the line helpers the grid uses.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

pub mod attr {
    pub const BOLD: u16 = 1;
    pub const ITALIC: u16 = 2;
    pub const UNDERLINE: u16 = 4;
    pub const INVERSE: u16 = 8;
    pub const DIM: u16 = 16;
    pub const STRIKE: u16 = 32;
    /// First cell of a double-width character.
    pub const WIDE: u16 = 64;
    /// Second cell of a double-width character; holds no glyph of its own.
    pub const SPACER: u16 = 128;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: u16,
}

impl Cell {
    pub const fn blank(bg: Color) -> Self {
        Self {
            ch: ' ',
            fg: Color::Default,
            bg,
            attrs: 0,
        }
    }
}

pub type Line = Vec<Cell>;

/// Drops trailing default blanks (and their capacity, keeping one cell so `line[0]` stays valid) from a line entering scrollback; readers
/// iterate to `line.len()` and the resize path pads history lines back to the column count.
pub(super) fn compact_history(mut line: Line) -> Line {
    let blank = Cell::blank(Color::Default);
    line.truncate(
        line.iter()
            .rposition(|cell| *cell != blank)
            .map_or(1, |last| last + 1),
    );
    line.shrink_to_fit();
    line
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Pen {
    pub fg: Color,
    pub bg: Color,
    pub attrs: u16,
}

pub(super) fn blank_line(cols: usize, bg: Color) -> Line {
    vec![Cell::blank(bg); cols]
}

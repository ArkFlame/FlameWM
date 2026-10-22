//! Cell grid and settings text to per-row runs. A run is a span of cells that share colours and flags; the UI draws
//! each run as one absolutely positioned text node on a fixed cell pitch. Pure functions only, so the same code
//! is unit tested and drives the screen.
use crate::screen::{Cell, Color, Screen, attr};
use crate::themes::Palette;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// First cell column of the run.
    pub col: usize,
    /// Width in cells (a wide glyph counts as two).
    pub cells: usize,
    pub text: String,
    pub fg: u32,
    /// `None` draws no background, so the window surface shows through.
    pub bg: Option<u32>,
    pub bold: bool,
}

/// Inclusive selection in absolute line coordinates (scrollback first), normalised so `start <= end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

impl Span {
    pub fn normalised(a: (usize, usize), b: (usize, usize)) -> Self {
        if a <= b { Self { start: a, end: b } } else { Self { start: b, end: a } }
    }

    fn contains(&self, row: usize, col: usize) -> bool {
        (row, col) >= self.start && (row, col) <= self.end
    }
}

pub struct GridOptions {
    /// Lines scrolled back from the live bottom; 0 shows the live grid.
    pub view_offset: usize,
    pub selection: Option<Span>,
    /// Draw the cursor (block style, inverted) when the terminal has focus and the child has not hidden it.
    pub cursor: bool,
}

/// xterm 256-colour palette index to RGB (16 ANSI from the theme, then the 6x6x6 cube and the greyscale ramp).
pub fn indexed_rgb(index: u8, palette: &Palette) -> u32 {
    if index < 16 {
        return palette.ansi[index as usize];
    }
    if index >= 232 {
        let level = 8 + 10 * u32::from(index - 232);
        return (level << 16) | (level << 8) | level;
    }
    const LEVELS: [u32; 6] = [0, 95, 135, 175, 215, 255];
    let cube = u32::from(index - 16);
    let (r, g, b) = (cube / 36, (cube / 6) % 6, cube % 6);
    (LEVELS[r as usize] << 16) | (LEVELS[g as usize] << 8) | LEVELS[b as usize]
}

fn resolve(color: Color, palette: &Palette, default: u32) -> u32 {
    match color {
        Color::Default => default,
        Color::Indexed(index) => indexed_rgb(index, palette),
        Color::Rgb(r, g, b) => (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b),
    }
}

/// Blends `fg` toward `bg` by half, the SGR "faint" rendering.
fn dimmed(fg: u32, bg: u32) -> u32 {
    let mix = |shift: u32| ((fg >> shift) & 0xff) / 2 + ((bg >> shift) & 0xff) / 2;
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// Colours of one cell after SGR inverse, dim, selection and cursor are applied.
fn cell_colours(cell: &Cell, palette: &Palette, selected: bool, cursor: bool) -> (u32, u32) {
    let mut fg = resolve(cell.fg, palette, palette.foreground);
    let mut bg = resolve(cell.bg, palette, palette.background);
    let inverse = cell.attrs & attr::INVERSE != 0;
    if inverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.attrs & attr::DIM != 0 {
        fg = dimmed(fg, bg);
    }
    if selected {
        bg = palette.selection;
        fg = palette.foreground;
    }
    if cursor {
        bg = palette.cursor;
        fg = palette.background;
    }
    (fg, bg)
}

/// Rows of runs for the live grid or scrollback view. Adjacent cells with equal colours and weight merge.
pub fn grid_runs(screen: &Screen, palette: &Palette, options: &GridOptions) -> Vec<Vec<Run>> {
    let total = screen.total_lines();
    let top = total.saturating_sub(screen.rows() + options.view_offset.min(screen.scrollback_len()));
    let (cursor_row, cursor_col) = screen.cursor();
    let show_cursor = options.cursor && options.view_offset == 0 && screen.cursor_visible;
    let mut rows = Vec::with_capacity(screen.rows());
    for row in 0..screen.rows() {
        let index = top + row;
        let Some(line) = screen.line_abs(index) else {
            rows.push(Vec::new());
            continue;
        };
        let mut runs: Vec<Run> = Vec::new();
        let mut col = 0;
        while col < line.len() {
            let cell = &line[col];
            if cell.attrs & attr::SPACER != 0 {
                col += 1;
                continue;
            }
            let width = if cell.attrs & attr::WIDE != 0 { 2 } else { 1 };
            let selected = options.selection.is_some_and(|span| span.contains(index, col));
            let cursor = show_cursor && row == cursor_row && col == cursor_col;
            let (fg, bg) = cell_colours(cell, palette, selected, cursor);
            let bold = cell.attrs & attr::BOLD != 0;
            let bg_drawn = bg_visible(bg, palette);
            let mergeable = runs.last().is_some_and(|last| last.fg == fg && last.bg == bg_drawn && last.bold == bold && last.col + last.cells == col);
            if mergeable {
                let last = runs.last_mut().expect("checked above");
                last.text.push(cell.ch);
                last.cells += width;
            } else {
                runs.push(Run { col, cells: width, text: cell.ch.to_string(), fg, bg: bg_drawn, bold });
            }
            col += width;
        }
        rows.push(runs);
    }
    rows
}

/// Background colour as drawn: the theme's default background is left undrawn so the window surface (and its
/// translucency) shows through.
fn bg_visible(bg: u32, palette: &Palette) -> Option<u32> {
    (bg != palette.background).then_some(bg)
}

/// Converts plain text into one run per line (settings, tab bar). Wide glyphs are not expected here.
pub fn text_run(col: usize, text: &str, fg: u32, bg: Option<u32>, bold: bool) -> Run {
    Run { col, cells: text.chars().count(), text: text.to_owned(), fg, bg, bold }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;
    use crate::themes::builtin_themes;

    fn palette() -> Palette {
        builtin_themes()[0].palette.clone()
    }

    fn feed(screen: &mut Screen, bytes: &[u8]) {
        Parser::new().advance(bytes, screen);
    }

    fn opts(cursor: bool) -> GridOptions {
        GridOptions { view_offset: 0, selection: None, cursor }
    }

    #[test]
    fn indexed_palette_covers_ansi_cube_and_grey() {
        let p = palette();
        assert_eq!(indexed_rgb(1, &p), p.ansi[1]);
        assert_eq!(indexed_rgb(16, &p), 0x000000);
        assert_eq!(indexed_rgb(231, &p), 0xffffff);
        assert_eq!(indexed_rgb(232, &p), 0x080808);
        assert_eq!(indexed_rgb(196, &p), 0xff0000);
    }

    #[test]
    fn equal_styled_cells_merge_into_one_run() {
        let p = palette();
        let mut screen = Screen::new(10, 1, 0);
        feed(&mut screen, b"abc\x1b[31mde\x1b[0m");
        let rows = grid_runs(&screen, &p, &opts(false));
        assert_eq!(rows[0].len(), 3, "typed text, red run, then the blank tail");
        assert_eq!(rows[0][0].text, "abc");
        assert_eq!(rows[0][0].bg, None, "default background draws nothing");
        assert_eq!(rows[0][1].text, "de");
        assert_eq!(rows[0][1].fg, p.ansi[1]);
        assert_eq!(rows[0][1].col, 3);
        assert_eq!(rows[0][2].col, 5);
    }

    #[test]
    fn inverse_and_cursor_swap_colours() {
        let p = palette();
        let mut screen = Screen::new(4, 1, 0);
        feed(&mut screen, b"\x1b[7mx\x1b[0m");
        let rows = grid_runs(&screen, &p, &opts(false));
        assert_eq!(rows[0][0].fg, p.background);
        assert_eq!(rows[0][0].bg, Some(p.foreground));
        let cursor_rows = grid_runs(&screen, &p, &opts(true));
        let cursor_run = cursor_rows[0].iter().find(|r| r.col == 1).expect("cursor run after the typed glyph");
        assert_eq!(cursor_run.bg, Some(p.cursor));
    }

    #[test]
    fn selection_overrides_colours_and_scrollback_view_offsets_rows() {
        let p = palette();
        let mut screen = Screen::new(3, 2, 10);
        feed(&mut screen, b"aaa\r\nbbb\r\nccc");
        let span = Span::normalised((1, 0), (1, 1));
        let view = grid_runs(&screen, &p, &GridOptions { view_offset: 1, selection: Some(span), cursor: true });
        assert_eq!(view[0][0].text, "aaa");
        assert_eq!(view[1][0].text, "bb");
        assert_eq!(view[1][0].bg, Some(p.selection));
        assert_eq!(view[1][0].col, 0);
        assert_eq!(view[1][1].text, "b", "unselected tail splits off");
        assert_eq!(view[1][1].col, 2);
        assert_eq!(view[1].len(), 2, "selected and unselected cells split into runs");
    }

    #[test]
    fn wide_glyph_counts_two_cells_and_skips_spacer() {
        let p = palette();
        let mut screen = Screen::new(4, 1, 0);
        feed(&mut screen, "界x".as_bytes());
        let rows = grid_runs(&screen, &p, &opts(false));
        assert_eq!(rows[0][0].text, "界x ", "blank tail merges with the default-styled x");
        assert_eq!(rows[0][0].cells, 4, "wide glyph takes two cells, spacer skipped");
    }

    #[test]
    fn dimmed_blends_toward_background() {
        assert_eq!(dimmed(0xffffff, 0x000000), 0x7f7f7f);
    }
}

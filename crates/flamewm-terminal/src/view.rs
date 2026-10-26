//! Cell grid and settings text to per-row runs. A run is a span of cells that share colours and flags; the UI draws
//! each run as one absolutely positioned text node on a fixed cell pitch. Pure functions only, so the same code
//! is unit tested and drives the screen.
use std::sync::Arc;

use crate::glyphs::{Arm, blend, glyph_rects};
use crate::screen::{Cell, Color, CursorShape, Dirty, Screen, attr};
use crate::themes::Palette;

#[derive(Clone, Debug, PartialEq)]
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
    /// Set for a box or block glyph: the run is one rectangle (in `fg`) instead of text. `col` and `cells` place it.
    pub shape: Option<Arm>,
}

/// One row of runs, shared: a cache hit clones the handle, not the runs.
pub type Row = Arc<[Run]>;

/// Inclusive selection in absolute line coordinates (scrollback first), normalised so `start <= end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

impl Span {
    #[must_use]
    pub fn normalised(a: (usize, usize), b: (usize, usize)) -> Self {
        if a <= b {
            Self { start: a, end: b }
        } else {
            Self { start: b, end: a }
        }
    }

    fn contains(&self, row: usize, col: usize) -> bool {
        (row, col) >= self.start && (row, col) <= self.end
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GridOptions {
    /// Lines scrolled back from the live bottom; 0 shows the live grid.
    pub view_offset: usize,
    pub selection: Option<Span>,
    /// Draw the cursor when the terminal has focus and the child has not hidden it.
    pub cursor: bool,
    /// Block inverts the cell; underline and bar draw a thin glyph in the cursor colour instead.
    pub cursor_shape: CursorShape,
}

/// xterm 256-colour palette index to RGB (16 ANSI from the theme, then the 6x6x6 cube and the greyscale ramp).
#[must_use]
pub fn indexed_rgb(index: u8, palette: &Palette) -> u32 {
    const LEVELS: [u32; 6] = [0, 95, 135, 175, 215, 255];
    if index < 16 {
        return palette.ansi[index as usize];
    }
    if index >= 232 {
        let level = 8 + 10 * u32::from(index - 232);
        return (level << 16) | (level << 8) | level;
    }
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
    blend(fg, bg, 1, 2)
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

/// Absolute index of the first visible line.
fn top_line(screen: &Screen, options: &GridOptions) -> usize {
    screen
        .total_lines()
        .saturating_sub(screen.rows() + options.view_offset.min(screen.scrollback_len()))
}

/// Whether the cursor is drawn: the live view only, and only while the child has not hidden it.
fn cursor_drawn(screen: &Screen, options: &GridOptions) -> bool {
    options.cursor && options.view_offset == 0 && screen.cursor_visible
}

/// Rows of runs for the live grid or scrollback view. Adjacent cells with equal colours and weight merge.
#[must_use]
pub fn grid_runs(screen: &Screen, palette: &Palette, options: &GridOptions) -> Vec<Vec<Run>> {
    let top = top_line(screen, options);
    (0..screen.rows())
        .map(|row| row_runs(screen, palette, options, top, row))
        .collect()
}

/// Runs of visible row `row`, which shows line `top + row`.
fn row_runs(
    screen: &Screen,
    palette: &Palette,
    options: &GridOptions,
    top: usize,
    row: usize,
) -> Vec<Run> {
    let index = top + row;
    let Some(line) = screen.line_abs(index) else {
        return Vec::new();
    };
    let (cursor_row, cursor_col) = screen.cursor();
    let show_cursor = cursor_drawn(screen, options);
    let mut runs: Vec<Run> = Vec::new();
    let mut col = 0;
    while col < line.len() {
        let cell = &line[col];
        if cell.attrs & attr::SPACER != 0 {
            col += 1;
            continue;
        }
        let width = if cell.attrs & attr::WIDE != 0 { 2 } else { 1 };
        let selected = options
            .selection
            .is_some_and(|span| span.contains(index, col));
        let cursor = show_cursor && row == cursor_row && col == cursor_col;
        let block = options.cursor_shape == CursorShape::Block;
        let (mut fg, bg) = cell_colours(cell, palette, selected, cursor && block);
        let mut ch = cell.ch;
        if cursor && !block {
            fg = palette.cursor;
            ch = if options.cursor_shape == CursorShape::Underline {
                '\u{2581}'
            } else {
                '\u{258f}'
            };
        }
        let bold = cell.attrs & attr::BOLD != 0;
        let bg_drawn = bg_visible(bg, palette);
        match glyph_rects(ch, fg, bg) {
            Some(rects) => {
                if bg_drawn.is_some() {
                    push_text(
                        &mut runs,
                        Text {
                            col,
                            cells: width,
                            ch: ' ',
                            fg,
                            bg: bg_drawn,
                            bold,
                        },
                    );
                }
                if !extend_bands(&mut runs, col, &rects) {
                    for (arm, colour) in rects {
                        runs.push(Run {
                            col,
                            cells: width,
                            text: "\u{a0}".into(),
                            fg: colour,
                            bg: Some(colour),
                            bold,
                            shape: Some(arm),
                        });
                    }
                }
            }
            None => push_text(
                &mut runs,
                Text {
                    col,
                    cells: width,
                    ch,
                    fg,
                    bg: bg_drawn,
                    bold,
                },
            ),
        }
        col += width;
    }
    runs
}

/// Grid runs of one pane, kept between frames. Only rows the screen marked dirty are rebuilt, plus the rows the
/// cursor or selection moved through. A changed palette, view offset, cursor shape or grid size rebuilds every row.
#[derive(Default)]
pub struct RunCache {
    key: Option<(Palette, usize, CursorShape)>,
    cursor: Option<(usize, usize)>,
    selection: Option<Span>,
    rows: Vec<Row>,
    /// Rows rebuilt by the last `update`.
    pub rebuilt: usize,
}

impl RunCache {
    /// Current runs for `options`, equal to `grid_runs` on the same screen. Takes the screen's dirty rows. Rows that
    /// are not rebuilt keep their allocation, so callers can tell them apart with `Arc::ptr_eq`.
    pub fn update(
        &mut self,
        screen: &mut Screen,
        palette: &Palette,
        options: &GridOptions,
    ) -> &[Row] {
        let dirty = screen.take_dirty();
        let rows = screen.rows();
        let top = top_line(screen, options);
        let cursor = cursor_drawn(screen, options).then(|| screen.cursor());
        let key_matches = self.key.as_ref().is_some_and(|(kept, offset, shape)| {
            kept == palette && *offset == options.view_offset && *shape == options.cursor_shape
        });
        let mut stale = vec![false; rows];
        if !key_matches || self.rows.len() != rows || dirty == Dirty::All {
            stale.fill(true);
        } else {
            if let Dirty::Rows(changed) = dirty {
                for live in changed {
                    mark_live(screen, top, live, &mut stale);
                }
            }
            if cursor != self.cursor {
                for live in [self.cursor, cursor].into_iter().flatten() {
                    mark_live(screen, top, live.0, &mut stale);
                }
            }
            if options.selection != self.selection {
                let spans = [self.selection, options.selection];
                let lo = spans.iter().flatten().map(|span| span.start.0).min();
                let hi = spans.iter().flatten().map(|span| span.end.0).max();
                if let (Some(lo), Some(hi)) = (lo, hi) {
                    for (row, slot) in stale.iter_mut().enumerate() {
                        if (lo..=hi).contains(&(top + row)) {
                            *slot = true;
                        }
                    }
                }
            }
        }
        if self.rows.len() != rows {
            self.rows = vec![Row::from(Vec::new()); rows];
        }
        if !key_matches {
            self.key = Some((palette.clone(), options.view_offset, options.cursor_shape));
        }
        self.rebuilt = stale.iter().filter(|slot| **slot).count();
        for row in (0..rows).filter(|&row| stale[row]) {
            self.rows[row] = Row::from(row_runs(screen, palette, options, top, row));
        }
        self.cursor = cursor;
        self.selection = options.selection;
        &self.rows
    }
}

/// Marks the visible row that shows live-grid row `live`, when that row is on screen.
fn mark_live(screen: &Screen, top: usize, live: usize, stale: &mut [bool]) {
    if let Some(slot) = (screen.scrollback_len() + live)
        .checked_sub(top)
        .and_then(|row| stale.get_mut(row))
    {
        *slot = true;
    }
}

/// One text cell as a run, merged into the previous run when colours and weight match.
struct Text {
    col: usize,
    cells: usize,
    ch: char,
    fg: u32,
    bg: Option<u32>,
    bold: bool,
}

fn push_text(runs: &mut Vec<Run>, cell: Text) {
    let mergeable = runs.last().is_some_and(|last| {
        last.shape.is_none()
            && last.fg == cell.fg
            && last.bg == cell.bg
            && last.bold == cell.bold
            && last.col + last.cells == cell.col
    });
    if mergeable {
        let last = runs.last_mut().expect("checked above");
        last.text.push(cell.ch);
        last.cells += cell.cells;
    } else {
        runs.push(Run {
            col: cell.col,
            cells: cell.cells,
            text: cell.ch.to_string(),
            fg: cell.fg,
            bg: cell.bg,
            bold: cell.bold,
            shape: None,
        });
    }
}

/// Grows the trailing band runs to cover one more cell when `rects` are full-width bands of the same colours. Returns
/// false when they do not line up, so the caller starts new runs.
fn extend_bands(runs: &mut [Run], col: usize, rects: &[(Arm, u32)]) -> bool {
    let n = rects.len();
    if n == 0 || runs.len() < n {
        return false;
    }
    let start = runs.len() - n;
    let tail = &mut runs[start..];
    let fits = tail.iter().zip(rects).all(|(run, (arm, colour))| {
        let full = |a: &Arm, cells: f32| a.x0 == 0.0 && a.x1 == cells;
        run.shape.is_some_and(|shape| {
            full(&shape, run.cells as f32) && shape.y0 == arm.y0 && shape.y1 == arm.y1
        }) && run.fg == *colour
            && run.col + run.cells == col
            && full(arm, 1.0)
    });
    if fits {
        for run in tail {
            run.cells += 1;
            if let Some(shape) = run.shape.as_mut() {
                shape.x1 = run.cells as f32;
            }
        }
    }
    fits
}

/// Background colour as drawn: the theme's default background is left undrawn so the window surface (and its
/// translucency) shows through.
fn bg_visible(bg: u32, palette: &Palette) -> Option<u32> {
    (bg != palette.background).then_some(bg)
}

/// Converts plain text into one run per line (settings, tab bar). Wide glyphs are not expected here.
#[must_use]
pub fn text_run(col: usize, text: &str, fg: u32, bg: Option<u32>, bold: bool) -> Run {
    Run {
        col,
        cells: text.chars().count(),
        text: text.to_owned(),
        fg,
        bg,
        bold,
        shape: None,
    }
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
        GridOptions {
            view_offset: 0,
            selection: None,
            cursor,
            cursor_shape: CursorShape::Block,
        }
    }

    #[test]
    fn indexed_palette_covers_ansi_cube_and_grey() {
        let p = palette();
        assert_eq!(indexed_rgb(1, &p), p.ansi[1]);
        assert_eq!(indexed_rgb(16, &p), 0x0000_0000);
        assert_eq!(indexed_rgb(231, &p), 0x00ff_ffff);
        assert_eq!(indexed_rgb(232, &p), 0x0008_0808);
        assert_eq!(indexed_rgb(196, &p), 0x00ff_0000);
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
        let cursor_run = cursor_rows[0]
            .iter()
            .find(|r| r.col == 1)
            .expect("cursor run after the typed glyph");
        assert_eq!(cursor_run.bg, Some(p.cursor));
    }

    #[test]
    fn selection_overrides_colours_and_scrollback_view_offsets_rows() {
        let p = palette();
        let mut screen = Screen::new(3, 2, 10);
        feed(&mut screen, b"aaa\r\nbbb\r\nccc");
        let span = Span::normalised((1, 0), (1, 1));
        let view = grid_runs(
            &screen,
            &p,
            &GridOptions {
                view_offset: 1,
                selection: Some(span),
                cursor: true,
                cursor_shape: CursorShape::Block,
            },
        );
        assert_eq!(view[0][0].text, "aaa");
        assert_eq!(view[1][0].text, "bb");
        assert_eq!(view[1][0].bg, Some(p.selection));
        assert_eq!(view[1][0].col, 0);
        assert_eq!(view[1][1].text, "b", "unselected tail splits off");
        assert_eq!(view[1][1].col, 2);
        assert_eq!(
            view[1].len(),
            2,
            "selected and unselected cells split into runs"
        );
    }

    #[test]
    fn wide_glyph_counts_two_cells_and_skips_spacer() {
        let p = palette();
        let mut screen = Screen::new(4, 1, 0);
        feed(&mut screen, "界x".as_bytes());
        let rows = grid_runs(&screen, &p, &opts(false));
        assert_eq!(
            rows[0][0].text, "界x ",
            "blank tail merges with the default-styled x"
        );
        assert_eq!(
            rows[0][0].cells, 4,
            "wide glyph takes two cells, spacer skipped"
        );
    }

    #[test]
    fn underline_and_bar_cursors_draw_a_glyph_in_the_cursor_colour() {
        let p = palette();
        let mut screen = Screen::new(4, 1, 0);
        feed(&mut screen, b"ab");
        for (shape, glyph) in [
            (CursorShape::Underline, '\u{2581}'),
            (CursorShape::Bar, '\u{258f}'),
        ] {
            let rows = grid_runs(
                &screen,
                &p,
                &GridOptions {
                    view_offset: 0,
                    selection: None,
                    cursor: true,
                    cursor_shape: shape,
                },
            );
            let cursor_run = rows[0]
                .iter()
                .find(|r| r.col == 2)
                .expect("cursor run at column 2");
            assert!(
                cursor_run.shape.is_some(),
                "{glyph} is drawn as a rectangle"
            );
            assert_eq!(cursor_run.fg, p.cursor);
            assert_eq!(cursor_run.bg, Some(p.cursor));
            assert_eq!(rows[0][0].text, "ab", "the typed text is untouched");
        }
    }

    #[test]
    fn dimmed_blends_toward_background() {
        assert_eq!(dimmed(0x00ff_ffff, 0x0000_0000), 0x007f_7f7f);
    }

    #[test]
    fn one_typed_char_rebuilds_exactly_one_row() {
        let p = palette();
        let mut screen = Screen::new(10, 4, 100);
        let mut cache = RunCache::default();
        let options = opts(true);
        feed(&mut screen, b"abc");
        cache.update(&mut screen, &p, &options);
        assert_eq!(cache.rebuilt, 4, "the first frame builds every row");
        feed(&mut screen, b"d");
        cache.update(&mut screen, &p, &options);
        assert_eq!(
            cache.rebuilt, 1,
            "the typed row only; the cursor moved within it"
        );
        cache.update(&mut screen, &p, &options);
        assert_eq!(cache.rebuilt, 0, "an unchanged screen rebuilds nothing");
    }

    #[test]
    fn unchanged_rows_are_shared_allocations() {
        let p = palette();
        let mut screen = Screen::new(10, 4, 100);
        let mut cache = RunCache::default();
        let options = opts(true);
        feed(&mut screen, b"abc");
        let before: Vec<Row> = cache.update(&mut screen, &p, &options).to_vec();
        feed(&mut screen, b"d");
        let after = cache.update(&mut screen, &p, &options).to_vec();
        assert_eq!(cache.rebuilt, 1);
        let shared = (0..before.len())
            .filter(|&row| Arc::ptr_eq(&before[row], &after[row]))
            .count();
        assert_eq!(
            shared,
            before.len() - 1,
            "only the typed row is a new allocation"
        );
        assert!(
            !Arc::ptr_eq(&before[0], &after[0]),
            "row 0 holds the typed char and was rebuilt"
        );
    }

    #[test]
    fn scroll_and_resize_rebuild_every_row() {
        let p = palette();
        let mut screen = Screen::new(5, 3, 100);
        let mut cache = RunCache::default();
        let options = opts(true);
        feed(&mut screen, b"1\r\n2\r\n3");
        cache.update(&mut screen, &p, &options);
        feed(&mut screen, b"\r\n");
        cache.update(&mut screen, &p, &options);
        assert_eq!(cache.rebuilt, 3, "a scroll moves every row");
        screen.resize(6, 3);
        cache.update(&mut screen, &p, &options);
        assert_eq!(cache.rebuilt, 3, "a resize rebuilds every row");
    }

    #[test]
    fn bench_row_rebuilds_per_typed_char_and_scroll() {
        let p = palette();
        let mut screen = Screen::new(40, 8, 100);
        let mut cache = RunCache::default();
        let options = opts(true);
        feed(&mut screen, b"x");
        cache.update(&mut screen, &p, &options);
        let mut typed_rebuilds = 0;
        for _ in 0..20 {
            feed(&mut screen, b"a");
            cache.update(&mut screen, &p, &options);
            typed_rebuilds += cache.rebuilt;
        }
        for _ in 0..screen.rows() {
            feed(&mut screen, b"\r\n");
            cache.update(&mut screen, &p, &options);
        }
        feed(&mut screen, b"\r\n");
        cache.update(&mut screen, &p, &options);
        let scroll_rebuilds = cache.rebuilt;
        eprintln!(
            "BENCH terminal_typed_chars=20 terminal_typed_row_rebuilds={typed_rebuilds} terminal_scroll_row_rebuilds={scroll_rebuilds}"
        );
        assert_eq!(typed_rebuilds, 20, "one row per typed char");
        assert_eq!(
            scroll_rebuilds,
            screen.rows(),
            "a scroll rebuilds every row"
        );
    }

    #[test]
    fn cached_runs_match_a_full_rebuild_over_random_updates() {
        let themes = builtin_themes();
        let fragments: &[&[u8]] = &[
            b"abc",
            "界x".as_bytes(),
            "e\u{301}".as_bytes(),
            b"\x1b[31mred\x1b[0m",
            b"\x1b[7mi\x1b[0m",
            b"\x1b[41mbg",
            b"\r\n",
            b"\n",
            b"\r",
            b"\x08",
            b"\t",
            b"\x1b[2K",
            b"\x1b[K",
            b"\x1b[1J",
            b"\x1b[2J",
            b"\x1b[3J",
            b"\x1b[3;2H",
            b"\x1b[L",
            b"\x1b[M",
            b"\x1b[P",
            b"\x1b[@",
            b"\x1b[S",
            b"\x1b[T",
            b"\x1b[2;4r",
            b"\x1b[r",
            b"\x1b[?1049h",
            b"\x1b[?1049l",
            b"\x1bM",
            b"\x1b[?25l\x1b[?25h",
            b"\x1b7x\x1b8",
            b"\x1b(0lqk\x1b(B",
        ];
        let mut rng: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |bound: usize| {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            (rng % bound as u64) as usize
        };
        let mut screen = Screen::new(8, 4, 16);
        let mut parser = Parser::new();
        let mut cache = RunCache::default();
        let shapes = [CursorShape::Block, CursorShape::Underline, CursorShape::Bar];
        let (mut offset, mut selection, mut cursor, mut shape, mut theme) =
            (0, None, true, CursorShape::Block, 0);
        for _ in 0..3000 {
            match next(12) {
                0 => screen.resize(3 + next(7), 2 + next(5)),
                1 => offset = next(screen.scrollback_len() + 1),
                2 => {
                    selection = (next(2) == 0).then(|| {
                        let total = screen.total_lines();
                        Span::normalised((next(total), next(8)), (next(total), next(8)))
                    });
                }
                3 => cursor = !cursor,
                4 => shape = shapes[next(3)],
                5 => theme = next(themes.len()),
                _ => {
                    for _ in 0..=next(3) {
                        parser.advance(fragments[next(fragments.len())], &mut screen);
                    }
                }
            }
            let options = GridOptions {
                view_offset: offset,
                selection,
                cursor,
                cursor_shape: shape,
            };
            let palette = &themes[theme].palette;
            let cached: Vec<Vec<Run>> = cache
                .update(&mut screen, palette, &options)
                .iter()
                .map(|row| row.to_vec())
                .collect();
            assert_eq!(
                cached,
                grid_runs(&screen, palette, &options),
                "cache diverged from a full rebuild"
            );
            assert!(cache.rebuilt <= screen.rows());
        }
    }
}

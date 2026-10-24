//! Projects the frame (rows of runs) into the compiled UI document. The document has a fixed pool of run nodes per
//! row (see `pool.rs`); only rows whose runs changed are written, and unused slots are hidden.
use std::borrow::Cow;
use std::sync::Arc;

use flamewm_ui_x11::{UiColor, UiDocumentAccess};

use crate::app::App;
use crate::pool::{POOL_ROWS, POOL_RUNS, run_id};
use crate::view::{Row, Run};

#[derive(Default)]
pub struct UiCache {
    /// `None` until the row is first written, so the placeholder text of unused nodes is hidden on the first frame.
    rows: Vec<Option<Row>>,
    font: Option<(u16, String)>,
    surface: Option<(u32, u8)>,
}

const fn rgb(value: u32) -> UiColor {
    UiColor { r: (value >> 16) as u8, g: (value >> 8) as u8, b: value as u8, a: 255 }
}

/// Fits a row into the pool: when a row has more runs than slots, the tail merges into the last slot. Those cells
/// take the first merged run's colours, the one visible limit of the pooled layout. A row that already fits is
/// borrowed, not copied.
fn fit(runs: &[Run], slots: usize) -> Cow<'_, [Run]> {
    if runs.len() <= slots {
        return Cow::Borrowed(runs);
    }
    let mut out = runs[..slots - 1].to_vec();
    let tail = &runs[slots - 1..];
    out.push(Run {
        col: tail[0].col,
        cells: tail.iter().map(|run| run.cells).sum(),
        text: tail.iter().map(|run| run.text.as_str()).collect(),
        fg: tail[0].fg,
        bg: tail[0].bg,
        bold: tail[0].bold,
        shape: None,
    });
    Cow::Owned(out)
}

impl App {
    /// Writes the current frame to `doc`, touching only the nodes whose content changed.
    pub fn project(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        self.project_surface(doc)?;
        self.project_font(doc)?;
        let frame = self.frame();
        self.ui.project_rows(doc, (self.cell_w, self.cell_h), &frame)
    }

    fn project_surface(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let surface = (self.palette.background, self.surface_alpha);
        if self.ui.surface == Some(surface) {
            return Ok(());
        }
        let color = UiColor { a: self.surface_alpha, ..rgb(self.palette.background) };
        // The window's root node carries the surface fill (the explorer does the same with its root). The grid node is
        // transparent, so the translucent fill shows through it.
        doc.background("terminal", color)?;
        // No node backdrop: a root blur made every paint read back and re-blur the whole window. The window backdrop
        // already supplies the blurred desktop, and the translucent fill composites over it.
        self.ui.surface = Some(surface);
        Ok(())
    }

    fn project_font(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let font = (self.config.font_size, self.config.font_family.clone());
        if self.ui.font.as_ref() == Some(&font) {
            return Ok(());
        }
        doc.font_family(font.1.clone())?;
        for row in 0..POOL_ROWS {
            for slot in 0..POOL_RUNS {
                doc.font_size(&run_id(row, slot), f32::from(self.config.font_size))?;
            }
        }
        self.ui.font = Some(font);
        self.ui.rows.clear();
        Ok(())
    }
}

impl UiCache {
    /// Writes every pooled row of `frame`. Rows whose runs did not change write nothing.
    pub fn project_rows(&mut self, doc: &mut impl UiDocumentAccess, cell: (f32, f32), frame: &[Row]) -> Result<(), String> {
        if self.rows.len() != POOL_ROWS {
            self.rows = vec![None; POOL_ROWS];
        }
        let empty = Row::from(Vec::new());
        for row in 0..POOL_ROWS {
            self.project_row(doc, cell, row, frame.get(row).unwrap_or(&empty))?;
        }
        Ok(())
    }

    /// Writes one row. A row with the same allocation as last frame, or the same runs in a new allocation, is
    /// skipped without copying its runs; only the handle is kept.
    fn project_row(&mut self, doc: &mut impl UiDocumentAccess, cell: (f32, f32), row: usize, runs: &Row) -> Result<(), String> {
        let previous = self.rows[row].clone();
        if previous.as_ref().is_some_and(|prev| Arc::ptr_eq(prev, runs) || prev[..] == runs[..]) {
            self.rows[row] = Some(runs.clone());
            return Ok(());
        }
        let fitted = fit(runs, POOL_RUNS);
        // `None` (never written) must write every slot, including the hidden ones.
        let previous = previous.as_deref().map(|prev| fit(prev, POOL_RUNS));
        for slot in 0..POOL_RUNS {
            if previous.as_ref().is_some_and(|prev| prev.get(slot) == fitted.get(slot)) {
                continue;
            }
            match fitted.get(slot) {
                Some(run) => Self::write_run(doc, cell, row, slot, run)?,
                None => doc.visible(&run_id(row, slot), false)?,
            }
        }
        self.rows[row] = Some(runs.clone());
        Ok(())
    }

    fn write_run(doc: &mut impl UiDocumentAccess, cell: (f32, f32), row: usize, slot: usize, run: &Run) -> Result<(), String> {
        let id = run_id(row, slot);
        // nowrap collapses ASCII spaces, so cells are written as no-break spaces to keep their width.
        doc.text(&id, run.text.replace(' ', "\u{a0}"))?;
        let (cell_w, cell_h) = cell;
        let (x, y, w, h) = match run.shape {
            Some(arm) => ((run.col as f32 + arm.x0) * cell_w, (row as f32 + arm.y0) * cell_h, (arm.x1 - arm.x0) * cell_w, (arm.y1 - arm.y0) * cell_h),
            None => (run.col as f32 * cell_w, row as f32 * cell_h, run.cells as f32 * cell_w, cell_h),
        };
        doc.position(&id, x, y)?;
        doc.size(&id, w, h)?;
        doc.foreground(&id, rgb(run.fg))?;
        match run.bg {
            Some(bg) => doc.background(&id, rgb(bg))?,
            None => doc.background_clear(&id)?,
        }
        doc.font_weight(&id, if run.bold { 700 } else { 400 })?;
        doc.visible(&id, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(col: usize, text: &str) -> Run {
        Run { col, cells: text.chars().count(), text: text.into(), fg: 0, bg: None, bold: false, shape: None }
    }

    #[test]
    fn rows_within_the_pool_are_unchanged() {
        let runs = vec![run(0, "ab"), run(2, "cd")];
        assert_eq!(&*fit(&runs, POOL_RUNS), runs.as_slice());
    }

    #[test]
    fn overflowing_rows_merge_their_tail_without_losing_cells() {
        let runs: Vec<Run> = (0..5).map(|i| run(i, "x")).collect();
        let fitted = fit(&runs, 3);
        assert_eq!(fitted.len(), 3);
        assert_eq!(fitted[2].col, 2);
        assert_eq!(fitted[2].cells, 3);
        assert_eq!(fitted[2].text, "xxx");
    }
}

/// htop-like steady-state benchmark: per tick, counters for dirty rows, rebuilt rows, run texts built, projected
/// nodes (text writes), document setter calls and frames. Run with `--nocapture` to see the numbers.
#[cfg(test)]
mod bench {
    use flamewm_ui_core::style::UiLayer;
    use flamewm_ui_x11::{Overflow, RuntimeImage, UiColor, UiDocumentAccess};

    use super::*;
    use crate::parser::Parser;
    use crate::screen::{CursorShape, Screen};
    use crate::themes::builtin_themes;
    use crate::view::{GridOptions, RunCache};

    /// Counts every document setter call; `texts` counts node text writes (projected run nodes).
    #[derive(Default)]
    struct Recorder {
        calls: usize,
        texts: usize,
    }

    impl UiDocumentAccess for Recorder {
        fn text(&mut self, _: &str, _: String) -> Result<(), String> { self.calls += 1; self.texts += 1; Ok(()) }
        fn image_rgb8(&mut self, _: &str, _: RuntimeImage) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn image_rgba8(&mut self, _: &str, _: RuntimeImage) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn visible(&mut self, _: &str, _: bool) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn toggle(&mut self, _: &str, _: &str, _: bool) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn position(&mut self, _: &str, _: f32, _: f32) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn size(&mut self, _: &str, _: f32, _: f32) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn background(&mut self, _: &str, _: UiColor) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn border(&mut self, _: &str, _: UiColor) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn foreground(&mut self, _: &str, _: UiColor) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn background_clear(&mut self, _: &str) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn border_clear(&mut self, _: &str) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn foreground_clear(&mut self, _: &str) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn font_size(&mut self, _: &str, _: f32) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn font_weight(&mut self, _: &str, _: u16) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn overflow(&mut self, _: &str, _: Overflow, _: Overflow) -> Result<(), String> { self.calls += 1; Ok(()) }
        fn layer(&mut self, _: &str, _: UiLayer) -> Result<(), String> { self.calls += 1; Ok(()) }
    }

    /// One htop-like tick: cursor home, then 40 coloured rows. Most cells repeat every tick; two CPU cells change.
    fn htop_tick(tick: usize) -> Vec<u8> {
        let mut out = b"\x1b[H".to_vec();
        for row in 0..40 {
            let cpu = if row == 3 || row == 17 { (tick * 7 + row) % 100 } else { (row * 2) % 100 };
            // Each line fits the 80-column screen so no text wraps into the next row.
            let line = format!(
                "\x1b[{};1H\x1b[1;32m{:>5} \x1b[0m\x1b[36mroot   \x1b[0m{:>3}% \x1b[33m||||||||||\x1b[0m htop-worker-{:02}  lorem ipsum dolor",
                row + 2,
                1000 + row,
                cpu,
                row,
            );
            out.extend(line.bytes());
        }
        out
    }

    /// A typed character overwrites one cell of one row: that row's single run is the only
    /// projected text node, and no other row is written.
    #[test]
    fn one_char_edit_projects_one_run_node() {
        let palette = builtin_themes()[0].palette.clone();
        let options = GridOptions { view_offset: 0, selection: None, cursor: false, cursor_shape: CursorShape::Block };
        let mut screen = Screen::new(80, 42, 0);
        let mut parser = Parser::new();
        let mut cache = RunCache::default();
        let mut ui = UiCache::default();
        parser.advance(b"hello world\r\nsecond line", &mut screen);
        let rows = cache.update(&mut screen, &palette, &options).to_vec();
        ui.project_rows(&mut Recorder::default(), (8.0, 16.0), &rows).expect("project");

        parser.advance(b"\x1b[1;3HX", &mut screen);
        let rows = cache.update(&mut screen, &palette, &options).to_vec();
        let mut doc = Recorder::default();
        ui.project_rows(&mut doc, (8.0, 16.0), &rows).expect("project");
        assert_eq!(doc.texts, 1, "one edited run projects one text node");
    }

    #[test]
    fn htop_steady_state_counters() {
        const TICKS: usize = 50;
        let palette = builtin_themes()[0].palette.clone();
        let options = GridOptions { view_offset: 0, selection: None, cursor: false, cursor_shape: CursorShape::Block };
        let mut screen = Screen::new(80, 42, 0);
        let mut parser = Parser::new();
        let mut cache = RunCache::default();
        let mut ui = UiCache::default();

        parser.advance(&htop_tick(0), &mut screen);
        let mut prev = cache.update(&mut screen, &palette, &options).to_vec();
        ui.project_rows(&mut Recorder::default(), (8.0, 16.0), &prev).expect("project");

        let (mut dirty, mut rebuilt, mut run_texts, mut texts, mut calls, mut frames) = (0, 0, 0, 0, 0, 0);
        for tick in 1..=TICKS {
            parser.advance(&htop_tick(tick), &mut screen);
            dirty += screen.pending_dirty_rows();
            let rows = cache.update(&mut screen, &palette, &options).to_vec();
            rebuilt += cache.rebuilt;
            // Each run of a rebuilt row owns one text string.
            run_texts += rows.iter().zip(&prev).filter(|(row, old)| !Arc::ptr_eq(row, old)).map(|(row, _)| row.len()).sum::<usize>();
            let mut doc = Recorder::default();
            ui.project_rows(&mut doc, (8.0, 16.0), &rows).expect("project");
            texts += doc.texts;
            calls += doc.calls;
            frames += 1;
            prev = rows;
        }
        let per = |total: usize| total as f64 / TICKS as f64;
        eprintln!(
            "htop bench ({TICKS} ticks, per tick): dirty_rows={:.2} rebuilt_rows={:.2} run_text_strings={:.2} projected_text_nodes={:.2} setter_calls={:.2} frames={:.2}",
            per(dirty), per(rebuilt), per(run_texts), per(texts), per(calls), per(frames)
        );
    }
}

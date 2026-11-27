//! Projects the frame (rows of runs) into the compiled UI document. The document has a fixed pool of run nodes per
//! row (see `pool.rs`); only rows whose runs changed are written, and unused slots are hidden.
use std::borrow::Cow;
use std::sync::Arc;

use flamewm_ui_x11::{UiColor, UiDocumentAccess};

use crate::app::App;
use crate::pool::{Pool, run_id_into};
use crate::view::{Row, Run};

/// Geometry, colours and weight last written to one pooled node. A run whose text changed but whose box and style
/// did not (a counter ticking, a log line scrolling) then writes only its text instead of every setter.
#[derive(Clone, Copy, Default)]
struct SlotState {
    /// False until the node has been written, so the first write sets every property.
    visible: bool,
    pos: (f32, f32),
    size: (f32, f32),
    fg: u32,
    bg: Option<u32>,
    bold: bool,
}

#[derive(Default)]
pub struct UiCache {
    /// `None` until the row is first written, so the placeholder text of unused nodes is hidden on the first frame.
    rows: Vec<Option<Row>>,
    font: Option<(u16, String)>,
    surface: Option<(u32, u8)>,
    surface_stroke: Option<UiColor>,
    /// Per pooled slot: the last attributes written (indexed `row * pool.runs + slot`).
    slots: Vec<SlotState>,
    /// The row handed to rows past the frame's end, kept so a frame allocates no empty row.
    empty: Option<Row>,
    /// Reused no-break-space text buffer for one run, so a projected row allocates no string per run.
    text: String,
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

/// Whether slot `slot` of a previous row equals slot `slot` of `fitted` (a row fitted by [`fit`]) without building
/// the previous row's merged tail. Only the last slot of an overflowing row is synthetic, and its cells take the
/// first merged run's style, so a later tail run changing colour still draws the same merged slot.
fn slot_matches(prev: &[Run], fitted: &[Run], slots: usize, slot: usize) -> bool {
    match fitted.get(slot) {
        Some(expected) => prev_slot_eq(prev, slots, slot, expected),
        // The fitted row has no such slot, so it matches only a previous row that already lacked it.
        None => prev.len() <= slots && prev.get(slot).is_none(),
    }
}

/// Compares one slot of `prev` under the same tail merge as [`fit`] against `expected`.
fn prev_slot_eq(prev: &[Run], slots: usize, slot: usize, expected: &Run) -> bool {
    if prev.len() <= slots || slot + 1 < slots {
        return prev.get(slot).is_some_and(|run| run == expected);
    }
    let head = &prev[slots - 1];
    head.col == expected.col
        && expected.cells == prev[slots - 1..].iter().map(|run| run.cells).sum::<usize>()
        && head.fg == expected.fg
        && head.bg == expected.bg
        && head.bold == expected.bold
        && expected.shape.is_none()
        && {
            // Compare the concatenated tail text against the merged run without allocating it.
            let mut rest = expected.text.as_str();
            prev[slots - 1..]
                .iter()
                .all(|run| match rest.strip_prefix(run.text.as_str()) {
                    Some(next) => {
                        rest = next;
                        true
                    }
                    None => false,
                })
                && rest.is_empty()
        }
}

impl App {
    /// Writes the current frame to `doc`, touching only the nodes whose content changed.
    pub fn project(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        self.advance_scroll();
        self.project_surface(doc)?;
        self.project_font(doc)?;
        let frame = self.frame();
        let pool = self.pool();
        let cell = (self.cell_w, self.cell_h);
        let written = self.ui.project_rows(doc, pool, cell, &frame);
        // Give the row buffer back: the next frame clears and refills it instead of allocating a new one.
        self.frame_rows = frame;
        written
    }

    fn project_surface(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let surface = (self.palette.background, self.surface_alpha);
        if self.ui.surface == Some(surface) && self.ui.surface_stroke == Some(self.surface_stroke) {
            return Ok(());
        }
        let color = UiColor {
            a: self.surface_alpha,
            ..UiColor::from_rgb(self.palette.background, 255)
        };
        // The window's root node carries the surface fill (the explorer does the same with its root). The grid node is
        // transparent, so the translucent fill shows through it.
        doc.background("terminal", color)?;
        self.ui.surface_stroke = Some(self.surface_stroke);
        // No node backdrop: a root blur made every paint read back and re-blur the whole window. The window backdrop
        // already supplies the blurred desktop, and the translucent fill composites over it.
        self.ui.surface = Some(surface);
        Ok(())
    }

    fn project_font(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        // Compare in place: cloning the family before the check allocated a `String` on every frame.
        let unchanged = self.ui.font.as_ref().is_some_and(|(size, family)| {
            *size == self.config.font_size && family == &self.config.font_family
        });
        if unchanged {
            return Ok(());
        }
        let font = (self.config.font_size, self.config.font_family.clone());
        doc.font_family(font.1.clone())?;
        let pool = self.pool();
        for row in 0..pool.rows {
            for slot in 0..pool.runs {
                doc.font_size(&run_id(row, slot), f32::from(self.config.font_size))?;
            }
        }
        self.ui.font = Some(font);
        self.ui.rows.clear();
        // The new font changes every box and the node's font size, so forget the written attributes too.
        self.ui.slots.fill(SlotState::default());
        Ok(())
    }
}

impl UiCache {
    /// Writes every pooled row of `frame`. Rows whose runs did not change write nothing.
    pub fn project_rows(
        &mut self,
        doc: &mut impl UiDocumentAccess,
        pool: Pool,
        cell: (f32, f32),
        frame: &[Row],
    ) -> Result<(), String> {
        if self.rows.len() != pool.rows {
            self.rows = vec![None; pool.rows];
        }
        // A new pool means new nodes, whose written attributes are unknown.
        let slots = pool.rows * pool.runs;
        if self.slots.len() != slots {
            self.slots = vec![SlotState::default(); slots];
        }
        let empty = self
            .empty
            .get_or_insert_with(|| Row::from(Vec::new()))
            .clone();
        for row in 0..pool.rows {
            self.project_row(doc, pool, cell, row, frame.get(row).unwrap_or(&empty))?;
        }
        Ok(())
    }

    /// Writes one row. A row with the same allocation as last frame, or the same runs in a new allocation, is
    /// skipped without copying its runs; only the handle is kept.
    fn project_row(
        &mut self,
        doc: &mut impl UiDocumentAccess,
        pool: Pool,
        cell: (f32, f32),
        row: usize,
        runs: &Row,
    ) -> Result<(), String> {
        // Take the previous handle instead of cloning it: a row is stored back once per frame, so the frame holds
        // one refcount per row rather than two. A failed write leaves the slot empty and the next frame rewrites it.
        let previous = self.rows[row].take();
        if previous
            .as_ref()
            .is_some_and(|prev| Arc::ptr_eq(prev, runs))
        {
            self.rows[row] = previous;
            return Ok(());
        }
        if previous.as_deref().is_some_and(|prev| prev[..] == runs[..]) {
            self.rows[row] = Some(runs.clone());
            return Ok(());
        }
        let fitted = fit(runs, pool.runs);
        // `None` (never written) must write every slot, including the hidden ones.
        let prev_runs = previous.as_deref();
        // One id buffer per row: a row formats an id per slot, not an allocation per slot.
        let mut id = String::new();
        let base = row * pool.runs;
        for slot in 0..pool.runs {
            // Compare against the previous row under the same merge, without allocating its fitted form.
            if prev_runs.is_some_and(|prev| slot_matches(prev, &fitted, pool.runs, slot)) {
                continue;
            }
            run_id_into(&mut id, row, slot);
            if let Some(run) = fitted.get(slot) {
                Self::write_run(
                    doc,
                    cell,
                    &id,
                    run,
                    row,
                    &mut self.text,
                    &mut self.slots[base + slot],
                )?;
            } else {
                doc.visible(&id, false)?;
                // Forget the written attributes, so showing this node again sets all of them.
                self.slots[base + slot] = SlotState::default();
            }
        }
        self.rows[row] = Some(runs.clone());
        Ok(())
    }

    fn write_run(
        doc: &mut impl UiDocumentAccess,
        cell: (f32, f32),
        id: &str,
        run: &Run,
        row: usize,
        text: &mut String,
        state: &mut SlotState,
    ) -> Result<(), String> {
        // nowrap collapses ASCII spaces, so cells are written as no-break spaces to keep their width.
        text.clear();
        if run.text.as_bytes().contains(&b' ') {
            text.extend(
                run.text
                    .chars()
                    .map(|c| if c == ' ' { '\u{a0}' } else { c }),
            );
        } else {
            // No space to convert: copy the bytes instead of decoding and re-encoding every character.
            text.push_str(&run.text);
        }
        doc.text_ref(id, text)?;
        let (cell_w, cell_h) = cell;
        let (x, y, w, h) = match run.shape {
            Some(arm) => (
                (run.col as f32 + arm.x0) * cell_w,
                (row as f32 + arm.y0) * cell_h,
                (arm.x1 - arm.x0) * cell_w,
                (arm.y1 - arm.y0) * cell_h,
            ),
            None => (
                run.col as f32 * cell_w,
                row as f32 * cell_h,
                run.cells as f32 * cell_w,
                cell_h,
            ),
        };
        let weight = if run.bold { 700 } else { 400 };
        // A node that was never written (or was hidden) needs every property.
        if !state.visible {
            doc.position(id, x, y)?;
            doc.size(id, w, h)?;
            doc.foreground(id, UiColor::from_rgb(run.fg, 255))?;
            match run.bg {
                Some(bg) => doc.background(id, UiColor::from_rgb(bg, 255))?,
                None => doc.background_clear(id)?,
            }
            doc.font_weight(id, weight)?;
            *state = SlotState {
                visible: true,
                pos: (x, y),
                size: (w, h),
                fg: run.fg,
                bg: run.bg,
                bold: run.bold,
            };
            return doc.visible(id, true);
        }
        // A counter ticking or a log line scrolling keeps the box and style, so only the text above is written.
        if state.pos != (x, y) {
            doc.position(id, x, y)?;
            state.pos = (x, y);
        }
        if state.size != (w, h) {
            doc.size(id, w, h)?;
            state.size = (w, h);
        }
        if state.fg != run.fg {
            doc.foreground(id, UiColor::from_rgb(run.fg, 255))?;
            state.fg = run.fg;
        }
        if state.bg != run.bg {
            match run.bg {
                Some(bg) => doc.background(id, UiColor::from_rgb(bg, 255))?,
                None => doc.background_clear(id)?,
            }
            state.bg = run.bg;
        }
        if state.bold != run.bold {
            doc.font_weight(id, weight)?;
            state.bold = run.bold;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(col: usize, text: &str) -> Run {
        Run {
            col,
            cells: text.chars().count(),
            text: text.into(),
            fg: 0,
            bg: None,
            bold: false,
            shape: None,
        }
    }

    #[test]
    fn rows_within_the_pool_are_unchanged() {
        let runs = vec![run(0, "ab"), run(2, "cd")];
        assert_eq!(&*fit(&runs, crate::pool::TIERS[2].runs), runs.as_slice());
    }

    #[test]
    fn dense_tui_row_preserves_per_cell_colors_within_supported_width() {
        // OpenCode's dense coloured block art used to overflow the 64-node pool,
        // silently turning the tail into a single-colored text run.
        let runs: Vec<Run> = (0..120)
            .map(|i| Run {
                col: i,
                cells: 1,
                text: "█".into(),
                fg: i as u32,
                bg: None,
                bold: false,
                shape: None,
            })
            .collect();
        assert!(crate::pool::TIERS[2].runs >= 120);
        let fitted = fit(&runs, crate::pool::TIERS[2].runs);
        assert_eq!(&*fitted, runs.as_slice());
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

    #[test]
    fn overflowing_row_slots_compare_under_the_same_merge() {
        let prev: Vec<Run> = (0..5).map(|i| run(i, "x")).collect();
        let fitted = fit(&prev, 3);
        assert!((0..3).all(|slot| slot_matches(&prev, &fitted, 3, slot)));

        let mut text_changed = prev.clone();
        text_changed[4].text = "y".into();
        let fitted = fit(&text_changed, 3);
        assert!(slot_matches(&prev, &fitted, 3, 0));
        assert!(!slot_matches(&prev, &fitted, 3, 2), "merged text differs");

        let mut head_changed = prev.clone();
        head_changed[2].fg = 9;
        let fitted = fit(&head_changed, 3);
        assert!(!slot_matches(&prev, &fitted, 3, 2), "merged colour differs");

        // Only the first merged run's style reaches the merged slot, so a later run's colour cannot change it.
        let mut later_changed = prev.clone();
        later_changed[4].fg = 9;
        let fitted = fit(&later_changed, 3);
        assert!(slot_matches(&prev, &fitted, 3, 2));
    }

    #[test]
    fn slots_past_a_shorter_row_stay_hidden() {
        let prev: Vec<Run> = (0..2).map(|i| run(i, "x")).collect();
        let fitted = fit(&prev, 3);
        assert!(slot_matches(&prev, &fitted, 3, 0));
        // The previous row already lacked the slot, so re-hiding it would be another no-op write.
        assert!(slot_matches(&prev, &fitted, 3, 2));
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
        fn text(&mut self, _: &str, _: String) -> Result<(), String> {
            self.calls += 1;
            self.texts += 1;
            Ok(())
        }
        fn image_rgb8(&mut self, _: &str, _: RuntimeImage) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn image_rgba8(&mut self, _: &str, _: RuntimeImage) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn visible(&mut self, _: &str, _: bool) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn toggle(&mut self, _: &str, _: &str, _: bool) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn position(&mut self, _: &str, _: f32, _: f32) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn size(&mut self, _: &str, _: f32, _: f32) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn background(&mut self, _: &str, _: UiColor) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn border(&mut self, _: &str, _: UiColor) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn foreground(&mut self, _: &str, _: UiColor) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn background_clear(&mut self, _: &str) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn border_clear(&mut self, _: &str) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn foreground_clear(&mut self, _: &str) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn font_size(&mut self, _: &str, _: f32) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn font_weight(&mut self, _: &str, _: u16) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn overflow(&mut self, _: &str, _: Overflow, _: Overflow) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
        fn layer(&mut self, _: &str, _: UiLayer) -> Result<(), String> {
            self.calls += 1;
            Ok(())
        }
    }

    /// One htop-like tick: cursor home, then 40 coloured rows. Most cells repeat every tick; two CPU cells change.
    fn htop_tick(tick: usize) -> Vec<u8> {
        let mut out = b"\x1b[H".to_vec();
        for row in 0..40 {
            let cpu = if row == 3 || row == 17 {
                (tick * 7 + row) % 100
            } else {
                (row * 2) % 100
            };
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
        let options = GridOptions {
            view_offset: 0,
            selection: None,
            cursor: false,
            cursor_shape: CursorShape::Block,
        };
        let mut screen = Screen::new(80, 42, 0);
        let mut parser = Parser::new();
        let mut cache = RunCache::default();
        let mut ui = UiCache::default();
        parser.advance(b"hello world\r\nsecond line", &mut screen);
        let rows = cache.update(&mut screen, &palette, &options).to_vec();
        ui.project_rows(
            &mut Recorder::default(),
            crate::pool::TIERS[2],
            (8.0, 16.0),
            &rows,
        )
        .expect("project");

        parser.advance(b"\x1b[1;3HX", &mut screen);
        let rows = cache.update(&mut screen, &palette, &options).to_vec();
        let mut doc = Recorder::default();
        ui.project_rows(&mut doc, crate::pool::TIERS[2], (8.0, 16.0), &rows)
            .expect("project");
        assert_eq!(doc.texts, 1, "one edited run projects one text node");
    }

    /// Stage timing breakdown of the terminal pipeline, best of 5 per stage. Not a pass/fail check:
    /// `cargo test -p flamewm-terminal --profile fast pipeline_breakdown -- --ignored --nocapture`.
    #[test]
    #[ignore = "benchmark: run with --ignored --nocapture"]
    fn pipeline_breakdown() {
        use crate::parser::Perform;
        use std::time::Instant;

        struct Null;
        impl Perform for Null {
            fn print(&mut self, c: char) {
                std::hint::black_box(c);
            }
            fn execute(&mut self, byte: u8) {
                std::hint::black_box(byte);
            }
            fn csi(&mut self, p: Option<u8>, params: &[Vec<u16>], i: &[u8], a: u8) {
                std::hint::black_box((p, params, i, a));
            }
            fn esc(&mut self, i: &[u8], f: u8) {
                std::hint::black_box((i, f));
            }
            fn osc(&mut self, s: &[Vec<u8>]) {
                std::hint::black_box(s);
            }
        }
        fn best(mut run: impl FnMut() -> f64) -> f64 {
            (0..5).map(|_| run()).fold(f64::MAX, f64::min)
        }
        fn secs(f: impl FnOnce()) -> f64 {
            let started = Instant::now();
            f();
            started.elapsed().as_secs_f64()
        }

        let plain: Vec<u8> = (0..60_000)
            .flat_map(|n| {
                format!("row {n:6} the quick brown fox jumps over the lazy dog\r\n").into_bytes()
            })
            .collect();
        let sgr: Vec<u8> = (0..60_000)
            .flat_map(|n| {
                format!(
                    "\x1b[1;3{}m{n:6}\x1b[0m \x1b[38;5;{}mjumps over\x1b[0m the lazy dog\r\n",
                    n % 7,
                    n % 256
                )
                .into_bytes()
            })
            .collect();
        let linefeeds = vec![b'\n'; 60_000];
        let palette = builtin_themes()[0].palette.clone();
        let options = |cursor| GridOptions {
            view_offset: 0,
            selection: None,
            cursor,
            cursor_shape: CursorShape::Block,
        };
        let mut rows_out: Vec<(&str, f64, usize, &str)> = Vec::new();

        // parse: VT state machine only, into a no-op performer.
        let parse_plain = best(|| secs(|| Parser::new().advance(&plain, &mut Null)));
        let parse_sgr = best(|| secs(|| Parser::new().advance(&sgr, &mut Null)));
        // parse + screen apply (cell writes, SGR, wrap); apply = total - parse.
        let total_plain = best(|| {
            secs(|| {
                let mut screen = Screen::new(120, 40, 10_000);
                Parser::new().advance(&plain, &mut screen);
                std::hint::black_box(&screen);
            })
        });
        let total_sgr = best(|| {
            secs(|| {
                let mut screen = Screen::new(120, 40, 10_000);
                Parser::new().advance(&sgr, &mut screen);
                std::hint::black_box(&screen);
            })
        });
        // scroll: a full screen fed bare linefeeds, one scroll_up (row removal, blank row, history pack) each.
        let scroll = best(|| {
            secs(|| {
                let mut screen = Screen::new(120, 40, 10_000);
                Parser::new().advance(b"\x1b[40;1H", &mut screen);
                Parser::new().advance(&linefeeds, &mut screen);
                std::hint::black_box(&screen);
            })
        });

        let mut mixed = Screen::new(120, 40, 10_000);
        Parser::new().advance(&sgr, &mut mixed);

        // shaping: a cold RunCache rebuilds every row (glyph_rects, run merge) of a mixed-text screen.
        let shape_full = best(|| {
            secs(|| {
                let mut cache = RunCache::default();
                std::hint::black_box(cache.update(&mut mixed, &palette, &options(false)));
            })
        });
        // dirty tracking: warm cache, one typed char per update (dirty row + key compare), 2000 updates.
        let mut warm = RunCache::default();
        warm.update(&mut mixed, &palette, &options(false));
        let typing = best(|| {
            secs(|| {
                for _ in 0..2000 {
                    Parser::new().advance(b"x\x08", &mut mixed);
                    std::hint::black_box(warm.update(&mut mixed, &palette, &options(false)));
                }
            }) / 2000.0
        });
        // cursor blink: warm cache, cursor drawn on/off alternately (rebuilds the cursor row only), 2000 updates.
        let blink = best(|| {
            secs(|| {
                for i in 0..2000 {
                    std::hint::black_box(warm.update(&mut mixed, &palette, &options(i % 2 == 0)));
                }
            }) / 2000.0
        });
        // paint: retained-UI projection of a full screen of runs (cold UiCache, every row written).
        let rows = {
            let mut cache = RunCache::default();
            cache.update(&mut mixed, &palette, &options(false)).to_vec()
        };
        let paint_full = best(|| {
            secs(|| {
                let mut ui = UiCache::default();
                ui.project_rows(
                    &mut Recorder::default(),
                    crate::pool::TIERS[1],
                    (8.0, 16.0),
                    &rows,
                )
                .expect("project");
            })
        });

        let per = |total: f64, n: f64| total * 1e9 / n;
        let bytes_plain = plain.len() as f64;
        let bytes_sgr = sgr.len() as f64;
        rows_out.push((
            "parse (plain)",
            per(parse_plain, bytes_plain),
            plain.len(),
            "ns/byte",
        ));
        rows_out.push((
            "parse (sgr)",
            per(parse_sgr, bytes_sgr),
            sgr.len(),
            "ns/byte",
        ));
        rows_out.push((
            "apply = total-parse (plain)",
            per(total_plain - parse_plain, bytes_plain),
            plain.len(),
            "ns/byte",
        ));
        rows_out.push((
            "apply = total-parse (sgr)",
            per(total_sgr - parse_sgr, bytes_sgr),
            sgr.len(),
            "ns/byte",
        ));
        rows_out.push((
            "scroll (linefeed, full screen)",
            per(scroll, 60_000.0),
            60_000,
            "ns/linefeed",
        ));
        rows_out.push((
            "shape: cold RunCache, 40 rows",
            per(shape_full, 40.0),
            40,
            "ns/row",
        ));
        rows_out.push((
            "dirty: typed char + update",
            typing * 1e9,
            2000,
            "ns/update",
        ));
        rows_out.push((
            "cursor blink repaint update",
            blink * 1e9,
            2000,
            "ns/update",
        ));
        rows_out.push((
            "paint: UI projection, 40 rows",
            per(paint_full, 40.0),
            40,
            "ns/row",
        ));
        for (name, value, n, unit) in &rows_out {
            eprintln!("pipeline {name}: {value:.0} {unit} (n={n}, best of 5)");
        }
    }

    #[test]
    fn htop_steady_state_counters() {
        const TICKS: usize = 50;
        let palette = builtin_themes()[0].palette.clone();
        let options = GridOptions {
            view_offset: 0,
            selection: None,
            cursor: false,
            cursor_shape: CursorShape::Block,
        };
        let mut screen = Screen::new(80, 42, 0);
        let mut parser = Parser::new();
        let mut cache = RunCache::default();
        let mut ui = UiCache::default();

        parser.advance(&htop_tick(0), &mut screen);
        let mut prev = cache.update(&mut screen, &palette, &options).to_vec();
        ui.project_rows(
            &mut Recorder::default(),
            crate::pool::TIERS[2],
            (8.0, 16.0),
            &prev,
        )
        .expect("project");

        let (mut dirty, mut rebuilt, mut run_texts, mut texts, mut calls, mut frames) =
            (0, 0, 0, 0, 0, 0);
        for tick in 1..=TICKS {
            parser.advance(&htop_tick(tick), &mut screen);
            dirty += screen.pending_dirty_rows();
            let rows = cache.update(&mut screen, &palette, &options).to_vec();
            rebuilt += cache.rebuilt;
            // Each run of a rebuilt row owns one text string.
            run_texts += rows
                .iter()
                .zip(&prev)
                .filter(|(row, old)| !Arc::ptr_eq(row, old))
                .map(|(row, _)| row.len())
                .sum::<usize>();
            let mut doc = Recorder::default();
            ui.project_rows(&mut doc, crate::pool::TIERS[2], (8.0, 16.0), &rows)
                .expect("project");
            texts += doc.texts;
            calls += doc.calls;
            frames += 1;
            prev = rows;
        }
        let per = |total: usize| total as f64 / TICKS as f64;
        eprintln!(
            "htop bench ({TICKS} ticks, per tick): dirty_rows={:.2} rebuilt_rows={:.2} run_text_strings={:.2} projected_text_nodes={:.2} setter_calls={:.2} frames={:.2}",
            per(dirty),
            per(rebuilt),
            per(run_texts),
            per(texts),
            per(calls),
            per(frames)
        );
    }
}

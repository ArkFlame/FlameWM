//! Grid composition: pane rows composed into window rows, the grid frame, run overlays and the cursor shape.

use super::layout::Rect;
use super::{App, View};
use crate::glyphs::Arm;
use crate::screen::CursorShape;
use crate::view::GridOptions;
use crate::view::Row;
use crate::view::Run;
use std::sync::Arc;

/// A pane's runs moved to its column, with the cells past its right edge dropped.
fn place_runs(line: &[Run], rect: Rect) -> impl Iterator<Item = Run> + '_ {
    line.iter()
        .filter(move |run| run.col < rect.cols)
        .map(move |run| {
            let mut run = if run.col + run.cells > rect.cols {
                clip(run, 0, rect.cols - run.col)
            } else {
                run.clone()
            };
            run.col += rect.x0;
            run
        })
}

/// A grid row composed from the pane rows that cover it, kept with those inputs.
pub(super) struct GridRow {
    /// Each covering pane's row and rectangle, top to bottom in pane order.
    inputs: Vec<(Row, Rect)>,
    row: Row,
}

/// Whether two input lists compose to the same row: the same pane-row allocations at the same rectangles.
fn same_inputs(old: &[(Row, Rect)], new: &[(Row, Rect)]) -> bool {
    old.len() == new.len()
        && old
            .iter()
            .zip(new)
            .all(|((a, ra), (b, rb))| Arc::ptr_eq(a, b) && ra == rb)
}

fn compose_row(inputs: &[(Row, Rect)]) -> Row {
    // The common frame has one pane starting at column 0: when every run already fits, the row is the pane's row, so
    // hand out the shared allocation instead of cloning each run's text into a new one.
    if let [(line, rect)] = inputs
        && rect.x0 == 0
        && line.iter().all(|run| run.col + run.cells <= rect.cols)
    {
        return line.clone();
    }
    inputs
        .iter()
        .flat_map(|(line, rect)| place_runs(line, *rect))
        .collect()
}

/// Replaces cells `[col, col + width)` of a row with `menu`, keeping the runs on either side. Borrows the row, so the
/// caller's runs are copied only into the result, not into a throwaway clone first.
pub(super) fn overlay(row: &[Run], col: usize, width: usize, menu: Run) -> Vec<Run> {
    let end = col + width;
    let (mut out, mut right) = (Vec::new(), Vec::new());
    for run in row {
        let run_end = run.col + run.cells;
        if run.col < col {
            out.push(clip(run, 0, col.min(run_end) - run.col));
        }
        if run_end > end {
            right.push(clip(run, end.max(run.col) - run.col, run.cells));
        }
    }
    out.push(menu);
    out.extend(right);
    out
}

/// The part of `run` between cells `from` and `to` (relative to the run). Text is cut by character, one per cell.
fn clip(run: &Run, from: usize, to: usize) -> Run {
    let text = run.text.chars().skip(from).take(to - from).collect();
    let shape = run.shape.map(|arm| Arm {
        x0: (arm.x0 - from as f32).max(0.0),
        x1: (arm.x1 - from as f32).min((to - from) as f32),
        ..arm
    });
    Run {
        col: run.col + from,
        cells: to - from,
        text,
        fg: run.fg,
        bg: run.bg,
        bold: run.bold,
        shape,
    }
}

impl App {
    /// Default cursor shape from the `cursor_style` setting; a program's DECSCUSR overrides it per pane.
    pub fn cursor_shape(&self) -> CursorShape {
        match self.config.cursor_style.as_str() {
            "underline" => CursorShape::Underline,
            "bar" => CursorShape::Bar,
            _ => CursorShape::Block,
        }
    }

    /// Grid rows of runs for the active view, with the tab bar as row 0. Every pane draws into its own rectangle, so
    /// a row holds the runs of all panes that share it, side by side and never overlapping.
    pub fn frame(&mut self) -> Vec<Row> {
        // The row list is reused across frames: the projector hands it back after writing the document, so a frame
        // allocates no container once the window has been drawn at this size.
        let mut rows = std::mem::take(&mut self.frame_rows);
        // Row 0 is the tab bar. Its runs are rebuilt each frame but almost never change, so keep the previous
        // allocation when they match and skip both the `Arc` allocation and the projector's re-comparison.
        let previous_bar = rows.first().cloned();
        rows.clear();
        let (bar, _) = self.tab_bar();
        rows.push(match previous_bar {
            Some(previous) if previous[..] == bar[..] => previous,
            _ => Row::from(bar),
        });
        match &self.view {
            View::Terminal => {
                // The banner takes the top rows of the grid; the grid's bottom rows give way to it.
                let banner = self.banner_rows();
                rows.extend(self.banner_lines_for(banner).into_iter().map(Row::from));
                self.grid_frame(banner, &mut rows);
                self.draw_find(&mut rows, banner);
                if let Some(suffix) = self.ghost() {
                    self.draw_ghost(&mut rows, suffix);
                }
                if let Some((row, col)) = self.menu {
                    self.draw_menu(&mut rows, row, col);
                }
            }
            View::Settings(settings) => {
                let palette = settings
                    .preview()
                    .cloned()
                    .unwrap_or_else(|| self.palette.clone());
                let body = settings.render(
                    self.cols,
                    self.grid_rows,
                    &self.themes,
                    &palette,
                    &self.config,
                );
                rows.extend(body.into_iter().map(Row::from));
            }
        }
        rows
    }

    /// Appends the pane grid below the banner to `rows`. A row is composed only when one of its pane rows is a new
    /// allocation or moved; otherwise the previous frame's row is handed out again.
    fn grid_frame(&mut self, banner: usize, rows: &mut Vec<Row>) {
        let height = self.grid_rows.saturating_sub(banner);
        let (cursor_shape, grid_rect) = (self.cursor_shape(), self.grid_rect());
        // Per-row input lists live across frames: after the first frame their slow growth is paid once, so an idle,
        // scrolling or flooding repaint allocates nothing for the lists themselves.
        let mut inputs = std::mem::take(&mut self.grid_inputs);
        if inputs.len() > height {
            inputs.truncate(height);
        }
        while inputs.len() < height {
            inputs.push(Vec::new());
        }
        for row_inputs in &mut inputs {
            row_inputs.clear();
        }
        let mut cache = std::mem::take(&mut self.pane_runs);
        if let Some(tab) = self.tabs.get_mut(self.active) {
            let focused = tab.focused;
            for (id, rect) in tab.root.rects(grid_rect) {
                let Some(pane) = tab.root.pane_mut(id) else {
                    continue;
                };
                let options = GridOptions {
                    view_offset: pane.view_offset,
                    selection: pane.selection,
                    // The cursor belongs to the pane with the keyboard.
                    cursor: id == focused,
                    // A shape the program set with DECSCUSR wins over the configured default.
                    cursor_shape: pane.session.screen.cursor_shape.unwrap_or(cursor_shape),
                };
                let entry = cache.entry(id).or_default();
                for (offset, line) in entry
                    .update(&mut pane.session.screen, &self.palette, &options)
                    .iter()
                    .enumerate()
                    .take(rect.rows)
                {
                    if let Some(target) = inputs.get_mut(rect.y0 + offset) {
                        target.push((line.clone(), rect));
                    }
                }
            }
        }
        self.pane_runs = cache;
        // The composed rows are updated in place, so the old allocations serve the new frame. A row whose inputs are
        // the same allocation at the same rectangle keeps its composed row untouched.
        let mut composed = std::mem::take(&mut self.grid_composed);
        if composed.len() > height {
            composed.truncate(height);
        }
        while composed.len() < height {
            composed.push(GridRow {
                inputs: Vec::new(),
                row: Row::from(Vec::new()),
            });
        }
        for (entry, row_inputs) in composed.iter_mut().zip(inputs.iter_mut()) {
            if !same_inputs(&entry.inputs, row_inputs) {
                entry.row = compose_row(row_inputs);
                // Keep the freshly built list in the entry and the old one as next frame's scratch: both keep their
                // capacity, so neither is reallocated.
                std::mem::swap(&mut entry.inputs, row_inputs);
                row_inputs.clear();
            }
        }
        // Append into the caller's row buffer: the composed rows are handles to shared run slices, so this copies no
        // text and allocates no container.
        rows.extend(composed.iter().map(|entry| entry.row.clone()));
        self.grid_composed = composed;
        self.grid_inputs = inputs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x0: usize, cols: usize) -> Rect {
        Rect {
            x0,
            y0: 0,
            cols,
            rows: 1,
        }
    }

    fn run(col: usize, text: &str) -> Run {
        Run {
            col,
            cells: text.chars().count(),
            text: text.to_owned(),
            fg: 0x00ff_ffff,
            bg: None,
            bold: false,
            shape: None,
        }
    }

    fn row(runs: Vec<Run>) -> Row {
        runs.into()
    }

    #[test]
    fn same_inputs_compares_allocation_rectangle_and_pane_count() {
        let line = row(vec![run(0, "ab")]);
        assert!(same_inputs(
            &[(line.clone(), rect(0, 4))],
            &[(line.clone(), rect(0, 4))]
        ));
        // A re-allocated row is a change even when its runs are value-equal: the
        // cache records that the pane rebuilt its line, so the row must recompose.
        let copy = row(vec![run(0, "ab")]);
        assert!(!same_inputs(
            &[(line.clone(), rect(0, 4))],
            &[(copy, rect(0, 4))]
        ));
        assert!(!same_inputs(
            &[(line.clone(), rect(0, 4))],
            &[(line.clone(), rect(1, 4))]
        ));
        assert!(!same_inputs(&[(line.clone(), rect(0, 4))], &[]));
    }

    #[test]
    fn compose_row_hands_out_the_single_pane_allocation_when_it_fits() {
        let line = row(vec![run(0, "ab"), run(2, "cd")]);
        let composed = compose_row(&[(line.clone(), rect(0, 8))]);
        assert!(
            Arc::ptr_eq(&composed, &line),
            "a lone pane that already fits keeps its shared row"
        );
        // A pane offset moves every run into the window's columns.
        let offset = compose_row(&[(line.clone(), rect(2, 8))]);
        assert!(!Arc::ptr_eq(&offset, &line));
        assert_eq!(
            offset.iter().map(|run| run.col).collect::<Vec<_>>(),
            vec![2, 4]
        );
        // A row wider than its pane is clipped to the pane.
        let clipped = compose_row(&[(line, rect(0, 3))]);
        assert_eq!(
            clipped.iter().map(|run| run.cells).sum::<usize>(),
            3,
            "cells past the pane's right edge are dropped"
        );
    }

    #[test]
    fn compose_row_places_panes_side_by_side_and_drops_runs_past_their_edge() {
        let left = row(vec![run(0, "abc")]);
        let right = row(vec![run(0, "xy"), run(2, "z")]);
        let composed = compose_row(&[(left, rect(0, 2)), (right, rect(2, 2))]);
        let cells: Vec<(usize, usize, &str)> = composed
            .iter()
            .map(|run| (run.col, run.cells, run.text.as_str()))
            .collect();
        // Left pane clips "abc" to two cells; the right pane starts at column 2
        // and its run that begins at its own right edge is dropped.
        assert_eq!(cells, vec![(0, 2, "ab"), (2, 2, "xy")]);
    }

    #[test]
    fn overlay_replaces_only_the_middle_and_clips_the_edges() {
        let line = row(vec![run(0, "abcdef")]);
        let menu = run(2, "XY");
        let out = overlay(&line, 2, 2, menu.clone());
        let texts: Vec<&str> = out.iter().map(|run| run.text.as_str()).collect();
        assert_eq!(texts, vec!["ab", "XY", "ef"]);
        assert_eq!(
            out.iter().map(|run| run.col).collect::<Vec<_>>(),
            vec![0, 2, 4]
        );
        assert_eq!(out[1], menu);
    }

    #[test]
    fn clip_cuts_text_and_keeps_shapes_inside_the_new_run() {
        let mut boxed = run(0, "abcd");
        boxed.shape = Some(Arm {
            x0: 0.0,
            x1: 4.0,
            y0: 0.0,
            y1: 1.0,
        });
        let cut = clip(&boxed, 1, 3);
        assert_eq!((cut.col, cut.cells, cut.text.as_str()), (1, 2, "bc"));
        let shape = cut.shape.expect("a clipped block glyph keeps its shape");
        assert_eq!((shape.x0, shape.x1), (0.0, 2.0));
    }
}

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
    inputs
        .iter()
        .flat_map(|(line, rect)| place_runs(line, *rect))
        .collect()
}

/// Replaces cells `[col, col + width)` of a row with `menu`, keeping the runs on either side.
pub(super) fn overlay(row: Vec<Run>, col: usize, width: usize, menu: Run) -> Vec<Run> {
    let end = col + width;
    let (mut out, mut right) = (Vec::new(), Vec::new());
    for run in row {
        let run_end = run.col + run.cells;
        if run.col < col {
            out.push(clip(&run, 0, col.min(run_end) - run.col));
        }
        if run_end > end {
            right.push(clip(&run, end.max(run.col) - run.col, run.cells));
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
        shape,
        ..run.clone()
    }
}

impl App {
    /// Cursor drawn for the terminal, from the `cursor_style` setting (programs cannot change it).
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
        let (bar, _) = self.tab_bar();
        let mut rows = vec![Row::from(bar)];
        match &self.view {
            View::Terminal => {
                // The banner takes the top rows of the grid; the grid's bottom rows give way to it.
                let banner = self.banner_rows();
                rows.extend(self.banner_lines_for(banner).into_iter().map(Row::from));
                rows.extend(self.grid_frame(banner));
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

    /// The pane grid below the banner. A row is composed only when one of its pane rows is a new allocation or
    /// moved; otherwise the previous frame's row is handed out again.
    fn grid_frame(&mut self, banner: usize) -> Vec<Row> {
        let rows = self.grid_rows.saturating_sub(banner);
        let (cursor_shape, grid_rect) = (self.cursor_shape(), self.grid_rect());
        let mut inputs: Vec<Vec<(Row, Rect)>> = vec![Vec::new(); rows];
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
                    cursor_shape,
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
        let mut previous = std::mem::take(&mut self.grid_composed).into_iter();
        let composed: Vec<GridRow> = inputs
            .into_iter()
            .map(|row_inputs| match previous.next() {
                Some(old) if same_inputs(&old.inputs, &row_inputs) => old,
                _ => GridRow {
                    row: compose_row(&row_inputs),
                    inputs: row_inputs,
                },
            })
            .collect();
        let grid = composed.iter().map(|entry| entry.row.clone()).collect();
        self.grid_composed = composed;
        grid
    }
}

//! Projects the frame (rows of runs) into the compiled UI document. The document has a fixed pool of run nodes per
//! row (see `pool.rs`); only rows whose runs changed are written, and unused slots are hidden.
use flamewm_ui_x11::{UiColor, UiDocumentAccess};

use crate::app::App;
use crate::pool::{POOL_ROWS, POOL_RUNS, run_id};
use crate::view::Run;

#[derive(Default)]
pub struct UiCache {
    rows: Vec<Vec<Run>>,
    font: Option<u16>,
    surface: Option<(u32, u8)>,
}

const fn rgb(value: u32) -> UiColor {
    UiColor { r: (value >> 16) as u8, g: (value >> 8) as u8, b: value as u8, a: 255 }
}

/// Fits a row into the pool: when a row has more runs than slots, the tail merges into the last slot. Those cells
/// take the first merged run's colours, the one visible limit of the pooled layout.
fn fit(runs: &[Run], slots: usize) -> Vec<Run> {
    if runs.len() <= slots {
        return runs.to_vec();
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
    });
    out
}

impl App {
    /// Writes the current frame to `doc`, touching only the nodes whose content changed.
    pub fn project(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let surface = (self.palette.background, self.surface_alpha);
        if self.ui.surface != Some(surface) {
            let color = UiColor { a: self.surface_alpha, ..rgb(self.palette.background) };
            doc.background("screen", color)?;
            self.ui.surface = Some(surface);
        }
        if self.ui.font != Some(self.config.font_size) {
            doc.font_family("monospace".to_owned())?;
            for row in 0..POOL_ROWS {
                for slot in 0..POOL_RUNS {
                    doc.font_size(&run_id(row, slot), f32::from(self.config.font_size))?;
                }
            }
            self.ui.font = Some(self.config.font_size);
            self.ui.rows.clear();
        }
        let frame = self.frame();
        let (cell_w, cell_h) = (self.cell_w, self.cell_h);
        if self.ui.rows.len() != POOL_ROWS {
            self.ui.rows = vec![Vec::new(); POOL_ROWS];
        }
        for row in 0..POOL_ROWS {
            let runs: &[Run] = frame.get(row).map_or(&[], Vec::as_slice);
            if self.ui.rows[row] == runs {
                continue;
            }
            let fitted = fit(runs, POOL_RUNS);
            for slot in 0..POOL_RUNS {
                let id = run_id(row, slot);
                match fitted.get(slot) {
                    Some(run) => {
                        // nowrap collapses ASCII spaces, so cells are written as no-break spaces to keep their width.
                        doc.text(&id, run.text.replace(' ', "\u{a0}"))?;
                        doc.position(&id, run.col as f32 * cell_w, row as f32 * cell_h)?;
                        doc.size(&id, run.cells as f32 * cell_w, cell_h)?;
                        doc.foreground(&id, rgb(run.fg))?;
                        match run.bg {
                            Some(bg) => doc.background(&id, rgb(bg))?,
                            None => doc.background_clear(&id)?,
                        }
                        doc.font_weight(&id, if run.bold { 700 } else { 400 })?;
                        doc.visible(&id, true)?;
                    }
                    None => doc.visible(&id, false)?,
                }
            }
            self.ui.rows[row] = runs.to_vec();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(col: usize, text: &str) -> Run {
        Run { col, cells: text.chars().count(), text: text.into(), fg: 0, bg: None, bold: false }
    }

    #[test]
    fn rows_within_the_pool_are_unchanged() {
        let runs = vec![run(0, "ab"), run(2, "cd")];
        assert_eq!(fit(&runs, POOL_RUNS), runs);
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

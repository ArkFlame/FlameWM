//! Pointer and wheel input: mouse selection, mouse reports to the child, and pane scrollback.

use super::overlay::{MENU, menu_width};
use super::{App, Pane, View};
use crate::screen::MouseMode;
use crate::view::Span;
use flamewm_ui_x11::UiActionEvent;
use flamewm_ui_x11::UiActionPhase;

const SCROLL_LINES_PER_NOTCH: usize = 3;

/// Absolute line index for a pane-local grid row under that pane's scroll position.
fn absolute_row(pane: &Pane, grid_row: usize) -> Option<usize> {
    let screen = &pane.session.screen;
    let top = screen
        .total_lines()
        .saturating_sub(screen.rows() + pane.view_offset.min(screen.scrollback_len()));
    Some(top + grid_row)
}

/// SGR (1006) or X10 mouse report for a button event in grid coordinates (0-based).
#[must_use]
pub fn mouse_report(
    sgr: bool,
    button: u8,
    phase: UiActionPhase,
    col: usize,
    row: usize,
    mode: MouseMode,
) -> Option<Vec<u8>> {
    let motion = matches!(phase, UiActionPhase::Motion | UiActionPhase::Hover);
    if motion && !matches!(mode, MouseMode::Drag | MouseMode::Any) {
        return None;
    }
    let (code, release) = match phase {
        UiActionPhase::Press => (u16::from(button), false),
        UiActionPhase::Release => (u16::from(button), true),
        _ if motion => (u16::from(button) + 32, false),
        _ => return None,
    };
    // X10 has no release code of its own: a release is reported as button 3.
    let code = if release && !sgr { 3 } else { code };
    Some(mouse_report_raw(sgr, code, !release, col, row))
}

pub(super) fn mouse_report_raw(
    sgr: bool,
    code: u16,
    press: bool,
    col: usize,
    row: usize,
) -> Vec<u8> {
    if sgr {
        let end = if press { 'M' } else { 'm' };
        format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes()
    } else {
        mouse_report_raw_x10(code as u8, col, row)
    }
}

fn mouse_report_raw_x10(code: u8, col: usize, row: usize) -> Vec<u8> {
    let clamp = |value: usize| (value + 33).min(255) as u8;
    vec![
        0x1b,
        b'[',
        b'M',
        code.saturating_add(32),
        clamp(col),
        clamp(row),
    ]
}

impl App {
    /// The pane a drag started in: its press set `drag_anchor`, and a motion elsewhere must not extend it.
    fn drag_pane(&self) -> Option<u64> {
        let tab = self.tabs.get(self.active)?;
        tab.root
            .leaves()
            .into_iter()
            .find(|pane| pane.drag_anchor.is_some())
            .map(|pane| pane.session.id)
    }

    /// Mouse and wheel actions from the `terminal.surface` region.
    /// Returns true when the projected document may have changed; false only on paths proven to change nothing.
    pub fn pointer(&mut self, event: &UiActionEvent) -> bool {
        if event.action == "scroll" {
            let dy = event
                .text
                .as_deref()
                .and_then(|t| t.split_once(','))
                .and_then(|(_, dy)| dy.parse::<f32>().ok())
                .unwrap_or(0.0);
            return dy != 0.0 && event.inside && self.wheel(dy < 0.0, event);
        }
        let row = (event.y / self.cell_h).floor().max(0.0) as usize;
        let col = (event.x / self.cell_w).floor().max(0.0) as usize;
        if let (View::Settings(settings), UiActionPhase::Hover | UiActionPhase::Motion) =
            (&mut self.view, event.phase)
        {
            let command = settings.hover_theme(row.saturating_sub(1), self.grid_rows, &self.themes);
            self.settings_command(command);
            return true;
        }
        // Closing an open menu changes the projection even when the click then falls through.
        let mut changed = false;
        if let Some((menu_row, menu_col)) = self.menu {
            if event.phase == UiActionPhase::Press && event.button == 1 {
                self.menu = None;
                changed = true;
                let inside = (menu_row..menu_row + MENU.len()).contains(&row)
                    && (menu_col..menu_col + menu_width()).contains(&col);
                if inside {
                    self.menu_action(row - menu_row);
                    return true;
                }
            }
        }
        let mouse_captured = !event.shift_down()
            && self
                .focused_pane()
                .is_some_and(|pane| pane.session.screen.mouse != MouseMode::Off);
        if event.phase == UiActionPhase::Release
            && event.button == 3
            && matches!(self.view, View::Terminal)
            && (row == 0 || !mouse_captured)
        {
            let top = (self.grid_rows + 1).saturating_sub(MENU.len());
            self.menu = Some((
                row.min(top),
                col.min(self.cols.saturating_sub(menu_width())),
            ));
            return true;
        }
        if row == 0 {
            if event.phase == UiActionPhase::Press && event.button == 1 {
                let (_, hits) = self.tab_bar();
                if let Some((_, _, hit)) = hits
                    .into_iter()
                    .find(|(start, end, _)| (*start..*end).contains(&col))
                {
                    self.tab_hit(hit);
                    return true;
                }
            }
            return changed;
        }
        if !matches!(self.view, View::Terminal) {
            return changed;
        }
        let Some(grid_row) = (row - 1).checked_sub(self.banner_rows()) else {
            return changed;
        };
        if grid_row >= self.grid_rows || col >= self.cols {
            return changed;
        }
        // The pane under the pointer, except during a drag: that press belongs to the pane it started in.
        let drag = self.drag_pane();
        let target = drag.or_else(|| self.pane_at(grid_row, col).map(|(id, _)| id));
        let Some(target) = target else { return changed };
        let Some(rect) = self.pane_rect(target) else {
            return changed;
        };
        let inside = rect.contains(grid_row, col);
        if !inside && drag.is_none() {
            return changed;
        }
        // A drag that leaves its pane keeps selecting up to the pane's edge.
        let pane_row = grid_row
            .saturating_sub(rect.y0)
            .min(rect.rows.saturating_sub(1));
        let pane_col = col.saturating_sub(rect.x0).min(rect.cols.saturating_sub(1));
        if inside && event.phase == UiActionPhase::Press && event.button == 1 {
            // Clicking a pane gives it the keyboard and the cursor.
            self.focus_pane(target);
            changed = true;
        }
        let shift = event.shift_down();
        let mouse = self
            .pane(target)
            .map(|pane| (pane.session.screen.mouse, pane.session.screen.mouse_sgr));
        if inside
            && let Some((mode, sgr)) = mouse
            && mode != MouseMode::Off
            && !shift
        {
            let button = event.button.saturating_sub(1).min(2) as u8;
            if let Some(bytes) = mouse_report(sgr, button, event.phase, pane_col, pane_row, mode) {
                // A motion report only goes to the child; any visible effect comes back as PTY output and commits then.
                let motion = matches!(event.phase, UiActionPhase::Motion | UiActionPhase::Hover);
                self.send_to(target, bytes);
                changed |= !motion;
            }
            return changed;
        }
        let Some(abs_row) = self
            .pane(target)
            .and_then(|pane| absolute_row(pane, pane_row))
        else {
            return changed;
        };
        match event.phase {
            UiActionPhase::Press if event.button == 1 => {
                changed = true;
                if let Some(pane) = self.pane_mut(target) {
                    pane.selection = None;
                    pane.drag_anchor = None;
                }
                match self.register_click(target, abs_row, pane_col) {
                    2 => self.select_word(target, abs_row, pane_col),
                    3 => self.select_line(target, abs_row),
                    _ => {
                        if let Some(pane) = self.pane_mut(target) {
                            pane.drag_anchor = Some((abs_row, pane_col));
                        }
                    }
                }
            }
            // The renderer only fires Press for button 1; buttons 2 and 3 are delivered on Release.
            UiActionPhase::Release if event.button == 2 => {
                self.paste_primary();
                changed = true;
            }
            UiActionPhase::Motion | UiActionPhase::Hover => {
                if let Some(pane) = self.pane_mut(target)
                    && let Some(anchor) = pane.drag_anchor
                {
                    let span = Span::normalised(anchor, (abs_row, pane_col));
                    if pane.selection != Some(span) {
                        pane.selection = Some(span);
                        changed = true;
                    }
                }
            }
            UiActionPhase::Release => {
                if let Some(anchor) = self
                    .pane_mut(target)
                    .and_then(|pane| pane.drag_anchor.take())
                {
                    changed = true;
                    if anchor == (abs_row, pane_col) {
                        if let Some(pane) = self.pane_mut(target) {
                            pane.selection = None;
                        }
                    } else {
                        self.select_span(target, Span::normalised(anchor, (abs_row, pane_col)));
                    }
                }
            }
            UiActionPhase::Press => {}
        }
        changed
    }

    /// Wheel over a pane: mouse report or arrow keys to that pane, else its scrollback moves. The pane under the
    /// pointer is used, so scrolling a pane never needs a click first. Returns true when something was sent or moved.
    fn wheel(&mut self, up: bool, event: &UiActionEvent) -> bool {
        let pointer_col = (event.x / self.cell_w).floor().max(0.0) as usize;
        let pointer_row = ((event.y / self.cell_h).floor().max(1.0) as usize)
            .saturating_sub(1)
            .saturating_sub(self.banner_rows());
        let target = self.pane_at(pointer_row, pointer_col).or_else(|| {
            self.focused_id()
                .and_then(|id| self.pane_rect(id).map(|rect| (id, rect)))
        });
        let Some((id, rect)) = target else {
            return false;
        };
        let Some(pane) = self.pane(id) else {
            return false;
        };
        let (mode, sgr, alt) = (
            pane.session.screen.mouse,
            pane.session.screen.mouse_sgr,
            pane.session.screen.alt_active(),
        );
        let app_cursor = pane.session.screen.app_cursor;
        let max = pane.session.screen.scrollback_len();
        let (col, row) = (
            pointer_col.saturating_sub(rect.x0),
            pointer_row.saturating_sub(rect.y0),
        );
        if mode != MouseMode::Off {
            let button = if up { 64 } else { 65 };
            let report = mouse_report_raw(sgr, button, true, col, row);
            self.send_to(id, report);
            true
        } else if alt {
            let arrow = match (up, app_cursor) {
                (true, true) => "\x1bOA",
                (true, false) => "\x1b[A",
                (false, true) => "\x1bOB",
                (false, false) => "\x1b[B",
            };
            let bytes = arrow.repeat(SCROLL_LINES_PER_NOTCH).into_bytes();
            self.send_to(id, bytes);
            true
        } else if let Some(pane) = self.pane_mut(id) {
            let before = pane.view_offset;
            pane.view_offset = if up {
                (pane.view_offset + SCROLL_LINES_PER_NOTCH).min(max)
            } else {
                pane.view_offset.saturating_sub(SCROLL_LINES_PER_NOTCH)
            };
            pane.view_offset != before
        } else {
            false
        }
    }
}

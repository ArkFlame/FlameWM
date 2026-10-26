//! Alt+Tab switcher overlay: a retained card listing every window in MRU order.
//!
//! The WM owns selection, focus and commit. This module only renders the latest
//! `WindowSwitcherChanged` snapshot, so it never becomes WM authority and the switch works with
//! the card hidden. A snapshot without `immediate` shows after [`HOLD_DELAY`]; a quick tap that
//! closes first never paints. The shell loop wakes at [`SwitcherController::deadline`].

use std::ops::Range;
use std::time::{Duration, Instant};

use flamewm_api::settings::SettingsSnapshot;
use flamewm_api::Rect;
use flamewm_control_wire::SwitcherEntry;
use flamewm_ui_x11::{
    decode_document, SurfaceConfig, SurfaceHandle, SurfaceInputMode, SurfaceRole, SurfaceRuntime,
    UiBackendError, UiDocumentAccess, UiTemplate,
};

use crate::settings_projection::{self, SurfaceKind};

const COMPILED_UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-switcher.rwr"));

/// Delay before a non-immediate open paints. Matches a quick Alt+Tab tap.
pub const HOLD_DELAY: Duration = Duration::from_millis(150);
/// Visible rows; the list scrolls with the selection past this many windows.
pub const ROWS: usize = 8;
/// Logical row height. Must match `.launcher-row` in `flamewm.css`.
pub const ROW_HEIGHT: i32 = 40;
pub const CARD_WIDTH: i32 = 420;
/// Gap above the row list (`.launcher-list` margin) plus the card padding and border.
const LIST_GAP: i32 = 6;
const CARD_CHROME: i32 = 18;

/// One snapshot from the WM, as the shell keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitcherFrame {
    pub open: bool,
    pub immediate: bool,
    pub selected: usize,
    pub entries: Vec<SwitcherEntry>,
}

/// Rows shown for `count` windows with `selected` in view: the window scrolls only once the
/// selection leaves the first `ROWS`.
#[must_use]
pub fn visible_window(count: usize, selected: usize) -> Range<usize> {
    let rows = ROWS.min(count);
    let first = selected
        .saturating_add(1)
        .saturating_sub(ROWS)
        .min(count.saturating_sub(rows));
    first..first + rows
}

/// Logical card size for `rows` visible rows.
#[must_use]
pub fn card_size(rows: usize) -> (i32, i32) {
    let rows = rows.clamp(1, ROWS) as i32;
    (CARD_WIDTH, CARD_CHROME + LIST_GAP + ROW_HEIGHT * rows)
}

/// Card centred on `output`, clamped inside it.
#[must_use]
pub fn placement(output: Rect, width: i32, height: i32) -> Rect {
    let width = width.clamp(1, output.width.max(1));
    let height = height.clamp(1, output.height.max(1));
    let x = output.x + (output.width - width) / 2;
    let y = output.y + (output.height - height) / 2;
    Rect::new(x, y, width, height)
}

/// Label for one row: the title, else the application class, else a fixed word.
#[must_use]
pub fn row_label(entry: &SwitcherEntry) -> &str {
    if !entry.title.is_empty() {
        &entry.title
    } else if !entry.app_id.is_empty() {
        &entry.app_id
    } else {
        "Window"
    }
}

/// Owns the single retained overlay surface and its show/hide timing.
#[derive(Default)]
pub struct SwitcherController {
    frame: Option<SwitcherFrame>,
    show_at: Option<Instant>,
    visible: bool,
    dirty: bool,
    surface: Option<SurfaceHandle>,
}

impl SwitcherController {
    /// The pending show deadline, if the card is waiting out its hold delay.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        if self.visible {
            None
        } else {
            self.show_at
        }
    }

    /// Applies at most one WM snapshot, shows the card when its delay has passed, and repaints
    /// only when the content changed. `target` is the output and scale the card centres on.
    pub fn update(
        &mut self,
        runtime: &mut SurfaceRuntime,
        frame: Option<SwitcherFrame>,
        target: impl FnOnce() -> (Rect, f32),
        settings: Option<&SettingsSnapshot>,
        now: Instant,
    ) -> Result<(), UiBackendError> {
        if let Some(frame) = frame {
            if self.apply(frame, now) {
                return match self.surface {
                    Some(surface) => runtime.hide(surface),
                    None => Ok(()),
                };
            }
        }
        if self.show_at.is_some_and(|at| now >= at) {
            self.show_at = None;
            self.visible = true;
            self.dirty = true;
        }
        if !self.visible || !self.dirty {
            return Ok(());
        }
        self.dirty = false;
        let Some(frame) = self.frame.clone() else {
            return Ok(());
        };
        self.paint_and_show(runtime, &frame, target, settings)
    }

    /// Records one snapshot. Returns true when the card must hide now.
    fn apply(&mut self, frame: SwitcherFrame, now: Instant) -> bool {
        if !frame.open {
            let was_visible = self.visible;
            *self = Self {
                surface: self.surface,
                ..Self::default()
            };
            return was_visible;
        }
        let immediate = frame.immediate;
        self.frame = Some(frame);
        self.dirty = true;
        if immediate {
            self.show_at = None;
            self.visible = true;
        } else if !self.visible && self.show_at.is_none() {
            self.show_at = Some(now + HOLD_DELAY);
        }
        false
    }

    fn paint_and_show(
        &mut self,
        runtime: &mut SurfaceRuntime,
        frame: &SwitcherFrame,
        target: impl FnOnce() -> (Rect, f32),
        settings: Option<&SettingsSnapshot>,
    ) -> Result<(), UiBackendError> {
        let (output, scale) = target();
        let surface = if let Some(surface) = self.surface {
            surface
        } else {
            let surface = create(runtime)?;
            self.surface = Some(surface);
            surface
        };
        let scale = runtime.apply_output_scale(surface, scale)?;
        let (width, height) = card_size(frame.entries.len());
        let width = (width as f32 * scale).round() as i32;
        let height = (height as f32 * scale).round() as i32;
        let card = placement(output, width, height);
        if let Some(settings) = settings {
            runtime.with_document(surface, |document| {
                settings_projection::project(document, settings, SurfaceKind::Switcher)
            })?;
            let (opacity, blur) = settings_projection::start_backdrop_percents(settings);
            runtime.set_popup_backdrop(surface, Some((opacity, blur)))?;
        }
        runtime.with_document(surface, |document| paint(document, frame))?;
        runtime.move_resize(
            surface,
            card.x,
            card.y,
            card.width as u32,
            card.height as u32,
        )?;
        runtime.show(surface)?;
        runtime.redraw(surface)
    }
}

fn create(runtime: &mut SurfaceRuntime) -> Result<SurfaceHandle, UiBackendError> {
    let document = decode_document(COMPILED_UI).map_err(UiBackendError::Document)?;
    let (width, height) = card_size(ROWS);
    runtime.create_surface(
        UiTemplate::new(document),
        SurfaceConfig {
            width: width as u32,
            height: height as u32,
            title: "FlameWM switcher".to_owned(),
            role: SurfaceRole::Overlay,
            input: SurfaceInputMode::PassThrough,
            initially_visible: false,
            x: 0,
            y: 0,
        },
    )
}

fn paint(document: &mut impl UiDocumentAccess, frame: &SwitcherFrame) -> Result<(), String> {
    let window = visible_window(frame.entries.len(), frame.selected);
    let highlight = frame.selected.saturating_sub(window.start);
    document.position(
        "switcher-highlight",
        0.0,
        (highlight as i32 * ROW_HEIGHT) as f32,
    )?;
    for slot in 0..ROWS {
        let row = slot + 1;
        let row_id = format!("switcher-row-{row}");
        let label_id = format!("switcher-row-{row}-label");
        match frame
            .entries
            .get(window.start + slot)
            .filter(|_| window.start + slot < window.end)
        {
            Some(entry) => {
                document.text(&label_id, row_label(entry).to_owned())?;
                document.visible(&row_id, true)?;
            }
            None => document.visible(&row_id, false)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, app_id: &str) -> SwitcherEntry {
        SwitcherEntry {
            title: title.to_owned(),
            app_id: app_id.to_owned(),
        }
    }

    fn frame(open: bool, immediate: bool, count: usize) -> SwitcherFrame {
        SwitcherFrame {
            open,
            immediate,
            selected: 0,
            entries: (0..count).map(|i| entry(&format!("w{i}"), "")).collect(),
        }
    }

    #[test]
    fn window_scrolls_only_past_the_visible_rows() {
        assert_eq!(visible_window(3, 2), 0..3);
        assert_eq!(visible_window(20, 0), 0..8);
        assert_eq!(visible_window(20, 7), 0..8);
        assert_eq!(visible_window(20, 8), 1..9);
        assert_eq!(visible_window(20, 19), 12..20);
        assert_eq!(visible_window(0, 0), 0..0);
    }

    #[test]
    fn card_height_follows_the_visible_row_count() {
        assert_eq!(
            card_size(1),
            (CARD_WIDTH, CARD_CHROME + LIST_GAP + ROW_HEIGHT)
        );
        assert_eq!(
            card_size(50).1,
            CARD_CHROME + LIST_GAP + ROW_HEIGHT * ROWS as i32
        );
    }

    #[test]
    fn card_centres_on_the_output_and_clamps() {
        let output = Rect::new(1920, 0, 1000, 800);
        assert_eq!(placement(output, 400, 200), Rect::new(2220, 300, 400, 200));
        assert_eq!(placement(output, 2000, 2000), Rect::new(1920, 0, 1000, 800));
    }

    #[test]
    fn row_label_prefers_title_then_app_class() {
        assert_eq!(row_label(&entry("Files", "org.files")), "Files");
        assert_eq!(row_label(&entry("", "org.files")), "org.files");
        assert_eq!(row_label(&entry("", "")), "Window");
    }

    #[test]
    fn non_immediate_open_waits_for_the_hold_delay() {
        let mut controller = SwitcherController::default();
        let now = Instant::now();
        assert!(!controller.apply(frame(true, false, 3), now));
        assert_eq!(controller.deadline(), Some(now + HOLD_DELAY));
        assert!(!controller.visible, "not shown before the delay");
    }

    #[test]
    fn immediate_open_shows_without_waiting() {
        let mut controller = SwitcherController::default();
        controller.apply(frame(true, true, 3), Instant::now());
        assert!(controller.visible);
        assert_eq!(controller.deadline(), None);
    }

    #[test]
    fn quick_tap_close_cancels_the_pending_show() {
        let mut controller = SwitcherController::default();
        let now = Instant::now();
        controller.apply(frame(true, false, 3), now);
        let hide = controller.apply(frame(false, false, 0), now);
        assert!(!hide, "a card that never showed has nothing to hide");
        assert_eq!(controller.deadline(), None);
        assert!(controller.frame.is_none());
    }

    #[test]
    fn close_after_show_requests_a_hide() {
        let mut controller = SwitcherController::default();
        controller.apply(frame(true, true, 2), Instant::now());
        assert!(controller.apply(frame(false, false, 0), Instant::now()));
        assert!(!controller.visible);
    }
}

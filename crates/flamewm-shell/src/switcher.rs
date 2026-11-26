//! Alt+Tab switcher overlay: a retained card listing every window in MRU order.
//!
//! The WM owns selection, focus and commit. This module only renders the latest
//! `WindowSwitcherChanged` snapshot, so it never becomes WM authority and the switch works with
//! the card hidden. A snapshot without `immediate` shows after [`HOLD_DELAY`]; a quick tap that
//! closes first never paints. The shell loop wakes at [`SwitcherController::deadline`].

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use flamewm_api::settings::SettingsSnapshot;
use flamewm_api::switcher::SwitcherThumb;
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
    /// Resolved previews aligned with `entries`; shared pixels, never copied into the frame.
    pub previews: Vec<Option<SwitcherPreview>>,
}

impl SwitcherFrame {
    /// Cheap content identity: labels, selection and preview identity, never pixel bytes. A
    /// repeated snapshot therefore repaints nothing.
    fn same_content(&self, other: &Self) -> bool {
        self.open == other.open
            && self.immediate == other.immediate
            && self.selected == other.selected
            && self.entries == other.entries
            && self.previews.len() == other.previews.len()
            && self
                .previews
                .iter()
                .zip(&other.previews)
                .all(|(left, right)| match (left, right) {
                    (None, None) => true,
                    (Some(left), Some(right)) => left.same_content(right),
                    _ => false,
                })
    }
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

/// One resolved preview: shared pixel buffer plus its size. The shell hands these to the UI
/// document by shared handle, so a repaint never copies preview bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitcherPreview {
    pub width: u32,
    pub height: u32,
    /// Content identity; `0` marks untracked legacy pixels that bypass the generation cache.
    pub generation: u64,
    pub pixels: Arc<Vec<u8>>,
}

impl SwitcherPreview {
    /// Cheap identity: generation and size, plus the buffer only for untracked legacy pixels.
    fn same_content(&self, other: &Self) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.generation == other.generation
            && (self.generation != 0
                || Arc::ptr_eq(&self.pixels, &other.pixels)
                || self.pixels == other.pixels)
    }

    /// Document source key. A non-zero generation is the WM's content identity, so it names the
    /// pixels without re-hashing them and lets identical previews share one upload.
    fn source(&self) -> String {
        if self.generation != 0 {
            return format!(
                "switcher-thumb-{}x{}-g{}",
                self.width, self.height, self.generation
            );
        }
        let mut hasher = DefaultHasher::new();
        self.pixels.hash(&mut hasher);
        format!(
            "switcher-thumb-{}x{}-{:x}",
            self.width,
            self.height,
            hasher.finish()
        )
    }
}

/// Takes one thumb's pixels as a preview, leaving the entry as a metadata-only reference.
fn take_preview(thumb: &mut SwitcherThumb) -> Option<SwitcherPreview> {
    let pixels = Arc::new(std::mem::take(&mut thumb.rgba));
    let expected = u64::from(thumb.width) * u64::from(thumb.height) * 4;
    (thumb.width > 0 && thumb.height > 0 && pixels.len() as u64 == expected).then_some({
        SwitcherPreview {
            width: thumb.width,
            height: thumb.height,
            generation: thumb.generation,
            pixels,
        }
    })
}

/// Preview pixels the shell holds, keyed by the WM's generation. Rebuilt from every snapshot, so
/// only generations the latest snapshot names survive; a closed snapshot names none.
#[derive(Debug, Clone, Default)]
pub struct SwitcherThumbCache(HashMap<u64, SwitcherPreview>);

impl SwitcherThumbCache {
    /// Stores fresh pixels and resolves references from the cache. Returns one preview per entry,
    /// aligned with `entries`. Pixels live in a shared handle, so a reference reuses the buffer
    /// without copying it. A reference whose pixels are missing drops to no preview (the row keeps
    /// its title).
    pub fn resolve(&mut self, entries: &mut [SwitcherEntry]) -> Vec<Option<SwitcherPreview>> {
        let mut next = HashMap::with_capacity(entries.len());
        let mut previews = Vec::with_capacity(entries.len());
        for entry in entries.iter_mut() {
            let Some(thumb) = entry.thumb.as_mut() else {
                previews.push(None);
                continue;
            };
            if thumb.generation == 0 {
                // LEGACY: untracked pixels render directly and are not cached.
                previews.push(take_preview(thumb));
                continue;
            }
            if thumb.is_reference() {
                let held = self
                    .0
                    .get(&thumb.generation)
                    .filter(|held| held.width == thumb.width && held.height == thumb.height);
                if let Some(held) = held {
                    previews.push(Some(held.clone()));
                } else {
                    entry.thumb = None;
                    previews.push(None);
                }
                continue;
            }
            let Some(preview) = take_preview(thumb) else {
                previews.push(None);
                continue;
            };
            next.insert(preview.generation, preview.clone());
            previews.push(Some(preview));
        }
        self.0 = next;
        previews
    }

    /// Number of previews held; for tests and diagnostics.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
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
        // A repeated snapshot (same selection, same preview generations) changes nothing on screen.
        if self
            .frame
            .as_ref()
            .is_none_or(|held| !held.same_content(&frame))
        {
            self.dirty = true;
        }
        self.frame = Some(frame);
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

/// Largest preview box inside a row; the row keeps its height, a thumbnail never grows it.
const THUMB_BOX_W: f32 = 64.0;
const THUMB_BOX_H: f32 = 36.0;

/// Opacity of a minimised or other-workspace preview. Not a CSS token: the UI compiler accepts only colour and length custom properties.
pub const HIDDEN_PREVIEW_OPACITY: f32 = 0.45;

/// Small label on a row whose window is not on screen; empty for a visible window.
fn state_label(entry: &SwitcherEntry) -> &'static str {
    if entry.minimised {
        "minimised"
    } else if entry.other_workspace {
        "other workspace"
    } else {
        ""
    }
}

/// Display size for a `w` x `h` preview fitted into the row box. Never upscales.
#[must_use]
pub fn thumb_fit(w: u32, h: u32) -> (f32, f32) {
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let scale = (THUMB_BOX_W / w).min(THUMB_BOX_H / h).min(1.0);
    (w * scale, h * scale)
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
        let icon_id = format!("switcher-row-{row}-icon");
        let thumb_id = format!("switcher-row-{row}-thumb");
        let index = window.start + slot;
        match frame.entries.get(index).filter(|_| index < window.end) {
            Some(entry) => {
                document.text_ref(&label_id, row_label(entry))?;
                document.visible(&row_id, true)?;
                let hidden = entry.minimised || entry.other_workspace;
                let dim = if hidden { HIDDEN_PREVIEW_OPACITY } else { 1.0 };
                document.opacity(&icon_id, dim)?;
                document.opacity(&thumb_id, dim)?;
                let state_id = format!("switcher-row-{row}-state");
                document.text_ref(&state_id, state_label(entry))?;
                document.visible(&state_id, hidden)?;
                if let Some(preview) = frame.previews.get(index).and_then(Option::as_ref) {
                    let (w, h) = thumb_fit(preview.width, preview.height);
                    // Shared pixels: the document keeps the same buffer, so an unchanged preview
                    // is not copied or re-uploaded on the next repaint.
                    document.image_rgba8_shared(
                        &thumb_id,
                        preview.source(),
                        preview.width,
                        preview.height,
                        Arc::clone(&preview.pixels),
                    )?;
                    document.size(&thumb_id, w, h)?;
                    document.visible(&icon_id, false)?;
                    document.visible(&thumb_id, true)?;
                } else {
                    document.visible(&icon_id, true)?;
                    document.visible(&thumb_id, false)?;
                }
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
            minimised: false,
            other_workspace: false,
            thumb: None,
        }
    }

    fn thumb_entry(generation: u64, rgba: Vec<u8>) -> SwitcherEntry {
        SwitcherEntry {
            title: String::new(),
            app_id: String::new(),
            minimised: false,
            other_workspace: false,
            thumb: Some(SwitcherThumb {
                width: 1,
                height: 1,
                generation,
                rgba,
            }),
        }
    }

    fn switcher_row(title: &str, minimised: bool, other_workspace: bool) -> SwitcherEntry {
        SwitcherEntry {
            title: title.to_owned(),
            app_id: String::new(),
            thumb: None,
            minimised,
            other_workspace,
        }
    }

    // A minimised or other-workspace row dims its preview and names its state; a visible row does neither.
    #[test]
    fn hidden_rows_dim_their_preview_and_name_their_state() {
        let frame = SwitcherFrame {
            open: true,
            immediate: false,
            selected: 0,
            entries: vec![
                switcher_row("Visible", false, false),
                switcher_row("Minimised", true, false),
                switcher_row("Elsewhere", false, true),
            ],
            previews: vec![None, None, None],
        };
        let mut inner = crate::test_recorder::NullDocument;
        let mut doc = crate::test_recorder::Recording::new(&mut inner);
        paint(&mut doc, &frame).expect("paint");
        let rec = &doc.recorded;
        assert_eq!(rec.opacities.get("switcher-row-1-thumb"), Some(&1.0));
        assert_eq!(
            rec.opacities.get("switcher-row-2-thumb"),
            Some(&HIDDEN_PREVIEW_OPACITY)
        );
        assert_eq!(
            rec.opacities.get("switcher-row-3-icon"),
            Some(&HIDDEN_PREVIEW_OPACITY)
        );
        assert_eq!(rec.visible.get("switcher-row-1-state"), Some(&false));
        assert_eq!(
            rec.texts.get("switcher-row-2-state").map(String::as_str),
            Some("minimised")
        );
        assert_eq!(
            rec.texts.get("switcher-row-3-state").map(String::as_str),
            Some("other workspace")
        );
    }

    // Regression: an unchanged preview arrives as a reference and the shell reuses the same
    // buffer handle, so resolution and repaint never copy preview bytes.
    #[test]
    fn thumb_cache_reuses_pixels_for_reference_entries() {
        let mut cache = SwitcherThumbCache::default();
        let mut first = vec![thumb_entry(5, vec![1, 2, 3, 255])];
        let fresh = cache.resolve(&mut first);
        let fresh = fresh[0].as_ref().expect("fresh preview");
        assert_eq!(*fresh.pixels, vec![1, 2, 3, 255]);
        assert_eq!(fresh.source(), "switcher-thumb-1x1-g5");
        assert!(
            first[0].thumb.as_ref().is_some_and(|t| t.rgba.is_empty()),
            "the entry keeps metadata only"
        );

        let mut again = vec![thumb_entry(5, Vec::new())];
        let reused = cache.resolve(&mut again);
        let reused = reused[0].as_ref().expect("cached preview");
        assert!(
            Arc::ptr_eq(&fresh.pixels, &reused.pixels),
            "a reference shares the cached buffer instead of copying it"
        );
    }

    // Regression: the cache keeps only generations the latest snapshot names; a missing one
    // yields no preview instead of an empty image.
    #[test]
    fn thumb_cache_prunes_unnamed_generations_and_drops_missing_references() {
        let mut cache = SwitcherThumbCache::default();
        let named = cache.resolve(&mut [
            thumb_entry(5, vec![1, 2, 3, 255]),
            thumb_entry(6, vec![0; 4]),
        ]);
        assert!(named.iter().all(Option::is_some));
        assert_eq!(cache.len(), 2);

        let kept = cache.resolve(&mut [thumb_entry(6, Vec::new())]);
        assert_eq!(cache.len(), 1, "generation 5 is no longer named");
        assert!(kept[0].is_some());

        let mut stale = vec![thumb_entry(5, Vec::new())];
        assert!(cache.resolve(&mut stale)[0].is_none());
        assert_eq!(stale[0].thumb, None);

        cache.resolve(&mut []);
        assert!(cache.is_empty(), "a closed snapshot names nothing");
    }

    // LEGACY-DEFAULT: pixels with generation 0 (older WMs) render directly, uncached.
    #[test]
    fn thumb_cache_leaves_untracked_pixels_alone() {
        let mut cache = SwitcherThumbCache::default();
        let mut rows = vec![thumb_entry(0, vec![9, 9, 9, 255])];
        let previews = cache.resolve(&mut rows);
        assert_eq!(
            previews[0].as_ref().map(|p| p.pixels.as_ref().clone()),
            Some(vec![9, 9, 9, 255])
        );
        assert!(cache.is_empty());
    }

    fn frame(open: bool, immediate: bool, count: usize) -> SwitcherFrame {
        SwitcherFrame {
            open,
            immediate,
            selected: 0,
            entries: (0..count).map(|i| entry(&format!("w{i}"), "")).collect(),
            previews: vec![None; count],
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

    // Idle repaint guard: a repeated snapshot with the same selection and preview generations
    // marks nothing dirty, while a selection move still repaints.
    #[test]
    fn repeated_snapshot_does_not_repaint() {
        let mut controller = SwitcherController::default();
        let now = Instant::now();
        controller.apply(frame(true, true, 2), now);
        controller.dirty = false;
        controller.apply(frame(true, true, 2), now);
        assert!(!controller.dirty, "an identical snapshot repaints nothing");
        let mut moved = frame(true, true, 2);
        moved.selected = 1;
        controller.apply(moved, now);
        assert!(controller.dirty, "a selection move repaints");
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

//! Alt+Tab switcher and new-popup focus handling.

use super::*;
use flamewm_api::switcher::SwitcherThumb;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Most previews kept across opens. Matches `MAX_THUMBS` in the X11 path
/// (`crates/flamewm-wm-x11/src/wm/switcher_thumbs.rs`).
const THUMB_CAP: usize = 24;

/// Shortest gap between two captures of one window while the switcher is open. Repaints inside it
/// wait for the next due moment, so a busy client cannot cost a readback per commit.
const MIN_REFRESH: Duration = Duration::from_millis(200);

/// Preview cache shared by every open. Last capture wins and the oldest capture is evicted at
/// `THUMB_CAP`, so a minimised or other-workspace window keeps its last frame across opens. The
/// bound keeps the memory fixed (about 1.5 MiB at the 160x100 box).
#[derive(Default)]
pub(super) struct ThumbCache {
    /// Oldest capture first, with its capture time. A repeat `store` for a window moves it to the back.
    slots: VecDeque<(WindowId, Instant, SwitcherThumb)>,
    /// Windows whose preview this open already sent, so later publishes send a reference only.
    announced: Vec<WindowId>,
}

/// Process-wide preview generations. Unique across opens, so the shell's pixel cache never serves
/// one window's pixels for another window of a later open.
static NEXT_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl ThumbCache {
    /// Keeps `thumb` as the preview for `id`. Wire references and malformed or empty buffers are
    /// refused, so the entry keeps its `title` and `app_id` row. Evicts the oldest preview at `THUMB_CAP`.
    /// A preview with generation `0` (untracked) gets a fresh generation here.
    pub(super) fn store(&mut self, id: WindowId, mut thumb: SwitcherThumb) -> bool {
        if thumb.is_reference() || !thumb.is_well_formed() || thumb.width == 0 || thumb.height == 0
        {
            return false;
        }
        if thumb.generation == 0 {
            thumb.generation =
                NEXT_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        }
        self.slots.retain(|(known, _, _)| *known != id);
        self.announced.retain(|known| *known != id);
        if self.slots.len() == THUMB_CAP {
            self.slots.pop_front();
        }
        self.slots.push_back((id, Instant::now(), thumb));
        true
    }

    /// Starts a new open: every kept preview's pixels are sent once again, so no open relies on
    /// pixels a previous open sent.
    pub(super) fn begin_open(&mut self) {
        self.announced.clear();
    }

    /// True when `id` has no preview yet or its last capture is at least `MIN_REFRESH` old.
    fn refresh_due(&self, id: WindowId, now: Instant) -> bool {
        self.slots
            .iter()
            .find(|(known, _, _)| *known == id)
            .is_none_or(|(_, captured, _)| now.duration_since(*captured) >= MIN_REFRESH)
    }

    /// The preview to publish for `id`, if one was stored for this open. The first publish carries
    /// pixels; later ones send the same generation without pixels, so a Tab press does not resend
    /// every preview.
    pub(super) fn wire(&mut self, id: WindowId) -> Option<SwitcherThumb> {
        let (_, _, thumb) = self.slots.iter().find(|(known, _, _)| *known == id)?;
        if self.announced.contains(&id) {
            return Some(SwitcherThumb {
                width: thumb.width,
                height: thumb.height,
                generation: thumb.generation,
                rgba: Vec::new(),
            });
        }
        self.announced.push(id);
        Some(thumb.clone())
    }
}

/// Alt+Tab state while the switcher is open: the window order, the highlighted entry and the captures.
pub(super) struct Switcher {
    order: Vec<WindowId>,
    selected: usize,
    /// Windows whose preview the render path still has to capture, in switcher order.
    pending: VecDeque<WindowId>,
    /// Listed windows that repainted while open. Each is re-queued once `MIN_REFRESH` has passed.
    dirty: Vec<WindowId>,
}

/// What `emit_switcher` publishes: window order, highlighted index, and one optional preview per entry.
type SwitcherView = (Vec<WindowId>, usize, Vec<Option<SwitcherThumb>>);

/// Previews queued for one open: the first `THUMB_CAP` windows in switcher order.
fn capture_queue(order: &[WindowId]) -> VecDeque<WindowId> {
    order.iter().take(THUMB_CAP).copied().collect()
}

impl Switcher {
    /// The list `emit_switcher` publishes. Takes the cache `&mut` because a preview is marked announced when sent.
    fn view(&self, cache: &mut ThumbCache) -> SwitcherView {
        let thumbs = self.order.iter().map(|id| cache.wire(*id)).collect();
        (self.order.clone(), self.selected, thumbs)
    }
}

impl Service {
    /// Alt+Tab: the first press opens the switcher on the visible windows with the second one
    /// highlighted; each further press advances the highlight. Nothing is focused until Alt is released.
    pub fn switcher_step(&self, state: &mut State) {
        let mut slot = self.switcher.borrow_mut();
        let mut thumbs = self.thumbs.borrow_mut();
        match slot.as_mut() {
            Some(switcher) if !switcher.order.is_empty() => {
                switcher.selected = (switcher.selected + 1) % switcher.order.len();
            }
            Some(_) => {}
            None => {
                let order = state.model.visible_top_first();
                if order.len() < 2 {
                    return;
                }
                thumbs.begin_open();
                *slot = Some(Switcher {
                    pending: capture_queue(&order),
                    order,
                    selected: 1,
                    dirty: Vec::new(),
                });
                // The render path captures the queued previews on the next frame.
                state.request_redraw();
            }
        }
        let view: Option<SwitcherView> = slot.as_ref().map(|switcher| switcher.view(&mut thumbs));
        drop(slot);
        drop(thumbs);
        self.emit_switcher(view);
    }

    /// Next window whose preview the render path should capture, or `None` when none is queued.
    pub(crate) fn next_switcher_capture(&self) -> Option<WindowId> {
        self.switcher.borrow_mut().as_mut()?.pending.pop_front()
    }

    /// True while the open switcher still has previews queued for capture.
    pub(crate) fn switcher_captures_pending(&self) -> bool {
        self.switcher
            .borrow()
            .as_ref()
            .is_some_and(|switcher| !switcher.pending.is_empty())
    }

    /// Keeps a rendered preview for the open switcher and republishes the list. With no open
    /// switcher, or a preview the cache refuses, nothing is published.
    pub(crate) fn store_switcher_thumb(&self, id: WindowId, thumb: SwitcherThumb) {
        let mut thumbs = self.thumbs.borrow_mut();
        let slot = self.switcher.borrow();
        let Some(switcher) = slot.as_ref() else {
            return;
        };
        if !thumbs.store(id, thumb) {
            return;
        }
        let view = switcher.view(&mut thumbs);
        drop(slot);
        drop(thumbs);
        self.emit_switcher(Some(view));
    }

    /// A surface of `id` committed new pixels. A listed window queues a preview refresh while the
    /// switcher is open; `promote_switcher_refreshes` releases it once `MIN_REFRESH` has passed.
    /// True when a refresh now waits, so the caller wakes the render path.
    pub(crate) fn note_switcher_commit(&self, id: WindowId) -> bool {
        let mut slot = self.switcher.borrow_mut();
        let Some(switcher) = slot.as_mut() else {
            return false;
        };
        if !switcher.order.contains(&id) || switcher.pending.contains(&id) {
            return false;
        }
        if !switcher.dirty.contains(&id) {
            switcher.dirty.push(id);
        }
        true
    }

    /// Moves refreshes that are due into the capture queue. True while refreshes still wait, so the
    /// render path keeps running until they come due.
    pub(crate) fn promote_switcher_refreshes(&self, now: Instant) -> bool {
        let mut slot = self.switcher.borrow_mut();
        let Some(switcher) = slot.as_mut() else {
            return false;
        };
        let thumbs = self.thumbs.borrow();
        for id in std::mem::take(&mut switcher.dirty) {
            if !thumbs.refresh_due(id, now) {
                switcher.dirty.push(id);
            } else if !switcher.pending.contains(&id) {
                switcher.pending.push_back(id);
            }
        }
        !switcher.dirty.is_empty()
    }

    /// Alt released: commits the highlighted window and closes the switcher overlay.
    pub(super) fn finish_switcher_if_released(&self, state: &mut State) {
        if state
            .seat
            .get_keyboard()
            .is_some_and(|keyboard| keyboard.modifier_state().alt)
        {
            return;
        }
        let Some(switcher) = self.switcher.borrow_mut().take() else {
            return;
        };
        self.emit_switcher(None);
        if let Some(id) = switcher.order.get(switcher.selected).copied() {
            apply_command(state, Command::Activate(id));
        }
    }

    pub(super) fn emit_switcher(&self, view: Option<SwitcherView>) {
        let signal = match view {
            Some((order, selected, thumbs)) => {
                let windows = self.mirror.borrow();
                let entries = order
                    .iter()
                    .zip(thumbs)
                    .map(|(id, thumb)| {
                        let (title, app_id) = windows
                            .windows
                            .iter()
                            .find(|window| window.reference.id == *id)
                            .map_or((String::new(), String::new()), |window| {
                                (window.title.clone(), window.app_id.as_str().to_owned())
                            });
                        SwitcherEntry {
                            title,
                            app_id,
                            thumb,
                        }
                    })
                    .collect();
                ControlSignal::WindowSwitcherChanged {
                    open: true,
                    immediate: false,
                    selected: u32::try_from(selected).unwrap_or(0),
                    entries,
                }
            }
            None => ControlSignal::WindowSwitcherChanged {
                open: false,
                immediate: false,
                selected: 0,
                entries: Vec::new(),
            },
        };
        if let Err(error) = self.server.emit_signal(&signal) {
            eprintln!("flamewm-wayland: switcher signal failed: {error}");
        }
    }

    /// The shell's launcher and OSD are override-redirect X windows, which the compositor never
    /// focuses. With no managed window focused, the newest popup takes the keyboard once it maps, so
    /// typing reaches it. Docks and the desktop background (strut or below) are never focused.
    pub(super) fn focus_new_popup(&self, state: &mut State) {
        if state.focused.is_some() {
            return;
        }
        let top = state
            .unmanaged
            .iter()
            .rev()
            .find(|entry| !entry.below && entry.strut.is_none() && entry.surface.is_mapped())
            .and_then(|entry| {
                entry
                    .surface
                    .wl_surface()
                    .map(|surface| (entry.surface.window_id(), surface))
            });
        let Some((window, surface)) = top else { return };
        if self.popup_focus.borrow().is_some_and(|last| last == window) {
            return;
        }
        *self.popup_focus.borrow_mut() = Some(window);
        if let Some(keyboard) = state.seat.get_keyboard() {
            keyboard.set_focus(state, Some(surface), crate::state::next_serial());
        }
    }
}

#[cfg(test)]
mod thumb_tests {
    use super::*;

    fn thumb(generation: u64, width: u32, height: u32) -> SwitcherThumb {
        SwitcherThumb {
            width,
            height,
            generation,
            rgba: vec![0; (width * height * 4) as usize],
        }
    }

    #[test]
    fn store_keeps_complete_previews_and_refuses_bad_ones() {
        let mut cache = ThumbCache::default();
        assert!(cache.store(7, thumb(1, 4, 2)));
        assert!(!cache.store(
            8,
            SwitcherThumb {
                rgba: Vec::new(),
                ..thumb(1, 4, 2)
            }
        ));
        assert!(!cache.store(
            8,
            SwitcherThumb {
                rgba: vec![0; 3],
                ..thumb(1, 4, 2)
            }
        ));
        assert!(!cache.store(8, thumb(1, 0, 0)));
        assert_eq!(cache.wire(7), Some(thumb(1, 4, 2)));
        assert_eq!(cache.wire(8), None);
    }

    #[test]
    fn store_replaces_per_window_and_evicts_oldest_at_cap() {
        let mut cache = ThumbCache::default();
        for id in 0..THUMB_CAP as WindowId {
            assert!(cache.store(id, thumb(1, 1, 1)));
        }
        assert!(cache.store(0, thumb(2, 2, 1)));
        assert_eq!(cache.slots.len(), THUMB_CAP);
        assert_eq!(cache.wire(0).map(|t| t.generation), Some(2));
        assert!(cache.store(999, thumb(1, 1, 1)));
        assert_eq!(cache.slots.len(), THUMB_CAP);
        assert_eq!(cache.wire(1), None, "oldest preview is evicted");
        assert!(cache.wire(0).is_some(), "refreshed preview survives");
    }

    #[test]
    fn store_assigns_generation_and_wire_sends_pixels_once() {
        let mut cache = ThumbCache::default();
        assert!(cache.store(3, thumb(0, 2, 2)));
        let first = cache.wire(3).expect("stored");
        assert_ne!(first.generation, 0);
        assert_eq!(first.rgba.len(), 16);
        let again = cache.wire(3).expect("stored");
        assert_eq!(again.generation, first.generation);
        assert!(again.is_reference(), "repeat publish is a reference");
    }

    #[test]
    fn refresh_waits_out_the_interval_per_window() {
        let mut cache = ThumbCache::default();
        assert!(
            cache.refresh_due(5, Instant::now()),
            "uncached window is due"
        );
        assert!(cache.store(5, thumb(1, 1, 1)));
        let now = Instant::now();
        assert!(!cache.refresh_due(5, now), "fresh capture waits");
        assert!(
            cache.refresh_due(5, now + MIN_REFRESH),
            "due after the interval"
        );
        assert!(cache.refresh_due(6, now), "other windows are independent");
    }

    #[test]
    fn begin_open_resends_pixels_once_per_open() {
        let mut cache = ThumbCache::default();
        assert!(cache.store(4, thumb(1, 2, 2)));
        assert!(!cache.wire(4).expect("stored").is_reference());
        assert!(cache.wire(4).expect("stored").is_reference());
        cache.begin_open();
        assert!(!cache.wire(4).expect("stored").is_reference());
    }

    #[test]
    fn capture_queue_is_capped_in_switcher_order() {
        let order: Vec<WindowId> = (0..THUMB_CAP as WindowId + 5).collect();
        let queue = capture_queue(&order);
        assert_eq!(queue.len(), THUMB_CAP);
        assert_eq!(queue.front(), Some(&0));
        assert_eq!(queue.back(), Some(&(THUMB_CAP as WindowId - 1)));
    }
}

//! Alt+Tab switcher and new-popup focus handling.

use super::*;
use flamewm_api::switcher::SwitcherThumb;
use std::collections::VecDeque;

/// Most previews kept for one open switcher. Matches `MAX_THUMBS` in the X11 path
/// (`crates/flamewm-wm-x11/src/wm/switcher_thumbs.rs`).
const THUMB_CAP: usize = 24;

/// Per-open preview cache. Previews are captured lazily while the switcher is open and dropped
/// when it closes, so nothing stays resident while idle (docs/backlog/alttab.md, section 5).
#[derive(Default)]
pub(super) struct ThumbCache {
    /// Oldest first. A repeat `store` for a window moves it to the back.
    slots: VecDeque<(WindowId, SwitcherThumb)>,
}

impl ThumbCache {
    /// Keeps `thumb` as the preview for `id`. Wire references and malformed or empty buffers are
    /// refused, so the entry keeps its `title` and `app_id` row. Evicts the oldest preview at `THUMB_CAP`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn store(&mut self, id: WindowId, thumb: SwitcherThumb) -> bool {
        if thumb.is_reference() || !thumb.is_well_formed() || thumb.width == 0 || thumb.height == 0
        {
            return false;
        }
        self.slots.retain(|(known, _)| *known != id);
        if self.slots.len() == THUMB_CAP {
            self.slots.pop_front();
        }
        self.slots.push_back((id, thumb));
        true
    }

    /// The preview to publish for `id`, if one was stored for this open.
    pub(super) fn wire(&self, id: WindowId) -> Option<SwitcherThumb> {
        self.slots
            .iter()
            .find(|(known, _)| *known == id)
            .map(|(_, thumb)| thumb.clone())
    }
}

/// Alt+Tab state while the switcher is open: the window order, the highlighted entry and previews.
pub(super) struct Switcher {
    order: Vec<WindowId>,
    selected: usize,
    thumbs: ThumbCache,
}

/// What `emit_switcher` publishes: window order, highlighted index, and one optional preview per entry.
type SwitcherView = (Vec<WindowId>, usize, Vec<Option<SwitcherThumb>>);

impl Service {
    /// Alt+Tab: the first press opens the switcher on the visible windows with the second one
    /// highlighted; each further press advances the highlight. Nothing is focused until Alt is released.
    pub fn switcher_step(&self, state: &mut State) {
        let mut slot = self.switcher.borrow_mut();
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
                *slot = Some(Switcher {
                    order,
                    selected: 1,
                    thumbs: ThumbCache::default(),
                });
            }
        }
        let view: Option<SwitcherView> = slot.as_ref().map(|switcher| {
            let thumbs = switcher
                .order
                .iter()
                .map(|id| switcher.thumbs.wire(*id))
                .collect();
            (switcher.order.clone(), switcher.selected, thumbs)
        });
        drop(slot);
        self.emit_switcher(view);
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
}

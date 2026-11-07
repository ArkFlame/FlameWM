//! Alt+Tab switcher and new-popup focus handling.

use super::*;

/// Alt+Tab state while the switcher is open: the window order and the highlighted entry.
pub(super) struct Switcher {
    order: Vec<WindowId>,
    selected: usize,
}

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
                *slot = Some(Switcher { order, selected: 1 });
            }
        }
        let view = slot
            .as_ref()
            .map(|switcher| (switcher.order.clone(), switcher.selected));
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

    pub(super) fn emit_switcher(&self, view: Option<(Vec<WindowId>, usize)>) {
        let signal = match view {
            Some((order, selected)) => {
                let windows = self.mirror.borrow();
                let entries = order
                    .iter()
                    .map(|id| {
                        windows
                            .windows
                            .iter()
                            .find(|window| window.reference.id == *id)
                            .map_or(
                                SwitcherEntry {
                                    title: String::new(),
                                    app_id: String::new(),
                                },
                                |window| SwitcherEntry {
                                    title: window.title.clone(),
                                    app_id: window.app_id.as_str().to_owned(),
                                },
                            )
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

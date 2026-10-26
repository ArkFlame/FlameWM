//! Show-desktop: iconifies every visible normal window and restores exactly that set later, in
//! the same stacking order, with the previously active window focused again.
//!
//! The state machine and the selection rules are pure; the `Wm` glue at the bottom performs the
//! X calls and publishes `_NET_SHOWING_DESKTOP`.

use super::*;

/// One managed client as the selection rules see it, bottom-to-top in stacking order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StackEntry {
    pub window: Window,
    pub normal: bool,
    pub visible: bool,
    pub minimized: bool,
}

/// Windows to iconify, bottom-to-top: visible normal windows that are not already iconic.
pub(crate) fn select_to_hide(stack: &[StackEntry]) -> Vec<Window> {
    stack
        .iter()
        .filter(|entry| entry.normal && entry.visible && !entry.minimized)
        .map(|entry| entry.window)
        .collect()
}

/// Windows to restore from the saved set, keeping the saved order. A window that was closed or
/// restored by hand in the meantime is dropped.
pub(crate) fn select_to_restore(
    saved: &[Window],
    still_iconic: impl Fn(Window) -> bool,
) -> Vec<Window> {
    saved
        .iter()
        .copied()
        .filter(|&window| still_iconic(window))
        .collect()
}

/// The show-desktop toggle: `Some` while the desktop is shown and holds what to restore.
#[derive(Debug, Default)]
pub(crate) struct ShowDesktop {
    saved: Option<SavedDesktop>,
}

#[derive(Debug)]
struct SavedDesktop {
    windows: Vec<Window>,
    active: Option<Window>,
}

impl ShowDesktop {
    pub(crate) fn is_shown(&self) -> bool {
        self.saved.is_some()
    }

    pub(crate) fn begin(&mut self, windows: Vec<Window>, active: Option<Window>) {
        self.saved = Some(SavedDesktop { windows, active });
    }

    /// Ends the shown state and returns the saved windows (bottom-to-top) and active window.
    pub(crate) fn finish(&mut self) -> Option<(Vec<Window>, Option<Window>)> {
        self.saved.take().map(|saved| (saved.windows, saved.active))
    }
}

impl<C: Connection> Wm<'_, C> {
    /// Toggles show-desktop. Showing iconifies the visible normal windows; hiding restores them.
    pub(super) fn toggle_show_desktop(&mut self) -> Result<(), ReplyError> {
        if super::key_diag_enabled() {
            eprintln!(
                "flamewm: show-desktop toggle shown={}",
                self.show_desktop.is_shown()
            );
        }
        if self.show_desktop.is_shown() {
            return self.restore_desktop();
        }
        let stack = self.stack_entries()?;
        let hide = select_to_hide(&stack);
        self.show_desktop.begin(hide.clone(), self.active);
        for window in hide {
            self.minimize(window)?;
        }
        self.set_showing_desktop(true)
    }

    fn restore_desktop(&mut self) -> Result<(), ReplyError> {
        let Some((saved, active)) = self.show_desktop.finish() else {
            return Ok(());
        };
        let targets = select_to_restore(&saved, |window| {
            self.clients
                .get(&window)
                .is_some_and(|state| state.minimized)
        });
        // Bottom-to-top: each restore raises its window, so the top window ends on top.
        for window in targets {
            self.restore_minimized(window)?;
        }
        if let Some(window) = active.filter(|window| self.clients.contains_key(window)) {
            // Focus without restacking keeps the restored order intact.
            self.focus_hover(window)?;
        }
        self.set_showing_desktop(false)
    }

    /// Managed clients in stacking order, bottom-to-top.
    fn stack_entries(&self) -> Result<Vec<StackEntry>, ReplyError> {
        let tree = self.conn.query_tree(self.screen().root)?.reply()?;
        Ok(tree
            .children
            .iter()
            .filter_map(|frame| self.frame_to_client.get(frame).copied())
            .filter_map(|window| {
                let state = self.clients.get(&window)?;
                Some(StackEntry {
                    window,
                    normal: state.kind == WindowKind::Normal,
                    visible: state.visible_on(self.current_workspace),
                    minimized: state.minimized,
                })
            })
            .collect())
    }

    fn set_showing_desktop(&self, shown: bool) -> Result<(), ReplyError> {
        self.set_property32(
            self.screen().root,
            self.atoms.net_showing_desktop,
            AtomEnum::CARDINAL,
            &[u32::from(shown)],
        )
    }

    /// Clients currently fullscreen, whether or not they are iconic.
    pub(super) fn fullscreen_clients(&self) -> Vec<Window> {
        self.clients
            .iter()
            .filter(|(_, state)| state.is_fullscreen())
            .map(|(window, _)| *window)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(window: Window, normal: bool, visible: bool, minimized: bool) -> StackEntry {
        StackEntry {
            window,
            normal,
            visible,
            minimized,
        }
    }

    #[test]
    fn hide_keeps_stacking_order_and_skips_docks_hidden_and_iconic() {
        let stack = [
            entry(1, true, true, false),
            entry(2, false, true, false),
            entry(3, true, false, false),
            entry(4, true, true, true),
            entry(5, true, true, false),
        ];
        assert_eq!(select_to_hide(&stack), vec![1, 5]);
    }

    #[test]
    fn restore_keeps_saved_order_and_drops_windows_no_longer_iconic() {
        let saved = [7, 3, 9];
        let restored = select_to_restore(&saved, |window| window != 3);
        assert_eq!(restored, vec![7, 9]);
    }

    #[test]
    fn toggle_state_machine_shows_then_restores_once() {
        let mut desktop = ShowDesktop::default();
        assert!(!desktop.is_shown());
        desktop.begin(vec![1, 5], Some(5));
        assert!(desktop.is_shown());
        assert_eq!(desktop.finish(), Some((vec![1, 5], Some(5))));
        assert!(!desktop.is_shown());
        assert_eq!(desktop.finish(), None, "a second restore has nothing to do");
    }

    #[test]
    fn show_then_restore_round_trip_on_empty_desktop_is_a_no_op_set() {
        let mut desktop = ShowDesktop::default();
        desktop.begin(select_to_hide(&[]), None);
        assert_eq!(desktop.finish(), Some((Vec::new(), None)));
    }
}

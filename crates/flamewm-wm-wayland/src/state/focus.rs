use super::*;

impl State {
    /// Single focus authority: raises the window, activates it and moves keyboard focus.
    ///
    /// While a session lock is active no other client may take focus: the lock surface is the
    /// only thing allowed to receive input, or the lock could be bypassed by keystrokes.
    pub fn set_focus(&mut self, id: Option<WindowId>) {
        if self.pin_focus_to_lock() {
            return;
        }
        let previous = self.focused;
        // Only the top of the visible stack can change when one window is raised to the end of the
        // stacking vector: a hidden window (minimised or on another workspace) leaves the visible
        // order alone, and a window already on top keeps it. Comparing that one id is exactly the
        // full `visible_top_first` list comparison the two `Vec<WindowId>` copies used to do, with
        // no allocation and no O(windows) walk per focus event.
        let top_before = self.model.visible_top_first_iter().next();
        self.focused = id;
        if let Some(id) = id {
            self.model.raise(id);
        }
        let stack_changed = self.model.visible_top_first_iter().next() != top_before;
        if let Some(prev) = previous.filter(|prev| Some(*prev) != id) {
            self.apply_geometry(prev);
        }
        if let Some(id) = id {
            self.apply_geometry(id);
        }
        let keyboard = self.seat.get_keyboard();
        let surface = id.and_then(|id| self.window_wl_surface(id));
        if focus_needs_restack(previous, id, stack_changed) {
            self.sync_xwayland_stacking();
        }
        if let Some(keyboard) = keyboard {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, surface, serial);
        }
        if let Some(window) = id.and_then(|id| self.x11_window(id)).cloned() {
            self.set_x11_input_focus(&window);
        }
        self.request_redraw();
    }

    /// Keeps the keyboard on the lock surface while locked.
    ///
    /// Returns `true` when a lock is active, meaning the caller must not move focus. A lock
    /// surface can appear or be replaced at any time, so the focus is re-asserted rather than
    /// assumed still correct.
    #[must_use]
    pub fn pin_focus_to_lock(&mut self) -> bool {
        if !self.session_locked {
            return false;
        }
        // With no live locker the keyboard is cleared, never handed to a window behind the lock.
        let surface = self.lock_surface().map(|lock| lock.wl_surface().clone());
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        }
        true
    }

    /// Focuses the topmost visible window, or clears focus when the workspace is empty.
    pub fn refocus(&mut self) {
        let next = self.model.visible_top_first_iter().next();
        self.set_focus(next);
    }

    /// `focusFollowsMouse`: focuses the window under the pointer without raising it.
    ///
    /// Entry must not restack, or sweeping the pointer across a stack would shuffle it. X11's
    /// `focus_hover` calls `focus_with_stacking(client, false)` for exactly this reason, and this
    /// is the same move: keyboard focus only.
    pub fn focus_hover(&mut self, id: WindowId) {
        if self.pin_focus_to_lock() {
            return;
        }
        if self.focused == Some(id) {
            return;
        }
        self.focused = Some(id);
        let keyboard = self.seat.get_keyboard();
        let surface = self.window_wl_surface(id);
        if let Some(keyboard) = keyboard {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, surface, serial);
        }
        if let Some(window) = self.x11_window(id).cloned() {
            self.set_x11_input_focus(&window);
        }
        self.request_redraw();
    }
}

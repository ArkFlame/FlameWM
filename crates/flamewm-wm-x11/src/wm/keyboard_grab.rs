//! Keyboard move (`Alt+F7`) and resize (`Alt+F8`) for the X11 WM. The geometry is the pure
//! `flamewm_window_core::keyboard_move_resize` grab; this owns the keyboard grab, the pointer warp
//! and applying each step to the floating client.

use flamewm_window_core::keyboard_move_resize::{KeyboardGrab, Mode, Step, key_for_keysym};

use super::*;

/// Smallest size a keyboard resize reaches, matching the `Super+Alt`/resize-shortcut floor.
const MIN_SIZE: flamewm_api::Size = flamewm_api::Size::new(160, 96);

pub(super) struct ActiveKeyboardGrab {
    client: Window,
    grab: KeyboardGrab,
}

fn core_rect(rect: Rect) -> flamewm_api::Rect {
    flamewm_api::Rect::new(rect.x, rect.y, rect.width as i32, rect.height as i32)
}

fn legacy_rect(rect: flamewm_api::Rect) -> Rect {
    Rect::new(
        rect.x,
        rect.y,
        rect.width.max(1) as u32,
        rect.height.max(1) as u32,
    )
}

impl<C: Connection> Wm<'_, C> {
    pub(super) fn begin_keyboard_move(&mut self) -> Result<(), ReplyError> {
        self.begin_keyboard_grab(Mode::Move)
    }

    pub(super) fn begin_keyboard_resize(&mut self) -> Result<(), ReplyError> {
        self.begin_keyboard_grab(Mode::Resize)
    }

    /// Grabs the keyboard for the active floating window; a refused grab leaves the window alone.
    fn begin_keyboard_grab(&mut self, mode: Mode) -> Result<(), ReplyError> {
        self.kbd_pending = None;
        let Some(client) = self.active else {
            return Ok(());
        };
        let Some(state) = self.clients.get(&client) else {
            return Ok(());
        };
        if self.kbd_grab.is_some() || !state.is_floating() {
            return Ok(());
        }
        let outer = core_rect(state.outer);
        let work = core_rect(self.work_area_for_rect(state.outer));
        let root = self.screen().root;
        let reply = self
            .conn
            .grab_keyboard(false, root, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?
            .reply()?;
        if u8::from(reply.status) != u8::from(GrabStatus::SUCCESS) {
            // The Alt+F7/F8 passive grab still holds the keyboard until its key is released.
            self.kbd_pending = Some(mode);
            return Ok(());
        }
        let grab = KeyboardGrab::begin(mode, outer, work, MIN_SIZE);
        let centre = grab.pointer();
        self.conn.warp_pointer(
            x11rb::NONE,
            root,
            0,
            0,
            0,
            0,
            centre.x as i16,
            centre.y as i16,
        )?;
        self.kbd_grab = Some(ActiveKeyboardGrab { client, grab });
        Ok(())
    }

    pub(super) fn retry_pending_keyboard_grab(&mut self) -> Result<(), ReplyError> {
        match self.kbd_pending {
            Some(mode) => self.begin_keyboard_grab(mode),
            None => Ok(()),
        }
    }

    /// Routes a `KeyPress` while the grab is open. Arrows step, Return commits, Escape cancels.
    pub(super) fn keyboard_grab_key(&mut self, keycode: u8) -> Result<(), ReplyError> {
        let mapping = self.conn.get_keyboard_mapping(keycode, 1)?.reply()?;
        let keysym = mapping.keysyms.first().copied().unwrap_or(0);
        let Some(key) = key_for_keysym(keysym) else {
            return Ok(());
        };
        let Some(active) = self.kbd_grab.as_mut() else {
            return Ok(());
        };
        let client = active.client;
        let (rect, done) = match active.grab.step(key) {
            Step::Continue(rect) => (rect, false),
            Step::Commit(rect) | Step::Cancel(rect) => (rect, true),
        };
        if done {
            self.end_keyboard_grab(None)?;
        }
        self.apply_floating_rect(client, legacy_rect(rect))
    }

    /// Ends the grab and releases the keyboard. `window` limits it to that client; `None` ends any.
    /// Escape, Return, a pointer button, or the target being unmanaged all end it. The server also
    /// drops the grab when the WM connection closes, so no grab outlives the WM.
    pub(super) fn end_keyboard_grab(&mut self, window: Option<Window>) -> Result<(), ReplyError> {
        self.kbd_pending = None;
        let matches = self
            .kbd_grab
            .as_ref()
            .is_some_and(|active| window.is_none_or(|w| active.client == w));
        if matches {
            self.kbd_grab = None;
            self.conn.ungrab_keyboard(CURRENT_TIME)?;
        }
        Ok(())
    }
}

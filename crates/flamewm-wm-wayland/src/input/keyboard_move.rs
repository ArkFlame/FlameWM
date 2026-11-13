//! Alt+F7/F8 keyboard move and resize on Wayland. `flamewm_window_core::keyboard_move_resize`
//! owns the geometry and the key mapping; this begins the grab on the focused floating window,
//! routes the keys while it is open, and applies each step to the model. A pointer button or the
//! focused window disappearing ends the grab; Escape and Return end it through the state machine.

use flamewm_api::shortcuts::{ACTION_KEYBOARD_MOVE, ACTION_KEYBOARD_RESIZE};
use flamewm_window_core::keyboard_move_resize::{KeyboardGrab, Mode, Step, key_for_keysym};

use super::*;
use crate::model::{Mode as WindowMode, Rect};

/// Smallest size a keyboard resize reaches, matching the X11 engine.
const MIN_SIZE: flamewm_api::Size = flamewm_api::Size::new(160, 96);

/// Grab mode for a keyboard move/resize action, or `None` for any other action.
pub(super) fn grab_mode(action: &str) -> Option<Mode> {
    match action {
        ACTION_KEYBOARD_MOVE => Some(Mode::Move),
        ACTION_KEYBOARD_RESIZE => Some(Mode::Resize),
        _ => None,
    }
}

fn api_rect(rect: Rect) -> flamewm_api::Rect {
    flamewm_api::Rect::new(rect.x, rect.y, rect.w, rect.h)
}

fn model_rect(rect: flamewm_api::Rect) -> Rect {
    Rect::new(rect.x, rect.y, rect.width, rect.height)
}

/// Advances an open grab by one keysym. `None` when the keysym is not a grab key. The returned
/// step is `Some` for a grab key; `Commit` and `Cancel` close the grab.
pub(super) fn step_for_keysym(grab: &mut KeyboardGrab, keysym: u32) -> Option<Step> {
    key_for_keysym(keysym).map(|key| grab.step(key))
}

impl State {
    /// Starts a keyboard move or resize of the focused window. Tiled, maximised, fullscreen and
    /// unfocused states are no-ops, as on X11.
    pub(super) fn begin_keyboard_move(&mut self, mode: Mode) {
        if self.kbd_grab.is_some() {
            return;
        }
        let Some(id) = self.focused else {
            return;
        };
        let Some(window) = self.model.get(id) else {
            return;
        };
        if window.mode != WindowMode::Normal {
            return;
        }
        let rect = window.rect;
        self.refresh_dock_struts();
        let work = self.work_area();
        let grab = KeyboardGrab::begin(mode, api_rect(rect), api_rect(work), MIN_SIZE);
        self.kbd_grab = Some((id, grab));
    }

    /// Ends an open keyboard grab without changing the window. Called on pointer buttons.
    pub(super) fn end_keyboard_move(&mut self) {
        self.kbd_grab = None;
    }

    /// Routes a key press while the grab is open. Returns true when the keysym was a grab key.
    pub(super) fn keyboard_grab_key(&mut self, keysym: u32) -> bool {
        let Some((id, grab)) = self.kbd_grab.as_mut() else {
            return false;
        };
        let id = *id;
        let Some(step) = step_for_keysym(grab, keysym) else {
            return false;
        };
        let rect = match step {
            Step::Continue(rect) => rect,
            Step::Commit(rect) | Step::Cancel(rect) => {
                self.kbd_grab = None;
                rect
            }
        };
        if self.model.get(id).is_none() {
            self.kbd_grab = None;
            return true;
        }
        self.model.set_rect(id, model_rect(rect));
        self.apply_geometry(id);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_window_core::keyboard_move_resize::Key;

    const WORK: flamewm_api::Rect = flamewm_api::Rect::new(0, 0, 1280, 752);
    const START: flamewm_api::Rect = flamewm_api::Rect::new(200, 120, 402, 332);
    const XK_LEFT: u32 = 0xff51;
    const XK_RIGHT: u32 = 0xff53;
    const XK_RETURN: u32 = 0xff0d;
    const XK_ESCAPE: u32 = 0xff1b;
    const XK_A: u32 = 0x61;

    fn grab(mode: Mode) -> KeyboardGrab {
        KeyboardGrab::begin(mode, START, WORK, MIN_SIZE)
    }

    #[test]
    fn actions_map_to_grab_modes() {
        assert_eq!(grab_mode(ACTION_KEYBOARD_MOVE), Some(Mode::Move));
        assert_eq!(grab_mode(ACTION_KEYBOARD_RESIZE), Some(Mode::Resize));
        assert_eq!(grab_mode("TileLeft"), None);
    }

    #[test]
    fn arrows_step_then_return_commits_the_moved_rect() {
        let mut g = grab(Mode::Move);
        assert_eq!(
            step_for_keysym(&mut g, XK_RIGHT),
            Some(Step::Continue(flamewm_api::Rect::new(216, 120, 402, 332)))
        );
        assert_eq!(
            step_for_keysym(&mut g, XK_RETURN),
            Some(Step::Commit(flamewm_api::Rect::new(216, 120, 402, 332)))
        );
    }

    #[test]
    fn escape_cancels_to_the_begin_rect() {
        let mut g = grab(Mode::Move);
        step_for_keysym(&mut g, XK_LEFT);
        assert_eq!(
            step_for_keysym(&mut g, XK_ESCAPE),
            Some(Step::Cancel(START))
        );
    }

    #[test]
    fn non_grab_keysyms_are_not_consumed() {
        let mut g = grab(Mode::Resize);
        assert_eq!(step_for_keysym(&mut g, XK_A), None);
        assert_eq!(g.step(Key::Commit), Step::Commit(START));
    }

    #[test]
    fn rect_conversion_round_trips() {
        let rect = Rect::new(7, 9, 300, 200);
        assert_eq!(model_rect(api_rect(rect)), rect);
    }
}

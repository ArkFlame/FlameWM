//! Settings preview policy for `flamewm-lock --preview`: the lock UI with no
//! grabs, no authentication, no power actions and no screensaver. The preview
//! closes on the first input after a grace period, so the click that launched
//! it from Settings cannot dismiss it.
use std::time::Duration;

/// Input inside this window after the preview maps is ignored.
pub const PREVIEW_GRACE: Duration = Duration::from_millis(400);

/// X core pointer button bits in `XQueryPointer`'s mask (Button1..Button5).
const BUTTON_MASK: u32 = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11) | (1 << 12);

/// One poll of the pointer and keyboard, read without any grab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewInput {
    pub x: i32,
    pub y: i32,
    /// Pressed mouse buttons, already masked to the button bits.
    pub buttons: u32,
    /// `XQueryKeymap` bitmap: one bit per keycode.
    pub keys: [u8; 32],
}

impl PreviewInput {
    /// Builds a sample from the raw pointer mask, keeping only the button bits.
    #[must_use]
    pub fn new(x: i32, y: i32, mask: u32, keys: [u8; 32]) -> Self {
        Self {
            x,
            y,
            buttons: mask & BUTTON_MASK,
            keys,
        }
    }
}

/// `true` when the preview must close. Inside the grace window nothing dismisses.
/// After it: any change of pointer position (even 1 px), any newly pressed mouse
/// button, or any newly pressed key.
#[must_use]
pub fn dismisses(since_map: Duration, previous: &PreviewInput, current: &PreviewInput) -> bool {
    if since_map < PREVIEW_GRACE {
        return false;
    }
    current.x != previous.x
        || current.y != previous.y
        || current.buttons & !previous.buttons != 0
        || current
            .keys
            .iter()
            .zip(previous.keys.iter())
            .any(|(now, before)| now & !before != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: i32, y: i32) -> PreviewInput {
        PreviewInput::new(x, y, 0, [0; 32])
    }

    #[test]
    fn nothing_dismisses_inside_the_grace_window() {
        let before = at(10, 10);
        let moved = at(200, 10);
        assert!(!dismisses(Duration::ZERO, &before, &moved));
        assert!(!dismisses(
            PREVIEW_GRACE - Duration::from_millis(1),
            &before,
            &moved
        ));
    }

    #[test]
    fn one_pixel_motion_dismisses_after_grace() {
        assert!(dismisses(PREVIEW_GRACE, &at(10, 10), &at(11, 10)));
        assert!(dismisses(PREVIEW_GRACE, &at(10, 10), &at(10, 9)));
    }

    #[test]
    fn still_pointer_and_unchanged_keys_do_not_dismiss() {
        assert!(!dismisses(Duration::from_secs(5), &at(10, 10), &at(10, 10)));
    }

    #[test]
    fn newly_pressed_button_dismisses_but_release_of_unrelated_bits_does_not() {
        let pressed = PreviewInput::new(10, 10, 1 << 8, [0; 32]);
        assert!(dismisses(PREVIEW_GRACE, &at(10, 10), &pressed));
        // Modifier bits are not buttons: Shift-state changes alone do not dismiss.
        let shift = PreviewInput::new(10, 10, 1 << 0, [0; 32]);
        assert!(!dismisses(PREVIEW_GRACE, &at(10, 10), &shift));
    }

    #[test]
    fn newly_pressed_key_dismisses_but_a_key_held_since_before_grace_does_not() {
        let mut held = [0u8; 32];
        held[3] = 0b0000_0100;
        let mut pressed_later = held;
        pressed_later[5] = 0b0000_0001;
        let before = PreviewInput::new(0, 0, 0, held);
        assert!(!dismisses(PREVIEW_GRACE, &before, &before));
        assert!(dismisses(
            PREVIEW_GRACE,
            &before,
            &PreviewInput::new(0, 0, 0, pressed_later)
        ));
        // Releasing a held key is not a press.
        assert!(!dismisses(
            PREVIEW_GRACE,
            &before,
            &PreviewInput::new(0, 0, 0, [0; 32])
        ));
    }

    /// Esc is X keycode 9: bit 1 of byte 1. Pressing it escapes the Ribbons preview.
    #[test]
    fn escape_press_dismisses_the_screensaver_preview() {
        let mut escape = [0u8; 32];
        escape[1] = 1 << 1;
        let idle = at(0, 0);
        assert!(!dismisses(
            PREVIEW_GRACE - Duration::from_millis(1),
            &idle,
            &PreviewInput::new(0, 0, 0, escape)
        ));
        assert!(dismisses(
            PREVIEW_GRACE,
            &idle,
            &PreviewInput::new(0, 0, 0, escape)
        ));
    }
}

//! `wl_seat` keyboard input for the Wayland lock presenter.
//!
//! The lock has to accept a password to be unlockable, and on Wayland that means taking keyboard
//! events from the seat. This binds `wl_seat`, compiles the keymap the compositor sends, and hands
//! decoded keys to a callback. Pointer input is deliberately absent: a lock screen has nothing to
//! click before it is unlocked.

use wayland_client::WEnum;
use wayland_client::protocol::wl_keyboard::{
    Event as KeyboardEvent, KeyState, KeymapFormat, WlKeyboard,
};
use wayland_client::protocol::wl_seat::Capability;

use crate::keys::{KeyMapper, to_key};
use crate::state::Key;

/// Keys decoded from the seat.
#[derive(Default)]
pub struct SeatInput {
    pub keyboard: Option<WlKeyboard>,
    mapper: Option<KeyMapper>,
    /// The keymap arrived but did not compile: every keystroke would be lost.
    pub keymap_failed: bool,
    /// Modifier mask the compositor last reported, applied to the next key.
    mods: (u32, u32, u32),
}

impl SeatInput {
    /// Whether the seat advertised a keyboard capability.
    #[must_use]
    pub fn has_keyboard(&self) -> bool {
        self.keyboard.is_some()
    }

    /// Compiles the keymap the compositor sent, replacing any previous one.
    pub fn set_keymap(&mut self, keymap: &str) {
        self.mapper = KeyMapper::new(keymap);
        self.keymap_failed = self.mapper.is_none();
    }

    /// Decodes one key event, returning the lock key it names.
    ///
    /// Presses only: the lock has no use for a release, and treating both would double every
    /// character.
    pub fn key(&mut self, keycode: u32, state: KeyState) -> Option<Key> {
        if state != KeyState::Pressed {
            return None;
        }
        let mapper = self.mapper.as_mut()?;
        mapper.update_mods(self.mods.0, self.mods.1, self.mods.2);
        // `wl_keyboard` carries evdev keycodes; xkb keycodes are those plus 8.
        to_key(mapper, keycode + EVDEV_TO_XKB)
    }

    /// Records the modifier mask the compositor reported.
    pub fn set_mods(&mut self, depressed: u32, latched: u32, locked: u32) {
        self.mods = (depressed, latched, locked);
    }

    /// Feeds a `wl_keyboard` event to `on_key` for every key the lock understands.
    pub fn handle(&mut self, event: KeyboardEvent, mut on_key: impl FnMut(Key)) {
        match event {
            KeyboardEvent::Keymap { format, fd, size } => {
                if matches!(format, WEnum::Value(KeymapFormat::XkbV1)) {
                    if let Some(text) = read_keymap(fd, size) {
                        self.set_keymap(&text);
                    }
                }
            }
            KeyboardEvent::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                ..
            } => self.set_mods(mods_depressed, mods_latched, mods_locked),
            KeyboardEvent::Key { key, state, .. } => {
                let WEnum::Value(state) = state else {
                    return;
                };
                if let Some(key) = self.key(key, state) {
                    on_key(key);
                }
            }
            _ => {}
        }
    }
}

/// Offset from an evdev keycode, as `wl_keyboard` reports it, to an xkb keycode.
const EVDEV_TO_XKB: u32 = 8;

/// Whether `capabilities` include a keyboard.
#[must_use]
pub fn advertises_keyboard(capabilities: &[Capability]) -> bool {
    capabilities.contains(&Capability::Keyboard)
}

/// Reads the keymap string out of the descriptor the compositor sent.
///
/// The descriptor is a shared file description, so its offset may already be past the start when
/// it reaches us. Seeking to the beginning first is what makes the read reliable.
fn read_keymap(fd: std::os::fd::OwnedFd, size: u32) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::from(fd);
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut bytes = vec![0u8; size as usize];
    file.read_exact(&mut bytes).ok()?;
    // xkb keymaps may carry a trailing NUL, which the compiler rejects.
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wayland_client::protocol::wl_keyboard::Event;

    const KEYMAP: &str = r#"xkb_keymap {
        xkb_keycodes "minimal+aliases(qwerty)" {
            minimum = 8; maximum = 255;
            <AC01> = 38; <RTRN> = 36; <BKSP> = 22;
        };
        xkb_types "minimal" {
            type "ALPHABETIC" {
                modifiers = Shift;
                map[None] = Level1;
                map[Shift] = Level2;
            };
        };
        xkb_compat "minimal" { };
        xkb_symbols "minimal" {
            key <AC01> { [ a, A ] };
            key <RTRN> { [ Return ] };
            key <BKSP> { [ BackSpace ] };
        };
    };"#;

    #[test]
    fn a_press_produces_a_key() {
        let mut input = SeatInput::default();
        input.set_keymap(KEYMAP);
        assert_eq!(
            input.key(38 - EVDEV_TO_XKB, KeyState::Pressed),
            Some(Key::Char('a'))
        );
    }

    #[test]
    fn a_release_produces_nothing() {
        // Treating a release as a press would double every character typed.
        let mut input = SeatInput::default();
        input.set_keymap(KEYMAP);
        assert_eq!(input.key(38 - EVDEV_TO_XKB, KeyState::Released), None);
    }

    #[test]
    fn enter_and_backspace_decode() {
        let mut input = SeatInput::default();
        input.set_keymap(KEYMAP);
        assert_eq!(
            input.key(36 - EVDEV_TO_XKB, KeyState::Pressed),
            Some(Key::Enter)
        );
        assert_eq!(
            input.key(22 - EVDEV_TO_XKB, KeyState::Pressed),
            Some(Key::Backspace)
        );
    }

    #[test]
    fn shift_reaches_the_decoder() {
        let mut input = SeatInput::default();
        input.set_keymap(KEYMAP);
        input.set_mods(1, 0, 0);
        assert_eq!(
            input.key(38 - EVDEV_TO_XKB, KeyState::Pressed),
            Some(Key::Char('A'))
        );
    }

    #[test]
    fn a_key_with_no_keymap_yet_is_dropped_safely() {
        let mut input = SeatInput::default();
        assert_eq!(input.key(38 - EVDEV_TO_XKB, KeyState::Pressed), None);
    }

    #[test]
    fn a_broken_keymap_is_flagged() {
        let mut input = SeatInput::default();
        input.set_keymap("not a keymap");
        assert!(input.keymap_failed, "a broken keymap must be reported");
        assert_eq!(input.key(38 - EVDEV_TO_XKB, KeyState::Pressed), None);
    }

    #[test]
    fn events_are_routed_through_handle() {
        let mut input = SeatInput::default();
        input.set_keymap(KEYMAP);
        let mut seen = Vec::new();
        input.handle(
            Event::Modifiers {
                serial: 1,
                mods_depressed: 1,
                mods_latched: 0,
                mods_locked: 0,
                group: 0,
            },
            |key| seen.push(key),
        );
        input.handle(
            Event::Key {
                serial: 1,
                time: 0,
                key: 38 - EVDEV_TO_XKB,
                state: WEnum::Value(KeyState::Pressed),
            },
            |key| seen.push(key),
        );
        assert_eq!(seen, vec![Key::Char('A')], "the modifier applied first");
    }

    #[test]
    fn keyboard_capability_is_detected() {
        assert!(advertises_keyboard(&[Capability::Keyboard]));
        assert!(!advertises_keyboard(&[Capability::Pointer]));
    }
}

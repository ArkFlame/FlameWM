//! Wayland keyboard input for the lock: the keymap a compositor sends, applied to key events.
//!
//! On X11 the lock reads keycodes off its own display. A Wayland client has no such display: it
//! receives an `xkb_v1` keymap string from the seat and key events carrying raw keycodes, so the
//! keymap has to be compiled here. This module owns that translation — raw keycode plus modifier
//! state in, one [`Key`] the lock state machine understands out.

use xkbcommon::xkb;
use xkbcommon::xkb::Keysym;

use crate::state::Key;

/// Keeps the compiled keymap and the modifier state the compositor last reported.
pub struct KeyMapper {
    state: xkb::State,
}

impl KeyMapper {
    /// Compiles `keymap` as sent by `wl_keyboard`, or returns `None` when the string is not a
    /// usable keymap. A broken keymap must not silently drop every keystroke, so the caller
    /// reports it rather than pressing on with no input.
    #[must_use]
    pub fn new(keymap: &str) -> Option<Self> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let map = xkb::Keymap::new_from_string(
            &context,
            keymap.to_owned(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )?;
        Some(Self {
            state: xkb::State::new(&map),
        })
    }

    /// Applies the modifier mask the compositor sent, so `Shift` and `AltGr` affect the keysym.
    pub fn update_mods(&mut self, mods_depressed: u32, mods_latched: u32, mods_locked: u32) {
        self.state
            .update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, 0);
    }

    /// The character a press of `keycode` produces, if it produces exactly one.
    #[must_use]
    pub fn character(&self, keycode: u32) -> Option<char> {
        let text = self.state.key_get_utf8(keycode.into());
        let mut characters = text.chars();
        let first = characters.next()?;
        // Control characters (Escape, Backspace, Enter) are handled by keysym, not as text.
        if first.is_control() || characters.next().is_some() {
            return None;
        }
        Some(first)
    }

    /// Whether Caps Lock (the `Lock` modifier) is on for the state last applied.
    #[must_use]
    pub fn caps_lock(&self) -> bool {
        self.state
            .mod_name_is_active(xkb::MOD_NAME_CAPS, xkb::STATE_MODS_EFFECTIVE)
    }

    /// The keysym for `keycode` under the current modifiers.
    #[must_use]
    pub fn keysym(&self, keycode: u32) -> Keysym {
        self.state.key_get_one_sym(keycode.into())
    }
}

/// Translates one key press into the lock's key vocabulary.
///
/// Returns `None` for keys the lock has no use for, so callers can ignore them without a
/// match on every keysym.
#[must_use]
pub fn to_key(mapper: &KeyMapper, keycode: u32) -> Option<Key> {
    match mapper.keysym(keycode) {
        Keysym::BackSpace => Some(Key::Backspace),
        Keysym::Escape => Some(Key::Escape),
        Keysym::Return | Keysym::KP_Enter => Some(Key::Enter),
        Keysym::F1 => Some(Key::Function(1)),
        Keysym::F2 => Some(Key::Function(2)),
        Keysym::F3 => Some(Key::Function(3)),
        Keysym::F4 => Some(Key::Function(4)),
        _ => mapper.character(keycode).map(Key::Char),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but real keymap string, laid out like the one `wl_keyboard.keymap` sends.
    const KEYMAP: &str = r#"xkb_keymap {
        xkb_keycodes "minimal+aliases(qwerty)" {
            minimum = 8;
            maximum = 255;
            <ESC> = 9;
            <AE01> = 10;
            <BKSP> = 22;
            <RTRN> = 36;
            <AC01> = 38;
            <SPCE> = 65;
            <FK01> = 67;
            <CAPS> = 66;
        };
        xkb_types "minimal" {
            type "ONE_LEVEL" { modifiers = None; map[None] = Level1; };
            type "ALPHABETIC" {
                modifiers = Shift;
                map[None] = Level1;
                map[Shift] = Level2;
            };
        };
        xkb_compat "minimal" { };
        xkb_symbols "minimal" {
            key <ESC>  { [ Escape ] };
            key <AE01> { [ 1, exclam ] };
            key <BKSP> { [ BackSpace ] };
            key <RTRN> { [ Return ] };
            key <AC01> { [ a, A ] };
            key <SPCE> { [ space ] };
            key <FK01> { [ F1 ] };
            key <CAPS> { [ Caps_Lock ] };
            modifier_map Lock { <CAPS> };
        };
    };"#;

    fn mapper() -> KeyMapper {
        KeyMapper::new(KEYMAP).expect("the fixture keymap compiles")
    }

    #[test]
    fn a_letter_becomes_a_character() {
        let mapper = mapper();
        assert_eq!(to_key(&mapper, 38), Some(Key::Char('a')));
    }

    #[test]
    fn shift_changes_the_character() {
        let mut mapper = mapper();
        mapper.update_mods(1, 0, 0);
        assert_eq!(to_key(&mapper, 38), Some(Key::Char('A')));
    }

    #[test]
    fn named_keys_map_to_the_lock_vocabulary() {
        let mapper = mapper();
        assert_eq!(to_key(&mapper, 22), Some(Key::Backspace));
        assert_eq!(to_key(&mapper, 36), Some(Key::Enter));
        assert_eq!(to_key(&mapper, 9), Some(Key::Escape));
        assert_eq!(to_key(&mapper, 67), Some(Key::Function(1)));
    }

    #[test]
    fn an_unused_key_is_ignored() {
        let mapper = mapper();
        // Space has no meaning in a password field's control set here.
        assert_eq!(to_key(&mapper, 65), Some(Key::Char(' ')));
        // An unmapped keycode must not panic or invent a character.
        assert_eq!(to_key(&mapper, 200), None);
    }

    #[test]
    fn caps_lock_follows_the_lock_modifier() {
        let mut mapper = mapper();
        assert!(!mapper.caps_lock());
        // The Lock modifier is bit 1 in the standard xkb modifier order.
        mapper.update_mods(0, 0, 2);
        assert!(mapper.caps_lock());
        mapper.update_mods(0, 0, 0);
        assert!(!mapper.caps_lock());
    }

    #[test]
    fn a_broken_keymap_is_reported() {
        assert!(KeyMapper::new("not a keymap").is_none());
    }
}

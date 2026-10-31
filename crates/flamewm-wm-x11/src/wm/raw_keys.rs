//! `XInput2` raw-key feed for the WM.
//!
//! Passive root key grabs do not fire while another client holds an active keyboard grab
//! (a fullscreen game). Raw key events are delivered regardless of grabs, so the WM selects
//! `XI_RawKeyPress`/`XI_RawKeyRelease` on the master devices and dispatches only the escape
//! chords (Alt+Tab, show-desktop, fullscreen escape) from that feed. Every other shortcut stays
//! on the passive path, and a key handled by one feed is claimed so the other feed skips it.
//!
//! Pure parsing, modifier tracking and claim bookkeeping live here; the X calls are the `Wm`
//! glue at the bottom of the file.

use std::io::IoSlice;

use super::*;
use flamewm_api::shortcuts::{
    ACTION_FULLSCREEN_ESCAPE, ACTION_SHOW_DESKTOP, ACTION_SHOW_DESKTOP_CTRL_ALT,
    ACTION_TOGGLE_START_MENU, ACTION_WINDOW_SWITCH,
};
use flamewm_x11_desktop::shortcuts::MOD_ALT;

const GE_GENERIC_EVENT: u8 = 35;
const XI_RAW_KEY_PRESS: u16 = 13;
const XI_RAW_KEY_RELEASE: u16 = 14;
const XI_ALL_MASTER_DEVICES: u16 = 1;
const XI_SELECT_EVENTS: u8 = 46;

/// One raw key record: press or release, the hardware keycode, and the server time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawKey {
    pub press: bool,
    pub keycode: u8,
    pub time: u32,
}

/// Parses an `XInput2` `XIRawKeyPress`/`XIRawKeyRelease` `GenericEvent`. `opcode` is the `XInput`
/// major opcode from `QueryExtension`; anything else returns `None`.
pub(crate) fn parse_raw_key(bytes: &[u8], opcode: u8) -> Option<RawKey> {
    if bytes.len() < 20 || bytes[0] & 0x7f != GE_GENERIC_EVENT || bytes[1] != opcode {
        return None;
    }
    let evtype = u16::from_ne_bytes([bytes[8], bytes[9]]);
    let press = match evtype {
        XI_RAW_KEY_PRESS => true,
        XI_RAW_KEY_RELEASE => false,
        _ => return None,
    };
    let time = u32::from_ne_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let detail = u32::from_ne_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let keycode = u8::try_from(detail).ok()?;
    Some(RawKey {
        press,
        keycode,
        time,
    })
}

/// Modifier bits held right now, derived from raw press/release records so the WM knows the
/// chord state while a client owns the keyboard. Bits follow X's modifier map (Shift=1, Lock=2,
/// Control=4, Mod1=Alt=8, Mod4=Super=0x40, ...).
#[derive(Debug, Clone)]
pub(crate) struct ModifierState {
    bits_for_keycode: Vec<u16>,
    held: u16,
}

impl ModifierState {
    /// `keycodes` is the `GetModifierMapping` table: `keycodes_per_modifier` entries per modifier,
    /// eight modifiers in order.
    pub(crate) fn from_mapping(keycodes_per_modifier: usize, keycodes: &[u8]) -> Self {
        let mut bits_for_keycode = vec![0u16; 256];
        if keycodes_per_modifier > 0 {
            for (index, chunk) in keycodes.chunks(keycodes_per_modifier).take(8).enumerate() {
                for &keycode in chunk {
                    if keycode != 0 {
                        bits_for_keycode[usize::from(keycode)] |= 1 << index;
                    }
                }
            }
        }
        Self {
            bits_for_keycode,
            held: 0,
        }
    }

    /// Seeds the held state from a pointer query (the server reports the live modifier mask).
    pub(crate) fn seed(&mut self, state: u16) {
        self.held = state & 0x00ff;
    }

    /// Modifier bits held before the next record is applied.
    pub(crate) fn held(&self) -> u16 {
        self.held
    }

    /// Applies one raw record and returns the modifier state after it.
    pub(crate) fn apply(&mut self, key: RawKey) -> u16 {
        let bit = self.bits_for_keycode[usize::from(key.keycode)];
        if key.press {
            self.held |= bit;
        } else {
            self.held &= !bit;
        }
        self.held
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> u16 {
        self.held
    }
}

/// Single-slot claim on the last key event handled by either feed. A passive `KeyPress` and its
/// raw twin share one server timestamp and keycode, so whichever arrives second is skipped.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeyClaim {
    last: Option<(u32, u8)>,
}

impl KeyClaim {
    /// Consumes the claim when the event was already handled by the other feed.
    pub(crate) fn take_if_claimed(&mut self, time: u32, keycode: u8) -> bool {
        if self.last == Some((time, keycode)) {
            self.last = None;
            true
        } else {
            false
        }
    }

    pub(crate) fn claim(&mut self, time: u32, keycode: u8) {
        self.last = Some((time, keycode));
    }
}

/// Actions the raw feed dispatches. Everything else is left to the passive grabs.
pub(crate) fn is_escape_chord(action: &str) -> bool {
    matches!(
        action,
        ACTION_WINDOW_SWITCH
            | ACTION_SHOW_DESKTOP
            | ACTION_SHOW_DESKTOP_CTRL_ALT
            | ACTION_FULLSCREEN_ESCAPE
    )
}

/// Raw XI events can arrive after a keyboard grab dropped modifier releases. Use the X
/// server's current pointer state as authority, ignoring lock bits and held mouse buttons.
fn show_desktop_chord_is_down(action: &str, server_state: u16) -> bool {
    use flamewm_x11_desktop::shortcuts::{MOD_CONTROL, MOD_LOCK, MOD_NUM_LOCK, MOD_SUPER};
    let held = (server_state & 0x00ff) & !(MOD_LOCK | MOD_NUM_LOCK);
    match action {
        ACTION_SHOW_DESKTOP => held == MOD_SUPER,
        ACTION_SHOW_DESKTOP_CTRL_ALT => held == (MOD_CONTROL | MOD_ALT),
        _ => false,
    }
}

/// Per-WM raw-key state, present once the `XInput2` selection succeeded.
#[derive(Debug)]
pub(crate) struct RawKeys {
    pub opcode: u8,
    pub modifiers: ModifierState,
    pub claim: KeyClaim,
}

fn request(opcode: u8, minor: u8, units: u16, body: &[u8]) -> Vec<u8> {
    let mut bytes = vec![opcode, minor];
    bytes.extend_from_slice(&units.to_ne_bytes());
    bytes.extend_from_slice(body);
    bytes
}

impl<C: Connection> Wm<'_, C> {
    /// Negotiates `XInput` 2.0 and selects raw key press/release on all master devices. A server
    /// without `XInput` leaves the feed off and the passive grabs keep working.
    pub(super) fn select_raw_keys(&mut self) -> Result<(), ReplyError> {
        let extension = self.conn.query_extension(b"XInputExtension")?.reply()?;
        if !extension.present {
            eprintln!("flamewm: XInput absent; raw escape keys disabled");
            return Ok(());
        }
        let opcode = extension.major_opcode;
        // No XIQueryVersion: a client that negotiates XI2 this way stops receiving raw key
        // records while another client holds a keyboard grab (verified live in Xephyr).

        let mask = (1u32 << XI_RAW_KEY_PRESS) | (1u32 << XI_RAW_KEY_RELEASE);
        let mut select_body = Vec::new();
        select_body.extend_from_slice(&self.screen().root.to_ne_bytes());
        select_body.extend_from_slice(&1u16.to_ne_bytes()); // num_masks
        select_body.extend_from_slice(&0u16.to_ne_bytes()); // pad
        select_body.extend_from_slice(&XI_ALL_MASTER_DEVICES.to_ne_bytes());
        select_body.extend_from_slice(&1u16.to_ne_bytes()); // mask_len in words
        select_body.extend_from_slice(&mask.to_ne_bytes());
        let select = request(opcode, XI_SELECT_EVENTS, 5, &select_body);
        self.conn
            .send_request_without_reply(&[IoSlice::new(&select)], Vec::new())?
            .check()?;

        if super::key_diag_enabled() {
            eprintln!("flamewm: raw keys selected opcode={opcode} select={select:02x?}");
        }
        let mapping = self.conn.get_modifier_mapping()?.reply()?;
        let mut modifiers = ModifierState::from_mapping(
            usize::from(mapping.keycodes_per_modifier()),
            &mapping.keycodes,
        );
        let pointer = self.conn.query_pointer(self.screen().root)?.reply()?;
        modifiers.seed(u16::from(pointer.mask));
        self.raw_keys = Some(RawKeys {
            opcode,
            modifiers,
            claim: KeyClaim::default(),
        });
        Ok(())
    }

    /// Handles one raw record. Only press records that resolve to an escape chord dispatch.
    pub(super) fn handle_raw_event(&mut self, bytes: &[u8]) -> Result<(), ReplyError> {
        let root = self.screen().root;
        let Some(raw) = self.raw_keys.as_mut() else {
            return Ok(());
        };
        let Some(key) = parse_raw_key(bytes, raw.opcode) else {
            return Ok(());
        };
        let before = raw.modifiers.held();
        let state = raw.modifiers.apply(key);
        // The raw feed owns the bare Start-bound Super tap: no passive grab reports that key.
        if key.press {
            let start_binding = is_bare_state(before)
                && crate::shortcut_action_for_key(key.keycode, before).as_deref()
                    == Some(ACTION_TOGGLE_START_MENU);
            let outcome = self
                .super_tap
                .raw_press(start_binding, key.keycode, key.time);
            if outcome == SuperTapOutcome::Toggle {
                self.changes.start_menu_toggle = true;
            }
        } else {
            self.super_tap.release_super(key.keycode, key.time);
        }
        let switching = self.switcher.is_open();
        if !key.press {
            // Alt released: the open switcher commits its selection.
            if switching && state & MOD_ALT == 0 {
                return self.switcher_commit();
            }
            return Ok(());
        }
        if switching {
            if raw.claim.take_if_claimed(key.time, key.keycode) {
                return Ok(());
            }
            raw.claim.claim(key.time, key.keycode);
            return self.switcher_key(key.keycode, state);
        }
        let action = crate::shortcut_action_for_key(key.keycode, state);
        if super::key_diag_enabled() {
            eprintln!(
                "flamewm: raw keypress detail={} state={state:04x} action={}",
                key.keycode,
                action.as_deref().unwrap_or("none")
            );
        }
        let Some(action) = action else {
            return Ok(());
        };
        if !is_escape_chord(&action) {
            return Ok(());
        }
        if matches!(
            action.as_str(),
            ACTION_SHOW_DESKTOP | ACTION_SHOW_DESKTOP_CTRL_ALT
        ) {
            let Ok(cookie) = self.conn.query_pointer(root) else {
                return Ok(());
            };
            let Ok(pointer) = cookie.reply() else {
                // Fail closed: an X11 grab can leave our raw modifier cache stale.
                return Ok(());
            };
            if !show_desktop_chord_is_down(&action, u16::from(pointer.mask)) {
                return Ok(());
            }
        }
        if raw.claim.take_if_claimed(key.time, key.keycode) {
            return Ok(());
        }
        raw.claim.claim(key.time, key.keycode);
        self.dispatch_escape_chord(&action)
    }

    /// Passive-path hook: true when the raw feed already handled this `KeyPress`.
    pub(super) fn raw_claimed(&mut self, time: u32, keycode: u8) -> bool {
        self.raw_keys
            .as_mut()
            .is_some_and(|raw| raw.claim.take_if_claimed(time, keycode))
    }

    /// Records that the passive path handled an escape chord so the raw twin is skipped.
    pub(super) fn claim_escape_chord(&mut self, time: u32, keycode: u8) {
        if let Some(raw) = self.raw_keys.as_mut() {
            raw.claim.claim(time, keycode);
        }
    }

    /// True when another client holds the keyboard. Probes with a grab that is released at once
    /// when nobody holds it, so it never leaves a grab behind.
    pub(super) fn keyboard_grabbed_by_other(&self) -> Result<bool, ReplyError> {
        let root = self.screen().root;
        let reply = self
            .conn
            .grab_keyboard(false, root, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?
            .reply()?;
        if u8::from(reply.status) == u8::from(GrabStatus::SUCCESS) {
            self.conn.ungrab_keyboard(CURRENT_TIME)?;
            return Ok(false);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(evtype: u16, keycode: u32, time: u32, opcode: u8) -> Vec<u8> {
        let mut bytes = vec![0u8; 32];
        bytes[0] = GE_GENERIC_EVENT;
        bytes[1] = opcode;
        bytes[8..10].copy_from_slice(&evtype.to_ne_bytes());
        bytes[12..16].copy_from_slice(&time.to_ne_bytes());
        bytes[16..20].copy_from_slice(&keycode.to_ne_bytes());
        bytes
    }

    #[test]
    fn bare_d_and_stale_raw_modifiers_never_toggle_desktop() {
        use flamewm_x11_desktop::shortcuts::{MOD_CONTROL, MOD_LOCK, MOD_NUM_LOCK, MOD_SUPER};
        assert!(!show_desktop_chord_is_down(ACTION_SHOW_DESKTOP, 0));
        assert!(!show_desktop_chord_is_down(ACTION_SHOW_DESKTOP_CTRL_ALT, 0));
        assert!(!show_desktop_chord_is_down(ACTION_SHOW_DESKTOP, MOD_ALT));
        assert!(!show_desktop_chord_is_down(
            ACTION_SHOW_DESKTOP,
            MOD_SUPER | MOD_ALT
        ));
        assert!(show_desktop_chord_is_down(ACTION_SHOW_DESKTOP, MOD_SUPER));
        assert!(show_desktop_chord_is_down(
            ACTION_SHOW_DESKTOP,
            MOD_SUPER | MOD_LOCK
        ));
        assert!(show_desktop_chord_is_down(
            ACTION_SHOW_DESKTOP,
            MOD_SUPER | MOD_NUM_LOCK | 0x100
        ));
        assert!(show_desktop_chord_is_down(
            ACTION_SHOW_DESKTOP_CTRL_ALT,
            MOD_CONTROL | MOD_ALT
        ));
        assert!(!show_desktop_chord_is_down(
            ACTION_SHOW_DESKTOP_CTRL_ALT,
            MOD_CONTROL
        ));
    }

    #[test]
    fn parses_press_and_release_for_our_opcode_only() {
        let press = record(XI_RAW_KEY_PRESS, 23, 1234, 131);
        assert_eq!(
            parse_raw_key(&press, 131),
            Some(RawKey {
                press: true,
                keycode: 23,
                time: 1234
            })
        );
        let release = record(XI_RAW_KEY_RELEASE, 23, 1235, 131);
        assert!(!parse_raw_key(&release, 131).unwrap().press);
        assert_eq!(parse_raw_key(&press, 132), None, "other extension opcode");
        assert_eq!(
            parse_raw_key(&record(2, 23, 1, 131), 131),
            None,
            "XI_KeyPress is not raw"
        );
        assert_eq!(parse_raw_key(&press[..10], 131), None, "short record");
    }

    #[test]
    fn modifier_state_tracks_press_and_release_through_mapping() {
        // Eight modifiers, one keycode each: Shift=50, Control=37, Alt(Mod1)=64, Super(Mod4)=133.
        let mut keycodes = vec![0u8; 8];
        keycodes[0] = 50;
        keycodes[2] = 37;
        keycodes[3] = 64;
        keycodes[6] = 133;
        let mut state = ModifierState::from_mapping(1, &keycodes);
        assert_eq!(
            state.apply(RawKey {
                press: true,
                keycode: 64,
                time: 1
            }),
            0x0008
        );
        assert_eq!(
            state.apply(RawKey {
                press: true,
                keycode: 23,
                time: 2
            }),
            0x0008,
            "plain key"
        );
        assert_eq!(
            state.apply(RawKey {
                press: true,
                keycode: 37,
                time: 3
            }),
            0x000c
        );
        assert_eq!(
            state.apply(RawKey {
                press: false,
                keycode: 64,
                time: 4
            }),
            0x0004
        );
        assert_eq!(
            state.apply(RawKey {
                press: true,
                keycode: 133,
                time: 5
            }),
            0x0044
        );
        state.seed(0x0041);
        assert_eq!(state.state(), 0x0041, "seed keeps only the modifier byte");
    }

    #[test]
    fn claim_skips_the_twin_exactly_once() {
        let mut claim = KeyClaim::default();
        assert!(!claim.take_if_claimed(10, 23), "nothing claimed yet");
        claim.claim(10, 23);
        assert!(
            !claim.take_if_claimed(10, 24),
            "different key is not a twin"
        );
        assert!(
            claim.take_if_claimed(10, 23),
            "same time and key is the twin"
        );
        assert!(!claim.take_if_claimed(10, 23), "a claim is consumed once");
    }

    #[test]
    fn only_escape_chords_use_the_raw_feed() {
        assert!(is_escape_chord(ACTION_WINDOW_SWITCH));
        assert!(is_escape_chord(ACTION_SHOW_DESKTOP));
        assert!(is_escape_chord(ACTION_SHOW_DESKTOP_CTRL_ALT));
        assert!(is_escape_chord(ACTION_FULLSCREEN_ESCAPE));
        assert!(!is_escape_chord("Launcher"));
        assert!(!is_escape_chord("VolumeUp"));
    }
}

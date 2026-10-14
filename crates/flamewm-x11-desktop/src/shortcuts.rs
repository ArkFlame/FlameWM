//! Shortcut (hotkey) grab ownership for the X11 desktop.
//!
//! The pure parts (binding parsing, keysym lookup, staged/active bookkeeping,
//! commit and rollback ordering) are independent of the X connection so they
//! can be tested with fakes. The X side effects live behind [`GrabBackend`];
//! [`XGrabBackend`] is the only implementation that touches the server.

use std::collections::{BTreeMap, BTreeSet};

use flamewm_api::shortcuts::KeyBinding;
use flamewm_api::{ErrorCode, FlameError, FlameResult};
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::ErrorKind;
use x11rb::protocol::xproto::{ConnectionExt, GrabMode, ModMask, Window};

pub const MOD_SHIFT: u16 = 1 << 0;
pub const MOD_LOCK: u16 = 1 << 1;
pub const MOD_CONTROL: u16 = 1 << 2;
pub const MOD_ALT: u16 = 1 << 3;
pub const MOD_NUM_LOCK: u16 = 1 << 4;
pub const MOD_HYPER: u16 = 1 << 5;
pub const MOD_SUPER: u16 = 1 << 6;

/// Lock state bits that must not change which binding fires.
const LOCK_BITS: u16 = MOD_LOCK | MOD_NUM_LOCK;

/// Keysym values from the X11 keysym definitions (keysymdef.h).
const KEYSYM_NAMES: &[(&str, u32)] = &[
    ("space", 0x0020),
    ("BackSpace", 0xff08),
    ("Tab", 0xff09),
    ("Return", 0xff0d),
    ("Enter", 0xff0d),
    ("Pause", 0xff13),
    ("Escape", 0xff1b),
    ("Home", 0xff50),
    ("Left", 0xff51),
    ("Up", 0xff52),
    ("Right", 0xff53),
    ("Down", 0xff54),
    ("Page_Up", 0xff55),
    ("Page_Down", 0xff56),
    ("End", 0xff57),
    ("Print", 0xff61),
    ("Insert", 0xff63),
    ("Menu", 0xff67),
    ("Shift_L", 0xffe1),
    ("Shift_R", 0xffe2),
    ("Control_L", 0xffe3),
    ("Control_R", 0xffe4),
    ("Meta_L", 0xffe7),
    ("Meta_R", 0xffe8),
    ("Alt_L", 0xffe9),
    ("Alt_R", 0xffea),
    ("Super_L", 0xffeb),
    ("Super_R", 0xffec),
    ("Hyper_L", 0xffed),
    ("Hyper_R", 0xffee),
    ("Delete", 0xffff),
    ("F1", 0xffbe),
    ("F2", 0xffbf),
    ("F3", 0xffc0),
    ("F4", 0xffc1),
    ("F5", 0xffc2),
    ("F6", 0xffc3),
    ("F7", 0xffc4),
    ("F8", 0xffc5),
    ("F9", 0xffc6),
    ("F10", 0xffc7),
    ("F11", 0xffc8),
    ("F12", 0xffc9),
    ("XF86AudioLowerVolume", 0x1008_ff11),
    ("XF86AudioMute", 0x1008_ff12),
    ("XF86AudioRaiseVolume", 0x1008_ff13),
];

/// Resolves a keysym name (case-insensitive) or a single ASCII letter/digit.
#[must_use]
pub fn keysym_for_name(name: &str) -> Option<u32> {
    let mut chars = name.chars();
    let single = match (chars.next(), chars.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    };
    if let Some(only) = single.filter(char::is_ascii_alphanumeric) {
        return Some(u32::from(only.to_ascii_lowercase()));
    }
    KEYSYM_NAMES
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, keysym)| *keysym)
}

/// Maps a modifier token to its X modifier bit, or `None` when the token is
/// not a modifier name.
#[must_use]
pub fn modifier_bit(token: &str) -> Option<u16> {
    match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(MOD_CONTROL),
        "shift" => Some(MOD_SHIFT),
        "alt" | "meta" => Some(MOD_ALT),
        "super" => Some(MOD_SUPER),
        "hyper" => Some(MOD_HYPER),
        _ => None,
    }
}

/// Left-hand keysym used when a binding is a bare modifier such as `Super`.
fn modifier_only_keysym(token: &str) -> Option<u32> {
    let name = match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => "Control_L",
        "shift" => "Shift_L",
        "alt" => "Alt_L",
        "meta" => "Meta_L",
        "super" => "Super_L",
        "hyper" => "Hyper_L",
        _ => return None,
    };
    keysym_for_name(name)
}

/// A parsed binding: one keysym plus the modifier bits that must be held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyChord {
    pub keysym: u32,
    pub modifiers: u16,
}

/// Parses a normalized binding such as `Ctrl+Super+Left` or `Super`.
///
/// The last token is the key. A single modifier token is a bare-modifier key
/// (for example `Super` means the left Super key).
pub fn parse_binding(normalized: &str) -> FlameResult<KeyChord> {
    let tokens: Vec<&str> = normalized.split('+').map(str::trim).collect();
    let Some((key, prefix)) = tokens.split_last() else {
        return Err(FlameError::new(ErrorCode::InvalidArgument, "empty binding"));
    };
    let mut modifiers = 0u16;
    for token in prefix {
        let bit = modifier_bit(token).ok_or_else(|| {
            FlameError::new(
                ErrorCode::InvalidArgument,
                format!("unknown shortcut modifier: {token}"),
            )
        })?;
        modifiers |= bit;
    }
    let keysym = if prefix.is_empty() {
        modifier_only_keysym(key).or_else(|| keysym_for_name(key))
    } else {
        keysym_for_name(key)
    }
    .ok_or_else(|| {
        FlameError::new(
            ErrorCode::NotFound,
            format!("shortcut key is not recognised: {key}"),
        )
    })?;
    Ok(KeyChord { keysym, modifiers })
}

/// Keysym-to-keycode lookup. Implemented by [`KeyboardTable`] for the live
/// server and by fakes in tests.
pub trait KeycodeLookup {
    fn keycodes_for(&self, keysym: u32) -> Vec<u8>;
}

/// Snapshot of the server keyboard mapping (GetKeyboardMapping).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyboardTable {
    min_keycode: u8,
    keysyms_per_keycode: usize,
    keysyms: Vec<u32>,
}

impl KeyboardTable {
    #[must_use]
    pub fn new(min_keycode: u8, keysyms_per_keycode: u8, keysyms: Vec<u32>) -> Self {
        Self {
            min_keycode,
            keysyms_per_keycode: usize::from(keysyms_per_keycode),
            keysyms,
        }
    }
}

impl KeycodeLookup for KeyboardTable {
    fn keycodes_for(&self, keysym: u32) -> Vec<u8> {
        if self.keysyms_per_keycode == 0 {
            return Vec::new();
        }
        self.keysyms
            .chunks(self.keysyms_per_keycode)
            .enumerate()
            .filter(|(_, row)| row.contains(&keysym))
            .filter_map(|(index, _)| {
                let offset = u8::try_from(index).ok()?;
                self.min_keycode.checked_add(offset)
            })
            .collect()
    }
}

/// One root-window passive grab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Grab {
    pub keycode: u8,
    pub modifiers: u16,
}

/// A grab owned by an action. An optional grab may fail without failing the
/// commit; see [`ShortcutGrabs::commit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub action: String,
    pub grab: Grab,
    pub optional: bool,
}

/// XF86 media keys are optional: another client or desktop may already own
/// them, and losing one must not drop the rest of the hotkey set.
#[must_use]
pub fn is_optional_keysym(keysym: u32) -> bool {
    (0x1008_ff00..=0x1008_ffff).contains(&keysym)
}

/// Resolves a chord to every keycode that produces its keysym, expanded over
/// the NumLock and CapsLock variants so locked keyboards still fire.
pub fn resolve(
    action: &str,
    chord: KeyChord,
    lookup: &dyn KeycodeLookup,
) -> FlameResult<Vec<Entry>> {
    let keycodes = lookup.keycodes_for(chord.keysym);
    if keycodes.is_empty() {
        return Err(FlameError::new(
            ErrorCode::NotFound,
            format!("shortcut for {action} is not mapped on this keyboard"),
        ));
    }
    let variants = [0, MOD_LOCK, MOD_NUM_LOCK, MOD_LOCK | MOD_NUM_LOCK];
    let mut entries = Vec::new();
    for keycode in keycodes {
        for variant in variants {
            entries.push(Entry {
                action: action.to_owned(),
                grab: Grab {
                    keycode,
                    modifiers: chord.modifiers | variant,
                },
                optional: is_optional_keysym(chord.keysym),
            });
        }
    }
    Ok(entries)
}

/// X side effects for grabs. The real implementation is [`XGrabBackend`].
pub trait GrabBackend {
    fn grab(&mut self, grab: Grab) -> FlameResult<()>;
    fn ungrab(&mut self, grab: Grab);
}

/// Grab set owned by the desktop: the active grabs plus an optional staged set
/// that only becomes active through [`ShortcutGrabs::commit`].
#[derive(Debug, Default)]
pub struct ShortcutGrabs {
    active: Vec<Entry>,
    staged: Option<Vec<Entry>>,
    /// Set when a failed commit could not re-establish the previous grabs.
    degraded: bool,
}

impl ShortcutGrabs {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            active: Vec::new(),
            staged: None,
            degraded: false,
        }
    }

    #[must_use]
    pub fn active(&self) -> &[Entry] {
        &self.active
    }

    #[must_use]
    pub fn has_staged(&self) -> bool {
        self.staged.is_some()
    }

    /// Resolves every binding and stages the result. Never touches the active
    /// grabs. Any failure clears the staged set and leaves the active set as is.
    pub fn prepare(
        &mut self,
        desired: &BTreeMap<String, KeyBinding>,
        lookup: &dyn KeycodeLookup,
    ) -> FlameResult<()> {
        self.staged = None;
        let staged = build_staged(desired, lookup)?;
        self.staged = Some(staged);
        Ok(())
    }

    /// Ungrabs the active set, grabs the staged set, and then makes it active.
    /// A failed optional grab is logged and skipped. On a failed required grab
    /// the partial grabs are released, the previous set is re-grabbed, and the
    /// error is returned.
    pub fn commit(&mut self, backend: &mut dyn GrabBackend) -> FlameResult<()> {
        let next = self.staged.take().ok_or_else(|| {
            FlameError::new(ErrorCode::InvalidArgument, "no shortcut set is staged")
        })?;
        for entry in &self.active {
            backend.ungrab(entry.grab);
        }
        let mut granted: Vec<Entry> = Vec::new();
        let mut skipped: BTreeSet<String> = BTreeSet::new();
        for entry in next {
            if skipped.contains(&entry.action) {
                continue;
            }
            match backend.grab(entry.grab) {
                Ok(()) => granted.push(entry),
                Err(error) if entry.optional => {
                    eprintln!(
                        "flamewm: optional shortcut for {} was not grabbed: {}",
                        entry.action, error.message
                    );
                    // A binding is all-or-nothing across its NumLock/CapsLock variants.
                    let mut kept = Vec::with_capacity(granted.len());
                    for granted_entry in granted {
                        if granted_entry.action == entry.action {
                            backend.ungrab(granted_entry.grab);
                        } else {
                            kept.push(granted_entry);
                        }
                    }
                    granted = kept;
                    skipped.insert(entry.action);
                }
                Err(error) => {
                    for granted_entry in granted {
                        backend.ungrab(granted_entry.grab);
                    }
                    let restored = self.regrab_active(backend);
                    return Err(match restored {
                        Ok(()) => error,
                        Err(restore_error) => {
                            self.degraded = true;
                            FlameError::new(
                                error.code,
                                format!("{}; {}", error.message, restore_error.message),
                            )
                        }
                    });
                }
            }
        }
        self.active = granted;
        self.degraded = false;
        Ok(())
    }

    /// Discards the staged set. If a previous commit left the previous grabs
    /// missing, they are re-grabbed here.
    pub fn rollback(&mut self, backend: &mut dyn GrabBackend) {
        self.staged = None;
        if self.degraded {
            if self.regrab_active(backend).is_ok() {
                self.degraded = false;
            }
        }
    }

    /// Returns the action bound to a key press, ignoring NumLock/CapsLock state.
    #[must_use]
    pub fn action_for_key(&self, keycode: u8, state: u16) -> Option<&str> {
        let stripped = state & !LOCK_BITS;
        self.active
            .iter()
            .find(|entry| {
                entry.grab.keycode == keycode && (entry.grab.modifiers & !LOCK_BITS) == stripped
            })
            .map(|entry| entry.action.as_str())
    }

    fn regrab_active(&self, backend: &mut dyn GrabBackend) -> FlameResult<()> {
        let mut failure: Option<FlameError> = None;
        for entry in &self.active {
            if let Err(error) = backend.grab(entry.grab) {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

fn build_staged(
    desired: &BTreeMap<String, KeyBinding>,
    lookup: &dyn KeycodeLookup,
) -> FlameResult<Vec<Entry>> {
    let mut staged: Vec<Entry> = Vec::new();
    let mut owners: BTreeMap<Grab, String> = BTreeMap::new();
    for (action, binding) in desired {
        let Some(value) = binding.0.as_deref() else {
            continue;
        };
        let chord = parse_binding(value)?;
        for entry in resolve(action, chord, lookup)? {
            if let Some(other) = owners.insert(entry.grab, action.clone()) {
                return Err(FlameError::new(
                    ErrorCode::Conflict,
                    format!("shortcut for {action} conflicts with {other}"),
                ));
            }
            staged.push(entry);
        }
    }
    Ok(staged)
}

/// Grab backend that issues XGrabKey/XUngrabKey on the root window of the
/// connection the desktop already owns.
pub struct XGrabBackend<'a, C: Connection> {
    conn: &'a C,
    root: Window,
}

impl<'a, C: Connection> XGrabBackend<'a, C> {
    pub fn new(conn: &'a C, root: Window) -> Self {
        Self { conn, root }
    }
}

impl<C: Connection> GrabBackend for XGrabBackend<'_, C> {
    fn grab(&mut self, grab: Grab) -> FlameResult<()> {
        let cookie = self
            .conn
            .grab_key(
                false,
                self.root,
                ModMask::from(grab.modifiers),
                grab.keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .map_err(|error| FlameError::new(ErrorCode::IoFailure, error.to_string()))?;
        match cookie.check() {
            Ok(()) => Ok(()),
            Err(ReplyError::X11Error(error)) if error.error_kind == ErrorKind::Access => {
                Err(FlameError::new(
                    ErrorCode::Conflict,
                    "shortcut is already grabbed by another client",
                ))
            }
            Err(error) => Err(FlameError::new(
                ErrorCode::Unavailable,
                format!("XGrabKey failed: {error}"),
            )),
        }
    }

    fn ungrab(&mut self, grab: Grab) {
        // Fire-and-forget: a failed ungrab leaves at most a stale grab that the
        // next commit replaces; there is no caller that can act on the error.
        let _ = self
            .conn
            .ungrab_key(grab.keycode, self.root, ModMask::from(grab.modifiers));
        let _ = self.conn.flush();
    }
}

/// Reads the server keyboard mapping for the full keycode range.
pub fn read_keyboard_table<C: Connection>(conn: &C) -> FlameResult<KeyboardTable> {
    let setup = conn.setup();
    let min = setup.min_keycode;
    let count = setup.max_keycode.saturating_sub(min).saturating_add(1);
    let reply = conn
        .get_keyboard_mapping(min, count)
        .map_err(|error| FlameError::new(ErrorCode::IoFailure, error.to_string()))?
        .reply()
        .map_err(|error| FlameError::new(ErrorCode::IoFailure, error.to_string()))?;
    Ok(KeyboardTable::new(
        min,
        reply.keysyms_per_keycode,
        reply.keysyms,
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// Keysym-to-keycode table with fixed keycodes for test keys.
    struct FakeLookup(BTreeMap<u32, Vec<u8>>);

    impl FakeLookup {
        fn standard() -> Self {
            let mut map = BTreeMap::new();
            map.insert(keysym_for_name("Super_L").unwrap(), vec![133]);
            map.insert(keysym_for_name("Left").unwrap(), vec![113]);
            map.insert(keysym_for_name("Right").unwrap(), vec![114]);
            map.insert(keysym_for_name("Up").unwrap(), vec![111]);
            map.insert(keysym_for_name("Down").unwrap(), vec![116]);
            map.insert(keysym_for_name("F4").unwrap(), vec![70]);
            map.insert(keysym_for_name("a").unwrap(), vec![38]);
            map.insert(keysym_for_name("A").unwrap(), vec![38]);
            Self(map)
        }
    }

    impl KeycodeLookup for FakeLookup {
        fn keycodes_for(&self, keysym: u32) -> Vec<u8> {
            self.0.get(&keysym).cloned().unwrap_or_default()
        }
    }

    /// Records held grabs and fails a configured grab to simulate BadAccess.
    #[derive(Default)]
    struct FakeBackend {
        held: BTreeSet<Grab>,
        fail_on: Option<Grab>,
        grab_calls: usize,
    }

    impl GrabBackend for FakeBackend {
        fn grab(&mut self, grab: Grab) -> FlameResult<()> {
            self.grab_calls += 1;
            if self.fail_on == Some(grab) {
                return Err(FlameError::new(
                    ErrorCode::Conflict,
                    "shortcut is already grabbed by another client",
                ));
            }
            self.held.insert(grab);
            Ok(())
        }

        fn ungrab(&mut self, grab: Grab) {
            self.held.remove(&grab);
        }
    }

    fn desired(pairs: &[(&str, &str)]) -> BTreeMap<String, KeyBinding> {
        pairs
            .iter()
            .map(|(action, binding)| ((*action).to_owned(), KeyBinding::assigned(*binding)))
            .collect()
    }

    fn base_grab(keycode: u8, modifiers: u16) -> Grab {
        Grab { keycode, modifiers }
    }

    #[test]
    fn unresolvable_key_name_is_rejected_with_typed_error() {
        let error = parse_binding("Ctrl+NoSuchKey").expect_err("unknown key must fail");
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[test]
    fn unknown_modifier_is_rejected_with_typed_error() {
        let error = parse_binding("Hyperdrive+Left").expect_err("unknown modifier must fail");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn keysym_missing_from_keyboard_table_is_rejected() {
        let empty = KeyboardTable::new(8, 2, vec![0, 0, 0, 0]);
        let mut grabs = ShortcutGrabs::new();
        let error = grabs
            .prepare(&desired(&[("WorkspaceLeft", "Ctrl+Super+Left")]), &empty)
            .expect_err("unmapped keysym must fail");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(!grabs.has_staged());
    }

    #[test]
    fn parse_resolves_modifiers_and_keysym() {
        let chord = parse_binding("Ctrl+Super+Left").expect("valid");
        assert_eq!(chord.keysym, keysym_for_name("Left").unwrap());
        assert_eq!(chord.modifiers, MOD_CONTROL | MOD_SUPER);
        let bare = parse_binding("Super").expect("bare modifier");
        assert_eq!(bare.keysym, keysym_for_name("Super_L").unwrap());
        assert_eq!(bare.modifiers, 0);
    }

    #[test]
    fn prepare_does_not_change_active_grabs() {
        let lookup = FakeLookup::standard();
        let mut backend = FakeBackend::default();
        let mut grabs = ShortcutGrabs::new();
        grabs
            .prepare(&desired(&[("WindowClose", "Alt+F4")]), &lookup)
            .unwrap();
        grabs.commit(&mut backend).unwrap();
        let active_before = grabs.active().to_vec();

        grabs
            .prepare(&desired(&[("WorkspaceLeft", "Ctrl+Super+Left")]), &lookup)
            .unwrap();

        assert!(grabs.has_staged());
        assert_eq!(grabs.active(), active_before.as_slice());
        assert!(backend.held.contains(&base_grab(70, MOD_ALT)));
        assert_eq!(backend.grab_calls, 4, "prepare must not issue grabs");
        assert_eq!(
            grabs.action_for_key(70, MOD_ALT),
            Some("WindowClose"),
            "active binding still fires until commit"
        );
    }

    #[test]
    fn failed_prepare_keeps_previous_staged_out_and_active_intact() {
        let lookup = FakeLookup::standard();
        let mut backend = FakeBackend::default();
        let mut grabs = ShortcutGrabs::new();
        grabs
            .prepare(&desired(&[("WindowClose", "Alt+F4")]), &lookup)
            .unwrap();
        grabs.commit(&mut backend).unwrap();

        let error = grabs
            .prepare(&desired(&[("WorkspaceLeft", "Ctrl+Bogus")]), &lookup)
            .expect_err("bad binding");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(!grabs.has_staged());
        assert_eq!(grabs.action_for_key(70, MOD_ALT), Some("WindowClose"));
    }

    #[test]
    fn commit_swaps_grab_set_and_ungrabs_previous() {
        let lookup = FakeLookup::standard();
        let mut backend = FakeBackend::default();
        let mut grabs = ShortcutGrabs::new();
        grabs
            .prepare(&desired(&[("WindowClose", "Alt+F4")]), &lookup)
            .unwrap();
        grabs.commit(&mut backend).unwrap();

        grabs
            .prepare(&desired(&[("WindowClose", "Ctrl+Super+Up")]), &lookup)
            .unwrap();
        grabs.commit(&mut backend).unwrap();

        assert!(!backend.held.contains(&base_grab(70, MOD_ALT)));
        assert!(
            backend
                .held
                .contains(&base_grab(111, MOD_CONTROL | MOD_SUPER))
        );
        assert_eq!(
            grabs.action_for_key(111, MOD_CONTROL | MOD_SUPER),
            Some("WindowClose")
        );
        assert_eq!(grabs.action_for_key(70, MOD_ALT), None);
    }

    #[test]
    fn conflicting_grab_rolls_back_to_previous_set() {
        let lookup = FakeLookup::standard();
        let mut backend = FakeBackend::default();
        let mut grabs = ShortcutGrabs::new();
        grabs
            .prepare(&desired(&[("WindowClose", "Alt+F4")]), &lookup)
            .unwrap();
        grabs.commit(&mut backend).unwrap();
        let previous = grabs.active().to_vec();
        let held_before = backend.held.clone();

        // The second grab of the new set collides with another client.
        backend.fail_on = Some(base_grab(113, MOD_CONTROL | MOD_SUPER));
        grabs
            .prepare(&desired(&[("WorkspaceLeft", "Ctrl+Super+Left")]), &lookup)
            .unwrap();
        let error = grabs
            .commit(&mut backend)
            .expect_err("BadAccess must surface as conflict");
        assert_eq!(error.code, ErrorCode::Conflict);
        assert_eq!(grabs.active(), previous.as_slice(), "active set restored");
        assert_eq!(backend.held, held_before, "server grabs restored");

        grabs.rollback(&mut backend);
        assert!(!grabs.has_staged());
        assert_eq!(backend.held, held_before);
    }

    #[test]
    fn rollback_discards_staged_without_touching_grabs() {
        let lookup = FakeLookup::standard();
        let mut backend = FakeBackend::default();
        let mut grabs = ShortcutGrabs::new();
        grabs
            .prepare(&desired(&[("WindowClose", "Alt+F4")]), &lookup)
            .unwrap();
        grabs.commit(&mut backend).unwrap();
        let held_before = backend.held.clone();

        grabs
            .prepare(&desired(&[("WindowClose", "Ctrl+Super+Up")]), &lookup)
            .unwrap();
        grabs.rollback(&mut backend);

        assert!(!grabs.has_staged());
        assert_eq!(backend.held, held_before);
        assert_eq!(grabs.action_for_key(70, MOD_ALT), Some("WindowClose"));
        assert!(
            grabs.commit(&mut backend).is_err(),
            "nothing staged to commit"
        );
    }

    #[test]
    fn duplicate_binding_across_actions_is_rejected_at_prepare() {
        let lookup = FakeLookup::standard();
        let mut grabs = ShortcutGrabs::new();
        let error = grabs
            .prepare(
                &desired(&[("WindowClose", "Alt+F4"), ("WorkspaceUp", "Alt+F4")]),
                &lookup,
            )
            .expect_err("two actions on one chord must conflict");
        assert_eq!(error.code, ErrorCode::Conflict);
        assert!(!grabs.has_staged());
    }

    #[test]
    fn lock_state_does_not_prevent_dispatch() {
        let lookup = FakeLookup::standard();
        let mut backend = FakeBackend::default();
        let mut grabs = ShortcutGrabs::new();
        grabs
            .prepare(&desired(&[("WindowClose", "Alt+F4")]), &lookup)
            .unwrap();
        grabs.commit(&mut backend).unwrap();
        assert_eq!(
            grabs.action_for_key(70, MOD_ALT | MOD_NUM_LOCK),
            Some("WindowClose")
        );
        assert_eq!(grabs.action_for_key(70, 0), None);
    }

    #[test]
    fn optional_media_grab_conflict_is_skipped_and_required_bindings_commit() {
        let lookup = FakeLookup(BTreeMap::from([
            (keysym_for_name("Left").unwrap(), vec![113]),
            (keysym_for_name("F4").unwrap(), vec![70]),
            (0x1008_ff13, vec![123]),
        ]));
        let mut backend = FakeBackend {
            fail_on: Some(base_grab(123, 0)),
            ..FakeBackend::default()
        };
        let mut grabs = ShortcutGrabs::new();
        grabs
            .prepare(
                &desired(&[
                    ("WorkspaceLeft", "Ctrl+Super+Left"),
                    ("WindowClose", "Alt+F4"),
                    ("VolumeUp", "XF86AudioRaiseVolume"),
                ]),
                &lookup,
            )
            .unwrap();
        grabs
            .commit(&mut backend)
            .expect("optional conflict must not fail the commit");
        assert!(backend.held.contains(&base_grab(113, MOD_CONTROL | MOD_SUPER)));
        assert!(backend.held.contains(&base_grab(70, MOD_ALT)));
        assert!(
            !backend.held.iter().any(|grab| grab.keycode == 123),
            "every NumLock/CapsLock variant of the skipped binding is released"
        );
        assert_eq!(grabs.action_for_key(70, MOD_ALT), Some("WindowClose"));
        assert_eq!(grabs.action_for_key(123, 0), None);
        assert_eq!(grabs.active().len(), 8);
    }

    #[test]
    fn required_grab_conflict_still_rolls_back_even_with_optional_bindings() {
        let lookup = FakeLookup(BTreeMap::from([
            (keysym_for_name("F4").unwrap(), vec![70]),
            (0x1008_ff13, vec![123]),
        ]));
        let mut backend = FakeBackend {
            fail_on: Some(base_grab(70, MOD_ALT)),
            ..FakeBackend::default()
        };
        let mut grabs = ShortcutGrabs::new();
        grabs
            .prepare(
                &desired(&[
                    ("VolumeUp", "XF86AudioRaiseVolume"),
                    ("WindowClose", "Alt+F4"),
                ]),
                &lookup,
            )
            .unwrap();
        let error = grabs
            .commit(&mut backend)
            .expect_err("required conflict must fail");
        assert_eq!(error.code, ErrorCode::Conflict);
        assert!(grabs.active().is_empty());
        assert!(backend.held.is_empty(), "optional grab released on rollback");
    }

    #[test]
    fn keyboard_table_maps_keysym_to_keycode_by_column() {
        let table = KeyboardTable::new(8, 2, vec![0xff1b, 0, 0x61, 0x41, 0xff51, 0]);
        assert_eq!(table.keycodes_for(0xff1b), vec![8]);
        assert_eq!(table.keycodes_for(0x41), vec![9]);
        assert_eq!(table.keycodes_for(0xff51), vec![10]);
        assert!(table.keycodes_for(0x62).is_empty());
    }

    #[test]
    fn xf86_audio_keysyms_resolve_to_xkb_values() {
        assert_eq!(keysym_for_name("XF86AudioLowerVolume"), Some(0x1008_ff11));
        assert_eq!(keysym_for_name("XF86AudioMute"), Some(0x1008_ff12));
        assert_eq!(keysym_for_name("XF86AudioRaiseVolume"), Some(0x1008_ff13));
        assert_eq!(keysym_for_name("xf86audiomute"), Some(0x1008_ff12));
    }

    #[test]
    fn xf86_audio_bindings_parse_as_bare_keys_without_modifiers() {
        assert_eq!(
            parse_binding("XF86AudioRaiseVolume").unwrap(),
            KeyChord {
                keysym: 0x1008_ff13,
                modifiers: 0,
            }
        );
        assert_eq!(
            parse_binding("XF86AudioMute").unwrap(),
            KeyChord {
                keysym: 0x1008_ff12,
                modifiers: 0,
            }
        );
    }
}

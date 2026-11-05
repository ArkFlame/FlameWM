use super::*;

/// Modifier state for chord matching.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

/// A parsed binding such as `Ctrl+Super+Left`. `key` is the lowercase keysym name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    pub mods: Mods,
    pub key: String,
}

/// Whether a chord on `key` may fire with `mods`. A single printable character always needs
/// Ctrl, Alt or Super (Shift alone is not enough: `Shift+d` is how you type `D`), and an empty
/// key never matches anything.
pub(super) fn chord_can_fire(mods: Mods, key: &str) -> bool {
    // Binding a printable key without a command modifier would fire while the user types.
    !key.is_empty()
        && (!flamewm_platform::shortcuts::is_typing_key(key) || mods.ctrl || mods.alt || mods.logo)
}

/// Parses a normalized binding from `flamewm_platform::shortcuts`.
pub fn parse_chord(raw: &str) -> Option<Chord> {
    let mut mods = Mods::default();
    let mut key = None;
    for part in raw
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" => mods.alt = true,
            "shift" => mods.shift = true,
            "super" | "logo" | "win" | "meta" => mods.logo = true,
            other => key = Some(other.to_owned()),
        }
    }
    key.map(|key| Chord { mods, key })
}

/// Action lookup built from the shared shortcut bindings (action name -> chord).
#[derive(Debug, Default)]
pub struct Shortcuts {
    entries: Vec<(String, Chord)>,
}

impl Shortcuts {
    pub fn from_bindings(bindings: &BTreeMap<String, KeyBinding>) -> Self {
        let entries = bindings
            .iter()
            .filter_map(|(action, binding)| {
                let chord = parse_chord(binding.0.as_deref()?)?;
                // A chord that could fire on ordinary typing never enters the table.
                chord_can_fire(chord.mods, &chord.key).then(|| (action.clone(), chord))
            })
            .collect();
        Self { entries }
    }

    /// Action for an exact modifier+key match; `key` is the keysym name (any case).
    ///
    /// Caps Lock and Num Lock are not part of [`Mods`], so they never affect the match. A chord
    /// on a single printable key also needs Ctrl, Alt or Super, so a stray `d` binding left in a
    /// registry can never fire while the user types.
    pub fn action_for(&self, mods: Mods, key: &str) -> Option<&str> {
        let key = key.to_ascii_lowercase();
        if !chord_can_fire(mods, &key) {
            return None;
        }
        self.entries
            .iter()
            .find(|(_, chord)| chord.mods == mods && chord.key == key)
            .map(|(action, _)| action.as_str())
    }
}

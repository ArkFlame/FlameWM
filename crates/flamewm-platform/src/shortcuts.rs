use std::collections::BTreeMap;

use flamewm_api::shortcuts::{
    ACTION_FOCUS_DOWN, ACTION_FOCUS_LEFT, ACTION_FOCUS_RIGHT, ACTION_FOCUS_UP,
    ACTION_FULLSCREEN_ESCAPE, ACTION_LAUNCHER, ACTION_MOVE_WINDOW_DOWN, ACTION_MOVE_WINDOW_LEFT,
    ACTION_MOVE_WINDOW_RIGHT, ACTION_MOVE_WINDOW_UP, ACTION_SCREENSHOT_FULL,
    ACTION_SCREENSHOT_REGION, ACTION_SCREENSHOT_WINDOW, ACTION_SESSION_LOCK, ACTION_SHOW_DESKTOP,
    ACTION_SHOW_DESKTOP_CTRL_ALT, ACTION_TOGGLE_START_MENU, ACTION_WINDOW_CLOSE,
    ACTION_WINDOW_CYCLE_NEXT, ACTION_WINDOW_CYCLE_PREV, ACTION_WINDOW_FLOAT,
    ACTION_WINDOW_FULLSCREEN, ACTION_WINDOW_MAXIMIZE, ACTION_WINDOW_MINIMIZE,
    ACTION_WINDOW_RESIZE_GROW, ACTION_WINDOW_RESIZE_SHRINK, ACTION_WINDOW_SWITCH,
    ACTION_WORKSPACE_ACTIVATE, ACTION_WORKSPACE_DOWN, ACTION_WORKSPACE_LEFT, ACTION_WORKSPACE_MOVE,
    ACTION_WORKSPACE_RIGHT, ACTION_WORKSPACE_UP, KeyBinding,
};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

/// Media-key actions. Their ids are local to the platform shortcut registry; they
/// are dispatched through `PlatformHost::media_audio_key`.
pub const ACTION_OPEN_FILES_SUPER: &str = "OpenFilesSuper";
pub const ACTION_OPEN_FILES_CTRL_ALT: &str = "OpenFilesCtrlAlt";
pub const ACTION_OPEN_TERMINAL_SUPER: &str = "OpenTerminalSuper";
pub const ACTION_OPEN_TERMINAL_CTRL_ALT: &str = "OpenTerminalCtrlAlt";
pub const ACTION_OPEN_FINDER: &str = "OpenFinder";
pub const ACTION_OPEN_FINDER_CTRL_ALT: &str = "OpenFinderCtrlAlt";
pub const ACTION_OPEN_ASSISTANT_SUPER: &str = "OpenAssistantSuper";
pub const ACTION_OPEN_ASSISTANT_CTRL_ALT: &str = "OpenAssistantCtrlAlt";
/// Assistant chords. Their default is unassigned: the WM grabs them only while `assistant.enabled`
/// is set, via `services::shortcuts::effective_registry`.
pub const ASSISTANT_CHORDS: [(&str, &str); 2] = [
    (ACTION_OPEN_ASSISTANT_SUPER, "Super+C"),
    (ACTION_OPEN_ASSISTANT_CTRL_ALT, "Ctrl+Alt+C"),
];
pub const ACTION_VOLUME_UP: &str = "VolumeUp";
pub const ACTION_VOLUME_DOWN: &str = "VolumeDown";
pub const ACTION_VOLUME_MUTE: &str = "VolumeMute";

pub const ACTIONS: [&str; 60] = [
    ACTION_TOGGLE_START_MENU,
    ACTION_LAUNCHER,
    ACTION_WORKSPACE_LEFT,
    ACTION_WORKSPACE_RIGHT,
    ACTION_WORKSPACE_UP,
    ACTION_WORKSPACE_DOWN,
    "Workspace1",
    "Workspace2",
    "Workspace3",
    "Workspace4",
    "Workspace5",
    "Workspace6",
    "Workspace7",
    "Workspace8",
    "Workspace9",
    "MoveToWorkspace1",
    "MoveToWorkspace2",
    "MoveToWorkspace3",
    "MoveToWorkspace4",
    "MoveToWorkspace5",
    "MoveToWorkspace6",
    "MoveToWorkspace7",
    "MoveToWorkspace8",
    "MoveToWorkspace9",
    ACTION_FOCUS_LEFT,
    ACTION_FOCUS_RIGHT,
    ACTION_FOCUS_UP,
    ACTION_FOCUS_DOWN,
    ACTION_MOVE_WINDOW_LEFT,
    ACTION_MOVE_WINDOW_RIGHT,
    ACTION_MOVE_WINDOW_UP,
    ACTION_MOVE_WINDOW_DOWN,
    ACTION_WINDOW_CLOSE,
    ACTION_WINDOW_MINIMIZE,
    ACTION_WINDOW_MAXIMIZE,
    ACTION_WINDOW_FULLSCREEN,
    ACTION_WINDOW_FLOAT,
    ACTION_WINDOW_RESIZE_GROW,
    ACTION_WINDOW_RESIZE_SHRINK,
    ACTION_WINDOW_CYCLE_NEXT,
    ACTION_WINDOW_CYCLE_PREV,
    ACTION_WINDOW_SWITCH,
    ACTION_SHOW_DESKTOP,
    ACTION_SHOW_DESKTOP_CTRL_ALT,
    ACTION_FULLSCREEN_ESCAPE,
    ACTION_SESSION_LOCK,
    ACTION_SCREENSHOT_FULL,
    ACTION_SCREENSHOT_REGION,
    ACTION_SCREENSHOT_WINDOW,
    ACTION_VOLUME_UP,
    ACTION_VOLUME_DOWN,
    ACTION_VOLUME_MUTE,
    ACTION_OPEN_FILES_SUPER,
    ACTION_OPEN_FILES_CTRL_ALT,
    ACTION_OPEN_TERMINAL_SUPER,
    ACTION_OPEN_TERMINAL_CTRL_ALT,
    ACTION_OPEN_FINDER,
    ACTION_OPEN_FINDER_CTRL_ALT,
    ACTION_OPEN_ASSISTANT_SUPER,
    ACTION_OPEN_ASSISTANT_CTRL_ALT,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutRegistry {
    bindings: BTreeMap<String, KeyBinding>,
    defaults: BTreeMap<String, KeyBinding>,
}

impl Default for ShortcutRegistry {
    fn default() -> Self {
        let defaults = BTreeMap::from([
            (
                ACTION_TOGGLE_START_MENU.to_owned(),
                KeyBinding::assigned("Super_L"),
            ),
            (
                ACTION_WORKSPACE_LEFT.to_owned(),
                KeyBinding::assigned("Ctrl+Super+Left"),
            ),
            (
                ACTION_WORKSPACE_RIGHT.to_owned(),
                KeyBinding::assigned("Ctrl+Super+Right"),
            ),
            (
                ACTION_WORKSPACE_UP.to_owned(),
                KeyBinding::assigned("Ctrl+Super+Up"),
            ),
            (
                ACTION_WORKSPACE_DOWN.to_owned(),
                KeyBinding::assigned("Ctrl+Super+Down"),
            ),
            // Hyprland workspace binds: Super+N activates, Super+Shift+N moves the window.
            (
                ACTION_WORKSPACE_ACTIVATE[0].to_owned(),
                KeyBinding::assigned("Super+1"),
            ),
            (
                ACTION_WORKSPACE_ACTIVATE[1].to_owned(),
                KeyBinding::assigned("Super+2"),
            ),
            (
                ACTION_WORKSPACE_ACTIVATE[2].to_owned(),
                KeyBinding::assigned("Super+3"),
            ),
            (
                ACTION_WORKSPACE_ACTIVATE[3].to_owned(),
                KeyBinding::assigned("Super+4"),
            ),
            (
                ACTION_WORKSPACE_ACTIVATE[4].to_owned(),
                KeyBinding::assigned("Super+5"),
            ),
            (
                ACTION_WORKSPACE_ACTIVATE[5].to_owned(),
                KeyBinding::assigned("Super+6"),
            ),
            (
                ACTION_WORKSPACE_ACTIVATE[6].to_owned(),
                KeyBinding::assigned("Super+7"),
            ),
            (
                ACTION_WORKSPACE_ACTIVATE[7].to_owned(),
                KeyBinding::assigned("Super+8"),
            ),
            (
                ACTION_WORKSPACE_ACTIVATE[8].to_owned(),
                KeyBinding::assigned("Super+9"),
            ),
            (
                ACTION_WORKSPACE_MOVE[0].to_owned(),
                KeyBinding::assigned("Super+Shift+1"),
            ),
            (
                ACTION_WORKSPACE_MOVE[1].to_owned(),
                KeyBinding::assigned("Super+Shift+2"),
            ),
            (
                ACTION_WORKSPACE_MOVE[2].to_owned(),
                KeyBinding::assigned("Super+Shift+3"),
            ),
            (
                ACTION_WORKSPACE_MOVE[3].to_owned(),
                KeyBinding::assigned("Super+Shift+4"),
            ),
            (
                ACTION_WORKSPACE_MOVE[4].to_owned(),
                KeyBinding::assigned("Super+Shift+5"),
            ),
            (
                ACTION_WORKSPACE_MOVE[5].to_owned(),
                KeyBinding::assigned("Super+Shift+6"),
            ),
            (
                ACTION_WORKSPACE_MOVE[6].to_owned(),
                KeyBinding::assigned("Super+Shift+7"),
            ),
            (
                ACTION_WORKSPACE_MOVE[7].to_owned(),
                KeyBinding::assigned("Super+Shift+8"),
            ),
            (
                ACTION_WORKSPACE_MOVE[8].to_owned(),
                KeyBinding::assigned("Super+Shift+9"),
            ),
            (
                ACTION_FOCUS_LEFT.to_owned(),
                KeyBinding::assigned("Super+Left"),
            ),
            (
                ACTION_FOCUS_RIGHT.to_owned(),
                KeyBinding::assigned("Super+Right"),
            ),
            (ACTION_FOCUS_UP.to_owned(), KeyBinding::assigned("Super+Up")),
            (
                ACTION_FOCUS_DOWN.to_owned(),
                KeyBinding::assigned("Super+Down"),
            ),
            (
                ACTION_MOVE_WINDOW_LEFT.to_owned(),
                KeyBinding::assigned("Super+Shift+Left"),
            ),
            (
                ACTION_MOVE_WINDOW_RIGHT.to_owned(),
                KeyBinding::assigned("Super+Shift+Right"),
            ),
            (
                ACTION_MOVE_WINDOW_UP.to_owned(),
                KeyBinding::assigned("Super+Shift+Up"),
            ),
            (
                ACTION_MOVE_WINDOW_DOWN.to_owned(),
                KeyBinding::assigned("Super+Shift+Down"),
            ),
            (
                ACTION_WINDOW_CLOSE.to_owned(),
                KeyBinding::assigned("Super+Q"),
            ),
            (
                ACTION_WINDOW_FULLSCREEN.to_owned(),
                KeyBinding::assigned("Super+Shift+F"),
            ),
            (
                ACTION_WINDOW_FLOAT.to_owned(),
                KeyBinding::assigned("Super+V"),
            ),
            (
                ACTION_WINDOW_RESIZE_GROW.to_owned(),
                KeyBinding::assigned("Super+equal"),
            ),
            (
                ACTION_WINDOW_RESIZE_SHRINK.to_owned(),
                KeyBinding::assigned("Super+minus"),
            ),
            (
                ACTION_WINDOW_CYCLE_NEXT.to_owned(),
                KeyBinding::assigned("Super+J"),
            ),
            (
                ACTION_WINDOW_CYCLE_PREV.to_owned(),
                KeyBinding::assigned("Super+K"),
            ),
            (
                ACTION_SCREENSHOT_FULL.to_owned(),
                KeyBinding::assigned("Print"),
            ),
            (
                ACTION_SCREENSHOT_REGION.to_owned(),
                KeyBinding::assigned("Shift+Print"),
            ),
            (
                ACTION_SCREENSHOT_WINDOW.to_owned(),
                KeyBinding::assigned("Ctrl+Print"),
            ),
            (
                ACTION_WINDOW_MINIMIZE.to_owned(),
                KeyBinding::assigned("Super+M"),
            ),
            (ACTION_WINDOW_MAXIMIZE.to_owned(), KeyBinding::unassigned()),
            (
                ACTION_WINDOW_SWITCH.to_owned(),
                KeyBinding::assigned("Alt+Tab"),
            ),
            (
                ACTION_SHOW_DESKTOP.to_owned(),
                KeyBinding::assigned("Super+d"),
            ),
            (
                ACTION_SHOW_DESKTOP_CTRL_ALT.to_owned(),
                KeyBinding::assigned("Ctrl+Alt+d"),
            ),
            (
                ACTION_FULLSCREEN_ESCAPE.to_owned(),
                KeyBinding::assigned("Ctrl+Alt+Escape"),
            ),
            (
                ACTION_LAUNCHER.to_owned(),
                KeyBinding::assigned("Super+space"),
            ),
            (
                ACTION_OPEN_FILES_SUPER.to_owned(),
                KeyBinding::assigned("Super+E"),
            ),
            (
                ACTION_OPEN_FILES_CTRL_ALT.to_owned(),
                KeyBinding::assigned("Ctrl+Alt+E"),
            ),
            (
                ACTION_OPEN_TERMINAL_SUPER.to_owned(),
                KeyBinding::assigned("Super+Return"),
            ),
            (
                ACTION_OPEN_TERMINAL_CTRL_ALT.to_owned(),
                KeyBinding::assigned("Ctrl+Alt+T"),
            ),
            (
                ACTION_OPEN_FINDER.to_owned(),
                KeyBinding::assigned("Super+F"),
            ),
            (
                ACTION_OPEN_FINDER_CTRL_ALT.to_owned(),
                KeyBinding::assigned("Ctrl+Alt+F"),
            ),
            (
                ACTION_OPEN_ASSISTANT_SUPER.to_owned(),
                KeyBinding::unassigned(),
            ),
            (
                ACTION_OPEN_ASSISTANT_CTRL_ALT.to_owned(),
                KeyBinding::unassigned(),
            ),
            (
                ACTION_SESSION_LOCK.to_owned(),
                KeyBinding::assigned("Super+L"),
            ),
            (
                ACTION_VOLUME_UP.to_owned(),
                KeyBinding::assigned("XF86AudioRaiseVolume"),
            ),
            (
                ACTION_VOLUME_DOWN.to_owned(),
                KeyBinding::assigned("XF86AudioLowerVolume"),
            ),
            (
                ACTION_VOLUME_MUTE.to_owned(),
                KeyBinding::assigned("XF86AudioMute"),
            ),
        ]);
        Self {
            bindings: defaults.clone(),
            defaults,
        }
    }
}

impl ShortcutRegistry {
    #[must_use]
    pub fn bindings(&self) -> &BTreeMap<String, KeyBinding> {
        &self.bindings
    }

    #[must_use]
    pub fn binding(&self, action: &str) -> Option<&KeyBinding> {
        self.bindings.get(action)
    }

    pub fn clear(&mut self, action: &str) -> FlameResult<()> {
        self.require_action(action)?;
        self.bindings
            .insert(action.to_owned(), KeyBinding::unassigned());
        Ok(())
    }

    pub fn reset_one(&mut self, action: &str) -> FlameResult<()> {
        let default = self
            .defaults
            .get(action)
            .cloned()
            .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "unknown shortcut action"))?;
        self.bindings.insert(action.to_owned(), default);
        Ok(())
    }

    pub fn set(&mut self, action: &str, raw: &str) -> FlameResult<()> {
        self.require_action(action)?;
        let Some(normalized) = checked_binding(action, raw)? else {
            return self.clear(action);
        };
        // The key moves to `action`: a binding owns one key, so the previous owner is unassigned.
        while let Some(previous) = self.conflict_for(action, &normalized) {
            self.bindings.insert(previous, KeyBinding::unassigned());
        }
        self.bindings
            .insert(action.to_owned(), KeyBinding::assigned(normalized));
        Ok(())
    }

    pub fn replace_all(&mut self, desired: &BTreeMap<String, KeyBinding>) -> FlameResult<()> {
        for action in desired.keys() {
            self.require_action(action)?;
        }

        let mut candidate = BTreeMap::new();
        for action in ACTIONS {
            let binding = match desired.get(action).and_then(|binding| binding.0.as_deref()) {
                Some(raw) => checked_binding(action, raw)?
                    .map_or_else(KeyBinding::unassigned, KeyBinding::assigned),
                None => KeyBinding::unassigned(),
            };
            candidate.insert(action.to_owned(), binding);
        }

        // Duplicates never fail the set: the first action (registry order) keeps the key.
        let mut seen = std::collections::BTreeSet::new();
        for action in ACTIONS {
            let Some(binding) = candidate.get_mut(action) else {
                continue;
            };
            if binding
                .0
                .as_ref()
                .is_some_and(|v| !seen.insert(v.to_ascii_lowercase()))
            {
                *binding = KeyBinding::unassigned();
            }
        }

        self.bindings = candidate;
        Ok(())
    }

    #[must_use]
    pub fn conflict_for(&self, action: &str, normalized: &str) -> Option<String> {
        self.bindings.iter().find_map(|(other_action, binding)| {
            if other_action == action {
                return None;
            }
            binding.0.as_ref().and_then(|value| {
                (normalize(value).eq_ignore_ascii_case(normalized)).then(|| other_action.clone())
            })
        })
    }

    fn require_action(&self, action: &str) -> FlameResult<()> {
        if ACTIONS.contains(&action) {
            Ok(())
        } else {
            Err(FlameError::new(
                ErrorCode::NotFound,
                format!("unknown shortcut action: {action}"),
            ))
        }
    }
}

#[must_use]
pub fn normalize(raw: &str) -> String {
    raw.trim()
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("+")
}

/// Normalizes and validates a raw binding; `None` means the action is cleared.
fn checked_binding(action: &str, raw: &str) -> FlameResult<Option<String>> {
    let normalized = normalize(raw);
    if normalized.is_empty() || normalized.eq_ignore_ascii_case("Escape") {
        return Ok(None);
    }
    validate_binding(&normalized)?;
    if is_modifier_only(&normalized) && action != ACTION_TOGGLE_START_MENU {
        return Err(FlameError::new(
            ErrorCode::InvalidArgument,
            "modifier-only binding is permitted only for ToggleStartMenu",
        ));
    }
    if is_bare_typing_key(&normalized) {
        return Err(FlameError::new(
            ErrorCode::InvalidArgument,
            "a single letter or digit needs Ctrl, Alt or Super: bound bare it would fire while typing",
        ));
    }
    Ok(Some(normalized))
}

/// True for a binding whose key is one printable character bound without Ctrl, Alt or Super.
/// Such a binding grabs an ordinary typing key: `d` would toggle its action on every keystroke.
/// Shift does not count (Shift+d is how you type `D`), and bare modifier keys ("Super") are
/// not typing keys and are handled by [`is_modifier_only`].
#[must_use]
pub fn is_bare_typing_key(normalized: &str) -> bool {
    let tokens: Vec<&str> = normalized.split('+').map(str::trim).collect();
    let Some((key, prefix)) = tokens.split_last() else {
        return false;
    };
    let mut chars = key.chars();
    let single =
        matches!(chars.next(), Some(only) if only.is_ascii_graphic()) && chars.next().is_none();
    single
        && !prefix.iter().any(|token| {
            matches!(
                token.to_ascii_lowercase().as_str(),
                "ctrl" | "control" | "alt" | "super" | "meta" | "win" | "logo" | "hyper"
            )
        })
}

pub fn validate_binding(normalized: &str) -> FlameResult<()> {
    if normalized.len() > 64 || normalized.chars().any(char::is_control) {
        return Err(FlameError::new(
            ErrorCode::InvalidArgument,
            "shortcut binding is invalid",
        ));
    }
    if normalized.split('+').any(str::is_empty) {
        return Err(FlameError::new(
            ErrorCode::InvalidArgument,
            "shortcut contains an empty token",
        ));
    }
    Ok(())
}

#[must_use]
pub fn is_modifier_only(normalized: &str) -> bool {
    normalized.split('+').all(|token| {
        matches!(
            token.to_ascii_lowercase().as_str(),
            "ctrl" | "shift" | "alt" | "super" | "meta" | "hyper" | "super_l" | "super_r"
        )
    })
}

#[cfg(test)]
mod tests {

    #[test]
    fn launcher_is_a_registered_action_with_super_space_default() {
        assert_eq!(ACTIONS.len(), 60);
        assert!(ACTIONS.contains(&ACTION_LAUNCHER));
        let defaults = ShortcutRegistry::default();
        assert_eq!(
            defaults.binding(ACTION_LAUNCHER),
            Some(&KeyBinding::assigned("Super+space"))
        );
    }
    use super::*;

    #[test]
    fn escape_clears_binding_instead_of_becoming_a_binding() {
        let mut registry = ShortcutRegistry::default();
        registry
            .set(ACTION_WINDOW_CLOSE, "Escape")
            .expect("Escape means clear");
        assert_eq!(
            registry.binding(ACTION_WINDOW_CLOSE),
            Some(&KeyBinding::unassigned())
        );
    }

    #[test]
    fn duplicate_binding_moves_the_key_to_the_new_action() {
        let mut registry = ShortcutRegistry::default();
        registry
            .set(ACTION_WORKSPACE_RIGHT, "Ctrl+Super+Left")
            .expect("duplicate takes the key over");
        assert_eq!(
            registry.binding(ACTION_WORKSPACE_RIGHT),
            Some(&KeyBinding::assigned("Ctrl+Super+Left"))
        );
        assert_eq!(
            registry.binding(ACTION_WORKSPACE_LEFT),
            Some(&KeyBinding::unassigned())
        );
    }

    #[test]
    fn session_lock_defaults_to_super_l_and_conflicts_with_other_actions() {
        let mut registry = ShortcutRegistry::default();
        assert_eq!(
            registry.binding(ACTION_SESSION_LOCK),
            Some(&KeyBinding::assigned("Super+L"))
        );
        assert!(ACTIONS.contains(&ACTION_SESSION_LOCK));
        registry
            .set(ACTION_WINDOW_CLOSE, "super+l")
            .expect("Super+L moves from the lock action");
        assert_eq!(
            registry.binding(ACTION_SESSION_LOCK),
            Some(&KeyBinding::unassigned())
        );
        registry
            .set(ACTION_SESSION_LOCK, "Ctrl+Super+K")
            .expect("lock can be rebound");
        assert_eq!(
            registry.binding(ACTION_SESSION_LOCK),
            Some(&KeyBinding::assigned("Ctrl+Super+K"))
        );
    }

    #[test]
    fn config_without_session_lock_key_loads_the_default_lock_binding() {
        // A settings file written before SessionLock existed has no hotkey.SessionLock entry.
        let persisted =
            BTreeMap::from([(ACTION_WINDOW_CLOSE.to_owned(), "Ctrl+Super+Q".to_owned())]);
        let effective = crate::services::shortcuts::effective_registry(&persisted, false)
            .expect("legacy config is valid");
        assert_eq!(
            effective.binding(ACTION_SESSION_LOCK),
            Some(&KeyBinding::assigned("Super+L"))
        );
        assert_eq!(
            effective.binding(ACTION_WINDOW_CLOSE),
            Some(&KeyBinding::assigned("Ctrl+Super+Q"))
        );
    }

    #[test]
    fn screenshot_shortcuts_are_three_independent_configurable_actions() {
        let mut registry = ShortcutRegistry::default();
        for (action, binding) in [
            (ACTION_SCREENSHOT_FULL, "Print"),
            (ACTION_SCREENSHOT_REGION, "Shift+Print"),
            (ACTION_SCREENSHOT_WINDOW, "Ctrl+Print"),
        ] {
            assert_eq!(
                registry.binding(action),
                Some(&KeyBinding::assigned(binding))
            );
            assert!(ACTIONS.contains(&action));
        }
        assert_eq!(
            registry.conflict_for(ACTION_SCREENSHOT_REGION, "Print"),
            Some(ACTION_SCREENSHOT_FULL.to_owned())
        );
        registry.clear(ACTION_SCREENSHOT_REGION).unwrap();
        assert_eq!(
            registry.binding(ACTION_SCREENSHOT_REGION),
            Some(&KeyBinding::unassigned())
        );
        registry.reset_one(ACTION_SCREENSHOT_REGION).unwrap();
        assert_eq!(
            registry.binding(ACTION_SCREENSHOT_REGION),
            Some(&KeyBinding::assigned("Shift+Print"))
        );
    }

    #[test]
    fn media_keys_default_to_xf86_bindings_without_changing_existing_defaults() {
        let registry = ShortcutRegistry::default();
        assert_eq!(
            registry.binding(ACTION_VOLUME_UP),
            Some(&KeyBinding::assigned("XF86AudioRaiseVolume"))
        );
        assert_eq!(
            registry.binding(ACTION_VOLUME_DOWN),
            Some(&KeyBinding::assigned("XF86AudioLowerVolume"))
        );
        assert_eq!(
            registry.binding(ACTION_VOLUME_MUTE),
            Some(&KeyBinding::assigned("XF86AudioMute"))
        );
        assert_eq!(
            registry.binding(ACTION_SESSION_LOCK),
            Some(&KeyBinding::assigned("Super+L"))
        );
        assert_eq!(
            registry.binding(ACTION_WINDOW_CLOSE),
            Some(&KeyBinding::assigned("Super+Q"))
        );
    }

    #[test]
    fn bare_letter_or_digit_binding_is_rejected() {
        let mut registry = ShortcutRegistry::default();
        for binding in ["d", "D", "5", "Shift+q", "Shift+D"] {
            assert!(
                is_bare_typing_key(&normalize(binding)),
                "{binding} must be recognised as a typing key"
            );
            assert_eq!(
                registry
                    .set(ACTION_SHOW_DESKTOP, binding)
                    .expect_err("a bare typing key must be rejected")
                    .code,
                ErrorCode::InvalidArgument,
                "{binding}"
            );
        }
        // The chords and the modifier-only start binding stay valid.
        for binding in ["Super+d", "Ctrl+Alt+d", "Ctrl+Super+Left", "XF86AudioMute"] {
            assert!(!is_bare_typing_key(&normalize(binding)), "{binding}");
        }
        registry
            .set(ACTION_SHOW_DESKTOP, "Super+d")
            .expect("Super+d is valid");
        // A persisted set that lost its modifier is refused as a whole, so the WM grabs the
        // defaults instead of installing a bare-`d` grab.
        let persisted = BTreeMap::from([(ACTION_SHOW_DESKTOP.to_owned(), "d".to_owned())]);
        let error = crate::services::shortcuts::effective_registry(&persisted, false)
            .expect_err("a persisted bare letter must not load");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }

    #[test]
    fn show_desktop_defaults_keep_both_explicit_chords() {
        let registry = ShortcutRegistry::default();
        assert_eq!(
            registry.binding(ACTION_SHOW_DESKTOP),
            Some(&KeyBinding::assigned("Super+d"))
        );
        assert_eq!(
            registry.binding(ACTION_SHOW_DESKTOP_CTRL_ALT),
            Some(&KeyBinding::assigned("Ctrl+Alt+d"))
        );
        assert!(ACTIONS.contains(&ACTION_SHOW_DESKTOP));
        assert!(ACTIONS.contains(&ACTION_SHOW_DESKTOP_CTRL_ALT));
        // Both bindings resolve to their own action, so neither is unassigned by a conflict.
        let effective = crate::services::shortcuts::effective_registry(&BTreeMap::new(), false)
            .expect("defaults are valid");
        assert_eq!(effective, registry);
    }

    #[test]
    fn finder_has_super_f_and_ctrl_alt_f_and_fullscreen_moves_off_super_f() {
        let registry = ShortcutRegistry::default();
        assert_eq!(
            registry.binding(ACTION_OPEN_FINDER),
            Some(&KeyBinding::assigned("Super+F"))
        );
        assert_eq!(
            registry.binding(ACTION_OPEN_FINDER_CTRL_ALT),
            Some(&KeyBinding::assigned("Ctrl+Alt+F"))
        );
        assert_eq!(
            registry.binding(ACTION_WINDOW_FULLSCREEN),
            Some(&KeyBinding::assigned("Super+Shift+F"))
        );
        let effective = crate::services::shortcuts::effective_registry(&BTreeMap::new(), false)
            .expect("defaults are valid");
        assert_eq!(effective, registry);
    }

    #[test]
    fn stored_fullscreen_override_on_super_f_keeps_it_and_finder_keeps_ctrl_alt_f() {
        let mut persisted = BTreeMap::new();
        persisted.insert(ACTION_WINDOW_FULLSCREEN.to_owned(), "Super+F".to_owned());
        let effective = crate::services::shortcuts::effective_registry(&persisted, false)
            .expect("persisted override is valid");
        assert_eq!(
            effective.binding(ACTION_WINDOW_FULLSCREEN),
            Some(&KeyBinding::assigned("Super+F"))
        );
        // Stored user choices win over the new default: registry order keeps fullscreen on Super+F.
        assert_eq!(
            effective.binding(ACTION_OPEN_FINDER),
            Some(&KeyBinding::unassigned())
        );
        assert_eq!(
            effective.binding(ACTION_OPEN_FINDER_CTRL_ALT),
            Some(&KeyBinding::assigned("Ctrl+Alt+F"))
        );
    }
}

use std::collections::BTreeMap;

use flamewm_api::shortcuts::{
    ACTION_SESSION_LOCK, ACTION_TOGGLE_START_MENU, ACTION_WINDOW_CLOSE, ACTION_WINDOW_MAXIMIZE,
    ACTION_WINDOW_MINIMIZE, ACTION_WORKSPACE_DOWN, ACTION_WORKSPACE_LEFT, ACTION_WORKSPACE_RIGHT,
    ACTION_WORKSPACE_UP, ACTION_SCREENSHOT_FULL, ACTION_SCREENSHOT_REGION,
    ACTION_SCREENSHOT_WINDOW, KeyBinding,
};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

/// Media-key actions. Their ids are local to the platform shortcut registry; they
/// are dispatched through `PlatformHost::media_audio_key`.
pub const ACTION_OPEN_FILES_SUPER: &str = "OpenFilesSuper";
pub const ACTION_OPEN_FILES_CTRL_ALT: &str = "OpenFilesCtrlAlt";
pub const ACTION_OPEN_TERMINAL_SUPER: &str = "OpenTerminalSuper";
pub const ACTION_OPEN_TERMINAL_CTRL_ALT: &str = "OpenTerminalCtrlAlt";
pub const ACTION_VOLUME_UP: &str = "VolumeUp";
pub const ACTION_VOLUME_DOWN: &str = "VolumeDown";
pub const ACTION_VOLUME_MUTE: &str = "VolumeMute";

pub const ACTIONS: [&str; 19] = [
    ACTION_TOGGLE_START_MENU,
    ACTION_WORKSPACE_LEFT,
    ACTION_WORKSPACE_RIGHT,
    ACTION_WORKSPACE_UP,
    ACTION_WORKSPACE_DOWN,
    ACTION_WINDOW_CLOSE,
    ACTION_WINDOW_MINIMIZE,
    ACTION_WINDOW_MAXIMIZE,
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
            (
                ACTION_WINDOW_CLOSE.to_owned(),
                KeyBinding::assigned("Alt+F4"),
            ),
            (ACTION_SCREENSHOT_FULL.to_owned(), KeyBinding::assigned("Print")),
            (ACTION_SCREENSHOT_REGION.to_owned(), KeyBinding::assigned("Shift+Print")),
            (ACTION_SCREENSHOT_WINDOW.to_owned(), KeyBinding::assigned("Ctrl+Print")),
            (ACTION_WINDOW_MINIMIZE.to_owned(), KeyBinding::unassigned()),
            (ACTION_WINDOW_MAXIMIZE.to_owned(), KeyBinding::unassigned()),
            (ACTION_OPEN_FILES_SUPER.to_owned(), KeyBinding::assigned("Super+E")),
            (ACTION_OPEN_FILES_CTRL_ALT.to_owned(), KeyBinding::assigned("Ctrl+Alt+E")),
            (ACTION_OPEN_TERMINAL_SUPER.to_owned(), KeyBinding::assigned("Super+T")),
            (ACTION_OPEN_TERMINAL_CTRL_ALT.to_owned(), KeyBinding::assigned("Ctrl+Alt+T")),
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
        if let Some(conflict) = self.conflict_for(action, &normalized) {
            return Err(FlameError::new(
                ErrorCode::Conflict,
                format!("shortcut conflicts with {conflict}"),
            ));
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

        let mut seen = BTreeMap::<String, String>::new();
        for (action, binding) in &candidate {
            let Some(value) = binding.0.as_ref() else {
                continue;
            };
            if let Some(conflict) = seen.insert(value.to_ascii_lowercase(), action.clone()) {
                return Err(FlameError::new(
                    ErrorCode::Conflict,
                    format!("shortcut conflicts with {conflict}"),
                ));
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
    Ok(Some(normalized))
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
    fn duplicate_binding_is_rejected_without_changing_existing_binding() {
        let mut registry = ShortcutRegistry::default();
        let before = registry.binding(ACTION_WORKSPACE_RIGHT).cloned();
        let error = registry
            .set(ACTION_WORKSPACE_RIGHT, "Ctrl+Super+Left")
            .expect_err("duplicate must fail");
        assert_eq!(error.code, ErrorCode::Conflict);
        assert_eq!(registry.binding(ACTION_WORKSPACE_RIGHT).cloned(), before);
    }

    #[test]
    fn session_lock_defaults_to_super_l_and_conflicts_with_other_actions() {
        let mut registry = ShortcutRegistry::default();
        assert_eq!(
            registry.binding(ACTION_SESSION_LOCK),
            Some(&KeyBinding::assigned("Super+L"))
        );
        assert!(ACTIONS.contains(&ACTION_SESSION_LOCK));
        let error = registry
            .set(ACTION_WINDOW_CLOSE, "super+l")
            .expect_err("Super+L is taken by the lock action");
        assert_eq!(error.code, ErrorCode::Conflict);
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
        let effective = crate::services::shortcuts::effective_registry(&persisted)
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
            assert_eq!(registry.binding(action), Some(&KeyBinding::assigned(binding)));
            assert!(ACTIONS.contains(&action));
        }
        assert_eq!(registry.conflict_for(ACTION_SCREENSHOT_REGION, "Print"), Some(ACTION_SCREENSHOT_FULL.to_owned()));
        registry.clear(ACTION_SCREENSHOT_REGION).unwrap();
        assert_eq!(registry.binding(ACTION_SCREENSHOT_REGION), Some(&KeyBinding::unassigned()));
        registry.reset_one(ACTION_SCREENSHOT_REGION).unwrap();
        assert_eq!(registry.binding(ACTION_SCREENSHOT_REGION), Some(&KeyBinding::assigned("Shift+Print")));
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
            Some(&KeyBinding::assigned("Alt+F4"))
        );
    }

    #[test]
    fn modifier_only_is_allowed_for_start_but_not_window_action() {
        let mut registry = ShortcutRegistry::default();
        registry
            .set(ACTION_TOGGLE_START_MENU, "Super")
            .expect("Start can use modifier-only");
        assert_eq!(
            registry
                .set(ACTION_WINDOW_CLOSE, "Super")
                .expect_err("must reject")
                .code,
            ErrorCode::InvalidArgument
        );
    }
}

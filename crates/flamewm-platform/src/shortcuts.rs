use std::collections::BTreeMap;

use flamewm_api::shortcuts::{
    ACTION_SESSION_LOCK, ACTION_TOGGLE_START_MENU, ACTION_WINDOW_CLOSE, ACTION_WINDOW_MAXIMIZE,
    ACTION_WINDOW_MINIMIZE, ACTION_WORKSPACE_DOWN, ACTION_WORKSPACE_LEFT, ACTION_WORKSPACE_RIGHT,
    ACTION_WORKSPACE_UP, KeyBinding,
};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

pub const ACTIONS: [&str; 9] = [
    ACTION_TOGGLE_START_MENU,
    ACTION_WORKSPACE_LEFT,
    ACTION_WORKSPACE_RIGHT,
    ACTION_WORKSPACE_UP,
    ACTION_WORKSPACE_DOWN,
    ACTION_WINDOW_CLOSE,
    ACTION_WINDOW_MINIMIZE,
    ACTION_WINDOW_MAXIMIZE,
    ACTION_SESSION_LOCK,
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
            (ACTION_WINDOW_MINIMIZE.to_owned(), KeyBinding::unassigned()),
            (ACTION_WINDOW_MAXIMIZE.to_owned(), KeyBinding::unassigned()),
            (
                ACTION_SESSION_LOCK.to_owned(),
                KeyBinding::assigned("Super+L"),
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

    pub fn reset_all(&mut self) {
        self.bindings = self.defaults.clone();
    }

    pub fn set(&mut self, action: &str, raw: &str) -> FlameResult<()> {
        self.require_action(action)?;
        let normalized = normalize(raw);
        if normalized.is_empty() {
            return self.clear(action);
        }
        if normalized.eq_ignore_ascii_case("Escape") {
            return self.clear(action);
        }
        validate_binding(&normalized)?;
        if is_modifier_only(&normalized) && action != ACTION_TOGGLE_START_MENU {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "modifier-only binding is permitted only for ToggleStartMenu",
            ));
        }
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
            let binding = desired
                .get(action)
                .cloned()
                .unwrap_or_else(KeyBinding::unassigned);
            let normalized = match binding.0 {
                Some(value) => {
                    let normalized = normalize(&value);
                    if normalized.is_empty() || normalized.eq_ignore_ascii_case("Escape") {
                        KeyBinding::unassigned()
                    } else {
                        validate_binding(&normalized)?;
                        if is_modifier_only(&normalized) && action != ACTION_TOGGLE_START_MENU {
                            return Err(FlameError::new(
                                ErrorCode::InvalidArgument,
                                "modifier-only binding is permitted only for ToggleStartMenu",
                            ));
                        }
                        KeyBinding::assigned(normalized)
                    }
                }
                None => KeyBinding::unassigned(),
            };
            candidate.insert(action.to_owned(), normalized);
        }

        let mut seen = BTreeMap::<String, String>::new();
        for (action, binding) in &candidate {
            let Some(value) = binding.0.as_ref() else {
                continue;
            };
            let normalized = normalize(value).to_ascii_lowercase();
            if let Some(conflict) = seen.insert(normalized, action.clone()) {
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
    let mut saw = false;
    for token in normalized.split('+') {
        saw = true;
        if !matches!(
            token.to_ascii_lowercase().as_str(),
            "ctrl" | "shift" | "alt" | "super" | "meta" | "hyper" | "super_l" | "super_r"
        ) {
            return false;
        }
    }
    saw
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

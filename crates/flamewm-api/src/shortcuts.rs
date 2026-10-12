use std::collections::BTreeMap;

pub const ACTION_TOGGLE_START_MENU: &str = "ToggleStartMenu";
pub const ACTION_WORKSPACE_LEFT: &str = "WorkspaceLeft";
pub const ACTION_WORKSPACE_RIGHT: &str = "WorkspaceRight";
pub const ACTION_WORKSPACE_UP: &str = "WorkspaceUp";
pub const ACTION_WORKSPACE_DOWN: &str = "WorkspaceDown";
pub const ACTION_WINDOW_CLOSE: &str = "WindowClose";
pub const ACTION_WINDOW_MINIMIZE: &str = "WindowMinimize";
pub const ACTION_WINDOW_MAXIMIZE: &str = "WindowMaximize";
pub const ACTION_SESSION_LOCK: &str = "SessionLock";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyBinding(pub Option<String>);

impl KeyBinding {
    #[must_use]
    pub fn assigned(value: impl Into<String>) -> Self {
        let value = value.into();
        Self((!value.is_empty()).then_some(value))
    }

    #[must_use]
    pub const fn unassigned() -> Self {
        Self(None)
    }

    #[must_use]
    pub const fn is_assigned(&self) -> bool {
        self.0.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutSnapshot {
    pub revision: u64,
    pub bindings: BTreeMap<String, KeyBinding>,
}

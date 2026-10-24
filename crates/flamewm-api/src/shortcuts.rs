use std::collections::BTreeMap;

pub const ACTION_TOGGLE_START_MENU: &str = "ToggleStartMenu";
pub const ACTION_LAUNCHER: &str = "Launcher";
pub const ACTION_WORKSPACE_LEFT: &str = "WorkspaceLeft";
pub const ACTION_WORKSPACE_RIGHT: &str = "WorkspaceRight";
pub const ACTION_WORKSPACE_UP: &str = "WorkspaceUp";
pub const ACTION_WORKSPACE_DOWN: &str = "WorkspaceDown";
pub const ACTION_WINDOW_CLOSE: &str = "WindowClose";
pub const ACTION_WINDOW_MINIMIZE: &str = "WindowMinimize";
pub const ACTION_WINDOW_MAXIMIZE: &str = "WindowMaximize";
pub const ACTION_WINDOW_SWITCH: &str = "WindowSwitch";
pub const ACTION_SESSION_LOCK: &str = "SessionLock";
pub const ACTION_SCREENSHOT_FULL: &str = "ScreenshotFull";
pub const ACTION_SCREENSHOT_REGION: &str = "ScreenshotRegion";
pub const ACTION_SCREENSHOT_WINDOW: &str = "ScreenshotWindow";

/// Direction a workspace navigation shortcut moves in the two-row topology, or `None` for any
/// other action. The grid itself lives in [`crate::workspace`].
#[must_use]
pub fn workspace_direction(action: &str) -> Option<crate::workspace::GridDirection> {
    match action {
        ACTION_WORKSPACE_LEFT => Some(crate::workspace::GridDirection::Left),
        ACTION_WORKSPACE_RIGHT => Some(crate::workspace::GridDirection::Right),
        ACTION_WORKSPACE_UP => Some(crate::workspace::GridDirection::Up),
        ACTION_WORKSPACE_DOWN => Some(crate::workspace::GridDirection::Down),
        _ => None,
    }
}

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

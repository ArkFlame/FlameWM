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
pub const ACTION_SHOW_DESKTOP: &str = "ShowDesktop";
pub const ACTION_SHOW_DESKTOP_CTRL_ALT: &str = "ShowDesktopCtrlAlt";
pub const ACTION_FULLSCREEN_ESCAPE: &str = "FullscreenEscape";
pub const ACTION_SESSION_LOCK: &str = "SessionLock";
pub const ACTION_SCREENSHOT_FULL: &str = "ScreenshotFull";
pub const ACTION_SCREENSHOT_REGION: &str = "ScreenshotRegion";
pub const ACTION_SCREENSHOT_WINDOW: &str = "ScreenshotWindow";

/// Absolute workspace activation, Hyprland-style `Super+1..9`. Index is 0-based.
pub const ACTION_WORKSPACE_ACTIVATE: [&str; 9] = [
    "Workspace1",
    "Workspace2",
    "Workspace3",
    "Workspace4",
    "Workspace5",
    "Workspace6",
    "Workspace7",
    "Workspace8",
    "Workspace9",
];
/// Move the active window to an absolute workspace, `Super+Shift+1..9`.
pub const ACTION_WORKSPACE_MOVE: [&str; 9] = [
    "MoveToWorkspace1",
    "MoveToWorkspace2",
    "MoveToWorkspace3",
    "MoveToWorkspace4",
    "MoveToWorkspace5",
    "MoveToWorkspace6",
    "MoveToWorkspace7",
    "MoveToWorkspace8",
    "MoveToWorkspace9",
];
pub const ACTION_FOCUS_LEFT: &str = "FocusLeft";
pub const ACTION_FOCUS_RIGHT: &str = "FocusRight";
pub const ACTION_FOCUS_UP: &str = "FocusUp";
pub const ACTION_FOCUS_DOWN: &str = "FocusDown";
pub const ACTION_MOVE_WINDOW_LEFT: &str = "MoveWindowLeft";
pub const ACTION_MOVE_WINDOW_RIGHT: &str = "MoveWindowRight";
pub const ACTION_MOVE_WINDOW_UP: &str = "MoveWindowUp";
pub const ACTION_MOVE_WINDOW_DOWN: &str = "MoveWindowDown";
pub const ACTION_WINDOW_FULLSCREEN: &str = "WindowFullscreen";
pub const ACTION_WINDOW_FLOAT: &str = "WindowFloat";
pub const ACTION_WINDOW_CYCLE_NEXT: &str = "WindowCycleNext";
pub const ACTION_WINDOW_CYCLE_PREV: &str = "WindowCyclePrev";
pub const ACTION_WINDOW_RESIZE_GROW: &str = "WindowResizeGrow";
pub const ACTION_WINDOW_RESIZE_SHRINK: &str = "WindowResizeShrink";

/// Absolute workspace index addressed by a `WorkspaceN` action.
#[must_use]
pub fn workspace_activate_index(action: &str) -> Option<usize> {
    ACTION_WORKSPACE_ACTIVATE
        .iter()
        .position(|candidate| *candidate == action)
}

/// Absolute workspace index addressed by a `MoveToWorkspaceN` action.
#[must_use]
pub fn workspace_move_index(action: &str) -> Option<usize> {
    ACTION_WORKSPACE_MOVE
        .iter()
        .position(|candidate| *candidate == action)
}

/// Spatial focus direction requested by a `Focus*` action.
#[must_use]
pub fn focus_direction(action: &str) -> Option<crate::workspace::GridDirection> {
    match action {
        ACTION_FOCUS_LEFT => Some(crate::workspace::GridDirection::Left),
        ACTION_FOCUS_RIGHT => Some(crate::workspace::GridDirection::Right),
        ACTION_FOCUS_UP => Some(crate::workspace::GridDirection::Up),
        ACTION_FOCUS_DOWN => Some(crate::workspace::GridDirection::Down),
        _ => None,
    }
}

/// Spatial move direction requested by a `MoveWindow*` action.
#[must_use]
pub fn move_direction(action: &str) -> Option<crate::workspace::GridDirection> {
    match action {
        ACTION_MOVE_WINDOW_LEFT => Some(crate::workspace::GridDirection::Left),
        ACTION_MOVE_WINDOW_RIGHT => Some(crate::workspace::GridDirection::Right),
        ACTION_MOVE_WINDOW_UP => Some(crate::workspace::GridDirection::Up),
        ACTION_MOVE_WINDOW_DOWN => Some(crate::workspace::GridDirection::Down),
        _ => None,
    }
}

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

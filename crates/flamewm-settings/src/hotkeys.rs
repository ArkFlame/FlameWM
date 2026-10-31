//! Split from `lib.rs` (`hotkeys`); see crate docs.

use super::*;

/// One curated hotkey row: its markup slot, the shortcut action it edits,
/// its display label, and the optional node that shows its binding.
pub(crate) struct HotkeyRow {
    pub(crate) slot: &'static str,
    pub(crate) action: &'static str,
    pub(crate) label: &'static str,
    pub(crate) node: &'static str,
}

pub(crate) static HOTKEY_ROWS: [HotkeyRow; 9] = [
    HotkeyRow {
        slot: "start",
        action: flamewm_api::shortcuts::ACTION_TOGGLE_START_MENU,
        label: "Toggle Start menu",
        node: "hotkey-key-start",
    },
    HotkeyRow {
        slot: "left",
        action: flamewm_api::shortcuts::ACTION_WORKSPACE_LEFT,
        label: "Desktop left",
        node: "hotkey-key-left",
    },
    HotkeyRow {
        slot: "right",
        action: flamewm_api::shortcuts::ACTION_WORKSPACE_RIGHT,
        label: "Desktop right",
        node: "hotkey-key-right",
    },
    HotkeyRow {
        slot: "up",
        action: flamewm_api::shortcuts::ACTION_WORKSPACE_UP,
        label: "Desktop up",
        node: "hotkey-key-up",
    },
    HotkeyRow {
        slot: "down",
        action: flamewm_api::shortcuts::ACTION_WORKSPACE_DOWN,
        label: "Desktop down",
        node: "hotkey-key-down",
    },
    HotkeyRow {
        slot: "shot-full",
        action: flamewm_api::shortcuts::ACTION_SCREENSHOT_FULL,
        label: "Capture all screens",
        node: "hotkey-key-shot-full",
    },
    HotkeyRow {
        slot: "shot-region",
        action: flamewm_api::shortcuts::ACTION_SCREENSHOT_REGION,
        label: "Capture and annotate region",
        node: "hotkey-key-shot-region",
    },
    HotkeyRow {
        slot: "shot-window",
        action: flamewm_api::shortcuts::ACTION_SCREENSHOT_WINDOW,
        label: "Capture active window",
        node: "hotkey-key-shot-window",
    },
    HotkeyRow {
        slot: "lock",
        action: flamewm_api::shortcuts::ACTION_SESSION_LOCK,
        label: "Lock screen",
        node: "hotkey-key-lock",
    },
];

#[derive(Debug, Clone, Copy)]
pub(crate) enum HotkeyOperation {
    Capture,
    Clear,
    Reset,
}

/// Which setting the shared file chooser writes on completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChooserTarget {
    Wallpaper,
    LockBackground,
    Avatar,
    StartIcon,
}

impl ChooserTarget {
    /// The managed media family the selected file is copied into.
    pub(crate) fn media_kind(self) -> MediaKind {
        match self {
            Self::Wallpaper => MediaKind::DesktopWallpaper,
            Self::LockBackground => MediaKind::LockWallpaper,
            Self::Avatar => MediaKind::Avatar,
            Self::StartIcon => MediaKind::StartIcon,
        }
    }
}

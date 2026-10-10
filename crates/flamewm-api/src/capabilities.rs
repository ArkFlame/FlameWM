//! Stable capability names exposed over Flame Control.

pub mod names {
    pub const PANEL_MULTI_OUTPUT: &str = "panel.multi-output";
    pub const PANEL_PER_OUTPUT_SCALE: &str = "panel.per-output-scale";
    pub const TASKBAR_ORDERING: &str = "taskbar.ordering";
    pub const TASKBAR_DRAG_REORDER: &str = "taskbar.drag-reorder";
    pub const TASKBAR_PINNED_RUNNING: &str = "taskbar.pinned-running";
    pub const WORKSPACE_PAGER: &str = "workspace.pager";
    pub const WORKSPACE_TRANSACTIONS: &str = "workspace.transactions";
    pub const SNAP_HALF_QUARTER: &str = "snap.half-quarter";
    pub const SNAP_PREVIEW: &str = "snap.preview";
    pub const WORKAREA_STRUTS: &str = "workarea.struts";
    pub const DESKTOP_SELECTION: &str = "desktop.selection";
    pub const DESKTOP_STICKY_NOTES: &str = "desktop.sticky-notes";
    pub const DESKTOP_MENU: &str = "desktop.menu";
    pub const SETTINGS_APPEARANCE: &str = "settings.appearance";
    pub const SETTINGS_DISPLAYS: &str = "settings.displays";
    pub const SETTINGS_HOTKEYS: &str = "settings.hotkeys";
    pub const SETTINGS_FONTS: &str = "settings.fonts";
    pub const ACCENT_LIVE_APPLY: &str = "settings.accent-live-apply";
    pub const ICON_THEME_FREEDESKTOP: &str = "settings.icon-theme";
    pub const INTEGRATION_MPRIS: &str = "integration.mpris";
    pub const INTEGRATION_NETWORK_MANAGER: &str = "integration.network-manager";
    pub const INTEGRATION_PULSE: &str = "integration.pulse";
    pub const INTEGRATION_DBUS: &str = "integration.dbus";
    pub const ENGINE_SHOW_DESKTOP: &str = "engine.show-desktop";
    pub const ENGINE_QUICK_SWITCH: &str = "engine.quick-switch";
    pub const ENGINE_EXTENDED_STATES: &str = "engine.extended-window-states";
    pub const ENGINE_RESTACK: &str = "engine.restack";
    pub const ENGINE_FULLSCREEN_MONITORS: &str = "engine.fullscreen-monitors";
    pub const ENGINE_INTERACTIVE_MOVERESIZE: &str = "engine.interactive-moveresize";
    pub const ENGINE_RANDR_TOPOLOGY: &str = "engine.randr-topology";
    pub const ENGINE_XEMBED_TRAY: &str = "engine.xembed-tray";
}

pub const CURRENT_CAPABILITIES: [&str; 31] = [
    names::PANEL_MULTI_OUTPUT,
    names::PANEL_PER_OUTPUT_SCALE,
    names::TASKBAR_ORDERING,
    names::TASKBAR_DRAG_REORDER,
    names::TASKBAR_PINNED_RUNNING,
    names::WORKSPACE_PAGER,
    names::WORKSPACE_TRANSACTIONS,
    names::SNAP_HALF_QUARTER,
    names::SNAP_PREVIEW,
    names::WORKAREA_STRUTS,
    names::DESKTOP_SELECTION,
    names::DESKTOP_STICKY_NOTES,
    names::DESKTOP_MENU,
    names::SETTINGS_APPEARANCE,
    names::SETTINGS_DISPLAYS,
    names::SETTINGS_HOTKEYS,
    names::SETTINGS_FONTS,
    names::ACCENT_LIVE_APPLY,
    names::ICON_THEME_FREEDESKTOP,
    names::INTEGRATION_MPRIS,
    names::INTEGRATION_NETWORK_MANAGER,
    names::INTEGRATION_PULSE,
    names::INTEGRATION_DBUS,
    names::ENGINE_SHOW_DESKTOP,
    names::ENGINE_QUICK_SWITCH,
    names::ENGINE_EXTENDED_STATES,
    names::ENGINE_RESTACK,
    names::ENGINE_FULLSCREEN_MONITORS,
    names::ENGINE_INTERACTIVE_MOVERESIZE,
    names::ENGINE_RANDR_TOPOLOGY,
    names::ENGINE_XEMBED_TRAY,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    items: Vec<String>,
}

impl Capabilities {
    #[must_use]
    pub fn current() -> Self {
        Self {
            items: CURRENT_CAPABILITIES
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
        }
    }

    #[must_use]
    pub fn has(&self, capability: &str) -> bool {
        self.items.iter().any(|item| item == capability)
    }

    #[must_use]
    pub fn items(&self) -> &[String] {
        &self.items
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl Default for Capabilities {
    fn default() -> Self {
        Self::current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_capabilities_include_workspace_transactions() {
        assert!(Capabilities::current().has(names::WORKSPACE_TRANSACTIONS));
    }
}

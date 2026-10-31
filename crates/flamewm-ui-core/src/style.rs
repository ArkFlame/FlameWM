//! Canonical `FlameWM` visual-state and shell-theme tokens ported from the native UI layer.

use crate::{Color, Palette};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualRole {
    Surface,
    Control,
    Navigation,
    Task,
    Menu,
    Overlay,
    Text,
    Icon,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualState {
    Normal = 0,
    Hovered = 1 << 0,
    Pressed = 1 << 1,
    Focused = 1 << 2,
    Selected = 1 << 3,
    Disabled = 1 << 4,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VisualStates(u8);

impl VisualStates {
    pub const NORMAL: Self = Self(0);

    #[must_use]
    pub const fn from_state(state: VisualState) -> Self {
        Self(state as u8)
    }

    #[must_use]
    pub const fn union(self, state: VisualState) -> Self {
        Self(self.0 | state as u8)
    }

    #[must_use]
    pub const fn contains(self, state: VisualState) -> bool {
        if matches!(state, VisualState::Normal) {
            self.0 == 0
        } else {
            self.0 & state as u8 != 0
        }
    }
}

impl From<VisualState> for VisualStates {
    fn from(value: VisualState) -> Self {
        Self::from_state(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visual {
    pub role: VisualRole,
    pub states: VisualStates,
    pub enabled: bool,
}

impl Visual {
    #[must_use]
    pub const fn new(role: VisualRole, state: VisualState) -> Self {
        Self::with_states(role, VisualStates::from_state(state))
    }

    #[must_use]
    pub const fn with_states(role: VisualRole, states: VisualStates) -> Self {
        Self {
            role,
            states,
            enabled: !states.contains(VisualState::Disabled),
        }
    }

    #[must_use]
    pub const fn has_state(self, state: VisualState) -> bool {
        self.states.contains(state)
    }

    #[must_use]
    pub const fn interactive(self) -> bool {
        self.enabled && !self.has_state(VisualState::Disabled)
    }
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UiLayer {
    SurfaceBackground = -2000,
    Background = -1000,
    Content = 0,
    Selection = 100,
    Floating = 200,
    Overlay = 500,
    Popover = 1000,
    Modal = 1100,
}

impl UiLayer {
    #[must_use]
    pub const fn z_index(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Material {
    pub fill: Color,
    pub border: Color,
    pub radius: u16,
    pub border_width: u16,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            fill: Color::SURFACE,
            border: Color::BORDER,
            radius: 0,
            border_width: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellMetrics {
    pub panel_height: u16,
    pub panel_pad_x: u16,
    pub panel_opacity_percent: u8,
    pub radius: u16,
    pub start_button_width: u16,
    pub start_icon_size: u16,
    pub task_button_width: u16,
    pub task_icon_size: u16,
    pub task_indicator_inset: u16,
    pub task_indicator_height: u16,
    pub task_focused_indicator_inset: u16,
    pub task_focused_indicator_height: u16,
    pub workspace_button_width: u16,
    pub workspace_button_height: u16,
    pub workspace_column_gap: u16,
    pub workspace_row_gap: u16,
    pub workspace_pad_x: u16,
    pub workspace_pad_y: u16,
    pub workspace_rows: u8,
    pub tray_button_width: u16,
    pub tray_icon_size: u16,
    pub clock_min_width: u16,
    pub clock_pad_left: u16,
    pub clock_pad_right: u16,
    pub clock_pad_top: u16,
    pub clock_pad_bottom: u16,
    pub clock_line_height: u16,
    pub popover_offset: u16,
    pub popover_padding: u16,
    pub start_menu_width: u16,
    pub start_menu_min_height: u16,
    pub start_submenu_width: u16,
    pub tray_popover_width: u16,
    pub clock_popover_width: u16,
}

impl Default for ShellMetrics {
    fn default() -> Self {
        Self {
            panel_height: 44,
            panel_pad_x: 2,
            panel_opacity_percent: 99,
            radius: 4,
            start_button_width: 43,
            start_icon_size: 24,
            task_button_width: 42,
            task_icon_size: 25,
            task_indicator_inset: 9,
            task_indicator_height: 2,
            task_focused_indicator_inset: 5,
            task_focused_indicator_height: 3,
            workspace_button_width: 22,
            workspace_button_height: 16,
            workspace_column_gap: 1,
            workspace_row_gap: 1,
            workspace_pad_x: 3,
            workspace_pad_y: 4,
            workspace_rows: flamewm_api::workspace::WORKSPACE_ROWS as u8,
            tray_button_width: 30,
            tray_icon_size: 18,
            clock_min_width: 70,
            clock_pad_left: 3,
            clock_pad_right: 5,
            clock_pad_top: 2,
            clock_pad_bottom: 1,
            clock_line_height: 16,
            popover_offset: 4,
            popover_padding: 13,
            start_menu_width: 550,
            start_menu_min_height: 349,
            start_submenu_width: 258,
            tray_popover_width: 310,
            clock_popover_width: 286,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsMetrics {
    pub navigation_width: u16,
    pub page_padding: u16,
    pub card_radius: u16,
    pub row_height: u16,
    pub control_height: u16,
}

impl Default for SettingsMetrics {
    fn default() -> Self {
        Self {
            navigation_width: 192,
            page_padding: 24,
            card_radius: 5,
            row_height: 44,
            control_height: 32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub palette: Palette,
    pub accent_hover: Color,
    pub accent_pressed: Color,
    pub accent_muted: Color,
    pub shell: ShellMetrics,
    pub settings: SettingsMetrics,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            palette: Palette::default(),
            accent_hover: Color(0x00F0_6433),
            accent_pressed: Color(0x00C9_4418),
            accent_muted: Color(0x006B_2A12),
            shell: ShellMetrics::default(),
            settings: SettingsMetrics::default(),
        }
    }
}

impl Theme {
    #[must_use]
    pub const fn color(self, role: VisualRole, state: VisualState) -> Color {
        self.color_for_states(role, VisualStates::from_state(state))
    }

    #[must_use]
    pub const fn color_for_states(self, role: VisualRole, states: VisualStates) -> Color {
        if states.contains(VisualState::Disabled) {
            return self.palette.text_muted;
        }
        if states.contains(VisualState::Pressed) {
            return self.accent_pressed;
        }
        if states.contains(VisualState::Hovered) {
            return self.accent_hover;
        }
        if states.contains(VisualState::Selected) || states.contains(VisualState::Focused) {
            return self.palette.accent;
        }
        match role {
            VisualRole::Text | VisualRole::Icon => self.palette.text,
            VisualRole::Control | VisualRole::Navigation | VisualRole::Overlay => {
                self.palette.surface_raised
            }
            VisualRole::Task | VisualRole::Menu | VisualRole::Surface => self.palette.surface,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_layer_ordering_matches_stacking_contract() {
        assert!(UiLayer::SurfaceBackground.z_index() < UiLayer::Background.z_index());
        assert!(UiLayer::Background.z_index() < UiLayer::Content.z_index());
        assert!(UiLayer::Content.z_index() < UiLayer::Selection.z_index());
        assert!(UiLayer::Selection.z_index() < UiLayer::Floating.z_index());
        assert!(UiLayer::Floating.z_index() < UiLayer::Overlay.z_index());
        assert!(UiLayer::Overlay.z_index() < UiLayer::Popover.z_index());
        assert!(UiLayer::Popover.z_index() < UiLayer::Modal.z_index());
        assert_eq!(UiLayer::SurfaceBackground.z_index(), -2000);
        assert_eq!(UiLayer::Background.z_index(), -1000);
        assert_eq!(UiLayer::Content.z_index(), 0);
        assert_eq!(UiLayer::Selection.z_index(), 100);
        assert_eq!(UiLayer::Floating.z_index(), 200);
        assert_eq!(UiLayer::Overlay.z_index(), 500);
        assert_eq!(UiLayer::Popover.z_index(), 1000);
        assert_eq!(UiLayer::Modal.z_index(), 1100);
    }

    #[test]
    fn selected_visual_uses_semantic_accent() {
        assert_eq!(
            Theme::default().color(VisualRole::Task, VisualState::Selected),
            Color::FLAME_RED
        );
    }

    #[test]
    fn combined_visual_state_uses_native_priority() {
        let states = VisualStates::from_state(VisualState::Selected).union(VisualState::Hovered);
        assert_eq!(
            Theme::default().color_for_states(VisualRole::Task, states),
            Theme::default().accent_hover
        );
        let disabled = states.union(VisualState::Disabled);
        assert_eq!(
            Theme::default().color_for_states(VisualRole::Task, disabled),
            Color::TEXT_MUTED
        );
    }

    #[test]
    fn normal_visual_state_is_exactly_the_empty_mask() {
        assert!(VisualStates::NORMAL.contains(VisualState::Normal));
        assert!(!VisualStates::from_state(VisualState::Hovered).contains(VisualState::Normal));
    }
}

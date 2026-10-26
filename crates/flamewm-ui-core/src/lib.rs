//! Renderer-neutral `FlameWM` visual design contracts.

pub mod context_menu;
pub mod controls;
pub mod icons;
pub mod image;
pub mod layout;
pub mod menu;
pub mod popover;
pub mod range;
pub mod scroll;
pub mod style;
pub mod virtual_list;

use flamewm_api::{OutputId, Point, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub const FLAME_RED: Self = Self(0x00EF_4048);
    pub const BACKGROUND: Self = Self(0x000A_0A0A);
    pub const SURFACE: Self = Self(0x0019_1B1D);
    pub const SURFACE_RAISED: Self = Self(0x0024_272A);
    pub const BORDER: Self = Self(0x0051_565B);
    pub const TEXT: Self = Self(0x00F1_F2F3);
    pub const TEXT_MUTED: Self = Self(0x00AE_B4BB);
    pub const DANGER: Self = Self(0x00E5_3935);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub accent: Color,
    pub background: Color,
    pub surface: Color,
    pub surface_raised: Color,
    pub border: Color,
    pub text: Color,
    pub text_muted: Color,
    pub danger: Color,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            accent: Color::FLAME_RED,
            background: Color::BACKGROUND,
            surface: Color::SURFACE,
            surface_raised: Color::SURFACE_RAISED,
            border: Color::BORDER,
            text: Color::TEXT,
            text_muted: Color::TEXT_MUTED,
            danger: Color::DANGER,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypographyRole {
    Body,
    Title,
    Caption,
    Button,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Typography {
    pub family: &'static str,
    pub base_px: u16,
    pub line_px: u16,
    pub bold: bool,
    pub size_offset: i16,
}

impl Default for Typography {
    fn default() -> Self {
        Self {
            family: "IBM Plex Sans",
            base_px: 13,
            line_px: 16,
            bold: false,
            size_offset: 0,
        }
    }
}

impl Typography {
    #[must_use]
    pub fn size_for(self, role: TypographyRole) -> u16 {
        let role_delta: i16 = match role {
            TypographyRole::Title => 8,
            TypographyRole::Caption => -2,
            TypographyRole::Body | TypographyRole::Button => 0,
        };
        let value = i32::from(self.base_px) + i32::from(self.size_offset) + i32::from(role_delta);
        u16::try_from(value.max(1)).unwrap_or(u16::MAX)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    pub panel: u16,
    pub titlebar: u16,
    pub settings_nav_width: u16,
    pub settings_page_padding: u16,
    pub settings_row: u16,
    pub control: u16,
    pub card_radius: u16,
    pub menu_radius: u16,
    pub space_small: u16,
    pub space_medium: u16,
    pub space_large: u16,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            panel: 44,
            titlebar: 32,
            settings_nav_width: 192,
            settings_page_padding: 24,
            settings_row: 44,
            control: 32,
            card_radius: 5,
            menu_radius: 8,
            space_small: 4,
            space_medium: 8,
            space_large: 16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IconRole {
    Start,
    Taskbar,
    Appearance,
    Desktop,
    Displays,
    Fonts,
    Hotkeys,
    About,
    NetworkOffline,
    NetworkWired,
    Wifi0,
    Wifi25,
    Wifi50,
    Wifi75,
    Wifi100,
    AudioMuted,
    AudioLow,
    AudioMedium,
    AudioHigh,
    MediaPrevious,
    MediaPlay,
    MediaPause,
    MediaNext,
    Power,
    Session,
    Trash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconTreatment {
    Symbolic,
    FullColor,
    Brand,
}

impl IconRole {
    #[must_use]
    pub const fn treatment(self) -> IconTreatment {
        match self {
            Self::Start => IconTreatment::Brand,
            _ => IconTreatment::Symbolic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Action {
        id: String,
        label: String,
        icon: Option<IconRole>,
        enabled: bool,
    },
    Separator,
    Header(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopoverDirection {
    Above,
    Below,
    LeftOf,
    RightOf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopoverPlacement {
    pub output: OutputId,
    pub rect: Rect,
    pub direction: PopoverDirection,
}

#[must_use]
pub fn anchor_popover(
    output: OutputId,
    output_rect: Rect,
    anchor: Rect,
    size: (i32, i32),
    direction: PopoverDirection,
    gap: i32,
) -> PopoverPlacement {
    let (width, height) = size;
    let origin = match direction {
        PopoverDirection::Above => Point::new(
            anchor.x + (anchor.width - width) / 2,
            anchor.y - height - gap,
        ),
        PopoverDirection::Below => {
            Point::new(anchor.x + (anchor.width - width) / 2, anchor.bottom() + gap)
        }
        PopoverDirection::LeftOf => Point::new(
            anchor.x - width - gap,
            anchor.y + (anchor.height - height) / 2,
        ),
        PopoverDirection::RightOf => Point::new(
            anchor.right() + gap,
            anchor.y + (anchor.height - height) / 2,
        ),
    };
    PopoverPlacement {
        output,
        rect: Rect::new(origin.x, origin.y, width, height).clamp_inside(output_rect),
        direction,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typography_roles_match_current_native_offsets() {
        let typography = Typography::default();
        assert_eq!(typography.size_for(TypographyRole::Body), 13);
        assert_eq!(typography.size_for(TypographyRole::Title), 21);
        assert_eq!(typography.size_for(TypographyRole::Caption), 11);
        assert_eq!(typography.size_for(TypographyRole::Button), 13);
    }

    #[test]
    fn popover_is_clamped_to_negative_origin_output() {
        let placement = anchor_popover(
            OutputId::new("HDMI-1"),
            Rect::new(-1920, 0, 1920, 1080),
            Rect::new(-20, 1000, 20, 44),
            (320, 300),
            PopoverDirection::Above,
            8,
        );
        assert!(placement.rect.x >= -1920);
        assert!(placement.rect.right() <= 0);
    }
}

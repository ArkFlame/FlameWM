//! Settings enums and small value types.

use super::*;

/// Generates `as_str` (wire text) and `parse` (wire text to variant) from one variant table.
macro_rules! wire_enum {
    ($ty:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        impl $ty {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }

            #[must_use]
            pub fn parse(value: &str) -> Option<Self> {
                match value {
                    $($wire => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

/// Clock notation on the lock screen (`lockClockFormat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockClockFormat {
    TwentyFour,
    Twelve,
}

wire_enum!(LockClockFormat { TwentyFour => "24h", Twelve => "12h" });

/// Lock-screen idle action after the Ribbons screensaver (`screensaverMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreensaverMode {
    Ribbons,
    /// Turn the display off while locked.
    None,
}

wire_enum!(ScreensaverMode { Ribbons => "ribbons", None => "none" });

/// Night Light timing (`nightShiftMode`). `Manual` uses `nightShiftStart`..`nightShiftEnd`;
/// `Sunset` follows local sunset to sunrise at the weather location (`flamewm_api::blue_light_sun`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NightShiftMode {
    #[default]
    Manual,
    Sunset,
}

wire_enum!(NightShiftMode { Manual => "manual", Sunset => "sunset" });

/// Taskbar layout (`taskbarStyle`). `Floating` insets the bar and rounds its corners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarStyle {
    Classic,
    Floating,
}

wire_enum!(TaskbarStyle { Classic => "classic", Floating => "floating" });

/// Taskbar surface fill (`taskbar.fill`). Independent of the layout `taskbarStyle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarFill {
    Solid,
    Gradient,
}

wire_enum!(TaskbarFill { Solid => "solid", Gradient => "gradient" });

/// Edge a gradient taskbar fades from (`taskbar.gradientEdge`). `Auto` follows the taskbar edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskbarGradientEdge {
    Auto,
    Top,
    Bottom,
    Left,
    Right,
}

wire_enum!(TaskbarGradientEdge { Auto => "auto", Top => "top", Bottom => "bottom", Left => "left", Right => "right" });

/// Dock placement (`dock.position`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockPosition {
    Bottom,
    Left,
    Right,
    Top,
}

wire_enum!(DockPosition { Bottom => "bottom", Left => "left", Right => "right", Top => "top" });

/// Dock corner shape (`dock.cornerStyle`). `Squared` draws the dock pill with zero radius.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockCornerStyle {
    Rounded,
    Squared,
}

wire_enum!(DockCornerStyle { Rounded => "rounded", Squared => "squared" });

/// Maximum pinned dock apps (`dock.pinned`).
pub const DOCK_PINNED_MAX: usize = 32;

/// Action run when the pointer rests in a screen corner (`hotCorner.*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotCornerAction {
    None,
    Overview,
    ShowDesktop,
    Lock,
}

wire_enum!(HotCornerAction {
    None => "none",
    Overview => "overview",
    ShowDesktop => "show-desktop",
    Lock => "lock",
});

/// Clock notation on the taskbar (`taskbarClockFormat`). Same `24h`/`12h` wire values as the lock
/// screen, so both keys share one type.
pub type ClockFormat = LockClockFormat;

/// Image source for the desktop wallpaper (`wallpaperMode`) and the lock background
/// (`lockBackgroundMode`). `default` is the bundled image, `solid` the plain colour, and `custom`
/// the saved image path. Switching modes never erases the saved path or colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundMode {
    Default,
    Solid,
    Custom,
}

wire_enum!(BackgroundMode { Default => "default", Solid => "solid", Custom => "custom" });

/// Desktop wallpaper placement (`wallpaperFit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallpaperFit {
    Fill,
    Fit,
    Stretch,
    Center,
    Tile,
}

wire_enum!(WallpaperFit {
    Fill => "fill",
    Fit => "fit",
    Stretch => "stretch",
    Center => "center",
    Tile => "tile",
});

/// Translucent surface policy for the taskbar or Start menu.
///
/// `blur_percent` (0..=100) maps to a blur radius of `blur_percent / 100 * 40` px at 1x (Tahoe blur range 30-40 px). Blur is
/// only applied when `opacity_percent < 100`; at 100% the radius is always 0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Backdrop {
    pub opacity_percent: u8,
    pub blur_percent: u8,
    pub radius_px: f32,
}

/// Typed view of the `screensaver*` keys. Ribbon width is in pixels at 1280x720.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreensaverSettings {
    pub mode: ScreensaverMode,
    pub after_lock: Duration,
    pub num_ribbons: u8,
    pub ribbon_width_px: f32,
    pub span_monitors: bool,
    pub display_off_minutes: u16,
}

/// Animation style shared by every element that animates. Consumers reuse one implementation per
/// variant; `None` means no animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationStyle {
    None,
    Slide,
    Fade,
    MagicLamp,
    Scale,
}

impl AnimationStyle {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Slide => "slide",
            Self::Fade => "fade",
            Self::MagicLamp => "magic-lamp",
            Self::Scale => "scale",
        }
    }

    /// Parses one stable key's wire value. `slide-up` is the legacy spelling of `slide`.
    pub(super) fn parse_for(key: &str, text: &str) -> Option<Self> {
        let style = match text {
            "none" => Self::None,
            "slide" | "slide-up" => Self::Slide,
            "fade" => Self::Fade,
            "magic-lamp" => Self::MagicLamp,
            "scale" => Self::Scale,
            _ => return None,
        };
        let allowed: &[Self] = match key {
            "startAnimation" => &[Self::None, Self::Slide, Self::Fade, Self::MagicLamp],
            "minimizeAnimation" => &[Self::None, Self::Fade, Self::MagicLamp],
            "maximizeAnimation" => &[Self::None, Self::Fade, Self::Scale],
            "windowOpenAnimation" => &[Self::None, Self::Scale, Self::Fade],
            "windowCloseAnimation" => &[Self::None, Self::Fade],
            _ => return None,
        };
        // Old releases erroneously serialized the maximize Scale effect as
        // "magic-lamp". Keep persisted profiles working, but normalize them to Scale.
        if key == "maximizeAnimation" && text == "magic-lamp" {
            return Some(Self::Scale);
        }
        allowed.contains(&style).then_some(style)
    }
}

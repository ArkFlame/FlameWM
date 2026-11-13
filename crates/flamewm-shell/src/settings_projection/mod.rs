use chrono::Local;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_api::PanelEdge;
use flamewm_image_core::scale::ScaleFilter;
use flamewm_image_core::{decode_bytes_at, ImageSourceFormat};
use flamewm_platform::chrome::ACCENT_SOFT_ALPHA;
use flamewm_platform::settings::ClockFormat;
use flamewm_skin::palette::Appearance;
use flamewm_skin::shell_tones::FIXED_TONES;
use flamewm_ui_core::context_menu::shape_radius;
use flamewm_ui_x11::surface_style::{
    project_surface_outline, surface_background as shared_surface_background,
};
use flamewm_ui_x11::{RuntimeImage, SlideConfig, SlideEdge, UiColor, UiDocumentAccess};

use crate::projection::{project_clock_at, ClockDisplay, TaskbarItems};

/// Panel image node that carries the custom Start icon (`ui/shell/panel.html`).
const START_ICON_ID: &str = "task-start-icon";
/// Square raster for the 25px `.task-start-icon` box.
const START_ICON_SIZE: u32 = 25;
/// Largest custom Start icon file read, so a bad path cannot stall the panel.
const MAX_START_ICON_BYTES: u64 = 8 * 1024 * 1024;

const DEFAULT_ANIMATION_SPEED: i64 = 100;
const DEFAULT_START_ANIMATION: &str = "none";
const DEFAULT_SURFACE_OPACITY: u8 = flamewm_api::settings_groups::DEFAULT_SURFACE_OPACITY;
const DEFAULT_SURFACE_BLUR_PERCENT: u8 = flamewm_ui_core::glass::DARK.blur_percent;
const DEFAULT_ACCENT_OPACITY: u8 = 100;
const DEFAULT_FONT_FAMILY: &str = "IBM Plex Sans";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceKind {
    Panel,
    Start,
    Launcher,
    Switcher,
    TaskMenu,
    Media,
    Osd,
    /// The auto-hide Dock pill: takes the taskbar material and outline, never its own colours.
    Dock,
}

mod project;
pub use project::*;
mod palette;
use palette::*;
mod surfaces;
pub use surfaces::*;
mod start;
pub use start::*;
mod taskbar;
pub mod usage;
pub use taskbar::*;
mod settings;
pub use settings::*;
mod glass;
pub use glass::*;
#[cfg(test)]
mod clock_settings_tests;
#[cfg(test)]
mod tests;

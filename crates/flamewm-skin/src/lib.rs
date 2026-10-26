//! Renderer-neutral `FlameWM` skin tokens promoted from the RWR 0.0.9 prototype.

pub mod icons;
pub mod metrics;
pub mod palette;
pub mod recipes;
pub mod typography;

pub use icons::{IconRole, IconSource};
pub use metrics::{
    DesktopMetrics, SettingsMetrics, SkinMetrics, StartMetrics, StickyMetrics, TaskbarMetrics,
    WindowChromeMetrics,
};
pub use palette::{AlphaColor, Palette, Rgb};
pub use typography::{FontWeight, TextStyle, Typography};

/// The complete promoted skin contract. Recipes refer to these centralized values.
pub const DEFAULT: SkinMetrics = SkinMetrics::RWR_0_0_9;

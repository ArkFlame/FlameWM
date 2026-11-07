//! Engine-neutral contracts shared by `FlameWM` product code and a window-manager engine.
//!
//! This crate intentionally contains no X11, D-Bus, `PulseAudio`, toolkit, or async-runtime
//! dependency. `FlameWM` backends implement the traits in [`ports`].

pub mod applications;
pub mod background;
pub mod blue_light;
pub mod blue_light_sun;
pub mod clock;
pub mod capabilities;
pub mod color;
pub mod display;
pub mod error;
pub mod events;
pub mod geometry;
pub mod graphics;
pub mod ids;
pub mod input;
pub mod panels;
pub mod paths;
pub mod ports;
pub mod session;
pub mod settings;
pub mod settings_groups;
pub mod shell;
pub mod shell_bootstrap;
pub mod shortcuts;
pub mod system;
pub mod weather;
pub mod window;
pub mod wm_features;
pub mod workspace;

pub use error::{ErrorCode, FlameError, FlameResult};
pub use geometry::{Point, Rect, Size};
pub use ids::{
    DesktopAppId, ModeId, OutputId, TaskEntryId, TransactionId, WindowRef, WorkspaceRef,
};

pub use panels::PanelEdge;
pub use system::{
    AudioEndpointKind, AudioEndpointSnapshot, AudioMuteAction, AudioSnapshot, AudioStreamSnapshot,
    AudioTarget, AudioVolumeAction, NetworkSecretRequestSnapshot,
};

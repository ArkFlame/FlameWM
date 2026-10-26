//! Engine-neutral contracts shared by `FlameWM` product code and a window-manager engine.
//!
//! This crate intentionally contains no X11, D-Bus, `PulseAudio`, toolkit, or async-runtime
//! dependency. `FlameWM` backends implement the traits in [`ports`].

pub mod applications;
pub mod background;
pub mod capabilities;
pub mod display;
pub mod error;
pub mod events;
pub mod geometry;
pub mod ids;
pub mod input;
pub mod panels;
pub mod ports;
pub mod root_rubber;
pub mod session;
pub mod settings;
pub mod shell_bootstrap;
pub mod shortcuts;
pub mod system;
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

//! J07 quick-control host process: owns audio/network/calendar surfaces.
//!
//! Role branching happens at process entry (`--quick-control-host`):
//! helper role (profiler name `flamewm-quick-control`) connects its own
//! [`ControlClient`], projects the snapshot through the existing taskbar
//! status projections, owns one popup surface per kind, answers parent
//! `OPEN` commands with fitted placement, and routes typed actions through
//! the existing dispatcher. The authentication secret travels out-of-band
//! (environment) and never appears in the supervisor protocol or logs.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::popup_controller::PopupError;
use crate::projection::project_weather_popup;
use crate::weather_host::{spawn_location_search, LocationSearch, SearchStatus};
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_control_core::ControlRequest;
use flamewm_control_dbus::{ControlClient, ControlSignalClient};
use flamewm_control_wire::ControlSignal;
use flamewm_dbus_reactor::BusKind;
use flamewm_debug::DebugEventId;
use flamewm_shell_core::status::{AudioSettings, AudioTab, NetworkQuery};
use flamewm_shell_core::weather::WeatherFeed;
use flamewm_ui_x11::{
    decode_document, ActionPhase, SlideConfig, SurfaceConfig, SurfaceHandle, SurfaceInputMode,
    SurfaceRole, SurfaceRuntime, UiBackendError, UiDocumentAccess, UiTemplate,
};

use self::popup::{
    arm_slide, close_helper, fit_helper, grab, measure_helper, popup_slide, prepare_commit, present,
};
use super::protocol::{
    decode_line, Command, OpenRequest, OpenResult, OpenScope, QuickControlKind, MAX_LINE_LEN,
};
use super::supervisor::{QuickControlSupervisor, SupervisorError};

#[path = "popup.rs"]
mod popup;

mod consts;
use consts::*;
mod titles;
pub use titles::*;
mod debug_util;
use debug_util::*;
mod error;
pub use error::*;
mod snapshot;
pub use snapshot::*;
mod state;
use state::*;
mod stdin;
use stdin::*;
mod run;
pub use run::*;
mod scoped;
use scoped::*;
mod tick;
use tick::*;
mod timings;
pub use timings::*;
mod open_request;
use open_request::*;
mod projection;
use projection::*;
mod helper_events;
pub use helper_events::*;
mod weather_network;
use weather_network::*;
mod audio_input;
use audio_input::*;
mod surfaces;
pub use surfaces::*;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod audio_routing_tests;

#[cfg(test)]
mod battery_popup_tests;

#[cfg(test)]
mod network_filter_reprojection_tests;

#[cfg(test)]
mod network_secret_prompt_tests;

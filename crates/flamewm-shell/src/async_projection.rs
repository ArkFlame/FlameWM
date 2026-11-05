//! Async projection: labels/visibility immediate, icons cached-or-placeholder.
//! Never resolves icons on the caller thread. Schedules worker requests only.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use flamewm_api::DesktopAppId;
use flamewm_api::session::SessionCapabilities;
use flamewm_shell_core::StartModel;
use flamewm_ui_x11::{RuntimeImage, UiColor, UiDocumentAccess};

use crate::ShellSnapshot;
use crate::icon_loader::{IconLoader, IconTarget};
use crate::projection::{PanelView, TaskbarItems};
use crate::start::StartCategory;

mod icon_binding;
mod icon_cache;
mod metrics;
mod panel;
mod start_view;
mod task_slots;
#[cfg(test)]
pub(crate) mod tests;

pub use icon_binding::*;
pub use icon_cache::*;
use metrics::*;
pub use panel::*;
pub use start_view::*;
pub use task_slots::*;

pub const TASK_SLOT_COUNT: usize = 8;
pub const START_APP_SLOT_COUNT: usize = 8;

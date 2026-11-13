use std::time::{Duration, Instant};
use std::{fs, path::PathBuf};

use flamewm_ui_x11::{
    ActionPhase, PointerButton, SurfaceEvent, SurfaceRuntime, UiBackendError, UiDocumentAccess,
};

use crate::entry::app::{is_popup_refusal, shell_slow_note, ShellLoop};
use flamewm_shell::quick_controls::host::is_quick_popup_title;
use flamewm_shell::quick_controls::QuickControlKind;
use flamewm_shell::runtime::InteractionContext;

/// C07 stage helper: open a static profiler span. Static labels only.
macro_rules! shell_stage {
    ($label:expr) => {{
        let __point = flamewm_profiler::ProfilePoint::new($label);
        __point.start()
    }};
}

mod dismiss;
mod drag;
mod helpers;
mod media_keys;
mod overflow_keys;
mod route;
mod taskdrag;
#[cfg(test)]
mod tests;
mod usage_keys;

pub(crate) use dismiss::*;
pub(crate) use drag::*;
use helpers::*;

const OUTSIDE_RELEASE_ACTION: &str = "surface.outside.release";

/// Tray volume source node; its release toggles the audio popup.
const TRAY_VOLUME_ACTION: &str = "volume.open";

const TASKBAR_SURFACE_ACTION: &str = "taskbar.surface";

/// Row action of the Start application context menu (add/remove favorite).
const START_MENU_FAVORITE_ACTION: &str = "start.menu.favorite";

const TASKBAR_MENU_SETTINGS_ACTION: &str = "taskbar.menu.settings";

const TASKBAR_MENU_TASK_MANAGER_ACTION: &str = "taskbar.menu.task-manager";

const GENERAL_SETTINGS_PROGRAM: &str = "flamewm-settings";

const TASK_MANAGER_PROGRAM: &str = "flamewm-task-manager";

const START_DRAG_THRESHOLD_PX: f32 = 6.0;

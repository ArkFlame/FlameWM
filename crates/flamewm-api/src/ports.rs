use std::collections::BTreeMap;

use crate::applications::ApplicationLaunchOptions;
use crate::background::BackgroundState;
use crate::display::DisplaySnapshot;
use crate::input::PointerPosition;
use crate::session::{SessionAction, SessionCapabilities};
use crate::shortcuts::KeyBinding;
use crate::window::WindowSnapshot;
use crate::wm_features::{
    FeatureAction, FullscreenMonitorSpan, InteractiveMoveResize, RestackMode, WindowFeature,
    WindowFeatureSnapshot,
};
use crate::workspace::{WorkspacePlan, WorkspaceSnapshot};
use crate::{
    DesktopAppId, ErrorCode, FlameError, FlameResult, ModeId, OutputId, Rect, TransactionId,
    WindowRef,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FdEvents(u8);

impl FdEvents {
    pub const READABLE: Self = Self(1 << 0);
    pub const WRITABLE: Self = Self(1 << 1);
    pub const ERROR: Self = Self(1 << 2);
    pub const HANGUP: Self = Self(1 << 3);

    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

pub type FdCallback = Box<dyn FnMut(i32, FdEvents)>;
pub type TimerCallback = Box<dyn FnMut()>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FdHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimerHandle(pub u64);

pub trait WindowPort {
    fn get(&self, window: WindowRef) -> FlameResult<WindowSnapshot>;
    fn snapshot(&self) -> FlameResult<Vec<WindowSnapshot>>;
    /// Resolve the desktop backend's current generation for a session-scoped window id.
    /// Backends should override this with O(1) lookup; the default is a safe snapshot fallback.
    fn current_window_ref(&self, window_id: u64) -> FlameResult<WindowRef> {
        if window_id == 0 {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "window id must be non-zero",
            ));
        }
        self.snapshot()?
            .into_iter()
            .find_map(|window| (window.reference.id == window_id).then_some(window.reference))
            .ok_or_else(|| {
                FlameError::new(
                    ErrorCode::NotFound,
                    "current window generation is unavailable",
                )
            })
    }
    fn activate(&mut self, window: WindowRef) -> FlameResult<()>;
    fn minimize(&mut self, window: WindowRef) -> FlameResult<()>;
    fn maximize(&mut self, window: WindowRef) -> FlameResult<()>;
    fn restore(&mut self, window: WindowRef) -> FlameResult<()>;
    fn close(&mut self, window: WindowRef) -> FlameResult<()>;
    fn set_outer_geometry(&mut self, window: WindowRef, geometry: Rect) -> FlameResult<()>;
    fn work_area(&self, window: WindowRef) -> FlameResult<Rect>;
    fn output(&self, window: WindowRef) -> FlameResult<OutputId>;

    /// Extended EWMH/ICCCM state. Backends that do not expose it yet retain a safe partial default.
    fn feature_snapshot(&self, window: WindowRef) -> FlameResult<WindowFeatureSnapshot> {
        let snapshot = self.get(window)?;
        Ok(WindowFeatureSnapshot {
            sticky: snapshot.sticky,
            ..WindowFeatureSnapshot::default()
        })
    }

    fn apply_window_feature(
        &mut self,
        _window: WindowRef,
        _feature: WindowFeature,
        _action: FeatureAction,
    ) -> FlameResult<()> {
        Err(FlameError::new(
            ErrorCode::Unsupported,
            "desktop backend does not expose extended window state mutation",
        ))
    }

    fn showing_desktop(&self) -> FlameResult<bool> {
        Err(FlameError::new(
            ErrorCode::Unsupported,
            "desktop backend does not expose Show Desktop state",
        ))
    }

    fn set_showing_desktop(&mut self, _show: bool) -> FlameResult<()> {
        Err(FlameError::new(
            ErrorCode::Unsupported,
            "desktop backend does not expose Show Desktop",
        ))
    }

    fn restack(
        &mut self,
        _window: WindowRef,
        _mode: RestackMode,
        _sibling: Option<WindowRef>,
    ) -> FlameResult<()> {
        Err(FlameError::new(
            ErrorCode::Unsupported,
            "desktop backend does not expose EWMH restacking",
        ))
    }

    fn set_fullscreen_monitors(
        &mut self,
        _window: WindowRef,
        _span: FullscreenMonitorSpan,
    ) -> FlameResult<()> {
        Err(FlameError::new(
            ErrorCode::Unsupported,
            "desktop backend does not expose fullscreen monitor spans",
        ))
    }

    fn begin_interactive_move_resize(
        &mut self,
        _request: InteractiveMoveResize,
    ) -> FlameResult<()> {
        Err(FlameError::new(
            ErrorCode::Unsupported,
            "desktop backend does not expose interactive EWMH moveresize",
        ))
    }

    fn cancel_interactive_move_resize(&mut self, _window: WindowRef) -> FlameResult<()> {
        Err(FlameError::new(
            ErrorCode::Unsupported,
            "desktop backend does not expose moveresize cancellation",
        ))
    }
}

pub trait WorkspacePort {
    fn workspace_snapshot(&self) -> FlameResult<WorkspaceSnapshot>;
    fn activate_workspace(&mut self, index: usize, expected_revision: u64) -> FlameResult<()>;
    fn move_window_to_workspace(&mut self, window: WindowRef, target: usize) -> FlameResult<()>;
    /// Atomically switch workspace while carrying one actively dragged window.
    ///
    /// The desktop backend must not expose an intermediate state where the workspace changed but the
    /// dragged window did not (or vice versa). The operation is atomic and is required by side-edge dwell.
    fn switch_workspace_with_window(
        &mut self,
        window: WindowRef,
        target: usize,
        expected_revision: u64,
    ) -> FlameResult<()>;
    fn apply_workspace_plan(&mut self, plan: &WorkspacePlan) -> FlameResult<()>;
}

pub trait DisplayPort {
    fn display_snapshot(&self) -> FlameResult<DisplaySnapshot>;
    fn apply_mode(&mut self, output: &OutputId, mode: ModeId) -> FlameResult<TransactionId>;
    fn keep_mode(&mut self, transaction: TransactionId) -> FlameResult<()>;
    fn revert_mode(&mut self, transaction: TransactionId) -> FlameResult<()>;
}

pub trait ShortcutPort {
    fn prepare_shortcuts(&mut self, desired: &BTreeMap<String, KeyBinding>) -> FlameResult<()>;
    fn commit_shortcuts(&mut self) -> FlameResult<()>;
    fn rollback_shortcuts(&mut self);
}

pub trait WorkAreaPort {
    fn base_work_areas(&self) -> FlameResult<Vec<(OutputId, Rect)>>;
    fn apply_flame_reservations(&mut self, reservations: &[(OutputId, Rect)]) -> FlameResult<()>;
    fn request_recompute(&mut self);
}

/// Adapter over `FlameWM`'s desktop event loop. Product code does not create a second reactor.
///
/// Lifetime contract learned from the desktop event-loop contract:
/// - `remove_poll` and `remove_timer` MUST be safe when called from inside their own callback;
/// - removal unregisters/stops the source immediately but defers destruction until callback return;
/// - one-shot timers are retired automatically after their callback;
/// - shutdown invalidates callbacks before releasing their backing registrations;
/// - callbacks captured under an older desktop generation MUST NOT mutate current state.
///
/// A concrete `FlameWM` backend may satisfy this naturally with ownership, but it must preserve
/// these observable semantics rather than allowing callback-time use-after-free equivalents.
pub trait MainLoopPort {
    fn add_poll(
        &mut self,
        fd: i32,
        events: FdEvents,
        callback: FdCallback,
    ) -> FlameResult<FdHandle>;
    fn remove_poll(&mut self, handle: FdHandle);
    fn add_timer(
        &mut self,
        delay_ms: u64,
        callback: TimerCallback,
        repeat: bool,
    ) -> FlameResult<TimerHandle>;
    fn remove_timer(&mut self, handle: TimerHandle);
    fn defer(&mut self, callback: TimerCallback) -> FlameResult<TimerHandle>;
}

pub trait InputPort {
    fn root_pointer(&self) -> FlameResult<PointerPosition>;
}

pub trait ApplicationPort {
    fn launch(&mut self, app: &DesktopAppId, options: &ApplicationLaunchOptions)
    -> FlameResult<()>;
    fn launch_uri(&mut self, uri: &str) -> FlameResult<()>;
}

pub trait SessionPort {
    fn session_capabilities(&self) -> SessionCapabilities;
    fn perform_session_action(&mut self, action: SessionAction) -> FlameResult<()>;
}

pub trait BackgroundPort {
    fn project_background(&mut self, state: &BackgroundState) -> FlameResult<()>;
    fn reload_background(&mut self) -> FlameResult<()>;
}

pub trait TrayPort {
    fn set_tray_owner(&mut self, output: &OutputId) -> FlameResult<()>;
    fn clear_tray_owner(&mut self) -> FlameResult<()>;
    fn tray_owner(&self) -> FlameResult<Option<OutputId>>;
}

/// Canonical engine adapter contract expected from a `FlameWM` desktop backend.
///
/// High-frequency move/render callbacks stay in-process. Flame Control is above this boundary and
/// is deliberately absent from pointer-motion and X event ordering paths.
///
/// Work-area reservations, the host-owned main loop, background projection, and tray ownership
/// are separate service contracts and are intentionally not required of every engine adapter.
pub trait EnginePorts:
    WindowPort + WorkspacePort + DisplayPort + ShortcutPort + InputPort + ApplicationPort + SessionPort
{
}

impl<T> EnginePorts for T where
    T: WindowPort
        + WorkspacePort
        + DisplayPort
        + ShortcutPort
        + InputPort
        + ApplicationPort
        + SessionPort
{
}

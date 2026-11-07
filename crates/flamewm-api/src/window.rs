use crate::{DesktopAppId, OutputId, Rect, WindowRef, WorkspaceRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Normal,
    Minimized,
    Maximized,
    Fullscreen,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSnapshot {
    pub reference: WindowRef,
    pub title: String,
    pub app_id: DesktopAppId,
    pub outer_geometry: Rect,
    pub restore_geometry: Rect,
    pub state: WindowState,
    pub sticky: bool,
    pub focused: bool,
    pub workspace: WorkspaceRef,
    pub output: OutputId,
    pub state_generation: u64,
    /// The client missed its `_NET_WM_PING` after a close request; `false` when the producer
    /// does not track liveness or the sender predates the field.
    pub unresponsive: bool,
    /// `_NET_WM_STATE_SKIP_TASKBAR`, or a utility/dock/splash window type: not listed as a task.
    pub skip_taskbar: bool,
    /// `_NET_WM_STATE_DEMANDS_ATTENTION`: set by the client or by a blocked focus request, and
    /// cleared when the window takes focus.
    pub attention: bool,
}

impl WindowSnapshot {
    #[must_use]
    pub const fn is_visible(&self) -> bool {
        !matches!(self.state, WindowState::Minimized | WindowState::Hidden)
    }
}

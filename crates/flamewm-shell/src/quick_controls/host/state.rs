use super::*;

/// Quick-control host state behind the reactor callbacks: one reactor
/// owns the X11 surface FD, the nonblocking parent command FD, and tick
/// timers. No per-popup thread, no async runtime.
pub(crate) struct HostState {
    pub(crate) surfaces: HelperSurfaces,
    pub(crate) snapshot: QuickControlSnapshot,
    pub(crate) dispatcher: Option<Arc<flamewm_control_dbus::MutationDispatcher>>,
    /// Coalesced live system update to project on the next UI turn.
    pub(crate) system_reprojection_pending: bool,
    /// Parsed parent commands awaiting the tick drain.
    pub(crate) queue: VecDeque<Command>,
    /// Set on parent EOF or SHUTDOWN; the tick stops the loop cleanly.
    pub(crate) shutdown: bool,
    /// Applied live cursor theme for this thread's helper windows.
    pub(crate) cursor_theme: crate::cursor_theme::CursorThemeSync,
    pub(crate) latest_sequence: u64,
    pub(crate) active_sequence: Option<u64>,
}

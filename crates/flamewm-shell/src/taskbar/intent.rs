use flamewm_api::{DesktopAppId, TaskEntryId, WindowRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskbarIntent {
    OpenStart,
    ActivateTask(TaskEntryId),
    Launch(DesktopAppId),
    RestoreAndActivate(WindowRef),
    Minimize(WindowRef),
    Activate(WindowRef),
    ActivateWorkspace(usize),
    OpenStatus(StatusIntent),
    OpenClock,
    Dock(DockingIntent),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusIntent {
    Audio,
    Media,
    Network,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockingIntent {
    MoveTo(flamewm_api::PanelEdge),
    ToggleVisibility,
}

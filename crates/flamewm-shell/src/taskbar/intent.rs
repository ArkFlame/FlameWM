use flamewm_api::{DesktopAppId, WindowRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskbarIntent {
    Launch(DesktopAppId),
    RestoreAndActivate(WindowRef),
    Minimize(WindowRef),
    Activate(WindowRef),
    ActivateWorkspace(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusIntent {
    Audio,
    Media,
    Network,
}

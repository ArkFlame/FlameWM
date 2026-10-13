#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAction {
    Lock,
    /// Runs the lock screen in non-grabbing preview mode; exits on any input.
    LockPreview,
    Logout,
    Suspend,
    Reboot,
    Shutdown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionCapabilities {
    pub lock: bool,
    pub logout: bool,
    pub suspend: bool,
    pub reboot: bool,
    pub shutdown: bool,
}

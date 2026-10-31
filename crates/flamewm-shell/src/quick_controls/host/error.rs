use super::*;

#[derive(Debug)]
pub enum HostError {
    Io(io::Error),
    Control(String),
    Ui(String),
}

/// Pure helper-process termination decision. Keeping this separate from the
/// process exit lets library callers/tests inspect the shutdown contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickControlShutdown {
    Completed,
    Failed,
}

#[must_use]
pub const fn quick_control_exit_code(shutdown: QuickControlShutdown) -> i32 {
    match shutdown {
        QuickControlShutdown::Completed => 0,
        QuickControlShutdown::Failed => 1,
    }
}

impl core::fmt::Display for HostError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "host i/o: {e}"),
            Self::Control(e) => write!(f, "host control: {e}"),
            Self::Ui(e) => write!(f, "host ui: {e}"),
        }
    }
}

impl std::error::Error for HostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Control(_) | Self::Ui(_) => None,
        }
    }
}

impl From<io::Error> for HostError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

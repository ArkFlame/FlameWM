use core::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidArgument,
    NotFound,
    StaleRevision,
    Conflict,
    Busy,
    Unsupported,
    Unavailable,
    PermissionDenied,
    Timeout,
    IoFailure,
    EngineRejected,
    InternalFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlameError {
    pub code: ErrorCode,
    pub message: String,
}

impl FlameError {
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    #[must_use]
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    #[must_use]
    pub fn stale(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::StaleRevision, message)
    }

    #[must_use]
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    #[must_use]
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unavailable, message)
    }

    #[must_use]
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, message)
    }

    /// Wraps any displayable I/O or protocol failure as `IoFailure`.
    #[must_use]
    pub fn io(error: impl fmt::Display) -> Self {
        Self::new(ErrorCode::IoFailure, error.to_string())
    }
}

impl fmt::Display for FlameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for FlameError {}

pub type FlameResult<T> = Result<T, FlameError>;

//! Typed errors for the remote filesystem client. Messages never contain secrets.

use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The URL could not be parsed or contains disallowed characters.
    InvalidUrl(String),
    /// The scheme is recognised but this build has no backend for it.
    UnsupportedProtocol(&'static str),
    /// Host key is not in known_hosts. Caller must probe, show the fingerprint, and trust it.
    HostKeyUnknown {
        host: String,
        port: u16,
    },
    /// Host key differs from known_hosts. Never accepted automatically.
    HostKeyChanged {
        host: String,
        port: u16,
    },
    /// Public-key/agent auth failed. `password_possible` means the server offered a password
    /// method, which this client does not answer (BatchMode, no secret handling).
    AuthFailed {
        host: String,
        password_possible: bool,
    },
    /// No progress within the configured timeout, or the connection timed out.
    Timeout(String),
    /// The transport dropped or ssh itself failed. Message is the first ssh stderr line.
    ConnectionLost(String),
    NotFound(String),
    PermissionDenied(String),
    AlreadyExists(String),
    IsDirectory(String),
    Cancelled,
    /// Remote command failed with a status not covered above.
    Remote {
        status: i32,
        message: String,
    },
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(why) => write!(f, "invalid remote URL: {why}"),
            Self::UnsupportedProtocol(s) => write!(f, "{s}:// is not supported by this build"),
            Self::HostKeyUnknown { host, port } => {
                write!(f, "host key for {host}:{port} is not in known_hosts")
            }
            Self::HostKeyChanged { host, port } => {
                write!(f, "host key for {host}:{port} CHANGED; refusing to connect")
            }
            Self::AuthFailed {
                host,
                password_possible,
            } => write!(
                f,
                "authentication to {host} failed{}",
                if *password_possible {
                    " (server requests a password; key or agent auth required)"
                } else {
                    ""
                }
            ),
            Self::Timeout(what) => write!(f, "timed out: {what}"),
            Self::ConnectionLost(msg) => write!(f, "connection failed: {msg}"),
            Self::NotFound(p) => write!(f, "not found: {p}"),
            Self::PermissionDenied(p) => write!(f, "permission denied: {p}"),
            Self::AlreadyExists(p) => write!(f, "already exists: {p}"),
            Self::IsDirectory(p) => write!(f, "is a directory: {p}"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Remote { status, message } => {
                write!(f, "remote command failed ({status}): {message}")
            }
            Self::Io(msg) => write!(f, "local I/O error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

//! `flamewm-remote-fs`: remote file access for the FlameWM explorer.
//!
//! Backends: `sftp://` and `ssh://` (shell-free exec of `ls`/`cat`/`mv` over a multiplexed OpenSSH
//! connection), and `fish://` (same executor; KDE-fish-style paths with `~` as home). `smb://` and
//! `ftp://` parse but return `Error::UnsupportedProtocol`.
//!
//! Architecture: one owner for remote I/O (this crate). Callers hold a [`RemoteFs`], submit
//! requests, and receive results over channels. No UI code lives here. See `INTEGRATION.md`.

pub mod cache;
pub mod cancel;
pub mod client;
pub mod entry;
pub mod error;
pub mod pool;
pub mod shell;
pub mod transport;
pub mod url;

pub use cancel::Cancel;
pub use client::{Config, HostKeyPrompt, ListEvent, Reply, RemoteFs, TransferEvent, pick_host_key};
pub use entry::{Entry, Kind};
pub use error::Error;
pub use url::{Protocol, Url};

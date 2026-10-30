//! FlameWM assistant core: the ARK AI chat client, the durable chat history store,
//! and speech/model traits. Model and voice downloads use the same curl-based fetcher.
//!
//! No UI dependencies and no async runtime. HTTPS is delegated to `curl`.

mod ark;
mod download;
mod process;
mod sha256;
mod speech;
mod sse;
mod store;
mod voice_setup;

pub use download::{
    check_pinned, default_models_dir, extract_zstd_tar, verify, verify_file, CurlFetcher,
    DiskModelStore, Fetcher,
};
pub use process::{
    default_config_path, expand, start_recording, ProcessRunner, Recording, SpeechConfig,
    SystemRunner,
};
pub use ark::{
    human_error, key_or_default, masked_key, parse_models, validate_key, ArkClient, ArkConfig, Turn,
    ARK_BASE_URL, DEFAULT_API_KEY, DEFAULT_MODEL, FALLBACK_MODELS, HISTORY_TURNS, SYSTEM_PROMPT,
};
pub use sha256::Sha256;
pub use speech::{
    encode_wav, tts_manifest, CommandSpeech, CommandTranscriber, ModelFile, ModelManifest,
    ModelStore, Speech, Stopper, Transcriber, UnconfiguredModelStore, UnconfiguredSpeech,
    UnconfiguredTranscriber, UNPINNED,
};
pub use voice_setup::{apply_defaults, default_voice_dir, ready_model, setup_default};
pub use store::{ChatStore, Message, MsgState, Role, SessionState};

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Every failure the assistant core reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssistantError {
    /// Invalid or unsupported configuration (URL scheme, disabled API server).
    Config(String),
    /// The endpoint or binary could not be reached.
    Unreachable(String),
    /// No bytes arrived within the idle limit.
    Timeout,
    /// The server answered with a non-200 status.
    Http { status: u16, message: String },
    /// The peer sent something this client cannot decode.
    Protocol(String),
    /// Stopped by a `CancelToken`.
    Cancelled,
    /// Local filesystem failure.
    Io(String),
    /// ARK AI ended the reply with an error; the text is the server's one-line message.
    RunFailed(String),
    /// Chat history or session state is unreadable or inconsistent.
    Store(String),
    /// A speech or model capability has no implementation configured.
    NotConfigured(String),
    /// A model has no pinned SHA-256, so it cannot be verified and is not downloaded.
    ChecksumUnavailable(String),
    /// A downloaded or installed model does not match its manifest.
    ChecksumMismatch(String),
}

impl fmt::Display for AssistantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(m) => write!(f, "configuration error: {m}"),
            Self::Unreachable(m) => write!(f, "unreachable: {m}"),
            Self::Timeout => f.write_str("timed out waiting for data"),
            Self::Http { status, message } => write!(f, "HTTP {status}: {message}"),
            Self::Protocol(m) => write!(f, "protocol error: {m}"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::Io(m) => write!(f, "I/O error: {m}"),
            Self::RunFailed(m) => f.write_str(m),
            Self::Store(m) => write!(f, "history store error: {m}"),
            Self::NotConfigured(m) => f.write_str(m),
            Self::ChecksumUnavailable(m) | Self::ChecksumMismatch(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for AssistantError {}

impl From<std::io::Error> for AssistantError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

/// Shared cancellation flag. Clones observe the same state.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    /// Serves one connection per entry of `responses`. Each response is written
    /// in pieces to exercise framing across reads. Returns the base URL and the
    /// raw request heads seen, in order.
    pub fn serve(responses: Vec<Vec<Vec<u8>>>) -> (String, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let handle = thread::spawn(move || {
            let mut seen = Vec::new();
            for pieces in responses {
                let (mut conn, _) = listener.accept().expect("accept");
                seen.push(read_request(&mut conn));
                for piece in pieces {
                    conn.write_all(&piece).expect("write");
                    conn.flush().expect("flush");
                    thread::sleep(Duration::from_millis(5));
                }
                // Hold the connection open briefly so cancel tests see a stall.
                thread::sleep(Duration::from_millis(400));
            }
            seen
        });
        (url, handle)
    }

    pub fn read_request(conn: &mut TcpStream) -> String {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            let n = conn.read(&mut tmp).expect("read");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                let len = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if buf.len() >= pos + 4 + len {
                    break;
                }
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }

    /// Per-test scratch directory under the repository `target-bin` (git-ignored).
    pub fn test_dir(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target-bin/assistant-core-tests")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test dir");
        dir
    }
}

//! WM-side client for the `flamewm-audio-helper` process.
//!
//! The helper owns libpulse. This client spawns it lazily on the first audio command, keeps its
//! stdin for requests, and forwards decoded snapshot events from a reader thread. The helper exits
//! after an idle period, so the next command respawns it.

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use flamewm_api::system::{AudioMuteAction, AudioSnapshot, AudioVolumeAction};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

use crate::audio_protocol::{AudioRequest, SnapshotDecoder, encode_request};

/// Executable name of the helper, installed next to the WM binary.
pub const HELPER_BINARY: &str = "flamewm-audio-helper";

type SnapshotCallback = Box<dyn FnMut(AudioSnapshot) + Send>;

#[derive(Default)]
struct Process {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
}

/// Lazily spawned audio helper; holds no libpulse state in the WM process.
pub struct AudioHelper {
    program: PathBuf,
    on_snapshot: Arc<Mutex<SnapshotCallback>>,
    process: RefCell<Process>,
}

impl AudioHelper {
    /// Create the client. The helper is not started until the first request.
    pub fn connect<F>(on_snapshot: F) -> FlameResult<Self>
    where
        F: FnMut(AudioSnapshot) + Send + 'static,
    {
        Ok(Self {
            program: helper_program(),
            on_snapshot: Arc::new(Mutex::new(Box::new(on_snapshot))),
            process: RefCell::new(Process::default()),
        })
    }

    /// Ask the helper for its current snapshot, starting it if needed.
    pub fn request_snapshot(&self) -> FlameResult<()> {
        self.send(&AudioRequest::Get)
    }

    pub fn set_volume(&self, action: AudioVolumeAction) -> FlameResult<()> {
        self.send(&AudioRequest::Volume(action))
    }

    pub fn set_mute(&self, action: AudioMuteAction) -> FlameResult<()> {
        self.send(&AudioRequest::Mute(action))
    }

    fn send(&self, request: &AudioRequest) -> FlameResult<()> {
        let line = encode_request(request);
        let mut process = self.process.borrow_mut();
        for _attempt in 0..2 {
            reap_exited(&mut process);
            if process.stdin.is_none() {
                self.spawn(&mut process)?;
            }
            let Some(stdin) = process.stdin.as_mut() else {
                break;
            };
            if writeln!(stdin, "{line}").and_then(|()| stdin.flush()).is_ok() {
                return Ok(());
            }
            process.stdin = None;
        }
        Err(FlameError::new(
            ErrorCode::Unavailable,
            "audio helper is not accepting requests",
        ))
    }

    fn spawn(&self, process: &mut Process) -> FlameResult<()> {
        let mut child = Command::new(&self.program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| {
                FlameError::new(
                    ErrorCode::IoFailure,
                    format!("failed to start {HELPER_BINARY}: {error}"),
                )
            })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        if let Some(stdout) = stdout {
            let callback = Arc::clone(&self.on_snapshot);
            thread::Builder::new()
                .name("flamewm-audio-reader".to_owned())
                .spawn(move || read_events(stdout, &callback))
                .map_err(|error| FlameError::new(ErrorCode::IoFailure, error.to_string()))?;
        }
        process.child = Some(child);
        process.stdin = stdin;
        Ok(())
    }
}

fn read_events(stdout: std::process::ChildStdout, callback: &Mutex<SnapshotCallback>) {
    let mut decoder = SnapshotDecoder::default();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else {
            break;
        };
        match decoder.push_line(&line) {
            Ok(Some(snapshot)) => {
                if let Ok(mut emit) = callback.lock() {
                    emit(snapshot);
                }
            }
            Ok(None) => {}
            Err(_) => decoder = SnapshotDecoder::default(),
        }
    }
}

fn reap_exited(process: &mut Process) {
    let exited = match process.child.as_mut() {
        Some(child) => matches!(child.try_wait(), Ok(Some(_)) | Err(_)),
        None => false,
    };
    if exited {
        process.child = None;
        process.stdin = None;
    }
}

fn helper_program() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(HELPER_BINARY)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(HELPER_BINARY))
}

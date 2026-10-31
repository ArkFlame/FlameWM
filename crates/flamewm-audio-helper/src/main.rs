//! Audio helper: owns the `PulseAudio` connection for the window manager.
//!
//! Reads requests on stdin and writes snapshot blocks on stdout. It exits when stdin closes or
//! when no request arrives for `IDLE_EXIT`; the window manager respawns it on the next request.

mod session;

use std::io::{self, BufRead, Write};
use std::process::ExitCode;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use flamewm_api::FlameResult;
use flamewm_api::system::{AudioMuteAction, AudioSnapshot, AudioVolumeAction, ServiceAvailability};
use flamewm_integrations_linux::audio_protocol::encode_snapshot;
use flamewm_integrations_linux::pulse::PulseAdapter;

use session::{Backend, READY_WAIT, serve};

const IDLE_EXIT: Duration = Duration::from_secs(60);

type Stdout = Arc<Mutex<io::Stdout>>;

impl Backend for PulseAdapter {
    fn snapshot(&self) -> AudioSnapshot {
        PulseAdapter::snapshot(self)
    }

    fn set_volume(&self, action: AudioVolumeAction) -> FlameResult<()> {
        PulseAdapter::set_volume(self, action)
    }

    fn set_mute(&self, action: AudioMuteAction) -> FlameResult<()> {
        PulseAdapter::set_mute(self, action)
    }
}

fn publish(stdout: &Stdout, snapshot: &AudioSnapshot) {
    if let Ok(mut out) = stdout.lock() {
        let _ = out.write_all(encode_snapshot(snapshot).as_bytes());
        let _ = out.flush();
    }
}

fn main() -> ExitCode {
    let stdout: Stdout = Arc::new(Mutex::new(io::stdout()));
    let publisher = Arc::clone(&stdout);
    let adapter = match PulseAdapter::connect(move |snapshot| publish(&publisher, &snapshot)) {
        Ok(adapter) => adapter,
        Err(error) => {
            eprintln!("flamewm-audio-helper: PulseAudio unavailable: {error}");
            publish(
                &stdout,
                &AudioSnapshot {
                    availability: ServiceAvailability::Unavailable,
                    ..AudioSnapshot::default()
                },
            );
            return ExitCode::FAILURE;
        }
    };

    let (lines, requests) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            if lines.send(line).is_err() {
                break;
            }
        }
    });

    serve(
        &adapter,
        &requests,
        &|snapshot| publish(&stdout, snapshot),
        IDLE_EXIT,
        READY_WAIT,
    );
    ExitCode::SUCCESS
}

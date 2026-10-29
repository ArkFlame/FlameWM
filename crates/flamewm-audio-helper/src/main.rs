//! Audio helper: owns the PulseAudio connection for the window manager.
//!
//! Reads requests on stdin and writes snapshot blocks on stdout. It exits when stdin closes or
//! when no request arrives for `IDLE_EXIT`; the window manager respawns it on the next request.

use std::io::{self, BufRead, Write};
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use flamewm_api::system::{AudioSnapshot, ServiceAvailability};
use flamewm_integrations_linux::audio_protocol::{AudioRequest, encode_snapshot, parse_request};
use flamewm_integrations_linux::pulse::PulseAdapter;

const IDLE_EXIT: Duration = Duration::from_secs(60);

type Stdout = Arc<Mutex<io::Stdout>>;

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

    loop {
        let line = match requests.recv_timeout(IDLE_EXIT) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        };
        match parse_request(&line) {
            Ok(AudioRequest::Get) => publish(&stdout, &adapter.snapshot()),
            Ok(AudioRequest::Volume(action)) => {
                if let Err(error) = adapter.set_volume(action) {
                    eprintln!("flamewm-audio-helper: volume rejected: {error}");
                }
            }
            Ok(AudioRequest::Mute(action)) => {
                if let Err(error) = adapter.set_mute(action) {
                    eprintln!("flamewm-audio-helper: mute rejected: {error}");
                }
            }
            Err(error) => eprintln!("flamewm-audio-helper: {error}"),
        }
    }
    ExitCode::SUCCESS
}

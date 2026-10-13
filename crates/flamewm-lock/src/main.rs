use std::path::PathBuf;
use std::process::ExitCode;

use flamewm_lock::instance::{Acquire, acquire};
use flamewm_lock::{document, run, session, signals};

fn main() -> ExitCode {
    if std::env::args().any(|arg| arg == "--self-test") {
        return match document::self_test() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("flamewm-lock: self-test failed: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if std::env::args().any(|arg| arg == "--preview") {
        // Settings preview: no single-instance guard (the spawner owns it), no grabs, no PAM.
        let Some(username) = session::username() else {
            eprintln!("flamewm-lock: cannot determine the session user; no preview");
            return ExitCode::FAILURE;
        };
        let settings = session::load_settings();
        return match run::run_preview(&settings, &username) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("flamewm-lock: preview: {error}");
                ExitCode::FAILURE
            }
        };
    }
    // Ignore before any lock work so a stray SIGTERM/SIGINT/SIGHUP never unlocks.
    signals::ignore_unlock_signals();
    let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
        eprintln!("flamewm-lock: XDG_RUNTIME_DIR is not set; refusing to lock");
        return ExitCode::FAILURE;
    };
    let pid_path = PathBuf::from(runtime_dir).join("flamewm-lock.pid");
    let _guard = match acquire(&pid_path) {
        Ok(Acquire::Acquired(guard)) => guard,
        Ok(Acquire::AlreadyRunning) => return ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("flamewm-lock: cannot take single-instance lock: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(username) = session::username() else {
        eprintln!("flamewm-lock: cannot determine the session user; not locking");
        return ExitCode::FAILURE;
    };
    let settings = session::load_settings();
    match run::run(&settings, &username) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("flamewm-lock: {error}");
            ExitCode::FAILURE
        }
    }
}

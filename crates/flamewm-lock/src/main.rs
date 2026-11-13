use std::path::PathBuf;
use std::process::ExitCode;

use flamewm_lock::instance::{Acquire, acquire};
use flamewm_lock::{document, session, signals};

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
    #[cfg(feature = "x11")]
    if std::env::args().any(|arg| arg == "--preview") {
        // Settings preview: no single-instance guard (the spawner owns it), no grabs, no PAM.
        let Some(username) = session::username() else {
            eprintln!("flamewm-lock: cannot determine the session user; no preview");
            return ExitCode::FAILURE;
        };
        let settings = session::load_settings();
        return match flamewm_lock::run::run_preview(&settings, &username) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("flamewm-lock: preview: {error}");
                ExitCode::FAILURE
            }
        };
    }
    #[cfg(feature = "x11")]
    if std::env::args().any(|arg| arg == "--preview-screensaver") {
        // Settings screensaver preview: ribbons only, no grabs, no PAM, no DPMS.
        let Some(username) = session::username() else {
            eprintln!("flamewm-lock: cannot determine the session user; no screensaver preview");
            return ExitCode::FAILURE;
        };
        let settings = session::load_settings();
        return match flamewm_lock::run::run_screensaver_preview(&settings, &username) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("flamewm-lock: screensaver preview: {error}");
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
    // X11 takes the keyboard grab before loading settings: the settings D-Bus call can take
    // seconds to time out, and keys typed meanwhile must never reach another client.
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return present_x11(&username);
    }
    let settings = session::load_settings();
    present_wayland_session(&settings, &username)
}

/// Locks a Wayland session through the lock protocol. It has no X server to paint through, so it
/// paints the same document with the software renderer.
fn present_wayland_session(
    settings: &flamewm_lock::settings::LockSettings,
    username: &str,
) -> ExitCode {
    #[cfg(feature = "wayland")]
    return present_wayland(settings, username);
    #[cfg(not(feature = "wayland"))]
    {
        let _ = (settings, username);
        eprintln!("flamewm-lock: a Wayland session needs the `wayland` feature; not locking");
        ExitCode::FAILURE
    }
}

#[cfg(feature = "x11")]
fn present_x11(username: &str) -> ExitCode {
    use flamewm_lock::x11::Session;

    let session = match Session::open_grabbed() {
        Ok(session) => session,
        Err(error) => {
            eprintln!("flamewm-lock: {error}");
            return ExitCode::FAILURE;
        }
    };
    let settings = session::load_settings();
    match flamewm_lock::run::run(session, &settings, username) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("flamewm-lock: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(feature = "x11"))]
fn present_x11(_username: &str) -> ExitCode {
    eprintln!("flamewm-lock: an X11 session needs the `x11` feature; not locking");
    ExitCode::FAILURE
}

/// Paints the lock for each configured surface: the same document the X11 path shows.
#[cfg(feature = "wayland")]
fn present_wayland(settings: &flamewm_lock::settings::LockSettings, username: &str) -> ExitCode {
    use flamewm_lock::document;
    use flamewm_lock::present::paint_document;
    use flamewm_lock::wayland;
    use flamewm_ui_document::UiDocumentView;

    let owned_settings = settings.clone();
    let owned_username = username.to_string();
    match wayland::run(settings, username, move |view, width, height| {
        let settings = &owned_settings;
        let _ = &owned_username;
        let template = document::template()?;
        let mut runtime = template.into_runtime_document();
        let background = document::background_image(settings, width, height);
        {
            let mut access = UiDocumentView::from_runtime(&mut runtime, None);
            document::apply_document(&mut access, settings, width, height, background)?;
            document::apply_view(&mut access, view)?;
        }
        paint_document(&runtime, width, height, 1.0)
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("flamewm-lock: {error}");
            ExitCode::FAILURE
        }
    }
}

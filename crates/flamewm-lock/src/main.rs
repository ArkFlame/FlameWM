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
    let settings = session::load_settings();
    present(&settings, &username)
}

/// Locks the session on whichever display server is in use.
///
/// A Wayland session has no X server to paint through, so it takes the lock protocol instead and
/// paints the same document with the software renderer.
fn present(settings: &flamewm_lock::settings::LockSettings, username: &str) -> ExitCode {
    let wants_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    if wants_wayland {
        #[cfg(feature = "wayland")]
        return present_wayland(settings, username);
        #[cfg(not(feature = "wayland"))]
        eprintln!("flamewm-lock: a Wayland session needs the `wayland` feature; not locking");
        #[cfg(not(feature = "wayland"))]
        return ExitCode::FAILURE;
    }
    #[cfg(feature = "x11")]
    return present_x11(settings, username);
    #[cfg(not(feature = "x11"))]
    eprintln!("flamewm-lock: an X11 session needs the `x11` feature; not locking");
    #[cfg(not(feature = "x11"))]
    ExitCode::FAILURE
}

#[cfg(feature = "x11")]
fn present_x11(settings: &flamewm_lock::settings::LockSettings, username: &str) -> ExitCode {
    match flamewm_lock::run::run(settings, username) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("flamewm-lock: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Paints the lock for each configured surface: the same document the X11 path shows.
#[cfg(feature = "wayland")]
fn present_wayland(settings: &flamewm_lock::settings::LockSettings, username: &str) -> ExitCode {
    use flamewm_lock::document;
    use flamewm_lock::present::paint_document;
    use flamewm_lock::secret::PasswordBuffer;
    use flamewm_lock::state::Status;
    use flamewm_lock::view::LockView;
    use flamewm_lock::wayland;
    use flamewm_ui_document::UiDocumentView;

    let owned_settings = settings.clone();
    let owned_username = username.to_string();
    match wayland::run(settings, username, move |width, height| {
        let settings = &owned_settings;
        let username = &owned_username;
        let view = LockView::project(
            &settings,
            None,
            &username,
            &PasswordBuffer::default(),
            Status::Idle,
        );
        let template = document::template()?;
        let mut runtime = template.into_runtime_document();
        let background = document::background_image(&settings, width, height);
        {
            let mut access = UiDocumentView::from_runtime(&mut runtime, None);
            document::apply_document(&mut access, &settings, width, height, background)?;
            document::apply_view(&mut access, &view)?;
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

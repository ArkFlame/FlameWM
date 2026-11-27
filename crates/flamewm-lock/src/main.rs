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
    present_wayland_session(&username)
}

/// Locks a Wayland session through the lock protocol. It has no X server to paint through, so it
/// paints the same document with the software renderer.
fn present_wayland_session(username: &str) -> ExitCode {
    #[cfg(feature = "wayland")]
    return present_wayland(username);
    #[cfg(not(feature = "wayland"))]
    {
        let _ = username;
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
    let result = flamewm_lock::run::run(session, &settings, username);
    // The X11 lock surfaces are gone on either outcome. Release the process-local decoded
    // wallpapers (built-in and custom) now instead of leaving them resident until process exit;
    // a later lock show re-reads the same bytes, so the pixels are unchanged.
    document::release_wallpaper_caches();
    match result {
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
fn present_wayland(username: &str) -> ExitCode {
    use std::cell::RefCell;
    use std::rc::Rc;

    use flamewm_lock::document;
    use flamewm_lock::present::paint_document;
    use flamewm_lock::wayland;
    use flamewm_ui_document::UiDocumentView;

    // Settings load after the lock request (see `wayland::run_with`) and feed the frame closure.
    let shared = Rc::new(RefCell::new(flamewm_lock::settings::LockSettings::default()));
    let painted = Rc::clone(&shared);
    // The blurred wallpaper is about 90% of a repaint (about 200 ms at 1080p), and it only changes with
    // the settings or the output size, so keep the last one instead of re-blurring on every keystroke.
    let backgrounds: Rc<RefCell<Option<(u32, u32, flamewm_ui_document::RuntimeImage)>>> =
        Rc::new(RefCell::new(None));
    let stale = Rc::clone(&backgrounds);
    // A second handle survives the frame closure so the cache can be emptied on unlock, before
    // this function returns, rather than at process teardown.
    let cache = Rc::clone(&backgrounds);
    let load = move || {
        let loaded = session::load_settings();
        *shared.borrow_mut() = loaded.clone();
        *stale.borrow_mut() = None;
        loaded
    };
    let result = wayland::run(load, username, move |view, width, height| {
        let current = painted.borrow();
        let settings = &*current;
        let template = document::template()?;
        let mut runtime = template.into_runtime_document();
        let background = {
            let mut cache = backgrounds.borrow_mut();
            match cache.as_ref() {
                Some((w, h, image)) if (*w, *h) == (width, height) => image.clone(),
                _ => {
                    let image = document::background_image(settings, width, height);
                    *cache = Some((width, height, image.clone()));
                    image
                }
            }
        };
        {
            let mut access = UiDocumentView::from_runtime(&mut runtime, None);
            document::apply_document(&mut access, settings, width, height, background)?;
            document::apply_view(&mut access, view)?;
            // The lock has no power actions on Wayland: the idle view keeps the rows disabled and
            // hides the confirm dialog and the Other list, which the document ships visible.
            let power = flamewm_lock::power::PowerUi::new(
                flamewm_api::session::SessionCapabilities::default(),
            )
            .view();
            document::apply_power(&mut access, width, height, &power)?;
        }
        paint_document(&runtime, width, height, 1.0)
    });
    // The lock surfaces are gone on either outcome. Drop the cached output-sized raster and the
    // process-local decoded wallpapers (built-in and custom) before returning; a later lock show
    // re-reads the same bytes, so the pixels are unchanged.
    cache.borrow_mut().take();
    document::release_wallpaper_caches();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("flamewm-lock: {error}");
            ExitCode::FAILURE
        }
    }
}

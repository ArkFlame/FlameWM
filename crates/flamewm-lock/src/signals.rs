//! Termination signals are ignored, never handled: the lock screen only ends
//! through a successful authentication.
use std::ffi::c_int;
use std::sync::atomic::{AtomicBool, Ordering};

const SIGHUP: c_int = 1;
const SIGINT: c_int = 2;
const SIGTERM: c_int = 15;
const SIG_IGN: usize = 1;

unsafe extern "C" {
    /// # Safety
    /// `handler` must be `SIG_IGN`, `SIG_DFL`, or a valid signal handler address.
    fn signal(signum: c_int, handler: usize) -> usize;
}

/// Sets SIGHUP, SIGINT and SIGTERM to ignored for the whole process.
pub fn ignore_unlock_signals() {
    for signum in [SIGHUP, SIGINT, SIGTERM] {
        // SAFETY: SIG_IGN is a valid disposition for these catchable signals.
        unsafe {
            signal(signum, SIG_IGN);
        }
    }
}

/// Set by the preview's SIGTERM handler; the preview loop closes when it sees this.
static PREVIEW_TERMINATE: AtomicBool = AtomicBool::new(false);

extern "C" fn on_preview_terminate(_signum: c_int) {
    PREVIEW_TERMINATE.store(true, Ordering::SeqCst);
}

/// Routes SIGTERM to the preview's exit flag. The preview never ignores it.
pub fn install_preview_terminate() {
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        signal(
            SIGTERM,
            on_preview_terminate as extern "C" fn(c_int) as usize,
        );
    }
}

/// `true` once SIGTERM arrived after [`install_preview_terminate`].
pub fn preview_terminate_requested() -> bool {
    PREVIEW_TERMINATE.load(Ordering::SeqCst)
}

//! Termination signals are ignored, never handled: the lock screen only ends
//! through a successful authentication.
use std::ffi::c_int;

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

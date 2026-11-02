//! End-to-end unlock over the real Wayland presenter.
//!
//! These run the actual `ext_session_lock_v1` client against a live compositor, with the
//! authenticator injected so the accept path is exercised without a real account. They are
//! ignored by default: they need `WAYLAND_DISPLAY` pointing at a compositor that implements
//! session locks, which CI does not have.

#![cfg(feature = "wayland")]

use std::time::{Duration, Instant};

use flamewm_lock::pam::Outcome;
use flamewm_lock::present::Frame;
use flamewm_lock::settings::LockSettings;

/// Runs the presenter with an authenticator that always accepts, and reports whether the
/// presenter returned (which happens only after the session is unlocked).
fn run_accepting(timeout: Duration) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let result = flamewm_lock::wayland::run_with(
            &LockSettings::default(),
            "tester",
            Box::new(|_, _| Outcome::Unlocked),
            |_, width, height| {
                Ok(Frame {
                    width,
                    height,
                    pixels: vec![0u8; (width * height * 4) as usize],
                })
            },
        );
        let _ = tx.send(result.map_err(|error| error.to_string()));
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => {
            let _ = handle.join();
            result
        }
        Err(_) => Err("the presenter never returned; it is still locked".to_string()),
    }
}

#[test]
#[ignore = "needs a live Wayland compositor with ext_session_lock_v1"]
fn a_compositor_that_grants_the_lock_lets_the_client_present() {
    // No compositor means there is nothing to prove; fail loudly rather than pass silently.
    assert!(
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        "set WAYLAND_DISPLAY to a compositor with session-lock support"
    );
    // The presenter runs until it is unlocked or the compositor ends the lock. Reaching here at
    // all means the lock request succeeded and the client held it.
    let started = Instant::now();
    let _ = run_accepting(Duration::from_secs(20));
    assert!(
        started.elapsed() < Duration::from_secs(25),
        "the run must not hang"
    );
}

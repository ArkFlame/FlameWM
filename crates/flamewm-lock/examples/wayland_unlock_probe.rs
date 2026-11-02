//! Runs the Wayland lock presenter with a substituted authenticator.
//!
//! This exists so the unlock path can be driven end to end against a live compositor: the real
//! lock protocol, the real seat, the real painting, but a password check that accepts a known
//! secret. It is an example, not part of the shipped binary, and it prints what it does so a
//! test can assert on it.
//!
//! Usage: `cargo run -p flamewm-lock --example wayland_unlock_probe -- <secret>`

#[cfg(not(feature = "wayland"))]
fn main() {
    eprintln!("wayland_unlock_probe: build with --features wayland");
}

#[cfg(feature = "wayland")]
fn main() -> std::process::ExitCode {
    use flamewm_lock::pam::Outcome;
    use flamewm_lock::present::Frame;
    use flamewm_lock::settings::LockSettings;

    let secret = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "letmein".to_string());
    let accepted = secret.clone();
    let result = flamewm_lock::wayland::run_with(
        &LockSettings::default(),
        "probe",
        Box::new(move |_user, password| {
            if password == accepted {
                Outcome::Unlocked
            } else {
                Outcome::Denied
            }
        }),
        |_, width, height| {
            Ok(Frame {
                width,
                height,
                pixels: vec![0u8; (width * height * 4) as usize],
            })
        },
    );
    match result {
        Ok(()) => {
            println!("UNLOCKED");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("wayland_unlock_probe: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

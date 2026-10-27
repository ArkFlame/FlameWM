//! One executable for the `FlameWM` applications, installed once with every application name as a
//! hard link to it.
//!
//! The applications statically link the same renderer, image decoders, X11 and D-Bus code. As
//! separate executables each process mapped its own copy of those pages; as links to one file the
//! kernel shares them between the WM, desktop, shell, terminal, explorer and settings. The name a
//! process was started as selects the application, and a hard link keeps `current_exe()` (and
//! with it every packaged-asset lookup) pointing at that name.

use std::path::Path;
use std::process::ExitCode;

/// The applications, by the executable name each one is installed as.
const APPLICATIONS: [&str; 7] = [
    "flamewm",
    "flamewm-desktop",
    "flamewm-shell",
    "flamewm-terminal",
    "flamewm-explorer",
    "flamewm-settings",
    "flamewm-task-manager",
];

fn main() -> ExitCode {
    let invoked = std::env::args_os()
        .next()
        .and_then(|arg0| {
            Path::new(&arg0)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    match invoked.as_str() {
        "flamewm" => flamewm_wm::entry::main(),
        "flamewm-desktop" => {
            // The desktop's own `main` returns its error; report it the way the standard
            // `Termination` impl did for the standalone binary.
            if let Err(error) = flamewm_desktop::entry::main() {
                eprintln!("Error: {error:?}");
                return ExitCode::FAILURE;
            }
        }
        "flamewm-shell" => flamewm_shell::entry::main(),
        "flamewm-terminal" => flamewm_terminal::entry::main(),
        "flamewm-explorer" => flamewm_explorer::entry::main(),
        "flamewm-settings" => flamewm_settings::entry::main(),
        "flamewm-task-manager" => {
            if let Err(error) = flamewm_task_manager::run() {
                eprintln!("flamewm-task-manager: {error}");
                return ExitCode::FAILURE;
            }
        },
        _ => {
            eprintln!(
                "flamewm-apps: start an application through its link: {}",
                APPLICATIONS.join(", ")
            );
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}

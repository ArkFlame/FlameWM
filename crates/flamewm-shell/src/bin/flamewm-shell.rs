use std::env;
use std::path::PathBuf;

use flamewm_session_core::diagnostics::{fallback_log_path, install_panic_hook, session_log_path};
use rustix::process::getuid;

fn main() {
    let fallback = fallback_log_path(getuid().as_raw());
    let home = env::var_os("HOME").map(PathBuf::from);
    install_panic_hook(session_log_path(
        env::var_os("XDG_STATE_HOME").as_deref(),
        home.as_deref(),
        &fallback,
    ));
    flamewm_shell::entry::main();
}

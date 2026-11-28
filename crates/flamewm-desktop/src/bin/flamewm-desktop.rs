use std::env;
use std::path::PathBuf;

use flamewm_session_core::diagnostics::{install_panic_hook, session_log_path};

fn main() -> Result<(), String> {
    let fallback = PathBuf::from("/tmp/flamewm-session.log");
    let home = env::var_os("HOME").map(PathBuf::from);
    install_panic_hook(session_log_path(
        env::var_os("XDG_STATE_HOME").as_deref(),
        home.as_deref(),
        &fallback,
    ));
    flamewm_desktop::entry::main()
}

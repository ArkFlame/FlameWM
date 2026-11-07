//! Logout and power capability checks for the session actions.

/// Whether this compositor may end the session itself.
///
/// Logout is only honest when the process chain is the one the session wrapper owns: the wrapper
/// started the compositor with `FLAMEWM_SESSION_LOGOUT_OWNER=1` and told it the wrapper's pid, and
/// the wrapper is still this process's parent. Anything else — a bare start, a reparented process,
/// an inherited variable — means there is no session to return, so the menu entry stays hidden
/// instead of offering an action that would do nothing.
pub(super) fn logout_supported() -> bool {
    managed_logout(
        std::env::var_os("FLAMEWM_SESSION_LOGOUT_OWNER").as_deref(),
        std::env::var_os("FLAMEWM_SESSION_SUPERVISOR_PID").as_deref(),
        rustix::process::getppid(),
    )
}

/// The wrapper claims ownership and names its own pid; that pid must be our parent.
pub(super) fn managed_logout(
    owner: Option<&std::ffi::OsStr>,
    supervisor: Option<&std::ffi::OsStr>,
    parent: Option<rustix::process::Pid>,
) -> bool {
    owner == Some(std::ffi::OsStr::new("1"))
        && supervisor
            .and_then(std::ffi::OsStr::to_str)
            .and_then(|pid| pid.parse::<i32>().ok())
            .and_then(rustix::process::Pid::from_raw)
            .is_some_and(|pid| Some(pid) == parent)
}

/// `systemctl`, when installed: suspend, reboot and shutdown go through logind rather than
/// reinventing session policy here.
pub(super) fn power_program() -> Option<&'static str> {
    ["/usr/bin/systemctl", "/bin/systemctl"]
        .into_iter()
        .find(|program| std::path::Path::new(program).is_file())
}

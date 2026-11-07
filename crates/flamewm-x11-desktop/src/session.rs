use super::*;

pub(super) const SESSION_SUPERVISOR_PID: &str = "FLAMEWM_SESSION_SUPERVISOR_PID";
pub(super) const SESSION_LOGOUT_OWNER: &str = "FLAMEWM_SESSION_LOGOUT_OWNER";

pub(super) static SESSION_LOGOUT_REQUESTED: AtomicBool = AtomicBool::new(false);

#[must_use]
pub fn session_logout_supported() -> bool {
    session_logout_owner_matches(
        std::env::var_os(SESSION_LOGOUT_OWNER).as_deref(),
        std::env::var_os(SESSION_SUPERVISOR_PID).as_deref(),
        rustix::process::getppid(),
    )
}

pub fn request_session_logout() {
    SESSION_LOGOUT_REQUESTED.store(true, Ordering::Release);
}

pub fn take_session_logout_request() -> bool {
    SESSION_LOGOUT_REQUESTED.swap(false, Ordering::AcqRel)
}

pub(super) fn supervisor_pid_matches(
    value: Option<&OsStr>,
    parent: Option<rustix::process::Pid>,
) -> bool {
    let Some(value) = value.and_then(|value| value.to_str()) else {
        return false;
    };
    value
        .parse::<i32>()
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|pid| Some(pid) == parent)
}

pub(super) fn session_logout_owner_matches(
    owner: Option<&OsStr>,
    supervisor: Option<&OsStr>,
    parent: Option<rustix::process::Pid>,
) -> bool {
    owner == Some(OsStr::new("1")) && supervisor_pid_matches(supervisor, parent)
}

#[cfg(test)]
mod session_logout_tests {
    use std::ffi::OsStr;

    use rustix::process::Pid;

    use super::{
        request_session_logout, session_logout_owner_matches, take_session_logout_request,
    };

    #[test]
    fn logout_requires_the_current_supervisor_process() {
        let supervisor = Pid::from_raw(1234).unwrap();
        assert!(session_logout_owner_matches(
            Some(OsStr::new("1")),
            Some(OsStr::new("1234")),
            Some(supervisor)
        ));
        assert!(!session_logout_owner_matches(
            None,
            Some(OsStr::new("1235")),
            Some(supervisor)
        ));
        assert!(!session_logout_owner_matches(
            Some(OsStr::new("1")),
            Some(OsStr::new("not-a-pid")),
            Some(supervisor)
        ));
        assert!(!session_logout_owner_matches(
            Some(OsStr::new("1")),
            Some(OsStr::new("1235")),
            Some(supervisor)
        ));
    }

    #[test]
    fn repeated_logout_requests_collapse_to_one_clean_exit_intent() {
        let _ = take_session_logout_request();
        request_session_logout();
        request_session_logout();
        assert!(take_session_logout_request());
        assert!(!take_session_logout_request());
    }
}

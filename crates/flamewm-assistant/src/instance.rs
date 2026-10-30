//! One assistant window per session, toggled by the shortcut. A pid file in the per-user runtime
//! folder names the open window. Launching again stops that process and exits, so the same key
//! opens and closes the assistant. A stale pid file (dead process, or a pid reused by another
//! program) is ignored.

use std::fs;
use std::path::{Path, PathBuf};

use rustix::process::{Pid, Signal, kill_process, test_kill_process};

const PROGRAM: &str = "flamewm-assistant";
/// Linux keeps only the first 15 bytes of a process name in `/proc/<pid>/comm`.
const COMM_LEN: usize = 15;

/// Result of claiming the session slot.
#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    /// This process owns the slot and should show its window.
    Owned,
    /// A previous window was open and has been stopped; this launch exits.
    Toggled,
}

/// Pid-file location: `$XDG_RUNTIME_DIR/flamewm-assistant.pid`, else a name under the cache.
#[must_use]
pub fn pid_path(runtime_dir: Option<&Path>, cache_dir: Option<&Path>) -> Option<PathBuf> {
    runtime_dir
        .or(cache_dir)
        .map(|dir| dir.join("flamewm-assistant.pid"))
}

/// True when `pid` is alive and its command name is the assistant program.
#[must_use]
pub fn is_live_assistant(pid: i32, proc_root: &Path) -> bool {
    if pid <= 0 {
        return false;
    }
    let Some(pid) = Pid::from_raw(pid) else {
        return false;
    };
    if test_kill_process(pid).is_err() {
        return false;
    }
    let expected = &PROGRAM[..COMM_LEN];
    fs::read_to_string(proc_root.join(pid.as_raw_pid().to_string()).join("comm"))
        .is_ok_and(|comm| comm.trim_end() == expected)
}

/// Stops the open assistant if there is one, otherwise records this process as the open one.
pub fn claim(path: &Path, proc_root: &Path) -> Claim {
    let me = std::process::id();
    if let Some(pid) = fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<i32>().ok())
        .filter(|&pid| pid > 0 && pid as u32 != me)
        .filter(|&pid| is_live_assistant(pid, proc_root))
    {
        if let Some(pid) = Pid::from_raw(pid) {
            let _ = kill_process(pid, Signal::TERM);
        }
        let _ = fs::remove_file(path);
        return Claim::Toggled;
    }
    let _ = fs::write(path, format!("{me}\n"));
    Claim::Owned
}

/// Removes the pid file when it still names this process.
pub fn release(path: &Path) {
    let me = std::process::id().to_string();
    if fs::read_to_string(path).is_ok_and(|text| text.trim() == me) {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_path_prefers_runtime_dir_then_cache() {
        assert_eq!(
            pid_path(Some(Path::new("/run/user/1")), Some(Path::new("/c"))),
            Some(PathBuf::from("/run/user/1/flamewm-assistant.pid"))
        );
        assert_eq!(
            pid_path(None, Some(Path::new("/c"))),
            Some(PathBuf::from("/c/flamewm-assistant.pid"))
        );
        assert_eq!(pid_path(None, None), None);
    }

    #[test]
    fn stale_or_foreign_pids_are_not_live() {
        let proc = std::env::temp_dir().join("flamewm-assistant-proc-test-none");
        assert!(!is_live_assistant(0, &proc));
        assert!(!is_live_assistant(-4, &proc));
        // The current process exists but its comm is the test binary, not the assistant.
        let me = std::process::id() as i32;
        assert!(!is_live_assistant(me, Path::new("/proc")));
    }

    #[test]
    fn comm_match_uses_the_kernel_truncated_name() {
        assert_eq!(&PROGRAM[..COMM_LEN], "flamewm-assista");
    }
}

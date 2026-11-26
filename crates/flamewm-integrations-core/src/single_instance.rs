//! Toggle-style single instance: a pid file names the open window of one program per session.
//! Launching again stops that process and exits, so the same shortcut opens and closes it.
//! Precedent: freedesktop/KDE `KDBusService` (one owner per app, later launches defer to it), reduced
//! to the pid-file form the shortcut toggle needs. A stale pid file (dead process, or a pid reused by
//! another program whose `comm` differs) is ignored.

use std::fs;
use std::path::Path;

use rustix::process::{Pid, Signal, kill_process, test_kill_process};

/// Result of claiming the session slot.
#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    /// This process owns the slot and should show its window.
    Owned,
    /// A previous window was open and has been stopped; this launch should exit.
    Toggled,
}

/// True when `pid` is alive and its `/proc/<pid>/comm` equals `comm` (the kernel-truncated name).
#[must_use]
pub fn is_live_program(pid: i32, proc_root: &Path, comm: &str) -> bool {
    if pid <= 0 {
        return false;
    }
    let Some(pid) = Pid::from_raw(pid) else {
        return false;
    };
    if test_kill_process(pid).is_err() {
        return false;
    }
    fs::read_to_string(proc_root.join(pid.as_raw_pid().to_string()).join("comm"))
        .is_ok_and(|text| text.trim_end() == comm)
}

/// Stops the open instance named by `path` if there is one, otherwise records this process as the open one.
#[must_use]
pub fn claim(path: &Path, proc_root: &Path, comm: &str) -> Claim {
    let me = std::process::id();
    if let Some(pid) = fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<i32>().ok())
        .filter(|&pid| pid > 0 && pid as u32 != me)
        .filter(|&pid| is_live_program(pid, proc_root, comm))
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
    fn stale_or_foreign_pids_are_not_live() {
        let proc = std::env::temp_dir().join("single-instance-proc-test-none");
        assert!(!is_live_program(0, &proc, "x"));
        assert!(!is_live_program(-4, &proc, "x"));
        assert!(!is_live_program(i32::MAX, &proc, "x"));
    }
}

//! One finder window per session, toggled by the shortcut. A pid file in the per-user runtime
//! folder names the open window. Launching again stops that process and exits, so the same key
//! opens and closes the finder. A stale pid file (dead process, or a pid reused by another program)
//! is ignored.

use std::fs;
use std::path::{Path, PathBuf};

use rustix::process::{Pid, Signal, kill_process, test_kill_process};

const PROGRAM: &str = "flamewm-finder";

/// Result of claiming the session slot.
#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    /// This process owns the slot and should show its window.
    Owned,
    /// A previous window was closed; this launch is the toggle-off and should exit.
    Toggled,
}

/// Pid-file location: `$XDG_RUNTIME_DIR/flamewm-finder.pid`, else a per-user name under the cache.
#[must_use]
pub fn pid_path(runtime_dir: Option<&Path>, cache_dir: Option<&Path>) -> Option<PathBuf> {
    runtime_dir
        .map(|dir| dir.join("flamewm-finder.pid"))
        .or_else(|| cache_dir.map(|dir| dir.join("flamewm-finder.pid")))
}

/// True when `pid` is alive and its command name is the finder program.
#[must_use]
pub fn is_live_finder(pid: i32, proc_root: &Path) -> bool {
    let Some(pid) = Pid::from_raw(pid) else {
        return false;
    };
    if test_kill_process(pid).is_err() {
        return false;
    }
    fs::read_to_string(proc_root.join(pid.as_raw_pid().to_string()).join("comm"))
        .is_ok_and(|comm| comm.trim_end() == PROGRAM)
}

/// Stops the open finder if there is one, otherwise records this process as the open one.
pub fn claim(path: &Path, proc_root: &Path) -> Claim {
    let me = std::process::id();
    if let Some(pid) = fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse::<i32>().ok())
        .filter(|&pid| pid > 0 && pid as u32 != me)
        .filter(|&pid| is_live_finder(pid, proc_root))
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
    fn pid_path_prefers_runtime_dir() {
        assert_eq!(
            pid_path(Some(Path::new("/run/user/1")), Some(Path::new("/c"))),
            Some(PathBuf::from("/run/user/1/flamewm-finder.pid"))
        );
        assert_eq!(
            pid_path(None, Some(Path::new("/c"))),
            Some(PathBuf::from("/c/flamewm-finder.pid"))
        );
        assert_eq!(pid_path(None, None), None);
    }

    #[test]
    fn missing_or_foreign_process_is_not_a_live_finder() {
        let proc = std::env::temp_dir().join(format!("finder-proc-{}", std::process::id()));
        fs::create_dir_all(proc.join("1")).unwrap();
        fs::write(proc.join("1").join("comm"), "bash\n").unwrap();
        assert!(!is_live_finder(0, &proc));
        assert!(!is_live_finder(i32::MAX, &proc));
        let _ = fs::remove_dir_all(&proc);
    }

    #[test]
    fn claim_owns_fresh_slot_and_release_clears_it() {
        let dir = std::env::temp_dir().join(format!("finder-claim-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("flamewm-finder.pid");
        let _ = fs::remove_file(&pid_file);
        assert_eq!(claim(&pid_file, &dir), Claim::Owned);
        assert_eq!(claim(&pid_file, &dir), Claim::Owned);
        release(&pid_file);
        assert!(!pid_file.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}

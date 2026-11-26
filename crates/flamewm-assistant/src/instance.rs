//! One assistant window per session, toggled by the shortcut. A pid file in the per-user runtime
//! folder names the open window. Launching again stops that process and exits, so the same key
//! opens and closes the assistant. The claim rules are owned by `flamewm_integrations_core::single_instance`.

use std::path::{Path, PathBuf};

pub use flamewm_integrations_core::single_instance::{Claim, release};

const PROGRAM: &str = "flamewm-assistant";
/// Linux keeps only the first 15 bytes of a process name in `/proc/<pid>/comm`.
const COMM_LEN: usize = 15;

/// Pid-file location: `$XDG_RUNTIME_DIR/flamewm-assistant.pid`, else a name under the cache.
#[must_use]
pub fn pid_path(runtime_dir: Option<&Path>, cache_dir: Option<&Path>) -> Option<PathBuf> {
    runtime_dir
        .or(cache_dir)
        .map(|dir| dir.join("flamewm-assistant.pid"))
}

/// True when `pid` is alive and its command name is the assistant program.
#[must_use]
#[cfg(test)]
pub fn is_live_assistant(pid: i32, proc_root: &Path) -> bool {
    flamewm_integrations_core::single_instance::is_live_program(
        pid,
        proc_root,
        &PROGRAM[..COMM_LEN],
    )
}

/// Stops the open assistant if there is one, otherwise records this process as the open one.
pub fn claim(path: &Path, proc_root: &Path) -> Claim {
    flamewm_integrations_core::single_instance::claim(path, proc_root, &PROGRAM[..COMM_LEN])
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

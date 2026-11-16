//! One finder window per session, toggled by the shortcut. A pid file in the per-user runtime
//! folder names the open window. Launching again stops that process and exits, so the same key
//! opens and closes the finder. The claim rules are owned by `flamewm_integrations_core::single_instance`.

use std::path::{Path, PathBuf};

pub use flamewm_integrations_core::single_instance::{Claim, release};

const PROGRAM: &str = "flamewm-finder";

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
    flamewm_integrations_core::single_instance::is_live_program(pid, proc_root, PROGRAM)
}

/// Stops the open finder if there is one, otherwise records this process as the open one.
pub fn claim(path: &Path, proc_root: &Path) -> Claim {
    flamewm_integrations_core::single_instance::claim(path, proc_root, PROGRAM)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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

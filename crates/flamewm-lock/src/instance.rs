use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

use rustix::fs::{FlockOperation, flock};
use rustix::io::Errno;

/// Holds the single-instance lock for as long as the value lives.
#[derive(Debug)]
pub struct InstanceGuard {
    _file: File,
}

#[derive(Debug)]
pub enum Acquire {
    Acquired(InstanceGuard),
    AlreadyRunning,
}

/// Takes an exclusive, non-blocking `flock` on `path`. The kernel releases it
/// when the process exits, so a crashed lock screen never wedges the next one.
pub fn acquire(path: &Path) -> io::Result<Acquire> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(Acquire::Acquired(InstanceGuard { _file: file })),
        Err(Errno::WOULDBLOCK) => Ok(Acquire::AlreadyRunning),
        Err(error) => Err(io::Error::from(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_reports_already_running_until_first_drops() {
        let dir = std::env::temp_dir().join(format!("flamewm-lock-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("flamewm-lock.pid");

        let first = match acquire(&path).unwrap() {
            Acquire::Acquired(guard) => guard,
            Acquire::AlreadyRunning => panic!("first instance must acquire"),
        };
        assert!(matches!(acquire(&path).unwrap(), Acquire::AlreadyRunning));
        drop(first);
        assert!(matches!(acquire(&path).unwrap(), Acquire::Acquired(_)));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}

//! Background CPU priority for worker threads that must never compete with the UI or reactor.
//!
//! Linux nice values are per thread: `setpriority(PRIO_PROCESS, gettid())` lowers only the calling
//! thread, so the event loop and render threads keep their priority. Child processes spawned by a
//! lowered thread inherit the value. `SCHED_IDLE` and `IOPRIO_CLASS_IDLE` need raw syscalls, which the
//! `unsafe_code = forbid` workspace lint does not allow here, so they are not applied.

use std::io;
use std::sync::Once;

use rustix::process::{getpriority_process, setpriority_process};
use rustix::thread::gettid;

/// Nice value applied to background workers (a positive increment only lowers priority).
pub const BACKGROUND_NICE: i32 = 10;

static FAILURE_LOGGED: Once = Once::new();

/// Lowers the calling thread to [`BACKGROUND_NICE`]. Never raises an already lower priority.
///
/// Returns `true` when the thread is at or below background priority afterwards. A failure is
/// logged once per process and otherwise ignored: the worker still runs at normal priority.
pub fn lower_current_thread() -> bool {
    let tid = Some(gettid());
    let target =
        getpriority_process(tid).map_or(BACKGROUND_NICE, |current| current.max(BACKGROUND_NICE));
    match setpriority_process(tid, target) {
        Ok(()) => true,
        Err(error) => {
            FAILURE_LOGGED.call_once(|| {
                eprintln!("flamewm-integrations-linux: background priority unavailable: {error}");
            });
            false
        }
    }
}

/// Nice value of the calling thread.
pub fn current_thread_nice() -> io::Result<i32> {
    getpriority_process(Some(gettid())).map_err(io::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nice_field_of_child(program: &str) -> i32 {
        // /proc/<pid>/stat: the nice value is field 19; fields after the `(comm)` start at field 3.
        let output = std::process::Command::new(program)
            .arg("/proc/self/stat")
            .output()
            .expect("spawn stat reader");
        let text = String::from_utf8(output.stdout).expect("utf8 stat");
        let after_comm = &text[text.rfind(')').expect("comm end") + 1..];
        after_comm
            .split_whitespace()
            .nth(19 - 3)
            .expect("nice field")
            .parse()
            .expect("nice integer")
    }

    #[test]
    fn lowered_thread_reads_back_background_nice() {
        std::thread::spawn(|| {
            let before = current_thread_nice().expect("getpriority before");
            assert!(lower_current_thread());
            // Never raises: the target is the higher nice of the inherited value and 10.
            assert_eq!(
                current_thread_nice().expect("getpriority"),
                before.max(BACKGROUND_NICE)
            );
        })
        .join()
        .expect("worker thread");
    }

    #[test]
    fn lowering_one_thread_leaves_the_calling_thread_unchanged() {
        let before = current_thread_nice().expect("getpriority before");
        std::thread::spawn(|| assert!(lower_current_thread()))
            .join()
            .expect("worker thread");
        assert_eq!(current_thread_nice().expect("getpriority after"), before);
    }

    #[test]
    fn lowering_never_raises_an_already_lower_priority() {
        std::thread::spawn(|| {
            assert!(lower_current_thread());
            let once = current_thread_nice().expect("getpriority once");
            // A second lowering must keep the value, not reset it to 10.
            assert!(lower_current_thread());
            assert_eq!(current_thread_nice().expect("getpriority twice"), once);
            assert!(once >= BACKGROUND_NICE);
        })
        .join()
        .expect("worker thread");
    }

    #[test]
    fn child_spawned_from_lowered_thread_inherits_background_nice() {
        std::thread::spawn(|| {
            assert!(lower_current_thread());
            let thread_nice = current_thread_nice().expect("getpriority");
            assert_eq!(nice_field_of_child("cat"), thread_nice);
            assert!(thread_nice >= BACKGROUND_NICE);
        })
        .join()
        .expect("worker thread");
    }
}

//! Visibility gate for the process sampler. While the window is obscured the sampler sleeps on a
//! condvar with no timer; uncovering the window wakes it at once so the next sample is taken now.

use std::sync::{Condvar, Mutex, MutexGuard};

pub struct SampleGate {
    visible: Mutex<bool>,
    changed: Condvar,
}

impl Default for SampleGate {
    fn default() -> Self {
        Self {
            visible: Mutex::new(true),
            changed: Condvar::new(),
        }
    }
}

impl SampleGate {
    fn lock(&self) -> MutexGuard<'_, bool> {
        // A poisoned flag still holds a valid bool; keep the gate usable.
        self.visible
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Records the window state and wakes a sleeping sampler when it becomes visible.
    pub fn set_visible(&self, visible: bool) {
        *self.lock() = visible;
        self.changed.notify_all();
    }

    /// Blocks while obscured. Returns `true` once visible, or `false` when `keep_running` reports
    /// shutdown. Shutdown must call `wake` so a sleeping sampler observes it.
    pub fn wait_visible(&self, keep_running: impl Fn() -> bool) -> bool {
        let mut visible = self.lock();
        while !*visible {
            if !keep_running() {
                return false;
            }
            visible = self
                .changed
                .wait(visible)
                .unwrap_or_else(|poison| poison.into_inner());
        }
        keep_running()
    }

    /// Wakes a sampler blocked in `wait_visible` so it can re-check shutdown.
    pub fn wake(&self) {
        let _guard = self.lock();
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::SampleGate;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn visible_gate_never_blocks() {
        let gate = SampleGate::default();
        assert!(gate.wait_visible(|| true));
    }

    #[test]
    fn obscured_sampler_sleeps_until_uncovered_then_resumes() {
        let gate = Arc::new(SampleGate::default());
        gate.set_visible(false);
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let worker = {
            let gate = Arc::clone(&gate);
            thread::spawn(move || {
                let resumed = gate.wait_visible(|| true);
                let _ = done_tx.send(resumed);
            })
        };
        assert!(
            done_rx.recv_timeout(Duration::from_millis(150)).is_err(),
            "must stay asleep while obscured"
        );
        gate.set_visible(true);
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(5)), Ok(true));
        worker.join().unwrap();
    }

    #[test]
    fn shutdown_releases_an_obscured_sampler() {
        let gate = Arc::new(SampleGate::default());
        gate.set_visible(false);
        let running = Arc::new(AtomicBool::new(true));
        let worker = {
            let gate = Arc::clone(&gate);
            let running = Arc::clone(&running);
            thread::spawn(move || gate.wait_visible(|| running.load(Ordering::Relaxed)))
        };
        thread::sleep(Duration::from_millis(50));
        running.store(false, Ordering::Relaxed);
        gate.wake();
        assert!(!worker.join().unwrap());
    }
}

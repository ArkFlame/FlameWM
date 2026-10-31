//! Request loop of the audio helper, generic over the `PulseAudio` backend so tests can drive it
//! with a fake backend and in-memory channels.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use flamewm_api::FlameResult;
use flamewm_api::system::{AudioMuteAction, AudioSnapshot, AudioVolumeAction, ServiceAvailability};
use flamewm_integrations_linux::audio_protocol::{AudioRequest, parse_request};

/// How long the first request of a helper process waits for `PulseAudio` to become ready.
pub const READY_WAIT: Duration = Duration::from_secs(2);
const READY_POLL: Duration = Duration::from_millis(10);

/// The `PulseAudio` operations the request loop needs.
pub trait Backend {
    fn snapshot(&self) -> AudioSnapshot;
    fn set_volume(&self, action: AudioVolumeAction) -> FlameResult<()>;
    fn set_mute(&self, action: AudioMuteAction) -> FlameResult<()>;
}

/// Wait until the backend reports ready, for at most `limit`. Returns whether it became ready.
pub fn wait_until_ready(backend: &impl Backend, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if backend.snapshot().availability == ServiceAvailability::Available {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        thread::sleep(READY_POLL.min(deadline - now));
    }
}

/// Serve requests until the channel closes or no request arrives for `idle`.
///
/// The first request of the process waits up to `ready_wait` for the backend to become ready, so a
/// request sent right after spawn is applied instead of rejected. Later requests do not wait.
pub fn serve(
    backend: &impl Backend,
    requests: &Receiver<String>,
    publish: &dyn Fn(&AudioSnapshot),
    idle: Duration,
    ready_wait: Duration,
) {
    let mut first = true;
    loop {
        let line = match requests.recv_timeout(idle) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return,
        };
        if std::mem::take(&mut first) && !wait_until_ready(backend, ready_wait) {
            eprintln!("flamewm-audio-helper: PulseAudio not ready after {ready_wait:?}");
        }
        match parse_request(&line) {
            Ok(AudioRequest::Get) => publish(&backend.snapshot()),
            Ok(AudioRequest::Volume(action)) => {
                if let Err(error) = backend.set_volume(action) {
                    eprintln!("flamewm-audio-helper: volume rejected: {error}");
                }
            }
            Ok(AudioRequest::Mute(action)) => {
                if let Err(error) = backend.set_mute(action) {
                    eprintln!("flamewm-audio-helper: mute rejected: {error}");
                }
            }
            Err(error) => eprintln!("flamewm-audio-helper: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};

    use flamewm_api::FlameError;

    /// Fake backend: rejects writes until `ready`, like the real context does before Ready.
    #[derive(Default)]
    struct FakeBackend {
        ready: Arc<AtomicBool>,
        applied: RefCell<Vec<String>>,
    }

    impl Backend for FakeBackend {
        fn snapshot(&self) -> AudioSnapshot {
            AudioSnapshot {
                availability: if self.ready.load(Ordering::SeqCst) {
                    ServiceAvailability::Available
                } else {
                    ServiceAvailability::Unavailable
                },
                ..AudioSnapshot::default()
            }
        }

        fn set_volume(&self, action: AudioVolumeAction) -> FlameResult<()> {
            if !self.ready.load(Ordering::SeqCst) {
                return Err(FlameError::unavailable("not ready"));
            }
            self.applied
                .borrow_mut()
                .push(format!("volume {}", action.percent));
            Ok(())
        }

        fn set_mute(&self, action: AudioMuteAction) -> FlameResult<()> {
            if !self.ready.load(Ordering::SeqCst) {
                return Err(FlameError::unavailable("not ready"));
            }
            self.applied
                .borrow_mut()
                .push(format!("mute {}", action.muted));
            Ok(())
        }
    }

    const VOLUME_LINE: &str = "volume sink:1 50 1 1";

    fn feed(lines: &[&str]) -> Receiver<String> {
        let (sender, receiver) = mpsc::channel();
        for line in lines {
            sender.send((*line).to_owned()).expect("queue request");
        }
        receiver
    }

    #[test]
    fn request_before_ready_is_applied_once_ready() {
        let backend = FakeBackend::default();
        let ready = Arc::clone(&backend.ready);
        let requests = feed(&[VOLUME_LINE]);
        let flip = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            ready.store(true, Ordering::SeqCst);
        });
        serve(
            &backend,
            &requests,
            &|_| {},
            Duration::from_secs(1),
            Duration::from_secs(5),
        );
        flip.join().expect("flip thread");
        assert_eq!(*backend.applied.borrow(), vec!["volume 50".to_owned()]);
    }

    #[test]
    fn request_is_rejected_only_after_the_first_ready_wait_expires() {
        let backend = FakeBackend::default();
        let requests = feed(&[VOLUME_LINE, VOLUME_LINE]);
        let started = Instant::now();
        serve(
            &backend,
            &requests,
            &|_| {},
            Duration::from_secs(1),
            Duration::from_millis(50),
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "later requests must not wait again"
        );
        assert!(backend.applied.borrow().is_empty());
    }

    #[test]
    fn get_before_ready_publishes_the_ready_snapshot() {
        let backend = FakeBackend::default();
        let ready = Arc::clone(&backend.ready);
        let requests = feed(&["get"]);
        let published = RefCell::new(Vec::new());
        let flip = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            ready.store(true, Ordering::SeqCst);
        });
        serve(
            &backend,
            &requests,
            &|snapshot| published.borrow_mut().push(snapshot.availability),
            Duration::from_secs(1),
            Duration::from_secs(5),
        );
        flip.join().expect("flip thread");
        assert_eq!(*published.borrow(), vec![ServiceAvailability::Available]);
    }

    #[test]
    fn idle_timeout_ends_the_loop_without_requests() {
        let backend = FakeBackend::default();
        let (_keep_open, requests) = mpsc::channel::<String>();
        let started = Instant::now();
        serve(
            &backend,
            &requests,
            &|_| {},
            Duration::from_millis(50),
            Duration::ZERO,
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn ready_requests_after_the_first_are_applied_immediately() {
        let backend = FakeBackend::default();
        backend.ready.store(true, Ordering::SeqCst);
        let requests = feed(&[VOLUME_LINE, "mute sink:1 1 1 1"]);
        serve(
            &backend,
            &requests,
            &|_| {},
            Duration::from_secs(1),
            READY_WAIT,
        );
        assert_eq!(
            *backend.applied.borrow(),
            vec!["volume 50".to_owned(), "mute true".to_owned()]
        );
    }
}

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};

use flamewm_integrations_linux::PortalFileChooser;

type WakeWriter = Arc<Mutex<UnixStream>>;
struct RegisteredWake {
    id: u64,
    writer: WakeWriter,
}
static NEXT_WAKE_ID: AtomicU64 = AtomicU64::new(1);
static COMPLETION_WAKE: OnceLock<Mutex<Option<RegisteredWake>>> = OnceLock::new();

/// Event-driven completion source installed by the Settings runtime before the UI loop starts.
pub struct WallpaperChooserWake {
    reader: UnixStream,
    id: u64,
}

impl WallpaperChooserWake {
    pub fn new() -> Result<Self, String> {
        let (reader, writer) = UnixStream::pair()
            .map_err(|error| format!("failed to create wallpaper completion wake: {error}"))?;
        reader
            .set_nonblocking(true)
            .map_err(|error| format!("failed to configure wallpaper completion wake: {error}"))?;
        let slot = COMPLETION_WAKE.get_or_init(|| Mutex::new(None));
        let id = NEXT_WAKE_ID.fetch_add(1, Ordering::Relaxed);
        *slot.lock().map_err(|_| "wallpaper wake lock poisoned".to_owned())? = Some(
            RegisteredWake {
                id,
                writer: Arc::new(Mutex::new(writer)),
            },
        );
        Ok(Self { reader, id })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.reader.as_raw_fd()
    }

    /// Drains queued wake bytes. The reactor invokes this only after worker completion.
    pub fn drain(&mut self) {
        let mut bytes = [0u8; 64];
        loop {
            match self.reader.read(&mut bytes) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }
}

impl Drop for WallpaperChooserWake {
    fn drop(&mut self) {
        if let Some(slot) = COMPLETION_WAKE.get() {
            if let Ok(mut slot) = slot.lock() {
                if slot.as_ref().is_some_and(|registered| registered.id == self.id) {
                    slot.take();
                }
            }
        }
    }
}

pub struct WallpaperChooserWorker {
    result: Receiver<Result<Option<PathBuf>, String>>,
    thread: Option<JoinHandle<()>>,
}

impl WallpaperChooserWorker {
    pub fn start() -> Result<Self, String> {
        let (sender, result) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("flamewm-settings-wallpaper-portal".to_owned())
            .spawn(move || {
                let outcome = PortalFileChooser::connect()
                    .and_then(|chooser| chooser.choose_wallpaper(""))
                    .map_err(|error| error.to_string());
                publish_completion(&sender, outcome);
            })
            .map_err(|error| format!("failed to start wallpaper chooser worker: {error}"))?;
        Ok(Self {
            result,
            thread: Some(thread),
        })
    }

    pub fn poll(&mut self) -> Option<Result<Option<PathBuf>, String>> {
        match self.result.try_recv() {
            Ok(result) => {
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
                Some(result)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(
                "wallpaper chooser worker disconnected without a result".to_owned(),
            )),
        }
    }
}

/// Wakes the Settings runtime after any background operation publishes a result.
/// The runtime registers this shared source with the reactor before accepting UI input.
pub(crate) fn notify_background_completion() {
    if let Some(writer) = COMPLETION_WAKE
        .get()
        .and_then(|slot| {
            slot.lock()
                .ok()?
                .as_ref()
                .map(|registered| Arc::clone(&registered.writer))
        })
    {
        if let Ok(mut writer) = writer.lock() {
            let _ = writer.write_all(&[1]);
        }
    }
}

fn publish_completion(
    sender: &mpsc::Sender<Result<Option<PathBuf>, String>>,
    outcome: Result<Option<PathBuf>, String>,
) {
    publish_background_completion(sender, outcome);
}

pub(crate) fn publish_background_completion<T>(sender: &mpsc::Sender<T>, result: T) {
    let _ = sender.send(result);
    notify_background_completion();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_completion_wakes_an_idle_reactor_source() {
        let stale_wake = WallpaperChooserWake::new().unwrap();
        // The wake signals result availability, not success. The UI polls the authoritative
        // result channel after this notification and can therefore preserve state on failure
        // or cancellation while still updating its feedback without another input event.
        let mut wake = WallpaperChooserWake::new().unwrap();
        drop(stale_wake);
        let mut byte = [0u8; 1];
        assert!(wake.reader.read(&mut byte).is_err());
        for outcome in [Err("portal unavailable".to_owned()), Ok(None)] {
            let (sender, result) = mpsc::channel();
            let producer = thread::spawn(move || publish_completion(&sender, outcome));
            // Joining is the deterministic completion barrier: the worker has sent its result
            // and written the wake byte before the nonblocking reactor FD is read.
            producer.join().unwrap();
            let received = result.try_recv().unwrap();
            match received {
                Err(message) => assert_eq!(message, "portal unavailable"),
                Ok(None) => {}
                Ok(Some(path)) => panic!("unexpected selected path: {}", path.display()),
            }
            assert_eq!(wake.reader.read(&mut byte).unwrap(), 1);
        }
        let (sender, result) = mpsc::channel();
        publish_background_completion(&sender, Err::<(), _>("link failed".to_owned()));
        assert_eq!(result.try_recv().unwrap().unwrap_err(), "link failed");
        assert_eq!(wake.reader.read(&mut byte).unwrap(), 1);
        wake.drain();
    }
}

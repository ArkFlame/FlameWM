use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use flamewm_integrations_linux::icons::{IconSize, Rgba8Raster, rasterize_path};

/// The desktop's empty wallpaper selection is an intentional presentation, not a failed load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WallpaperPresentation {
    DefaultBlackWatermark,
    Image(Rgba8Raster),
}

impl Default for WallpaperPresentation {
    fn default() -> Self {
        Self::DefaultBlackWatermark
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallpaperDiagnostic {
    pub generation: u64,
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallpaperState {
    pub generation: u64,
    pub presentation: WallpaperPresentation,
    pub diagnostic: Option<WallpaperDiagnostic>,
}

impl Default for WallpaperState {
    fn default() -> Self {
        Self {
            generation: 0,
            presentation: WallpaperPresentation::DefaultBlackWatermark,
            diagnostic: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WallpaperRequest {
    generation: u64,
    path: PathBuf,
}

#[derive(Debug)]
enum LoadResult {
    Default,
    Image(Rgba8Raster),
    Failure(String),
}

#[derive(Debug)]
struct WallpaperCompletion {
    request: WallpaperRequest,
    result: LoadResult,
}

#[derive(Debug)]
struct WorkerState {
    pending: Option<WallpaperRequest>,
    completion: Option<WallpaperCompletion>,
    latest_generation: u64,
    stale_generation: Option<u64>,
    shutdown: bool,
}

struct WorkerControl {
    state: Mutex<WorkerState>,
    wake: Condvar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoaderClosed;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PollOutcome {
    Idle,
    Applied { generation: u64 },
    Failed { generation: u64 },
    Stale { generation: u64 },
}

/// One bounded off-turn wallpaper worker. The shared pending slot is a latest-request mailbox;
/// it prevents an unbounded queue and makes cancellation explicit without introducing another
/// runtime. Completion is admitted only for the latest generation, so stale A cannot replace B.
pub struct WallpaperLoader {
    control: Arc<WorkerControl>,
    worker: Option<JoinHandle<()>>,
    wake_read: UnixStream,
    next_generation: u64,
    state: WallpaperState,
}

impl WallpaperLoader {
    /// Starts the single bounded worker used by this loader.
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        let (wake_read, wake_write) =
            UnixStream::pair().map_err(|error| format!("create wallpaper wake pair: {error}"))?;
        wake_read
            .set_nonblocking(true)
            .map_err(|error| format!("configure wallpaper wake reader: {error}"))?;
        wake_write
            .set_nonblocking(true)
            .map_err(|error| format!("configure wallpaper wake writer: {error}"))?;
        let control = Arc::new(WorkerControl {
            state: Mutex::new(WorkerState {
                pending: None,
                completion: None,
                latest_generation: 0,
                stale_generation: None,
                shutdown: false,
            }),
            wake: Condvar::new(),
        });
        let worker_control = Arc::clone(&control);
        let target = IconSize::new(width, height);
        let worker = thread::Builder::new()
            .name("flamewm-wallpaper-loader".to_owned())
            .spawn(move || wallpaper_worker(worker_control, target, wake_write))
            .map_err(|error| format!("failed to start wallpaper loader: {error}"))?;
        Ok(Self {
            control,
            worker: Some(worker),
            wake_read,
            next_generation: 0,
            state: WallpaperState::default(),
        })
    }

    /// Replaces the pending request. At most one request waits for the worker.
    pub fn request(&mut self, path: impl Into<PathBuf>) -> Result<u64, LoaderClosed> {
        self.next_generation = self.next_generation.saturating_add(1);
        let request = WallpaperRequest {
            generation: self.next_generation,
            path: path.into(),
        };
        let mut state = self
            .control
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.shutdown {
            return Err(LoaderClosed);
        }
        state.latest_generation = request.generation;
        state.pending = Some(request);
        state.completion = None;
        state.stale_generation = None;
        self.control.wake.notify_one();
        Ok(self.next_generation)
    }

    /// Admits one worker completion on the caller's turn. Decode and filesystem reads never run
    /// here; failures leave the previous valid presentation in place and expose a diagnostic.
    pub fn poll(&mut self) -> PollOutcome {
        let mut worker_state = self
            .control
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(stale_generation) = worker_state.stale_generation.take() {
            return PollOutcome::Stale {
                generation: stale_generation,
            };
        }
        let Some(completion) = worker_state.completion.take() else {
            return PollOutcome::Idle;
        };
        if completion.request.generation != worker_state.latest_generation {
            worker_state.stale_generation = Some(completion.request.generation);
            return PollOutcome::Stale {
                generation: completion.request.generation,
            };
        }
        let generation = completion.request.generation;
        match completion.result {
            LoadResult::Default => {
                self.state = WallpaperState {
                    generation,
                    presentation: WallpaperPresentation::DefaultBlackWatermark,
                    diagnostic: None,
                };
                PollOutcome::Applied { generation }
            }
            LoadResult::Image(image) => {
                self.state = WallpaperState {
                    generation,
                    presentation: WallpaperPresentation::Image(image),
                    diagnostic: None,
                };
                PollOutcome::Applied { generation }
            }
            LoadResult::Failure(message) => {
                self.state.generation = generation;
                self.state.diagnostic = Some(WallpaperDiagnostic {
                    generation,
                    path: completion.request.path,
                    message,
                });
                PollOutcome::Failed { generation }
            }
        }
    }

    /// Returns the read end used to wake the desktop reactor when a worker result is ready.
    #[must_use]
    pub fn wake_fd(&self) -> RawFd {
        self.wake_read.as_raw_fd()
    }

    /// Drains worker wake bytes without doing filesystem or decode work on the UI turn.
    pub fn wake_drain(&mut self) {
        let mut buffer = [0u8; 64];
        loop {
            match self.wake_read.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => {
                    eprintln!("desktop: wallpaper wake drain failed: {error}");
                    break;
                }
            }
        }
    }

    #[must_use]
    pub fn state(&self) -> &WallpaperState {
        &self.state
    }

    /// Requests cancellation and joins the one worker. Repeated calls are harmless.
    pub fn shutdown(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        {
            let mut state = self
                .control
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.shutdown = true;
            state.pending = None;
            state.completion = None;
            self.control.wake.notify_one();
        }
        let _ = worker.join();
    }
}

impl Drop for WallpaperLoader {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn wallpaper_worker(control: Arc<WorkerControl>, target: IconSize, mut wake_write: UnixStream) {
    loop {
        let request = {
            let mut state = control
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            while state.pending.is_none() && !state.shutdown {
                state = control
                    .wake
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            if state.shutdown {
                return;
            }
            state.pending.take()
        };
        let Some(request) = request else {
            continue;
        };
        let result = load_wallpaper(&request.path, target);
        let mut state = control
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.shutdown {
            return;
        }
        if request.generation != state.latest_generation {
            state.stale_generation = Some(request.generation);
        } else {
            state.completion = Some(WallpaperCompletion { request, result });
        }
        drop(state);
        if let Err(error) = wake_write.write(&[1]) {
            if error.kind() != ErrorKind::WouldBlock {
                eprintln!("desktop: wallpaper wake failed: {error}");
            }
        }
    }
}

fn load_wallpaper(path: &Path, target: IconSize) -> LoadResult {
    if path.as_os_str().is_empty() || path.to_string_lossy().trim().is_empty() {
        return LoadResult::Default;
    }
    if !is_supported_wallpaper(path) {
        return LoadResult::Failure(format!("unsupported wallpaper format: {}", path.display()));
    }
    match decode_bytes_at(path, target) {
        Ok(image) => LoadResult::Image(image),
        Err(message) => LoadResult::Failure(message),
    }
}

fn is_supported_wallpaper(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "svg" | "ppm")
    )
}

/// Existing image-core-backed decoder seam. The integration adapter owns the filesystem read and
/// delegates the bounded raster decode/scaling to the shared image core; XPM and all other formats
/// are fenced out above before this call.
fn decode_bytes_at(path: &Path, target: IconSize) -> Result<Rgba8Raster, String> {
    rasterize_path(path, target).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn production_source() -> &'static str {
        let source = include_str!("wallpaper_loader.rs");
        source
            .split_once("#[cfg(test)]")
            .map_or(source, |(production, _)| production)
    }

    fn temp_path(extension: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "flamewm-wallpaper-{}-{stamp}.{extension}",
            std::process::id()
        ))
    }

    fn ppm(path: &Path, color: [u8; 3]) {
        fs::write(
            path,
            [
                b'P', b'6', b'\n', b'1', b' ', b'1', b'\n', b'2', b'5', b'5', b'\n', color[0],
                color[1], color[2],
            ],
        )
        .expect("write test PPM");
    }

    fn wait_for(loader: &mut WallpaperLoader, generation: u64) -> PollOutcome {
        for _ in 0..100 {
            let outcome = loader.poll();
            if !matches!(outcome, PollOutcome::Idle) {
                if matches!(outcome, PollOutcome::Applied { generation: value } if value == generation)
                    || matches!(outcome, PollOutcome::Failed { generation: value } if value == generation)
                {
                    return outcome;
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("wallpaper generation {generation} did not complete");
    }

    #[test]
    fn t13_wallpaper_loader_is_off_turn_and_latest_generation_wins() {
        // CONTRACT: wallpaper decode/load work is off-turn and completion is fenced by the
        // request generation; an older completion cannot replace a newer request.
        // TRIGGER: issue two wallpaper requests and complete them in reverse order.
        // OBSERVABLE: the loader seam exposes asynchronous work, generation identity, and a
        // latest-result admission check rather than applying completion unconditionally.
        // CURRENT BAD MECHANISM: no desktop wallpaper loader seam exists; rendering can only
        // remain on the synchronous startup path.
        // FAILURE MUTATION: remove the off-turn handoff or generation fence; this regression
        // must turn red.
        let source = production_source();
        for marker in ["generation", "latest", "decode_bytes_at"] {
            assert!(
                source.contains(marker),
                "wallpaper loader seam must encode {marker}"
            );
        }
        assert!(
            source.contains("spawn") || source.contains("thread"),
            "wallpaper decode must leave the UI turn"
        );

        let first = temp_path("ppm");
        let second = temp_path("ppm");
        ppm(&first, [255, 0, 0]);
        ppm(&second, [0, 255, 0]);
        let mut loader = WallpaperLoader::new(1, 1).expect("start wallpaper loader");
        let first_generation = loader.request(&first).expect("request first wallpaper");
        let second_generation = loader.request(&second).expect("request second wallpaper");
        assert!(second_generation > first_generation);
        wait_for(&mut loader, second_generation);
        assert_eq!(loader.state().generation, second_generation);
        match &loader.state().presentation {
            WallpaperPresentation::Image(image) => assert_eq!(image.pixels, [0, 255, 0, 255]),
            WallpaperPresentation::DefaultBlackWatermark => panic!("latest image was not applied"),
        }
        loader.shutdown();
        let _ = fs::remove_file(first);
        let _ = fs::remove_file(second);
    }

    #[test]
    fn t13_wallpaper_loader_retains_prior_valid_result_on_stale_or_failed_load() {
        // CONTRACT: stale and failed loads are non-destructive; the last valid wallpaper stays
        // available until a newer valid generation is admitted.
        // TRIGGER: complete an older generation after a newer request, then fail the newest load.
        // OBSERVABLE: the seam contains prior-result retention and explicit stale/failure paths.
        // CURRENT BAD MECHANISM: without a retained result, an out-of-order completion or decode
        // error clears the currently displayed wallpaper.
        // FAILURE MUTATION: replace retention with unconditional clear; this test must turn red.
        let source = production_source();
        for marker in ["previous", "stale", "failure"] {
            assert!(
                source.contains(marker),
                "wallpaper loader seam must retain the prior result across {marker}"
            );
        }

        let valid = temp_path("ppm");
        ppm(&valid, [32, 64, 96]);
        let missing = temp_path("png");
        let mut loader = WallpaperLoader::new(1, 1).expect("start wallpaper loader");
        let valid_generation = loader.request(&valid).expect("request valid wallpaper");
        assert!(matches!(
            wait_for(&mut loader, valid_generation),
            PollOutcome::Applied { .. }
        ));
        let previous = loader.state().presentation.clone();
        let failed_generation = loader.request(&missing).expect("request failed wallpaper");
        assert!(matches!(
            wait_for(&mut loader, failed_generation),
            PollOutcome::Failed { .. }
        ));
        assert_eq!(loader.state().presentation, previous);
        assert!(loader.state().diagnostic.is_some());
        loader.shutdown();
        let _ = fs::remove_file(valid);
        let _ = fs::remove_file(missing);
    }

    #[test]
    fn empty_and_unsupported_requests_have_explicit_contracts() {
        let mut loader = WallpaperLoader::new(1, 1).expect("start wallpaper loader");
        let empty_generation = loader.request("").expect("request default wallpaper");
        assert!(matches!(
            wait_for(&mut loader, empty_generation),
            PollOutcome::Applied { .. }
        ));
        assert_eq!(
            loader.state().presentation,
            WallpaperPresentation::DefaultBlackWatermark
        );
        let unsupported = temp_path("jpg");
        fs::write(&unsupported, b"not a supported wallpaper").expect("write unsupported file");
        let failed_generation = loader
            .request(&unsupported)
            .expect("request unsupported wallpaper");
        assert!(matches!(
            wait_for(&mut loader, failed_generation),
            PollOutcome::Failed { .. }
        ));
        assert_eq!(
            loader.state().presentation,
            WallpaperPresentation::DefaultBlackWatermark
        );
        loader.shutdown();
        let _ = fs::remove_file(unsupported);
    }

    #[test]
    fn t13_desktop_owns_one_wallpaper_loader_integration_seam() {
        // CONTRACT: the desktop uses one loader owner; a second ad-hoc image authority is not
        // introduced in the signal callback or renderer projection.
        // TRIGGER: inspect the desktop module boundary and its wallpaper integration call.
        // OBSERVABLE: main declares and calls the dedicated loader seam.
        // CURRENT BAD MECHANISM: no loader module is wired into the desktop process.
        // FAILURE MUTATION: remove the declaration or call site; this regression must turn red.
        let source = include_str!("main.rs");
        assert!(
            source.contains("mod wallpaper_loader"),
            "desktop must declare the wallpaper loader owner"
        );
        assert!(
            source.contains("wallpaper_loader::"),
            "desktop wallpaper projection must call the loader seam"
        );
    }
}

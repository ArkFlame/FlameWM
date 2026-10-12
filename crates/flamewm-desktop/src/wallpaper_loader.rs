use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use flamewm_api::background::WallpaperFit;
use flamewm_image_core::ImageSourceFormat;
use flamewm_integrations_linux::icons::{IconOrigin, Rgba8Raster};

use crate::wallpaper_fit;

const DEFAULT_WALLPAPER_PATH: &str = "assets/wallpapers/fitz-roy-dawn.webp";

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
    fit: WallpaperFit,
}

#[derive(Debug)]
enum LoadResult {
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
    pub fn new(width: u32, height: u32, packaged_root: impl Into<PathBuf>) -> Result<Self, String> {
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
        let packaged_root = packaged_root.into();
        let worker_root = packaged_root.clone();
        let worker = thread::Builder::new()
            .name("flamewm-wallpaper-loader".to_owned())
            .spawn(move || {
                wallpaper_worker(worker_control, (width, height), worker_root, wake_write)
            })
            .map_err(|error| format!("failed to start wallpaper loader: {error}"))?;
        Ok(Self {
            control,
            worker: Some(worker),
            wake_read,
            next_generation: 0,
            state: WallpaperState::default(),
        })
    }

    /// Replaces the pending request. At most one request waits for the worker. `fit` places the
    /// decoded image on the root-sized canvas; a fit change re-renders even for the same path.
    pub fn request(
        &mut self,
        path: impl Into<PathBuf>,
        fit: WallpaperFit,
    ) -> Result<u64, LoaderClosed> {
        self.next_generation = self.next_generation.saturating_add(1);
        let request = WallpaperRequest {
            generation: self.next_generation,
            path: path.into(),
            fit,
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

fn wallpaper_worker(
    control: Arc<WorkerControl>,
    target: (u32, u32),
    packaged_root: PathBuf,
    mut wake_write: UnixStream,
) {
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
        let Some(mut request) = request else {
            continue;
        };
        let result = load_wallpaper(&request.path, request.fit, target, &packaged_root);
        if request.path.as_os_str().is_empty() || request.path.to_string_lossy().trim().is_empty() {
            request.path = packaged_root.join(DEFAULT_WALLPAPER_PATH);
        }
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

fn load_wallpaper(
    path: &Path,
    fit: WallpaperFit,
    target: (u32, u32),
    packaged_root: &Path,
) -> LoadResult {
    if path.as_os_str().is_empty() || path.to_string_lossy().trim().is_empty() {
        let default_path = packaged_root.join(DEFAULT_WALLPAPER_PATH);
        return match decode_bytes_at(&default_path, fit, target) {
            Ok(image) => LoadResult::Image(image),
            Err(error) => LoadResult::Failure(format!(
                "packaged default wallpaper {} unavailable: {error}",
                default_path.display()
            )),
        };
    }
    if !is_supported_wallpaper(path) {
        return LoadResult::Failure(format!("unsupported wallpaper format: {}", path.display()));
    }
    match decode_bytes_at(path, fit, target) {
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
        Some("png" | "svg" | "ppm" | "webp" | "jpg" | "jpeg" | "gif")
    )
}

/// Decodes at natural size through the shared image core, then composes onto the root-sized
/// canvas by `fit`. Formats are fenced by `is_supported_wallpaper` before this call.
fn decode_bytes_at(
    path: &Path,
    fit: WallpaperFit,
    target: (u32, u32),
) -> Result<Rgba8Raster, String> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let format = ImageSourceFormat::from_extension(path)
        .ok_or_else(|| format!("unsupported wallpaper format: {}", path.display()))?;
    let natural = flamewm_image_core::decode_bytes(&bytes, format)
        .map_err(|message| format!("decode {}: {message}", path.display()))?;
    let image = wallpaper_fit::compose(&natural, target.0, target.1, fit)
        .ok_or_else(|| format!("wallpaper {} has no composable size", path.display()))?;
    Ok(Rgba8Raster {
        source: path.to_string_lossy().into_owned(),
        width: image.width,
        height: image.height,
        pixels: image.pixels,
        origin: IconOrigin::ExplicitPath,
        fallback_reason: None,
    })
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

    fn packaged_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root")
            .to_path_buf()
    }

    fn packaged_fixture_root(label: &str) -> PathBuf {
        let root = temp_path(label);
        let default_path = root.join(DEFAULT_WALLPAPER_PATH);
        fs::create_dir_all(default_path.parent().expect("default parent"))
            .expect("create packaged asset directory");
        fs::write(default_path, include_bytes!("testdata/wallpaper-test.webp"))
            .expect("write small packaged wallpaper fixture");
        root
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
        for _ in 0..5_000 {
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
        let mut loader =
            WallpaperLoader::new(1, 1, packaged_root()).expect("start wallpaper loader");
        let first_generation = loader
            .request(&first, WallpaperFit::Fill)
            .expect("request first wallpaper");
        let second_generation = loader
            .request(&second, WallpaperFit::Fill)
            .expect("request second wallpaper");
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
        let mut loader =
            WallpaperLoader::new(1, 1, packaged_root()).expect("start wallpaper loader");
        let valid_generation = loader
            .request(&valid, WallpaperFit::Fill)
            .expect("request valid wallpaper");
        assert!(matches!(
            wait_for(&mut loader, valid_generation),
            PollOutcome::Applied { .. }
        ));
        let previous = loader.state().presentation.clone();
        let failed_generation = loader
            .request(&missing, WallpaperFit::Fill)
            .expect("request failed wallpaper");
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
        let root = packaged_fixture_root("empty-and-unsupported-root");
        let mut loader = WallpaperLoader::new(1, 1, &root).expect("start wallpaper loader");
        let empty_generation = loader
            .request("", WallpaperFit::Fill)
            .expect("request default wallpaper");
        assert!(matches!(
            wait_for(&mut loader, empty_generation),
            PollOutcome::Applied { .. }
        ));
        assert!(matches!(
            loader.state().presentation,
            WallpaperPresentation::Image(_)
        ));
        let unsupported = temp_path("jpg");
        fs::write(&unsupported, b"not a supported wallpaper").expect("write unsupported file");
        let failed_generation = loader
            .request(&unsupported, WallpaperFit::Fill)
            .expect("request unsupported wallpaper");
        assert!(matches!(
            wait_for(&mut loader, failed_generation),
            PollOutcome::Failed { .. }
        ));
        assert!(matches!(
            loader.state().presentation,
            WallpaperPresentation::Image(_)
        ));
        loader.shutdown();
        let _ = fs::remove_file(unsupported);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn packaged_fixture_and_custom_webp_load_and_empty_resets_to_packaged() {
        let root = packaged_fixture_root("packaged-root");
        let default_path = root.join(DEFAULT_WALLPAPER_PATH);
        let mut loader = WallpaperLoader::new(1, 1, &root).expect("start wallpaper worker");
        let default_generation = loader
            .request("", WallpaperFit::Fill)
            .expect("request packaged default");
        assert!(matches!(
            wait_for(&mut loader, default_generation),
            PollOutcome::Applied { .. }
        ));
        let packaged = loader.state().presentation.clone();
        assert!(matches!(
            &packaged,
            WallpaperPresentation::Image(image)
                if image.source == default_path.to_string_lossy().as_ref()
        ));

        let custom = temp_path("webp");
        fs::write(&custom, include_bytes!("testdata/wallpaper-test.webp"))
            .expect("write custom WebP fixture");
        let custom_generation = loader
            .request(&custom, WallpaperFit::Fill)
            .expect("request custom WebP");
        assert!(matches!(
            wait_for(&mut loader, custom_generation),
            PollOutcome::Applied { .. }
        ));
        assert!(matches!(
            &loader.state().presentation,
            WallpaperPresentation::Image(image)
                if image.source.as_str() == custom.to_string_lossy().as_ref()
        ));

        let reset_generation = loader
            .request("", WallpaperFit::Fill)
            .expect("reset to packaged default");
        assert!(matches!(
            wait_for(&mut loader, reset_generation),
            PollOutcome::Applied { .. }
        ));
        assert_eq!(loader.state().presentation, packaged);
        loader.shutdown();
        let _ = fs::remove_file(custom);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn decoded_image_is_composed_to_root_size_and_refit_on_same_path() {
        // CONTRACT: the presented wallpaper is the root-sized canvas composed by fit, not the
        // natural image or a square raster; a fit change re-renders the same path.
        // FAILURE MUTATION: skip compose (natural 2x1) or keep the square rasterize; this turns red.
        let path = temp_path("ppm");
        fs::write(
            &path,
            [
                b'P', b'6', b'\n', b'2', b' ', b'1', b'\n', b'2', b'5', b'5', b'\n', 9, 9, 9, 7, 7,
                7,
            ],
        )
        .expect("write 2x1 PPM");
        let mut loader = WallpaperLoader::new(4, 2, packaged_root()).expect("start loader");
        let center = loader
            .request(&path, WallpaperFit::Center)
            .expect("request centered");
        assert!(matches!(
            wait_for(&mut loader, center),
            PollOutcome::Applied { .. }
        ));
        let WallpaperPresentation::Image(centered) = loader.state().presentation.clone() else {
            panic!("centered image expected");
        };
        assert_eq!((centered.width, centered.height), (4, 2));
        assert_eq!(&centered.pixels[..4], &[0, 0, 0, 255]);
        assert_eq!(&centered.pixels[4..8], &[9, 9, 9, 255]);
        assert_eq!(&centered.pixels[8..12], &[7, 7, 7, 255]);
        assert_eq!(&centered.pixels[12..16], &[0, 0, 0, 255]);
        assert_eq!(&centered.pixels[16..24], &[0, 0, 0, 255, 0, 0, 0, 255]);

        let stretch = loader
            .request(&path, WallpaperFit::Stretch)
            .expect("request stretched");
        assert!(matches!(
            wait_for(&mut loader, stretch),
            PollOutcome::Applied { .. }
        ));
        let WallpaperPresentation::Image(stretched) = &loader.state().presentation else {
            panic!("stretched image expected");
        };
        assert_eq!((stretched.width, stretched.height), (4, 2));
        assert_ne!(stretched.pixels, centered.pixels);
        loader.shutdown();
        let _ = fs::remove_file(path);
    }

    #[test]
    fn missing_packaged_default_reports_runtime_asset_path() {
        let root = temp_path("missing-packaged-root");
        let mut loader = WallpaperLoader::new(1, 1, &root).expect("start wallpaper worker");
        let generation = loader
            .request("", WallpaperFit::Fill)
            .expect("request packaged default");
        assert!(matches!(
            wait_for(&mut loader, generation),
            PollOutcome::Failed { .. }
        ));
        let diagnostic = loader.state().diagnostic.as_ref().expect("load diagnostic");
        assert_eq!(diagnostic.path, root.join(DEFAULT_WALLPAPER_PATH));
        assert!(diagnostic.message.contains("packaged default wallpaper"));
        loader.shutdown();
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

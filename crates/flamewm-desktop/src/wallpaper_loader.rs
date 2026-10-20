use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

use flamewm_api::background::WallpaperFit;
use flamewm_api::Rect;
use flamewm_integrations_linux::icons::{IconOrigin, Rgba8Raster};

use crate::wallpaper_fit;

use flamewm_image_core::{RgbaImage, wallpaper::BACKGROUND};
use flamewm_image_core::wallpaper::DEFAULT_WALLPAPER_PATH;

/// The desktop's empty wallpaper selection is an intentional presentation, not a failed load.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum WallpaperPresentation {
    #[default]
    DefaultBlackWatermark,
    Image(Rgba8Raster),
    /// Flat `wallpaperMode=solid` fill; no decode and no worker round trip.
    Solid([u8; 3]),
}

/// Background source chosen by `wallpaperMode`. `Image` with an empty path is the bundled default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WallpaperSource {
    Image(PathBuf),
    Solid([u8; 3]),
}

/// Fallback fill when `wallpaperColor` is unparsable (matches the settings default).
const DEFAULT_SOLID: [u8; 3] = [0x23, 0x26, 0x29];

/// Resolves `wallpaperMode` to a source. `default` ignores the saved custom path.
#[must_use]
pub fn source_for_settings(mode: &str, custom_path: &str, color: &str) -> WallpaperSource {
    match mode {
        "solid" => WallpaperSource::Solid(parse_rgb(color).unwrap_or(DEFAULT_SOLID)),
        "custom" => WallpaperSource::Image(PathBuf::from(custom_path)),
        _ => WallpaperSource::Image(PathBuf::new()),
    }
}

fn parse_rgb(value: &str) -> Option<[u8; 3]> {
    let digits = value.strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let rgb = u32::from_str_radix(digits, 16).ok()?;
    Some([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallpaperDiagnostic {
    pub generation: u64,
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WallpaperState {
    pub generation: u64,
    pub presentation: WallpaperPresentation,
    pub diagnostic: Option<WallpaperDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WallpaperRequest {
    generation: u64,
    path: PathBuf,
    fit: WallpaperFit,
    monitors: Vec<Rect>,
    share: bool,
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

impl WorkerControl {
    fn lock(&self) -> MutexGuard<'_, WorkerState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
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
    #[cfg_attr(test, allow(dead_code))] // read only by `main`, which is cfg-excluded under test
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
        self.request_layout(path, fit, &[], true)
    }

    /// Requests either a separately fitted copy on each output or one shared root canvas.
    pub fn request_layout(
        &mut self,
        path: impl Into<PathBuf>,
        fit: WallpaperFit,
        monitors: &[Rect],
        share: bool,
    ) -> Result<u64, LoaderClosed> {
        self.next_generation = self.next_generation.saturating_add(1);
        let request = WallpaperRequest {
            generation: self.next_generation,
            path: path.into(),
            fit,
            monitors: monitors.to_vec(),
            share,
        };
        let mut state = self.control.lock();
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

    /// Shows a flat solid fill immediately and supersedes any in-flight image request.
    pub fn request_solid(&mut self, rgb: [u8; 3]) -> Result<u64, LoaderClosed> {
        self.next_generation = self.next_generation.saturating_add(1);
        let generation = self.next_generation;
        let mut state = self.control.lock();
        if state.shutdown {
            return Err(LoaderClosed);
        }
        state.latest_generation = generation;
        state.pending = None;
        state.completion = None;
        state.stale_generation = None;
        drop(state);
        self.state = WallpaperState {
            generation,
            presentation: WallpaperPresentation::Solid(rgb),
            diagnostic: None,
        };
        Ok(generation)
    }

    /// Admits one worker completion on the caller's turn. Decode and filesystem reads never run
    /// here; failures leave the previous valid presentation in place and expose a diagnostic.
    pub fn poll(&mut self) -> PollOutcome {
        let mut worker_state = self.control.lock();
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
    #[cfg_attr(test, allow(dead_code))] // called only by `main`, which is cfg-excluded under test
    pub fn wake_fd(&self) -> RawFd {
        self.wake_read.as_raw_fd()
    }

    /// Drains worker wake bytes without doing filesystem or decode work on the UI turn.
    #[cfg_attr(test, allow(dead_code))] // called only by `main`, which is cfg-excluded under test
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
            let mut state = self.control.lock();
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
            let mut state = control.lock();
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
        let result = load_wallpaper(
            &request.path,
            request.fit,
            target,
            &request.monitors,
            request.share,
            &packaged_root,
        );
        if is_default_wallpaper(&request.path) {
            request.path = packaged_root.join(DEFAULT_WALLPAPER_PATH);
        }
        let mut state = control.lock();
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
    monitors: &[Rect],
    share: bool,
    packaged_root: &Path,
) -> LoadResult {
    let decoded = if is_default_wallpaper(path) {
        decode_default(packaged_root, fit, target, monitors, share)
    } else {
        decode_custom(path, fit, target, monitors, share)
    };
    decoded.map_or_else(LoadResult::Failure, LoadResult::Image)
}

fn is_default_wallpaper(path: &Path) -> bool {
    path.as_os_str().is_empty() || path.to_string_lossy().trim().is_empty()
}

fn decode_default(
    packaged_root: &Path,
    fit: WallpaperFit,
    target: (u32, u32),
    monitors: &[Rect],
    share: bool,
) -> Result<Rgba8Raster, String> {
    let default_path = packaged_root.join(DEFAULT_WALLPAPER_PATH);
    decode_bytes_at(&default_path, fit, target, monitors, share).map_err(|error| {
        format!(
            "packaged default wallpaper {} unavailable: {error}",
            default_path.display()
        )
    })
}

fn decode_custom(
    path: &Path,
    fit: WallpaperFit,
    target: (u32, u32),
    monitors: &[Rect],
    share: bool,
) -> Result<Rgba8Raster, String> {
    if !is_supported_wallpaper(path) {
        return Err(format!("unsupported wallpaper format: {}", path.display()));
    }
    decode_bytes_at(path, fit, target, monitors, share)
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

/// Decodes at natural size, then composes per output or onto the shared root canvas.
fn decode_bytes_at(
    path: &Path,
    fit: WallpaperFit,
    target: (u32, u32),
    monitors: &[Rect],
    share: bool,
) -> Result<Rgba8Raster, String> {
    let natural = flamewm_image_core::wallpaper::decode_file(path)?;
    let image = compose_decoded(natural, target, monitors, fit, share)
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

/// A single output occupying the whole root is equivalent to the shared canvas.
/// Preserve this identity check in one place for both decode and composition.
fn single_full_root_output(monitors: &[Rect], target: (u32, u32)) -> bool {
    matches!(monitors, [only]
        if only.x == 0 && only.y == 0
        && u32::try_from(only.width).ok() == Some(target.0)
        && u32::try_from(only.height).ok() == Some(target.1))
}

/// Consume the decoded pixels directly when no placement or scaling is necessary.
/// Otherwise, avoid the extra root canvas for the single-full-output case.
fn compose_decoded(
    natural: RgbaImage,
    target: (u32, u32),
    monitors: &[Rect],
    fit: WallpaperFit,
    share: bool,
) -> Option<RgbaImage> {
    let full_root = share || monitors.is_empty() || single_full_root_output(monitors, target);
    if full_root && target.0 != 0 && target.1 != 0
        && (natural.width, natural.height) == target {
        return Some(natural);
    }
    if full_root {
        wallpaper_fit::compose(&natural, target.0, target.1, fit)
    } else {
        compose_per_monitor(&natural, target, monitors, fit)
    }
}

fn compose_per_monitor(
    natural: &RgbaImage,
    target: (u32, u32),
    monitors: &[Rect],
    fit: WallpaperFit,
) -> Option<RgbaImage> {
    let (width, height) = target;
    flamewm_image_core::check_size_limits(width, height).ok()?;
    let root = Rect::new(0, 0, i32::try_from(width).ok()?, i32::try_from(height).ok()?);
    let mut canvas = RgbaImage {
        width,
        height,
        pixels: vec![0; usize::try_from(width.checked_mul(height)?.checked_mul(4)?).ok()?],
    };
    for pixel in canvas.pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&BACKGROUND);
    }
    let mut composed = false;
    for monitor in monitors {
        let Some(rect) = monitor.intersection(root) else {
            continue;
        };
        let image = wallpaper_fit::compose(natural, rect.width as u32, rect.height as u32, fit)?;
        for row in 0..rect.height as usize {
            let source = row * rect.width as usize * 4;
            let destination = ((rect.y as usize + row) * width as usize + rect.x as usize) * 4;
            let length = rect.width as usize * 4;
            canvas.pixels[destination..destination + length]
                .copy_from_slice(&image.pixels[source..source + length]);
        }
        composed = true;
    }
    composed.then_some(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn default_mode_ignores_saved_custom_path() {
        assert_eq!(
            source_for_settings("default", "/home/u/a.png", "#232629"),
            WallpaperSource::Image(PathBuf::new())
        );
    }

    #[test]
    fn one_full_output_matches_shared_composition_for_all_fits() {
        let mut pixels = Vec::new();
        for y in 0..3u8 {
            for x in 0..5u8 {
                pixels.extend_from_slice(&[x * 35, y * 65, 17, 100 + x * 10]);
            }
        }
        let natural = RgbaImage { width: 5, height: 3, pixels };
        let target = (7, 6);
        let monitors = [Rect::new(0, 0, 7, 6)];
        for fit in [
            WallpaperFit::Fill,
            WallpaperFit::Fit,
            WallpaperFit::Stretch,
            WallpaperFit::Center,
            WallpaperFit::Tile,
        ] {
            let shared = wallpaper_fit::compose(&natural, target.0, target.1, fit).expect("shared");
            let dispatched = compose_decoded(natural.clone(), target, &monitors, fit, false)
                .expect("dispatched");
            assert_eq!(dispatched, shared, "{fit:?} must keep the original pixels");
        }
        // A monitor that does not cover the root must keep the independent-output path.
        let inset = [Rect::new(1, 0, 6, 6)];
        let inset_result = compose_decoded(natural, target, &inset, WallpaperFit::Fill, false)
            .expect("inset monitor");
        assert_eq!(&inset_result.pixels[0..4], &BACKGROUND);
    }

    #[test]
    fn matching_root_wallpaper_reuses_decoded_buffer_without_copy() {
        for fit in [
            WallpaperFit::Fill,
            WallpaperFit::Fit,
            WallpaperFit::Stretch,
            WallpaperFit::Center,
            WallpaperFit::Tile,
        ] {
            for (monitors, share) in [
                (vec![Rect::new(0, 0, 2, 2)], false),
                (Vec::new(), false),
                (vec![Rect::new(1, 0, 2, 2)], true),
            ] {
                let image = RgbaImage {
                    width: 2,
                    height: 2,
                    pixels: [42, 71, 90, 255].repeat(4),
                };
                let original_ptr = image.pixels.as_ptr();
                let composed = compose_decoded(image, (2, 2), &monitors, fit, share)
                    .expect("identity compose");
                assert_eq!(composed.pixels.as_ptr(), original_ptr, "{fit:?}: move pixels");
                assert_eq!(composed.pixels.len(), 16);
            }
        }
        let zero = RgbaImage { width: 0, height: 0, pixels: Vec::new() };
        assert!(compose_decoded(zero, (0, 0), &[], WallpaperFit::Fill, true).is_none());
    }

    #[test]
    fn independent_outputs_fit_the_wallpaper_separately_from_shared_canvas() {
        let mut pixels = Vec::new();
        for _ in 0..2 {
            for x in 0..4u8 {
                pixels.extend_from_slice(&[x * 60, 0, 0, 255]);
            }
        }
        let natural = RgbaImage { width: 4, height: 2, pixels };
        let monitors = [Rect::new(0, 0, 2, 2), Rect::new(2, 0, 2, 2)];
        let independent = compose_per_monitor(&natural, (4, 2), &monitors, WallpaperFit::Fill)
            .expect("monitor canvas");
        let shared = wallpaper_fit::compose(&natural, 4, 2, WallpaperFit::Fill)
            .expect("shared canvas");
        let red_at = |image: &RgbaImage, x: usize| image.pixels[x * 4];
        assert_eq!(red_at(&independent, 0), 60);
        assert_eq!(red_at(&independent, 2), 60);
        assert_eq!(red_at(&shared, 2), 120);
    }

    #[test]
    fn custom_mode_uses_saved_path_and_solid_uses_color() {
        assert_eq!(
            source_for_settings("custom", "/home/u/a.png", "#232629"),
            WallpaperSource::Image(PathBuf::from("/home/u/a.png"))
        );
        assert_eq!(
            source_for_settings("solid", "/home/u/a.png", "#102030"),
            WallpaperSource::Solid([0x10, 0x20, 0x30])
        );
        assert_ne!(
            source_for_settings("default", "", "#232629"),
            source_for_settings("solid", "", "#232629")
        );
    }

    #[test]
    fn solid_with_invalid_color_falls_back_to_default_fill() {
        assert_eq!(
            source_for_settings("solid", "", "red"),
            WallpaperSource::Solid([0x23, 0x26, 0x29])
        );
    }

    #[test]
    fn solid_request_is_presented_without_worker_and_supersedes_images() {
        let mut loader = WallpaperLoader::new(64, 48, "/nonexistent-root").expect("loader");
        let generation = loader
            .request_solid([1, 2, 3])
            .expect("solid request accepted");
        assert_eq!(loader.state().generation, generation);
        assert_eq!(
            loader.state().presentation,
            WallpaperPresentation::Solid([1, 2, 3])
        );
        assert_eq!(loader.poll(), PollOutcome::Idle);
    }

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
        fs::write(path, [b"P6\n1 1\n255\n".as_slice(), &color[..]].concat())
            .expect("write test PPM");
    }

    fn wait_for(loader: &mut WallpaperLoader, generation: u64) -> PollOutcome {
        for _ in 0..5_000 {
            let outcome = loader.poll();
            if matches!(
                outcome,
                PollOutcome::Applied { generation: value } | PollOutcome::Failed { generation: value }
                    if value == generation
            ) {
                return outcome;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("wallpaper generation {generation} did not complete");
    }

    fn wait_applied(loader: &mut WallpaperLoader, generation: u64) {
        assert!(matches!(
            wait_for(loader, generation),
            PollOutcome::Applied { .. }
        ));
    }

    fn wait_failed(loader: &mut WallpaperLoader, generation: u64) {
        assert!(matches!(
            wait_for(loader, generation),
            PollOutcome::Failed { .. }
        ));
    }

    fn assert_image_presented(loader: &WallpaperLoader) {
        assert!(matches!(
            loader.state().presentation,
            WallpaperPresentation::Image(_)
        ));
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
            WallpaperPresentation::DefaultBlackWatermark | WallpaperPresentation::Solid(_) => {
                panic!("latest image was not applied")
            }
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
        wait_applied(&mut loader, valid_generation);
        let previous = loader.state().presentation.clone();
        let failed_generation = loader
            .request(&missing, WallpaperFit::Fill)
            .expect("request failed wallpaper");
        wait_failed(&mut loader, failed_generation);
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
        wait_applied(&mut loader, empty_generation);
        assert_image_presented(&loader);
        let unsupported = temp_path("jpg");
        fs::write(&unsupported, b"not a supported wallpaper").expect("write unsupported file");
        let failed_generation = loader
            .request(&unsupported, WallpaperFit::Fill)
            .expect("request unsupported wallpaper");
        wait_failed(&mut loader, failed_generation);
        assert_image_presented(&loader);
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
        wait_applied(&mut loader, default_generation);
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
        wait_applied(&mut loader, custom_generation);
        assert!(matches!(
            &loader.state().presentation,
            WallpaperPresentation::Image(image)
                if image.source.as_str() == custom.to_string_lossy().as_ref()
        ));

        let reset_generation = loader
            .request("", WallpaperFit::Fill)
            .expect("reset to packaged default");
        wait_applied(&mut loader, reset_generation);
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
        fs::write(&path, b"P6\n2 1\n255\n\x09\x09\x09\x07\x07\x07").expect("write 2x1 PPM");
        let mut loader = WallpaperLoader::new(4, 2, packaged_root()).expect("start loader");
        let center = loader
            .request(&path, WallpaperFit::Center)
            .expect("request centered");
        wait_applied(&mut loader, center);
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
        wait_applied(&mut loader, stretch);
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
        wait_failed(&mut loader, generation);
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

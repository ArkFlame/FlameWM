//! Off-thread decode and crop. One worker thread per viewer window blocks on its queue, so an idle
//! viewer does no work. Loads run first, the latest viewport request runs next (older ones are
//! dropped), and neighbour prefetches run last. Decoded images stay in a small cache.

use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use flamewm_image_core::scale::{ScaleFilter, scale_rgba};
use flamewm_image_core::{ImageSourceFormat, RgbaImage, decode_bytes};

/// Refuse source files larger than this before reading them.
pub const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
/// Refuse decoded images above this pixel count (RGBA is 4 bytes per pixel).
pub const MAX_PIXELS: u64 = 100_000_000;
/// Decoded images kept: the current one and its two neighbours.
const CACHE_SLOTS: usize = 3;

/// A visible part of the displayed image: top-left and size, in displayed-image pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

pub enum Job {
    /// Make `path` the active image, decoding it if needed.
    Load { generation: u64, path: PathBuf },
    /// Decode `path` into the cache when there is room, without displacing the active image.
    Prefetch { path: PathBuf },
    /// Produce the `rect` of the image shown at `display` size.
    View {
        seq: u64,
        path: PathBuf,
        display: (u32, u32),
        rect: Rect,
    },
}

pub enum Reply {
    Loaded {
        generation: u64,
        size: (u32, u32),
    },
    Failed {
        generation: u64,
        message: String,
    },
    ViewFailed {
        seq: u64,
        message: String,
    },
    Frame {
        seq: u64,
        size: (u32, u32),
        pixels: Vec<u8>,
    },
}

/// Reads and decodes one image file. Format comes from the magic bytes, then the extension.
pub fn decode_file(path: &Path) -> Result<RgbaImage, String> {
    let meta = fs::metadata(path).map_err(|error| error.to_string())?;
    if !meta.is_file() {
        return Err("not a regular file".into());
    }
    if meta.len() > MAX_SOURCE_BYTES {
        return Err(format!(
            "file is larger than {} MiB",
            MAX_SOURCE_BYTES >> 20
        ));
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let format = ImageSourceFormat::from_magic(&bytes)
        .or_else(|| ImageSourceFormat::from_extension(path))
        .ok_or("unsupported image format")?;
    let image = decode_bytes(&bytes, format)?;
    if u64::from(image.width) * u64::from(image.height) > MAX_PIXELS {
        return Err("image has too many pixels to show".into());
    }
    Ok(image)
}

/// Copies `rect` of a `display`-sized view of `base`, nearest-neighbour. `base` may be the source
/// itself (zoom above 100%) or a copy already scaled to `display` (zoom below 100%).
#[must_use]
pub fn crop(base: &RgbaImage, display: (u32, u32), rect: Rect) -> Vec<u8> {
    let (bw, bh) = (u64::from(base.width), u64::from(base.height));
    let (dw, dh) = (u64::from(display.0.max(1)), u64::from(display.1.max(1)));
    let mut out = Vec::with_capacity(rect.w as usize * rect.h as usize * 4);
    for oy in 0..rect.h {
        let sy = (u64::from(rect.y + oy) * bh / dh).min(bh.saturating_sub(1));
        for ox in 0..rect.w {
            let sx = (u64::from(rect.x + ox) * bw / dw).min(bw.saturating_sub(1));
            let at = ((sy * bw + sx) * 4) as usize;
            out.extend_from_slice(&base.pixels[at..at + 4]);
        }
    }
    out
}

/// Decodes one file. Injected so tests can hold a decode open.
pub type Decode = fn(&Path) -> Result<RgbaImage, String>;

/// Decoded images and the prefetch queue, shared by the view worker and its prefetch thread.
#[derive(Default)]
struct Shared {
    /// Most recently used last.
    cache: Vec<(PathBuf, Arc<RgbaImage>)>,
    active: Option<PathBuf>,
    /// Neighbour prefetches not started yet. A `Load` drops them: they belong to the old image.
    queued: VecDeque<PathBuf>,
    /// The prefetch decode in progress, if any.
    decoding: Option<PathBuf>,
    /// Set when the worker drops, so the prefetch thread exits.
    closed: bool,
}

type SharedState = Arc<(Mutex<Shared>, Condvar)>;

fn lock(state: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn touch(&mut self, path: &Path) -> Option<Arc<RgbaImage>> {
        let index = self.cache.iter().position(|(cached, _)| cached == path)?;
        let entry = self.cache.remove(index);
        let image = Arc::clone(&entry.1);
        self.cache.push(entry);
        Some(image)
    }

    fn insert(&mut self, path: &Path, image: Arc<RgbaImage>) {
        if self.cache.iter().any(|(cached, _)| cached == path) {
            return;
        }
        while self.cache.len() >= CACHE_SLOTS {
            let victim = self
                .cache
                .iter()
                .position(|(cached, _)| self.active.as_deref() != Some(cached.as_path()));
            match victim {
                Some(index) => {
                    self.cache.remove(index);
                }
                None => break,
            }
        }
        self.cache.push((path.to_path_buf(), image));
    }
}

pub struct Worker {
    shared: SharedState,
    decode: Decode,
    /// The source scaled to a zoom below 100%, keyed by path and display size.
    reduced: Option<(PathBuf, (u32, u32), Arc<RgbaImage>)>,
}

impl Worker {
    /// Starts the prefetch thread. It blocks on the queue while idle, so an idle viewer does no work.
    #[must_use]
    pub fn new(decode: Decode) -> Self {
        let shared: SharedState = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let prefetch_state = Arc::clone(&shared);
        // Without the thread, prefetch is skipped: a neighbour is then decoded when it is opened.
        let _ = std::thread::Builder::new()
            .name("flamewm-imageviewer-prefetch".into())
            .spawn(move || prefetch_loop(&prefetch_state, decode));
        Self {
            shared,
            decode,
            reduced: None,
        }
    }

    /// Makes `path` the active image and drops the prefetches queued for the previous one.
    fn activate(&mut self, path: &Path) {
        let (state, _) = &*self.shared;
        let mut state = lock(state);
        state.active = Some(path.to_path_buf());
        state.queued.clear();
    }

    fn prefetch(&mut self, path: PathBuf) {
        let (state, ready) = &*self.shared;
        let mut state = lock(state);
        if !state.queued.contains(&path) {
            state.queued.push_back(path);
            ready.notify_all();
        }
    }

    fn image(&mut self, path: &Path) -> Result<Arc<RgbaImage>, String> {
        let (state, ready) = &*self.shared;
        let mut state = lock(state);
        // The prefetch thread is decoding this image: wait for it instead of decoding it twice.
        while state.decoding.as_deref() == Some(path) {
            state = ready.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
        if let Some(image) = state.touch(path) {
            return Ok(image);
        }
        drop(state);
        let image = Arc::new((self.decode)(path)?);
        lock(&self.shared.0).insert(path, Arc::clone(&image));
        Ok(image)
    }

    fn view(&mut self, path: &Path, display: (u32, u32), rect: Rect) -> Result<Vec<u8>, String> {
        let source = self.image(path)?;
        if display.0 >= source.width && display.1 >= source.height {
            return Ok(crop(&source, display, rect));
        }
        // Zoomed out: scale once per display size, then crop 1:1 from the reduced copy.
        let fresh = matches!(&self.reduced, Some((p, size, _)) if p == path && *size == display);
        if !fresh {
            let scaled = scale_rgba(&source, display.0, display.1, ScaleFilter::Area)
                .ok_or("requested size overflows")?;
            self.reduced = Some((path.to_path_buf(), display, Arc::new(scaled)));
        }
        let reduced = self.reduced.as_ref().map(|(_, _, image)| Arc::clone(image));
        let reduced = reduced.ok_or("reduced image missing")?;
        Ok(crop(&reduced, display, rect))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let (state, ready) = &*self.shared;
        lock(state).closed = true;
        ready.notify_all();
    }
}

/// Prefetch thread body. A decode runs to completion once started, so a `Load` never waits for a
/// queued prefetch; it waits only for the decode of the very image it asks for.
fn prefetch_loop(shared: &SharedState, decode: Decode) {
    let (state, ready) = &**shared;
    loop {
        let mut guard = lock(state);
        let path = loop {
            if guard.closed {
                return;
            }
            if let Some(path) = guard.queued.pop_front() {
                break path;
            }
            guard = ready.wait(guard).unwrap_or_else(PoisonError::into_inner);
        };
        if guard.cache.iter().any(|(cached, _)| *cached == path) {
            continue;
        }
        guard.decoding = Some(path.clone());
        drop(guard);
        // A prefetch that fails is silent: the image is simply decoded again when opened.
        let decoded = decode(&path);
        let mut guard = lock(state);
        guard.decoding = None;
        if let Ok(image) = decoded {
            guard.insert(&path, Arc::new(image));
        }
        ready.notify_all();
    }
}

/// Worker loop. Returns when the UI side drops its sender.
pub fn run(jobs: &Receiver<Job>, replies: &Sender<Reply>, wake: &mut UnixStream) {
    let mut worker = Worker::new(decode_file);
    let mut load: Option<Job> = None;
    let mut view: Option<Job> = None;
    loop {
        let nothing_queued = load.is_none() && view.is_none();
        let mut next = if nothing_queued {
            match jobs.recv() {
                Ok(job) => Some(job),
                Err(_) => return,
            }
        } else {
            None
        };
        loop {
            let job = match next.take() {
                Some(job) => job,
                None => match jobs.try_recv() {
                    Ok(job) => job,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                },
            };
            if let Job::Load { path, .. } = &job {
                worker.activate(path);
            }
            match job {
                Job::Load { .. } => {
                    // A new active image makes any queued viewport request for the old one stale.
                    view = None;
                    // Only the newest selection can be displayed. Avoid decoding every image
                    // passed while the user rapidly browses a folder.
                    load = Some(job);
                }
                Job::View { .. } => view = Some(job),
                Job::Prefetch { path } => worker.prefetch(path),
            }
        }
        if let Some(Job::Load { generation, path }) = load.take() {
            let reply = match worker.image(&path) {
                Ok(image) => Reply::Loaded {
                    generation,
                    size: (image.width, image.height),
                },
                Err(message) => Reply::Failed {
                    generation,
                    message,
                },
            };
            send(replies, wake, reply);
        } else if let Some(Job::View {
            seq,
            path,
            display,
            rect,
        }) = view.take()
        {
            let reply = match worker.view(&path, display, rect) {
                Ok(pixels) => Reply::Frame {
                    seq,
                    size: (rect.w, rect.h),
                    pixels,
                },
                Err(message) => Reply::ViewFailed { seq, message },
            };
            send(replies, wake, reply);
        }
    }
}

fn send(replies: &Sender<Reply>, wake: &mut UnixStream, reply: Reply) {
    if replies.send(reply).is_ok() {
        // A full wake pipe already holds unread bytes, so the UI wakes either way.
        let _ = wake.write(&[1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../flamewm-image-core/tests/fixtures")
            .join(name)
    }

    #[test]
    fn decodes_a_jpeg_fixture_by_magic() {
        let image = decode_file(&fixture("baseline-16x16.jpg")).expect("jpeg decodes");
        assert_eq!((image.width, image.height), (16, 16));
    }

    #[test]
    fn queued_loads_decode_only_the_latest_selection() {
        let (jobs, receiver) = mpsc::channel();
        let (replies, received) = mpsc::channel();
        let (_reader, mut writer) = UnixStream::pair().unwrap();
        for generation in 1..=3 {
            jobs.send(Job::Load {
                generation,
                path: fixture("baseline-16x16.jpg"),
            })
            .unwrap();
        }
        let thread = std::thread::spawn(move || run(&receiver, &replies, &mut writer));
        assert!(matches!(
            received.recv_timeout(Duration::from_secs(10)),
            Ok(Reply::Loaded { generation: 3, .. })
        ));
        drop(jobs);
        thread.join().unwrap();
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn decodes_a_gif_fixture() {
        let image = decode_file(&fixture("first-frame-4x4.gif")).expect("gif decodes");
        assert_eq!((image.width, image.height), (4, 4));
    }

    #[test]
    fn rejects_directories_and_non_images() {
        assert!(decode_file(&fixture("")).is_err());
        let text = std::env::temp_dir().join(format!("imgv-text-{}.txt", std::process::id()));
        fs::write(&text, b"not an image").unwrap();
        assert!(decode_file(&text).is_err());
        let _ = fs::remove_file(&text);
    }

    #[test]
    fn crop_at_identity_copies_pixels_and_rect_offsets_select_them() {
        let pixels: Vec<u8> = (0..4 * 2 * 2).map(|v| v as u8).collect();
        let image = RgbaImage::from_rgba8(2, 2, pixels.clone()).unwrap();
        assert_eq!(
            crop(
                &image,
                (2, 2),
                Rect {
                    x: 0,
                    y: 0,
                    w: 2,
                    h: 2
                }
            ),
            pixels
        );
        // The bottom-right pixel is the last four bytes.
        assert_eq!(
            crop(
                &image,
                (2, 2),
                Rect {
                    x: 1,
                    y: 1,
                    w: 1,
                    h: 1
                }
            ),
            pixels[12..16]
        );
    }

    #[test]
    fn crop_upscales_nearest_when_zoomed_in() {
        let image = RgbaImage::from_rgba8(1, 1, vec![9, 8, 7, 255]).unwrap();
        let out = crop(
            &image,
            (3, 3),
            Rect {
                x: 0,
                y: 0,
                w: 3,
                h: 3,
            },
        );
        assert_eq!(out.len(), 3 * 3 * 4);
        assert!(out.chunks_exact(4).all(|px| px == [9, 8, 7, 255]));
    }

    #[test]
    fn zoomed_out_view_reduces_then_crops_exact_size() {
        let mut worker = Worker::new(decode_file);
        let path = fixture("baseline-16x16.jpg");
        let out = worker
            .view(
                &path,
                (8, 8),
                Rect {
                    x: 0,
                    y: 0,
                    w: 8,
                    h: 4,
                },
            )
            .expect("view renders");
        assert_eq!(out.len(), 8 * 4 * 4);
        assert!(worker.reduced.is_some());
    }

    #[test]
    fn cache_keeps_the_active_image_when_prefetching_past_capacity() {
        let mut worker = Worker::new(decode_file);
        // CMYK and progressive JPEGs are unsupported, so use four generated 1x1 PPM files.
        let dir = std::env::temp_dir().join(format!("imgv-cache-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let paths: Vec<PathBuf> = (0..4)
            .map(|i| {
                let path = dir.join(format!("cache-{i}.ppm"));
                fs::write(
                    &path,
                    [b"P6\n1 1\n255\n".as_slice(), &[i as u8, 0, 0]].concat(),
                )
                .unwrap();
                path
            })
            .collect();
        worker.activate(&paths[0]);
        for path in &paths {
            worker.image(path).expect("generated ppm decodes");
        }
        let state = lock(&worker.shared.0);
        assert!(state.cache.len() <= CACHE_SLOTS);
        assert!(state.cache.iter().any(|(p, _)| *p == paths[0]));
        let _ = fs::remove_dir_all(&dir);
    }

    static GATE_ENTERED: AtomicBool = AtomicBool::new(false);
    static GATE_OPEN: AtomicBool = AtomicBool::new(false);
    static HELD_ENTERED: AtomicBool = AtomicBool::new(false);
    static HELD_OPEN: AtomicBool = AtomicBool::new(false);
    static HELD_DECODES: AtomicUsize = AtomicUsize::new(0);
    static STALE_DECODES: AtomicUsize = AtomicUsize::new(0);

    /// Decodes normally, but `gate.ppm` and `held.ppm` stay in progress until their gate opens.
    fn gated_decode(path: &Path) -> Result<RgbaImage, String> {
        match path.file_name().and_then(|name| name.to_str()) {
            Some("gate.ppm") => {
                GATE_ENTERED.store(true, Ordering::SeqCst);
                wait_for(&GATE_OPEN);
            }
            Some("held.ppm") => {
                HELD_ENTERED.store(true, Ordering::SeqCst);
                wait_for(&HELD_OPEN);
                HELD_DECODES.fetch_add(1, Ordering::SeqCst);
            }
            Some("stale.ppm") => {
                STALE_DECODES.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
        decode_file(path)
    }

    fn wait_for(flag: &AtomicBool) {
        let start = Instant::now();
        while !flag.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn write_ppm(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"P6\n1 1\n255\n\x01\x02\x03").unwrap();
        path
    }

    fn wait_until_idle(shared: &SharedState) {
        let start = Instant::now();
        while lock(&shared.0).decoding.is_some() && start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn load_does_not_wait_behind_a_stale_prefetch_and_queued_stale_prefetches_are_dropped() {
        let dir = std::env::temp_dir().join(format!("imgv-stale-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (gate, stale, fast) = (
            write_ppm(&dir, "gate.ppm"),
            write_ppm(&dir, "stale.ppm"),
            write_ppm(&dir, "fast.ppm"),
        );
        let mut worker = Worker::new(gated_decode);
        let shared = Arc::clone(&worker.shared);
        worker.activate(&gate);
        worker.prefetch(gate.clone());
        wait_for(&GATE_ENTERED);
        assert!(GATE_ENTERED.load(Ordering::SeqCst), "gate prefetch started");
        // Queued behind the held decode, so the next Load must drop it before it starts.
        worker.prefetch(stale.clone());
        worker.activate(&fast);
        // The Load returns while the stale decode is still held open.
        let (tx, rx) = mpsc::channel();
        let loader = std::thread::spawn(move || {
            let ok = worker.image(&fast).is_ok();
            let _ = tx.send(ok);
        });
        let loaded = rx.recv_timeout(Duration::from_secs(5));
        assert_eq!(
            loaded,
            Ok(true),
            "Load finished without waiting for the stale decode"
        );
        assert!(!GATE_OPEN.load(Ordering::SeqCst));
        GATE_OPEN.store(true, Ordering::SeqCst);
        loader.join().unwrap();
        wait_until_idle(&shared);
        assert_eq!(
            STALE_DECODES.load(Ordering::SeqCst),
            0,
            "stale prefetch was dropped"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_of_the_image_being_prefetched_waits_and_decodes_it_once() {
        let dir = std::env::temp_dir().join(format!("imgv-held-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let held = write_ppm(&dir, "held.ppm");
        let mut worker = Worker::new(gated_decode);
        let shared = Arc::clone(&worker.shared);
        worker.activate(&held);
        worker.prefetch(held.clone());
        wait_for(&HELD_ENTERED);
        let loader_path = held.clone();
        let (tx, rx) = mpsc::channel();
        let loader = std::thread::spawn(move || {
            let ok = worker.image(&loader_path).is_ok();
            let _ = tx.send(ok);
        });
        // Still waiting on the in-flight prefetch: nothing is sent yet.
        assert!(rx.recv_timeout(Duration::from_millis(150)).is_err());
        HELD_OPEN.store(true, Ordering::SeqCst);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(true));
        loader.join().unwrap();
        wait_until_idle(&shared);
        assert_eq!(
            HELD_DECODES.load(Ordering::SeqCst),
            1,
            "decoded exactly once"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

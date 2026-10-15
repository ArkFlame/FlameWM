//! Titlebar surface material: `surfaceColor` at `surfaceOpacity` over the wallpaper behind the
//! frame, blurred per `surfaceBlurPercent` (same `blur_radius_px` policy as the panel/Start).
//!
//! There is no compositor, so the WM bakes `blend(color @ opacity, blurred backdrop)` into the
//! frame's opaque titlebar pixels. The backdrop is the wallpaper composed by the shared
//! `flamewm_image_core::wallpaper` owner, never a root `XGetImage`: reading the root would
//! include the frame's own previous pixels (self-feedback) and whatever windows overlap it.
//! Like the panel, the blurred layer is the wallpaper only.
//!
//! Memory: one root-sized wallpaper canvas plus at most [`CACHE_ENTRIES`] titlebar-sized
//! backdrops. Recompute happens only on a new (rect, source generation, blur radius, root size).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use flamewm_api::background::WallpaperFit;
use flamewm_image_core::RgbaImage;
use flamewm_image_core::wallpaper::{self, DEFAULT_WALLPAPER_PATH};
use flamewm_render_x11::backdrop::blur_radius_px;

use super::drag_backdrop::{DragSnapshot, Layer};

/// Titlebar backdrops kept (about 300 KB each at 1080p width).
pub const CACHE_ENTRIES: usize = 16;
/// Backdrop used when no wallpaper pixels are available (frame background colour).
pub const FALLBACK_BACKDROP: [u8; 3] = [0x1b, 0x1e, 0x20];

/// Wallpaper the desktop owner paints, as named by the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WallpaperSource {
    Solid([u8; 3]),
    /// Empty `path` is the bundled default wallpaper.
    Image { path: String, fit: WallpaperFit },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackdropKey {
    pub rect: (i32, i32, u32, u32),
    pub generation: u32,
    pub radius_tenths: u32,
    pub root: (u32, u32),
}

#[derive(Debug, Default)]
pub struct BackdropCache {
    entries: Vec<(BackdropKey, Vec<u8>)>,
    pub hits: u64,
    pub misses: u64,
}

impl BackdropCache {
    pub fn get_or_insert_with(
        &mut self,
        key: BackdropKey,
        make: impl FnOnce() -> Vec<u8>,
    ) -> &[u8] {
        if let Some(index) = self.entries.iter().position(|(k, _)| *k == key) {
            self.hits += 1;
            let entry = self.entries.remove(index);
            self.entries.push(entry);
        } else {
            self.misses += 1;
            if self.entries.len() >= CACHE_ENTRIES {
                self.entries.remove(0);
            }
            self.entries.push((key, make()));
        }
        &self.entries.last().expect("entry just stored").1
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Debug)]
struct DragBackdrop {
    owner: u64,
    snapshot: DragSnapshot,
    blurred: Option<Blurred>,
}

/// Blurred titlebar crop: `(origin x, origin y, w, h, radius tenths, wallpaper generation)`.
type BlurKey = (i32, i32, u32, u32, u32, u32);

#[derive(Debug, Clone, PartialEq)]
struct Blurred {
    key: BlurKey,
    pixels: Vec<u8>,
}

#[derive(Default)]
struct State {
    source: Option<WallpaperSource>,
    generation: u32,
    /// Composed root-sized wallpaper for `(root size)`; `None` image means decode failed.
    canvas: Option<((u32, u32), Option<RgbaImage>)>,
    cache: BackdropCache,
    /// Content behind the frame being dragged, frozen at drag start.
    drag: Option<DragBackdrop>,
    /// Final blurred crop of the last drag, kept per owner after release. No full-root snapshot
    /// is retained.
    retained: Option<(u64, Blurred)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Publish the wallpaper source; returns the backdrop generation (bumped on change).
pub fn set_wallpaper_source(source: WallpaperSource) -> u32 {
    let Ok(mut guard) = STATE.lock() else { return 0 };
    let state = guard.get_or_insert_with(State::default);
    if state.source.as_ref() != Some(&source) {
        state.source = Some(source);
        state.generation = state.generation.wrapping_add(1);
        state.canvas = None;
        state.cache.clear();
        state.retained = None;
    }
    state.generation
}

/// Freezes what is behind frame `owner` at drag start. `raw_root` is the live root as 24-bit
/// LSB `BGRX` (`None` when the capture failed; the wallpaper is used instead). The frame's own
/// rect `mask` is always wallpaper, so the titlebar never samples its own pixels.
pub fn begin_drag(owner: u64, root: (u32, u32), raw_root: Option<Vec<u8>>, mask: (i32, i32, u32, u32)) {
    let Ok(mut guard) = STATE.lock() else { return };
    let state = guard.get_or_insert_with(State::default);
    let captured = raw_root.as_deref().map(bgrx_to_rgba);
    state.retained = None;
    state.drag = build_drag(state, owner, root, captured, mask);
}

/// Ends the drag of `owner`: the snapshot is dropped and its final blurred crop is kept.
pub fn end_drag(owner: u64) {
    let Ok(mut guard) = STATE.lock() else { return };
    if let Some(state) = guard.as_mut() {
        end_drag_in(state, owner);
    }
}

/// Drops everything kept for `owner` (window closed).
pub fn forget(owner: u64) {
    let Ok(mut guard) = STATE.lock() else { return };
    if let Some(state) = guard.as_mut() {
        forget_in(state, owner);
    }
}

fn end_drag_in(state: &mut State, owner: u64) {
    if let Some(drag) = state.drag.take_if(|drag| drag.owner == owner) {
        state.retained = drag.blurred.map(|blurred| (owner, blurred));
    }
}

fn forget_in(state: &mut State, owner: u64) {
    state.drag = state.drag.take().filter(|drag| drag.owner != owner);
    if state.retained.as_ref().is_some_and(|(kept, _)| *kept == owner) {
        state.retained = None;
    }
}

/// Titlebar pixels for `owner`: cropped from the live drag snapshot (blurred like the wallpaper
/// path, reused while origin, size, radius and wallpaper generation are unchanged), or the
/// final crop kept after release while its key still matches. `None` means use the wallpaper.
/// A kept crop is dropped as soon as its key no longer matches.
#[must_use]
pub fn drag_titlebar_pixels(
    owner: u64,
    origin: (i32, i32),
    width: u32,
    height: u32,
    rgb: (u8, u8, u8),
    alpha: u8,
    blur_percent: u8,
) -> Option<Vec<u8>> {
    let mut guard = STATE.lock().ok()?;
    titlebar_in(guard.as_mut()?, owner, origin, (width, height), rgb, alpha, blur_percent)
}

fn titlebar_in(
    state: &mut State,
    owner: u64,
    origin: (i32, i32),
    (width, height): (u32, u32),
    rgb: (u8, u8, u8),
    alpha: u8,
    blur_percent: u8,
) -> Option<Vec<u8>> {
    let radius = blur_radius_px(opacity_percent(alpha), blur_percent);
    let key = (origin.0, origin.1, width, height, (radius * 10.0).round() as u32, state.generation);
    if let Some(drag) = state.drag.as_mut().filter(|drag| drag.owner == owner) {
        if drag.blurred.as_ref().map(|blurred| blurred.key) != Some(key) {
            let pixels = blur_crop(&drag.snapshot, origin, width, height, radius);
            drag.blurred = Some(Blurred { key, pixels });
        }
        return drag.blurred.as_ref().map(|blurred| compose_rgba(&blurred.pixels, rgb, alpha));
    }
    let (kept_owner, kept) = state.retained.as_ref()?;
    if *kept_owner != owner {
        return None;
    }
    if kept.key != key {
        state.retained = None;
        return None;
    }
    Some(compose_rgba(&kept.pixels, rgb, alpha))
}

/// `w x h` crop at `origin`, Gaussian-blurred with `radius` using a padded gather so the blur
/// has real neighbours.
fn blur_crop(snapshot: &DragSnapshot, origin: (i32, i32), w: u32, h: u32, radius: f32) -> Vec<u8> {
    let pad = radius.ceil() as u32;
    let (pw, ph) = (w + 2 * pad, h + 2 * pad);
    let mut region = snapshot.crop_clamped(origin.0 - pad as i32, origin.1 - pad as i32, pw, ph);
    if radius > 0.0 {
        if let Err(error) = flamewm_image_core::blur::gaussian_blur_rgba(&mut region, pw, ph, radius) {
            eprintln!("flamewm-wm-x11: drag backdrop blur skipped: {error:?}");
        }
    }
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for row in 0..h {
        let start = (((row + pad) * pw + pad) * 4) as usize;
        out.extend_from_slice(&region[start..start + w as usize * 4]);
    }
    out
}

fn build_drag(
    state: &mut State,
    owner: u64,
    root: (u32, u32),
    captured: Option<Vec<u8>>,
    mask: (i32, i32, u32, u32),
) -> Option<DragBackdrop> {
    let (rw, rh) = root;
    let base = match captured {
        Some(pixels) if pixels.len() == rw as usize * rh as usize * 4 => pixels,
        _ => wallpaper_rect(state.source.as_ref(), &mut state.canvas, (0, 0, rw, rh), root),
    };
    let (mx, my, mw, mh) = mask;
    let masked = wallpaper_rect(state.source.as_ref(), &mut state.canvas, mask, root);
    let layer = Layer { x: mx, y: my, w: mw, h: mh, rgba: &masked };
    DragSnapshot::build((0, 0), rw, rh, &base, &[layer])
        .ok()
        .map(|snapshot| DragBackdrop { owner, snapshot, blurred: None })
}

/// Opaque straight RGBA from 24-bit LSB `BGRX` root pixels.
#[must_use]
pub fn bgrx_to_rgba(bgrx: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bgrx.len());
    for px in bgrx.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    out
}

/// Wallpaper pixels for the root-relative `rect`, edge-clamped; solid or fallback colour when
/// no image is available.
fn wallpaper_rect(
    source: Option<&WallpaperSource>,
    canvas: &mut Option<((u32, u32), Option<RgbaImage>)>,
    rect: (i32, i32, u32, u32),
    root: (u32, u32),
) -> Vec<u8> {
    let (x, y, w, h) = rect;
    let solid = |rgb: [u8; 3]| [rgb[0], rgb[1], rgb[2], 255].repeat(w as usize * h as usize);
    let image = match source {
        Some(WallpaperSource::Solid(rgb)) => return solid(*rgb),
        Some(WallpaperSource::Image { path, fit }) => {
            if canvas.as_ref().map(|(size, _)| *size) != Some(root) {
                *canvas = Some((root, compose_canvas(path, *fit, root)));
            }
            canvas.as_ref().and_then(|(_, image)| image.as_ref())
        }
        None => None,
    };
    let Some(image) = image else { return solid(FALLBACK_BACKDROP) };
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for row in 0..h {
        let sy = (y + row as i32).clamp(0, image.height as i32 - 1) as usize;
        for col in 0..w {
            let sx = (x + col as i32).clamp(0, image.width as i32 - 1) as usize;
            let at = (sy * image.width as usize + sx) * 4;
            out.extend_from_slice(&[image.pixels[at], image.pixels[at + 1], image.pixels[at + 2], 255]);
        }
    }
    out
}

/// One channel of `src @ alpha` over `dst`, rounded.
#[must_use]
pub fn blend_over(src: u8, alpha: u8, dst: u8) -> u8 {
    let a = u32::from(alpha);
    ((u32::from(src) * a + u32::from(dst) * (255 - a) + 127) / 255) as u8
}

/// Opaque RGBA of `rgb @ alpha` over each pixel of the straight-RGBA `backdrop`.
#[must_use]
pub fn compose_rgba(backdrop: &[u8], rgb: (u8, u8, u8), alpha: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(backdrop.len());
    for px in backdrop.chunks_exact(4) {
        out.extend_from_slice(&[
            blend_over(rgb.0, alpha, px[0]),
            blend_over(rgb.1, alpha, px[1]),
            blend_over(rgb.2, alpha, px[2]),
            255,
        ]);
    }
    out
}

/// Opacity percent from the 8-bit token (`percent * 255 / 100` inverse).
#[must_use]
pub fn opacity_percent(alpha: u8) -> u8 {
    ((u32::from(alpha) * 100 + 127) / 255) as u8
}

/// Flat opaque titlebar over [`FALLBACK_BACKDROP`] (no geometry / no wallpaper).
#[must_use]
pub fn flat_pixels(width: u32, height: u32, rgb: (u8, u8, u8), alpha: u8) -> Vec<u8> {
    let backdrop = [FALLBACK_BACKDROP[0], FALLBACK_BACKDROP[1], FALLBACK_BACKDROP[2], 255]
        .repeat(width as usize * height as usize);
    compose_rgba(&backdrop, rgb, alpha)
}

/// Opaque RGBA titlebar pixels for the frame at root `origin`, `width` x `height`.
#[must_use]
pub fn titlebar_pixels(
    origin: (i32, i32),
    root: (u32, u32),
    width: u32,
    height: u32,
    rgb: (u8, u8, u8),
    alpha: u8,
    blur_percent: u8,
) -> Vec<u8> {
    let radius = blur_radius_px(opacity_percent(alpha), blur_percent);
    let Ok(mut guard) = STATE.lock() else {
        return flat_pixels(width, height, rgb, alpha);
    };
    let state = guard.get_or_insert_with(State::default);
    let key = BackdropKey {
        rect: (origin.0, origin.1, width, height),
        generation: state.generation,
        radius_tenths: (radius * 10.0).round() as u32,
        root,
    };
    let State { source, canvas, cache, .. } = state;
    let backdrop = cache.get_or_insert_with(key, || {
        build_backdrop(source.as_ref(), canvas, key.rect, root, radius)
    });
    compose_rgba(backdrop, rgb, alpha)
}

fn build_backdrop(
    source: Option<&WallpaperSource>,
    canvas: &mut Option<((u32, u32), Option<RgbaImage>)>,
    rect: (i32, i32, u32, u32),
    root: (u32, u32),
    radius: f32,
) -> Vec<u8> {
    let (x, y, w, h) = rect;
    let solid = |rgb: [u8; 3]| [rgb[0], rgb[1], rgb[2], 255].repeat(w as usize * h as usize);
    let image = match source {
        Some(WallpaperSource::Solid(rgb)) => return solid(*rgb),
        Some(WallpaperSource::Image { path, fit }) => {
            if canvas.as_ref().map(|(size, _)| *size) != Some(root) {
                *canvas = Some((root, compose_canvas(path, *fit, root)));
            }
            canvas.as_ref().and_then(|(_, image)| image.as_ref())
        }
        None => None,
    };
    let Some(image) = image else { return solid(FALLBACK_BACKDROP) };
    // Padded gather with edge clamping so the blur has real neighbours and off-screen columns
    // replicate the nearest wallpaper pixel.
    let pad = radius.ceil() as u32;
    let (pw, ph) = (w + 2 * pad, h + 2 * pad);
    let mut region = Vec::with_capacity(pw as usize * ph as usize * 4);
    for row in 0..ph {
        let sy = (y + row as i32 - pad as i32).clamp(0, image.height as i32 - 1) as usize;
        for col in 0..pw {
            let sx = (x + col as i32 - pad as i32).clamp(0, image.width as i32 - 1) as usize;
            let at = (sy * image.width as usize + sx) * 4;
            region.extend_from_slice(&[
                image.pixels[at],
                image.pixels[at + 1],
                image.pixels[at + 2],
                255,
            ]);
        }
    }
    if radius > 0.0 {
        if let Err(error) = flamewm_image_core::blur::gaussian_blur_rgba(&mut region, pw, ph, radius) {
            eprintln!("flamewm-wm-x11: titlebar backdrop blur skipped: {error:?}");
        }
    }
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for row in 0..h {
        let start = (((row + pad) * pw + pad) * 4) as usize;
        out.extend_from_slice(&region[start..start + w as usize * 4]);
    }
    out
}

fn compose_canvas(path: &str, fit: WallpaperFit, root: (u32, u32)) -> Option<RgbaImage> {
    let file = if path.trim().is_empty() {
        packaged_root().join(DEFAULT_WALLPAPER_PATH)
    } else {
        PathBuf::from(path)
    };
    let composed = wallpaper::decode_file(&file)
        .map_err(|error| error.to_string())
        .and_then(|natural| {
            wallpaper::compose(&natural, root.0, root.1, fit)
                .ok_or_else(|| "wallpaper has no composable size".to_string())
        });
    match composed {
        Ok(image) => Some(image),
        Err(error) => {
            eprintln!("flamewm-wm-x11: titlebar backdrop wallpaper {}: {error}", file.display());
            None
        }
    }
}

/// Installed data root (`<prefix>/usr/share/flamewm`) when present, else the source tree; the
/// same resolution the desktop wallpaper owner uses for the bundled default.
fn packaged_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::env::current_exe()
        .ok()
        .and_then(|exe| {
            let bin = exe.parent()?;
            let usr = bin.parent()?;
            (bin.file_name()? == "bin" && usr.file_name()? == "usr")
                .then(|| usr.join("share/flamewm"))
        })
        .filter(|root| root.join("assets").is_dir())
        .unwrap_or(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_math_at_opacity_0_50_100_over_known_backdrop() {
        let backdrop = [200u8, 100, 0, 255];
        assert_eq!(compose_rgba(&backdrop, (0, 0, 0), 0), vec![200, 100, 0, 255]);
        assert_eq!(compose_rgba(&backdrop, (0, 0, 0), 127), vec![100, 50, 0, 255]);
        assert_eq!(compose_rgba(&backdrop, (10, 20, 30), 255), vec![10, 20, 30, 255]);
    }

    #[test]
    fn opacity_percent_round_trips_every_percent() {
        for percent in 0..=100u8 {
            let alpha = (u16::from(percent) * 255 / 100) as u8;
            assert_eq!(opacity_percent(alpha), percent);
        }
    }

    #[test]
    fn cache_hits_on_same_key_and_misses_on_any_field_change() {
        let key = BackdropKey { rect: (1, 2, 4, 2), generation: 1, radius_tenths: 0, root: (100, 50) };
        let mut cache = BackdropCache::default();
        cache.get_or_insert_with(key, || vec![1]);
        cache.get_or_insert_with(key, || panic!("must hit"));
        assert_eq!((cache.hits, cache.misses), (1, 1));
        for changed in [
            BackdropKey { rect: (2, 2, 4, 2), ..key },
            BackdropKey { generation: 2, ..key },
            BackdropKey { radius_tenths: 240, ..key },
            BackdropKey { root: (200, 50), ..key },
        ] {
            cache.get_or_insert_with(changed, || vec![2]);
        }
        assert_eq!(cache.misses, 5);
    }

    #[test]
    fn cache_is_bounded() {
        let mut cache = BackdropCache::default();
        for i in 0..(CACHE_ENTRIES as i32 * 2) {
            let key = BackdropKey { rect: (i, 0, 1, 1), generation: 0, radius_tenths: 0, root: (1, 1) };
            cache.get_or_insert_with(key, || vec![0; 4]);
        }
        assert_eq!(cache.len(), CACHE_ENTRIES);
    }

    #[test]
    fn flat_fallback_without_wallpaper_is_opaque_and_never_errors() {
        let px = flat_pixels(3, 2, (0, 0, 0), 0);
        assert_eq!(px.len(), 3 * 2 * 4);
        assert_eq!(&px[..4], &[0x1b, 0x1e, 0x20, 255]);
    }

    #[test]
    fn bgrx_converts_to_opaque_rgba() {
        assert_eq!(bgrx_to_rgba(&[3, 2, 1, 0]), vec![1, 2, 3, 255]);
    }

    #[test]
    fn drag_snapshot_uses_capture_outside_mask_and_wallpaper_inside() {
        let mut state = State { source: Some(WallpaperSource::Solid([9, 9, 9])), ..State::default() };
        let captured: Vec<u8> = (0..4 * 4).flat_map(|i| [i as u8, 0, 0, 255]).collect();
        // Mask the 2x2 block at (1,1) of a 4x4 root; it must read as wallpaper colour.
        let drag = build_drag(&mut state, 7, (4, 4), Some(captured.clone()), (1, 1, 2, 2)).unwrap();
        assert_eq!(drag.snapshot.crop(0, 0, 1, 1), captured[0..4].to_vec());
        assert_eq!(drag.snapshot.crop(1, 1, 1, 1), vec![9, 9, 9, 255]);
        assert_eq!(drag.snapshot.crop(2, 2, 1, 1), vec![9, 9, 9, 255]);
        assert_eq!(drag.snapshot.crop(3, 3, 1, 1), captured[60..64].to_vec());
    }

    #[test]
    fn drag_blur_keeps_uniform_colour_and_zero_radius_is_plain_crop() {
        let mut state = State { source: Some(WallpaperSource::Solid([0, 0, 0])), ..State::default() };
        let uniform: Vec<u8> = [40, 80, 120, 255].repeat(8 * 8);
        let drag = build_drag(&mut state, 1, (8, 8), Some(uniform), (0, 0, 0, 0)).unwrap();
        let blurred = blur_crop(&drag.snapshot, (2, 2), 3, 3, 2.0);
        assert!(blurred.chunks_exact(4).all(|px| px == [40, 80, 120, 255]));
        assert_eq!(blur_crop(&drag.snapshot, (2, 2), 3, 3, 0.0), drag.snapshot.crop(2, 2, 3, 3));
    }

    #[test]
    fn kept_crop_survives_release_and_drops_on_move_wallpaper_or_forget() {
        let mut state = State { source: Some(WallpaperSource::Solid([9, 9, 9])), ..State::default() };
        let captured: Vec<u8> = [200, 0, 0, 255].repeat(8 * 8);
        state.drag = build_drag(&mut state, 3, (8, 8), Some(captured), (0, 0, 0, 0));
        let live = titlebar_in(&mut state, 3, (2, 2), (3, 2), (0, 0, 0), 0, 0).unwrap();
        assert_eq!(&live[..4], &[200, 0, 0, 255]);
        end_drag_in(&mut state, 3);
        assert!(state.drag.is_none() && state.retained.is_some(), "snapshot dropped, crop kept");
        assert_eq!(titlebar_in(&mut state, 3, (2, 2), (3, 2), (0, 0, 0), 0, 0), Some(live.clone()));
        assert_eq!(titlebar_in(&mut state, 4, (2, 2), (3, 2), (0, 0, 0), 0, 0), None, "other owner");
        assert!(titlebar_in(&mut state, 3, (5, 2), (3, 2), (0, 0, 0), 0, 0).is_none(), "moved");
        assert!(state.retained.is_none(), "moved frame drops the kept crop");
        state.drag = build_drag(&mut state, 3, (8, 8), Some([200, 0, 0, 255].repeat(64)), (0, 0, 0, 0));
        end_drag_in(&mut state, 3);
        state.generation += 1;
        assert!(titlebar_in(&mut state, 3, (2, 2), (3, 2), (0, 0, 0), 0, 0).is_none(), "wallpaper change");
        state.drag = build_drag(&mut state, 3, (8, 8), Some([200, 0, 0, 255].repeat(64)), (0, 0, 0, 0));
        end_drag_in(&mut state, 3);
        forget_in(&mut state, 3);
        assert!(state.retained.is_none(), "closed window drops the kept crop");
    }

    #[test]
    fn drag_snapshot_falls_back_to_wallpaper_without_capture() {
        let mut state = State { source: Some(WallpaperSource::Solid([5, 6, 7])), ..State::default() };
        let drag = build_drag(&mut state, 1, (2, 2), None, (0, 0, 1, 1)).unwrap();
        assert_eq!(drag.snapshot.crop(1, 1, 1, 1), vec![5, 6, 7, 255]);
    }

    #[test]
    fn drag_snapshot_rejects_wrong_capture_length_and_uses_wallpaper() {
        let mut state = State { source: Some(WallpaperSource::Solid([5, 6, 7])), ..State::default() };
        let drag = build_drag(&mut state, 1, (2, 2), Some(vec![0; 4]), (0, 0, 0, 0)).unwrap();
        assert_eq!(drag.snapshot.crop(1, 1, 1, 1), vec![5, 6, 7, 255]);
    }

    #[test]
    fn solid_source_blends_over_wallpaper_colour_and_tracks_generation() {
        let g1 = set_wallpaper_source(WallpaperSource::Solid([100, 100, 100]));
        assert_eq!(set_wallpaper_source(WallpaperSource::Solid([100, 100, 100])), g1);
        let px = titlebar_pixels((-10, -5), (64, 64), 4, 2, (0, 0, 0), 255, 80);
        assert_eq!(&px[..4], &[0, 0, 0, 255]);
        let half = titlebar_pixels((0, 0), (64, 64), 4, 2, (0, 0, 0), 0, 80);
        assert_eq!(&half[..4], &[100, 100, 100, 255]);
        assert_ne!(set_wallpaper_source(WallpaperSource::Solid([1, 2, 3])), g1);
    }
}

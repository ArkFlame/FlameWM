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
use flamewm_image_core::blur::{BlurScratch, gaussian_blur_rgba_with};
use flamewm_image_core::wallpaper::{self, DEFAULT_WALLPAPER_PATH};
use flamewm_render_x11::backdrop::blur_radius_px;

use super::drag_backdrop::{DragSnapshot, Layer};

/// Titlebar backdrops kept (about 300 KB each at 1080p width).
pub const CACHE_ENTRIES: usize = 16;
/// Total `Vec<u8>` bytes kept across backdrops; the newest entry is always kept.
pub const CACHE_BYTE_BUDGET: usize = 4 * 1024 * 1024;
/// Backdrop used when no wallpaper pixels are available (frame background colour).
pub const FALLBACK_BACKDROP: [u8; 3] = [0x1b, 0x1e, 0x20];
/// Grid cell edge for the drag blur. A region blurred for a cell covers it plus this much slack
/// to the right and bottom, so every origin inside the cell crops from one blur.
const DRAG_SLACK_X: u32 = 512;
const DRAG_SLACK_Y: u32 = 32;
/// Frame rows the drag snapshot masks with wallpaper. Titlebars are at most 72px (config clamp),
/// the cell slack adds `DRAG_SLACK_Y` and the blur pad is at most 24px per side
/// (`MAX_BLUR_RADIUS_PX`), so 160 rows cover every row a titlebar blur reads from the frame.
const DRAG_MASK_BAND_Y: u32 = 160;

/// Wallpaper the desktop owner paints, as named by the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WallpaperSource {
    Solid([u8; 3]),
    /// Empty `path` is the bundled default wallpaper.
    Image {
        path: String,
        fit: WallpaperFit,
    },
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
            // Entries of a previous root size are stale once the root changes.
            self.entries.retain(|(k, _)| k.root == key.root);
            if self.entries.len() >= CACHE_ENTRIES {
                self.entries.remove(0);
            }
            self.entries.push((key, make()));
            while self.entries.len() > 1 && self.bytes() > CACHE_BYTE_BUDGET {
                self.entries.remove(0);
            }
        }
        &self.entries.last().expect("entry just stored").1
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    fn contains(&self, key: &BackdropKey) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    fn bytes(&self) -> usize {
        self.entries.iter().map(|(_, pixels)| pixels.len()).sum()
    }
}

#[derive(Debug)]
struct DragBackdrop {
    owner: u64,
    snapshot: DragSnapshot,
    blurred: Option<Blurred>,
    /// Interactive resize: the blurred region is served as-is (edge-clamped) for every size until
    /// the drag ends, so a resize step never re-blurs. The final size is blurred after release.
    frozen: bool,
    /// Blur work buffers reused by every move step of this drag.
    scratch: BlurScratch,
    /// Padded crop reused by every blur of this drag.
    crop: Vec<u8>,
}

/// Blurred titlebar crop: `(origin x, origin y, w, h, radius tenths, wallpaper generation)`.
type BlurKey = (i32, i32, u32, u32, u32, u32);

#[derive(Debug, Clone, PartialEq)]
struct Blurred {
    /// Exact key of the last crop served; the retained crop matches on it.
    key: BlurKey,
    /// Grid cell the region was blurred for.
    cell: (i32, i32),
    /// `(w + DRAG_SLACK) x (h + DRAG_SLACK)` straight RGBA blurred at `cell`.
    region: Vec<u8>,
    /// Width and height of `region` in pixels.
    region_w: u32,
    region_h: u32,
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
    /// Decoded natural wallpaper image for the masked drag path, kept per file so a drag start
    /// does not decode the wallpaper again. Dropped when the wallpaper source changes.
    natural: Option<(PathBuf, RgbaImage)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Publish the wallpaper source; returns the backdrop generation (bumped on change).
pub fn set_wallpaper_source(source: WallpaperSource) -> u32 {
    let Ok(mut guard) = STATE.lock() else {
        return 0;
    };
    let state = guard.get_or_insert_with(State::default);
    if state.source.as_ref() != Some(&source) {
        state.source = Some(source);
        state.generation = state.generation.wrapping_add(1);
        state.canvas = None;
        state.natural = None;
        state.cache.clear();
        state.retained = None;
        // Decode now, not on the first drag press: a full wallpaper decode there cost ~280ms CPU.
        if let Some(WallpaperSource::Image { path, .. }) = state.source.as_ref() {
            let file = wallpaper_file(path);
            state.natural = wallpaper::decode_file(&file).ok().map(|image| (file, image));
        }
    }
    state.generation
}

/// Freezes what is behind frame `owner` at drag start. `raw_root` is the live root as 24-bit
/// LSB `BGRX` (`None` when the capture failed; the wallpaper is used instead). The frame's own
/// rect `mask` is always wallpaper, so the titlebar never samples its own pixels.
pub fn begin_drag(
    owner: u64,
    root: (u32, u32),
    raw_root: Option<Vec<u8>>,
    mask: (i32, i32, u32, u32),
) {
    let Ok(mut guard) = STATE.lock() else { return };
    let state = guard.get_or_insert_with(State::default);
    let captured = raw_root.map(bgrx_to_rgba);
    state.retained = None;
    state.drag = build_drag(state, owner, root, captured, band_mask(mask));
}

/// The part of the frame `mask` the titlebar can read: its top [`DRAG_MASK_BAND_Y`] rows. A
/// titlebar blur reads the snapshot only to `titlebar + DRAG_SLACK_Y + padding` below the frame
/// top, so the rest of the frame never needs wallpaper composed for it. Composing the whole frame
/// cost ~50ms of CPU at every drag start.
fn band_mask(mask: (i32, i32, u32, u32)) -> (i32, i32, u32, u32) {
    (mask.0, mask.1, mask.2, mask.3.min(DRAG_MASK_BAND_Y))
}

/// Freezes (or thaws) the drag backdrop of `owner`. While frozen, titlebar pixels come from the
/// blur already taken for the drag, clamped to its extent, so a resize never re-blurs. Thawing
/// does not blur: the next paint computes the blur for the size it is actually painting.
pub fn set_drag_frozen(owner: u64, frozen: bool) {
    let Ok(mut guard) = STATE.lock() else { return };
    if let Some(state) = guard.as_mut() {
        set_drag_frozen_in(state, owner, frozen);
    }
}

fn set_drag_frozen_in(state: &mut State, owner: u64, frozen: bool) {
    if let Some(drag) = state.drag.as_mut().filter(|drag| drag.owner == owner) {
        drag.frozen = frozen;
    }
}

/// Ends the drag of `owner`: the snapshot is dropped and its final blurred crop is kept.
pub fn end_drag(owner: u64) {
    let Ok(mut guard) = STATE.lock() else { return };
    if let Some(state) = guard.as_mut() {
        end_drag_in(state, owner);
    }
}

/// Drops the root-sized wallpaper canvas; rebuilt lazily on the next miss. Called when a
/// transition finishes.
pub fn release_backdrop_canvas() {
    let Ok(mut guard) = STATE.lock() else { return };
    if let Some(state) = guard.as_mut() {
        release_canvas_in(state);
    }
}

fn release_canvas_in(state: &mut State) {
    state.canvas = None;
}

/// Resident root canvas bytes (0 when released or decode failed).
#[cfg(test)]
fn canvas_bytes(state: &State) -> usize {
    state
        .canvas
        .as_ref()
        .and_then(|(_, image)| image.as_ref())
        .map_or(0, |image| image.pixels.len())
}

/// Drops everything kept for `owner` (window closed).
pub fn forget(owner: u64) {
    let Ok(mut guard) = STATE.lock() else { return };
    if let Some(state) = guard.as_mut() {
        forget_in(state, owner);
    }
}

fn end_drag_in(state: &mut State, owner: u64) {
    if let Some(mut drag) = state.drag.take_if(|drag| drag.owner == owner) {
        drag.snapshot.release();
        state.retained = drag.blurred.map(|blurred| (owner, blurred));
    }
}

fn forget_in(state: &mut State, owner: u64) {
    state.drag = state.drag.take().filter(|drag| drag.owner != owner);
    if state
        .retained
        .as_ref()
        .is_some_and(|(kept, _)| *kept == owner)
    {
        state.retained = None;
    }
}

/// Titlebar pixels for `owner`: cropped from the live drag snapshot (blurred like the wallpaper
/// path). The blur is reused while the origin stays in its [`DRAG_SLACK`] grid cell and size,
/// radius and wallpaper generation are unchanged, so each cell is blurred once per drag. Or the
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
    titlebar_in(
        guard.as_mut()?,
        owner,
        origin,
        (width, height),
        rgb,
        alpha,
        blur_percent,
    )
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
    let radius_tenths = (radius * 10.0).round() as u32;
    let generation = state.generation;
    let key = (origin.0, origin.1, width, height, radius_tenths, generation);
    if let Some(drag) = state.drag.as_mut().filter(|drag| drag.owner == owner) {
        if drag.frozen {
            if let Some(blurred) = drag.blurred.as_ref() {
                // Interactive resize: reuse the blur taken at drag start; no recompute.
                let crop = slice_region_clamped(blurred, origin, width, height);
                return Some(compose_rgba(&crop, rgb, alpha));
            }
        }
        let cell = drag_cell(origin);
        let reusable = drag.blurred.as_ref().is_some_and(|blurred| {
            blurred.cell == cell
                && (blurred.key.2, blurred.key.3, blurred.key.4, blurred.key.5)
                    == (width, height, radius_tenths, generation)
        });
        if reusable {
            if let Some(blurred) = drag.blurred.as_mut() {
                blurred.key = key;
            }
        } else {
            let region_w = width + DRAG_SLACK_X;
            let region_h = height + DRAG_SLACK_Y;
            let region = blur_region(
                &mut drag.scratch,
                &drag.snapshot,
                cell,
                region_w,
                region_h,
                radius,
                &mut drag.crop,
            );
            drag.blurred = Some(Blurred {
                key,
                cell,
                region,
                region_w,
                region_h,
            });
        }
        let blurred = drag.blurred.as_ref()?;
        let crop = slice_region(
            &blurred.region,
            width + DRAG_SLACK_X,
            origin,
            cell,
            width,
            height,
        );
        return Some(compose_rgba(&crop, rgb, alpha));
    }
    let (kept_owner, kept) = state.retained.as_ref()?;
    if *kept_owner != owner {
        return None;
    }
    if kept.key != key {
        // Same place, same wallpaper and radius, different size (a resize just released): serve
        // the kept blur mirror-extended. A full wallpaper blur here stalls the release paint.
        let same_place = (kept.key.0, kept.key.1, kept.key.4, kept.key.5)
            == (key.0, key.1, key.4, key.5);
        if !same_place {
            state.retained = None;
            return None;
        }
        let crop = slice_region_clamped(kept, origin, width, height);
        return Some(compose_rgba(&crop, rgb, alpha));
    }
    let crop = slice_region(
        &kept.region,
        width + DRAG_SLACK_X,
        origin,
        kept.cell,
        width,
        height,
    );
    Some(compose_rgba(&crop, rgb, alpha))
}

/// Grid cell (top-left) of the [`DRAG_SLACK`]-sized cell containing `origin`.
fn drag_cell(origin: (i32, i32)) -> (i32, i32) {
    let (slack_x, slack_y) = (DRAG_SLACK_X as i32, DRAG_SLACK_Y as i32);
    (
        origin.0.div_euclid(slack_x) * slack_x,
        origin.1.div_euclid(slack_y) * slack_y,
    )
}

/// `w x h` crop at `origin` out of a row-major region of width `region_w` whose top-left is
/// `cell`. `origin` must lie inside the region.
fn slice_region(
    region: &[u8],
    region_w: u32,
    origin: (i32, i32),
    cell: (i32, i32),
    w: u32,
    h: u32,
) -> Vec<u8> {
    let ox = (origin.0 - cell.0) as usize;
    let oy = (origin.1 - cell.1) as usize;
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for row in 0..h as usize {
        let start = ((oy + row) * region_w as usize + ox) * 4;
        out.extend_from_slice(&region[start..start + w as usize * 4]);
    }
    out
}

/// `w x h` crop at `origin` out of a blurred region. Pixels outside the region reflect back
/// across its nearest edge (mirror tiling), so a titlebar grown past the blurred extent continues
/// the existing blur with no seam, and a left/top grow reflects the leading edge.
fn slice_region_clamped(blurred: &Blurred, origin: (i32, i32), w: u32, h: u32) -> Vec<u8> {
    let (rw, rh) = (i64::from(blurred.region_w), i64::from(blurred.region_h));
    let ox = i64::from(origin.0 - blurred.cell.0);
    let oy = i64::from(origin.1 - blurred.cell.1);
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for row in 0..i64::from(h) {
        let sy = flamewm_render_x11::backdrop::mirror_index(oy + row, 0, rh);
        for col in 0..i64::from(w) {
            let sx = flamewm_render_x11::backdrop::mirror_index(ox + col, 0, rw);
            let at = (sy * rw as usize + sx) * 4;
            out.extend_from_slice(&blurred.region[at..at + 4]);
        }
    }
    out
}

#[cfg(test)]
mod mirror_clamp_tests {
    use super::*;

    fn blurred_2x1() -> Blurred {
        // 2-wide, 1-high region: [A, B] with cell origin (0, 0).
        Blurred {
            key: (0, 0, 2, 1, 0, 0),
            cell: (0, 0),
            region: vec![10, 0, 0, 255, 20, 0, 0, 255],
            region_w: 2,
            region_h: 1,
        }
    }

    #[test]
    fn grow_right_past_region_mirrors_the_right_edge() {
        // Columns 0..4 at origin 0 -> A, B, B, A (reflected), no repeated-edge plateau.
        let out = slice_region_clamped(&blurred_2x1(), (0, 0), 4, 1);
        let reds: Vec<u8> = out.chunks_exact(4).map(|px| px[0]).collect();
        assert_eq!(reds, vec![10, 20, 20, 10]);
    }

    #[test]
    fn grow_left_before_region_mirrors_the_leading_edge() {
        // Origin one pixel left of the region: column -1 reflects column 0.
        let out = slice_region_clamped(&blurred_2x1(), (-1, 0), 3, 1);
        let reds: Vec<u8> = out.chunks_exact(4).map(|px| px[0]).collect();
        assert_eq!(reds, vec![10, 10, 20]);
    }

    #[test]
    fn inside_region_is_an_exact_crop() {
        let out = slice_region_clamped(&blurred_2x1(), (0, 0), 2, 1);
        assert_eq!(out, blurred_2x1().region);
    }
}

/// `w x h` region at `origin`, Gaussian-blurred with `radius` using a padded gather so the blur
/// has real neighbours.
fn blur_region(
    scratch: &mut BlurScratch,
    snapshot: &DragSnapshot,
    origin: (i32, i32),
    w: u32,
    h: u32,
    radius: f32,
    crop: &mut Vec<u8>,
) -> Vec<u8> {
    let pad = radius.ceil() as u32;
    let (pw, ph) = (w + 2 * pad, h + 2 * pad);
    snapshot.crop_clamped_into(origin.0 - pad as i32, origin.1 - pad as i32, pw, ph, crop);
    if radius > 0.0 {
        if let Err(error) = gaussian_blur_rgba_with(scratch, crop, pw, ph, radius) {
            eprintln!("flamewm-wm-x11: drag backdrop blur skipped: {error:?}");
        }
    }
    // `origin` sits `pad` inside the padded region, whose top-left is `origin - pad`.
    slice_region(
        crop,
        pw,
        origin,
        (origin.0 - pad as i32, origin.1 - pad as i32),
        w,
        h,
    )
}

/// Fresh reference blur of the `w x h` crop at `origin`: the cell region's crop, so it equals
/// what the drag cache serves for that origin.
#[cfg(test)]
fn blur_crop(
    scratch: &mut BlurScratch,
    snapshot: &DragSnapshot,
    origin: (i32, i32),
    w: u32,
    h: u32,
    radius: f32,
) -> Vec<u8> {
    let cell = drag_cell(origin);
    let mut crop = Vec::new();
    let region = blur_region(
        scratch,
        snapshot,
        cell,
        w + DRAG_SLACK_X,
        h + DRAG_SLACK_Y,
        radius,
        &mut crop,
    );
    slice_region(&region, w + DRAG_SLACK_X, origin, cell, w, h)
}

fn build_drag(
    state: &mut State,
    owner: u64,
    root: (u32, u32),
    captured: Option<Vec<u8>>,
    mask: (i32, i32, u32, u32),
) -> Option<DragBackdrop> {
    let (rw, rh) = root;
    let (mx, my, mw, mh) = mask;
    let captured = captured.filter(|pixels| pixels.len() == rw as usize * rh as usize * 4);
    // Masked first: without a capture it reads the canvas, which `wallpaper_base` may consume.
    let masked = mask_pixels(
        state.source.as_ref(),
        &mut state.canvas,
        &mut state.natural,
        mask,
        root,
        captured.is_some(),
    );
    let base =
        captured.unwrap_or_else(|| wallpaper_base(state.source.as_ref(), &mut state.canvas, root));
    let layer = Layer {
        x: mx,
        y: my,
        w: mw,
        h: mh,
        rgba: &masked,
    };
    DragSnapshot::build((0, 0), rw, rh, base, &[layer])
        .ok()
        .map(|snapshot| DragBackdrop {
            owner,
            snapshot,
            blurred: None,
            frozen: false,
            scratch: BlurScratch::new(),
            crop: Vec::new(),
        })
}

/// Opaque straight RGBA from 24-bit LSB `BGRX` root pixels, swapped in place. A trailing
/// partial pixel is dropped.
#[must_use]
pub fn bgrx_to_rgba(mut bgrx: Vec<u8>) -> Vec<u8> {
    bgrx.truncate(bgrx.len() / 4 * 4);
    for px in bgrx.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    bgrx
}

/// Root-sized wallpaper as an owned base. A matching decoded canvas is moved out (no copy) and
/// made opaque; it is then rebuilt lazily on next use. Otherwise falls back to `wallpaper_rect`.
fn wallpaper_base(
    source: Option<&WallpaperSource>,
    canvas: &mut Option<((u32, u32), Option<RgbaImage>)>,
    root: (u32, u32),
) -> Vec<u8> {
    match canvas.take() {
        Some((size, Some(mut image)))
            if size == root && image.width == root.0 && image.height == root.1 =>
        {
            for px in image.pixels.chunks_exact_mut(4) {
                px[3] = 255;
            }
            return image.pixels;
        }
        other => *canvas = other,
    }
    wallpaper_rect(source, canvas, (0, 0, root.0, root.1), root)
}

/// Wallpaper pixels for the frame `mask`. With a root capture present, an image mask is composed
/// from the decoded natural image, so no root canvas is built or kept. Otherwise, or when that is
/// refused, it comes from the cached root canvas.
fn mask_pixels(
    source: Option<&WallpaperSource>,
    canvas: &mut Option<((u32, u32), Option<RgbaImage>)>,
    natural: &mut Option<(PathBuf, RgbaImage)>,
    mask: (i32, i32, u32, u32),
    root: (u32, u32),
    has_capture: bool,
) -> Vec<u8> {
    if has_capture {
        if let Some(WallpaperSource::Image { path, fit }) = source {
            let file = wallpaper_file(path);
            if natural.as_ref().is_none_or(|(kept, _)| *kept != file) {
                *natural = wallpaper::decode_file(&file)
                    .ok()
                    .map(|image| (file.clone(), image));
            }
            if let Some(pixels) = natural
                .as_ref()
                .and_then(|(_, image)| masked_from_natural(image, *fit, mask, root))
            {
                return pixels;
            }
        }
    }
    wallpaper_rect(source, canvas, mask, root)
}

/// Root-relative `mask` pixels from the decoded `natural` image, edge-clamped exactly as
/// [`wallpaper_rect`] clamps the root canvas. Only the clamped span is composed, via
/// [`wallpaper::compose_region`]. `None` for an empty mask or root, or a refused composition.
fn masked_from_natural(
    natural: &RgbaImage,
    fit: WallpaperFit,
    mask: (i32, i32, u32, u32),
    root: (u32, u32),
) -> Option<Vec<u8>> {
    let (x, y, w, h) = mask;
    if w == 0 || h == 0 || root.0 == 0 || root.1 == 0 {
        return None;
    }
    let clamp = |v: i32, len: u32| v.clamp(0, len as i32 - 1);
    let (x0, x1) = (clamp(x, root.0), clamp(x + w as i32 - 1, root.0));
    let (y0, y1) = (clamp(y, root.1), clamp(y + h as i32 - 1, root.1));
    let (sw, sh) = ((x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32);
    let span = wallpaper::compose_region(natural, root.0, root.1, fit, (x0, y0, sw, sh))?;
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for row in 0..h as i32 {
        let sy = (clamp(y + row, root.1) - y0) as usize;
        for col in 0..w as i32 {
            let sx = (clamp(x + col, root.0) - x0) as usize;
            let at = (sy * sw as usize + sx) * 4;
            out.extend_from_slice(&[
                span.pixels[at],
                span.pixels[at + 1],
                span.pixels[at + 2],
                255,
            ]);
        }
    }
    Some(out)
}

/// Wallpaper pixels for the root-relative `rect`, edge-clamped; solid or fallback colour when
/// no image is available.
fn solid_rgba(rgb: [u8; 3], w: u32, h: u32) -> Vec<u8> {
    [rgb[0], rgb[1], rgb[2], 255].repeat(w as usize * h as usize)
}

/// Root canvas for an image source, composed once per root size. `None` for solid or no source,
/// and when the image fails to decode.
fn canvas_image<'a>(
    source: Option<&WallpaperSource>,
    canvas: &'a mut Option<((u32, u32), Option<RgbaImage>)>,
    root: (u32, u32),
) -> Option<&'a RgbaImage> {
    let Some(WallpaperSource::Image { path, fit }) = source else {
        return None;
    };
    if canvas.as_ref().map(|(size, _)| *size) != Some(root) {
        *canvas = Some((root, compose_canvas(path, *fit, root)));
    }
    canvas.as_ref().and_then(|(_, image)| image.as_ref())
}

fn wallpaper_rect(
    source: Option<&WallpaperSource>,
    canvas: &mut Option<((u32, u32), Option<RgbaImage>)>,
    rect: (i32, i32, u32, u32),
    root: (u32, u32),
) -> Vec<u8> {
    let (x, y, w, h) = rect;
    if let Some(WallpaperSource::Solid(rgb)) = source {
        return solid_rgba(*rgb, w, h);
    }
    let Some(image) = canvas_image(source, canvas, root) else {
        return solid_rgba(FALLBACK_BACKDROP, w, h);
    };
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for row in 0..h {
        let sy = (y + row as i32).clamp(0, image.height as i32 - 1) as usize;
        for col in 0..w {
            let sx = (x + col as i32).clamp(0, image.width as i32 - 1) as usize;
            let at = (sy * image.width as usize + sx) * 4;
            out.extend_from_slice(&[
                image.pixels[at],
                image.pixels[at + 1],
                image.pixels[at + 2],
                255,
            ]);
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

/// Whether the titlebar needs the captured backdrop at all. An opaque titlebar (255) is its flat
/// colour whatever lies behind it, so the drag capture and the wallpaper canvas are skipped.
#[must_use]
pub fn needs_drag_backdrop(alpha: u8) -> bool {
    alpha < 255
}

/// Flat opaque titlebar over [`FALLBACK_BACKDROP`] (no geometry / no wallpaper).
#[must_use]
pub fn flat_pixels(width: u32, height: u32, rgb: (u8, u8, u8), alpha: u8) -> Vec<u8> {
    compose_rgba(&solid_rgba(FALLBACK_BACKDROP, width, height), rgb, alpha)
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
    titlebar_fill_in(state, origin, root, (width, height), rgb, alpha, radius)
}

/// Non-drag fill. The root-sized wallpaper canvas is kept across consecutive misses (they reuse
/// it) and dropped on the first cache hit that is not inside a drag, since hits never read it.
/// `release_backdrop_canvas` drops it when the transition ends.
fn titlebar_fill_in(
    state: &mut State,
    origin: (i32, i32),
    root: (u32, u32),
    (width, height): (u32, u32),
    rgb: (u8, u8, u8),
    alpha: u8,
    radius: f32,
) -> Vec<u8> {
    if !needs_drag_backdrop(alpha) {
        return flat_pixels(width, height, rgb, alpha);
    }
    let key = BackdropKey {
        rect: (origin.0, origin.1, width, height),
        generation: state.generation,
        radius_tenths: (radius * 10.0).round() as u32,
        root,
    };
    let State {
        source,
        canvas,
        cache,
        drag,
        ..
    } = state;
    let hit = cache.contains(&key);
    let out = compose_rgba(
        cache.get_or_insert_with(key, || {
            build_backdrop(source.as_ref(), canvas, key.rect, root, radius)
        }),
        rgb,
        alpha,
    );
    // A hit never reads the canvas; drop it unless an active drag may still rebuild misses.
    if hit && drag.is_none() {
        *canvas = None;
    }
    out
}

fn build_backdrop(
    source: Option<&WallpaperSource>,
    canvas: &mut Option<((u32, u32), Option<RgbaImage>)>,
    rect: (i32, i32, u32, u32),
    root: (u32, u32),
    radius: f32,
) -> Vec<u8> {
    let (x, y, w, h) = rect;
    if let Some(WallpaperSource::Solid(rgb)) = source {
        return solid_rgba(*rgb, w, h);
    }
    // Padded gather with edge clamping so the blur has real neighbours and off-screen columns
    // replicate the nearest wallpaper pixel.
    let pad = radius.ceil() as u32;
    let (pw, ph) = (w + 2 * pad, h + 2 * pad);
    let mut region = wallpaper_rect(
        source,
        canvas,
        (x - pad as i32, y - pad as i32, pw, ph),
        root,
    );
    if radius > 0.0 {
        if let Err(error) =
            flamewm_image_core::blur::gaussian_blur_rgba(&mut region, pw, ph, radius)
        {
            eprintln!("flamewm-wm-x11: titlebar backdrop blur skipped: {error:?}");
        }
    }
    slice_region(&region, pw, (x, y), (x - pad as i32, y - pad as i32), w, h)
}

/// Wallpaper file for `path`; an empty path is the bundled default.
fn wallpaper_file(path: &str) -> PathBuf {
    if path.trim().is_empty() {
        packaged_root().join(DEFAULT_WALLPAPER_PATH)
    } else {
        PathBuf::from(path)
    }
}

fn compose_canvas(path: &str, fit: WallpaperFit, root: (u32, u32)) -> Option<RgbaImage> {
    let file = wallpaper_file(path);
    let composed = wallpaper::decode_file(&file)
        .map_err(|error| error.to_string())
        .and_then(|natural| {
            wallpaper::compose(&natural, root.0, root.1, fit)
                .ok_or_else(|| "wallpaper has no composable size".to_string())
        });
    match composed {
        Ok(image) => Some(image),
        Err(error) => {
            eprintln!(
                "flamewm-wm-x11: titlebar backdrop wallpaper {}: {error}",
                file.display()
            );
            None
        }
    }
}

/// Installed data root (`<prefix>/usr/share/flamewm`) when present, else the source tree; the
/// same resolution the desktop wallpaper owner uses for the bundled default.
pub(crate) fn packaged_root() -> PathBuf {
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

    /// Writes a 7x5 PPM under `tag` and returns a fill-mode state sourced from it.
    fn ppm_state(tag: &str) -> (State, PathBuf) {
        let path = std::env::temp_dir().join(format!("flamewm-{tag}-{}.ppm", std::process::id()));
        let mut bytes = b"P6\n7 5\n255\n".to_vec();
        bytes.extend([90u8, 40, 10].repeat(7 * 5));
        std::fs::write(&path, &bytes).unwrap();
        let source = WallpaperSource::Image {
            path: path.to_string_lossy().into_owned(),
            fit: WallpaperFit::Fill,
        };
        (
            State {
                source: Some(source),
                ..State::default()
            },
            path,
        )
    }

    /// Fill-mode state whose image path never decodes.
    fn missing_image_state() -> State {
        State {
            source: Some(WallpaperSource::Image {
                path: "/nonexistent/flamewm-test-wallpaper.png".to_owned(),
                fit: WallpaperFit::Fill,
            }),
            ..State::default()
        }
    }

    #[test]
    fn blend_math_at_opacity_0_50_100_over_known_backdrop() {
        let backdrop = [200u8, 100, 0, 255];
        assert_eq!(
            compose_rgba(&backdrop, (0, 0, 0), 0),
            vec![200, 100, 0, 255]
        );
        assert_eq!(
            compose_rgba(&backdrop, (0, 0, 0), 127),
            vec![100, 50, 0, 255]
        );
        assert_eq!(
            compose_rgba(&backdrop, (10, 20, 30), 255),
            vec![10, 20, 30, 255]
        );
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
        let key = BackdropKey {
            rect: (1, 2, 4, 2),
            generation: 1,
            radius_tenths: 0,
            root: (100, 50),
        };
        let mut cache = BackdropCache::default();
        cache.get_or_insert_with(key, || vec![1]);
        cache.get_or_insert_with(key, || panic!("must hit"));
        assert_eq!((cache.hits, cache.misses), (1, 1));
        for changed in [
            BackdropKey {
                rect: (2, 2, 4, 2),
                ..key
            },
            BackdropKey {
                generation: 2,
                ..key
            },
            BackdropKey {
                radius_tenths: 240,
                ..key
            },
            BackdropKey {
                root: (200, 50),
                ..key
            },
        ] {
            cache.get_or_insert_with(changed, || vec![2]);
        }
        assert_eq!(cache.misses, 5);
    }

    #[test]
    fn cache_is_bounded() {
        let mut cache = BackdropCache::default();
        for i in 0..(CACHE_ENTRIES as i32 * 2) {
            let key = BackdropKey {
                rect: (i, 0, 1, 1),
                generation: 0,
                radius_tenths: 0,
                root: (1, 1),
            };
            cache.get_or_insert_with(key, || vec![0; 4]);
        }
        assert_eq!(cache.len(), CACHE_ENTRIES);
    }

    #[test]
    fn cache_evicts_oldest_past_byte_budget_and_keeps_newest() {
        let key = |i: i32| BackdropKey {
            rect: (i, 0, 1, 1),
            generation: 0,
            radius_tenths: 0,
            root: (1, 1),
        };
        let mut cache = BackdropCache::default();
        for i in 0..3 {
            cache.get_or_insert_with(key(i), || vec![0; 1_500_000]);
        }
        assert_eq!(
            cache.len(),
            2,
            "third 1.5 MB entry exceeds the 4 MiB budget"
        );
        cache.get_or_insert_with(key(2), || panic!("newest must be kept"));
        cache.get_or_insert_with(key(0), || vec![0; 4]);
        assert_eq!(cache.misses, 4, "oldest was evicted");

        let mut oversize = BackdropCache::default();
        oversize.get_or_insert_with(key(0), || vec![0; CACHE_BYTE_BUDGET + 1]);
        assert_eq!(
            oversize.len(),
            1,
            "newest entry is kept even when alone over budget"
        );
    }

    #[test]
    fn cache_drops_entries_of_previous_root_size() {
        let key = |root| BackdropKey {
            rect: (0, 0, 1, 1),
            generation: 0,
            radius_tenths: 0,
            root,
        };
        let mut cache = BackdropCache::default();
        cache.get_or_insert_with(key((100, 50)), || vec![0; 4]);
        cache.get_or_insert_with(key((100, 50)), || panic!("same root must hit"));
        cache.get_or_insert_with(
            BackdropKey {
                rect: (1, 0, 1, 1),
                ..key((100, 50))
            },
            || vec![0; 4],
        );
        cache.get_or_insert_with(key((200, 80)), || vec![0; 4]);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.entries[0].0.root, (200, 80));
    }

    #[test]
    fn flat_fallback_without_wallpaper_is_opaque_and_never_errors() {
        let px = flat_pixels(3, 2, (0, 0, 0), 0);
        assert_eq!(px.len(), 3 * 2 * 4);
        assert_eq!(&px[..4], &[0x1b, 0x1e, 0x20, 255]);
    }

    #[test]
    fn bgrx_converts_to_opaque_rgba() {
        assert_eq!(bgrx_to_rgba(vec![3, 2, 1, 0]), vec![1, 2, 3, 255]);
        // Trailing partial pixel is dropped, matching the old chunks_exact copy.
        assert_eq!(bgrx_to_rgba(vec![3, 2, 1, 0, 9, 9]), vec![1, 2, 3, 255]);
    }

    #[test]
    fn wallpaper_base_moves_matching_canvas_and_keeps_failed_decode() {
        let image = RgbaImage {
            width: 2,
            height: 1,
            pixels: vec![1, 2, 3, 0, 4, 5, 6, 0],
        };
        let mut canvas = Some(((2, 1), Some(image)));
        let base = wallpaper_base(Some(&WallpaperSource::Solid([0; 3])), &mut canvas, (2, 1));
        assert_eq!(base, vec![1, 2, 3, 255, 4, 5, 6, 255]);
        assert!(canvas.is_none(), "canvas moved out, rebuilt lazily");

        let image_source = WallpaperSource::Image {
            path: String::new(),
            fit: WallpaperFit::Fill,
        };
        let mut failed = Some(((2, 1), None));
        let base = wallpaper_base(Some(&image_source), &mut failed, (2, 1));
        assert_eq!(base, [0x1b, 0x1e, 0x20, 255].repeat(2));
        assert_eq!(failed, Some(((2, 1), None)), "failed decode stays cached");
    }

    #[test]
    fn fallback_backdrop_blur_stays_flat() {
        let mut canvas = None;
        let px = build_backdrop(None, &mut canvas, (0, 0, 4, 2), (8, 8), 3.0);
        assert_eq!(
            px,
            [
                FALLBACK_BACKDROP[0],
                FALLBACK_BACKDROP[1],
                FALLBACK_BACKDROP[2],
                255
            ]
            .repeat(8)
        );
    }

    #[test]
    fn needs_drag_backdrop_only_when_titlebar_is_translucent() {
        assert!(!needs_drag_backdrop(255));
        assert!(needs_drag_backdrop(254));
        assert!(needs_drag_backdrop(0));
    }

    #[test]
    fn opaque_fill_is_flat_and_never_allocates_canvas() {
        let mut state = missing_image_state();
        let px = titlebar_fill_in(&mut state, (0, 0), (64, 64), (4, 2), (10, 20, 30), 255, 0.0);
        assert_eq!(px, flat_pixels(4, 2, (10, 20, 30), 255));
        assert!(
            state.canvas.is_none(),
            "opaque titlebar must not compose the root canvas"
        );
        assert_eq!(
            state.cache.misses, 0,
            "opaque titlebar must not build a backdrop"
        );
    }

    #[test]
    fn translucent_fill_matches_reference_pipeline() {
        let rgb = (0, 0, 0);
        for alpha in [254u8, 0] {
            let mut solid = State {
                source: Some(WallpaperSource::Solid([100, 100, 100])),
                ..State::default()
            };
            let px = titlebar_fill_in(&mut solid, (0, 0), (64, 64), (4, 2), rgb, alpha, 0.0);
            let expected = compose_rgba(&[100, 100, 100, 255].repeat(8), rgb, alpha);
            assert_eq!(px, expected, "solid source, alpha {alpha}");

            let mut missing = missing_image_state();
            let px = titlebar_fill_in(&mut missing, (0, 0), (64, 64), (4, 2), rgb, alpha, 0.0);
            let fallback = FALLBACK_BACKDROP;
            let expected = compose_rgba(
                &[fallback[0], fallback[1], fallback[2], 255].repeat(8),
                rgb,
                alpha,
            );
            assert_eq!(px, expected, "undecodable image, alpha {alpha}");
        }
    }

    #[test]
    fn end_drag_keeps_decoded_canvas_for_next_drag() {
        let mut state = State::default();
        state.drag = build_drag(&mut state, 3, (1, 1), Some(vec![0; 4]), (0, 0, 0, 0));
        state.canvas = Some(((1, 1), None));
        end_drag_in(&mut state, 3);
        assert!(
            state.canvas.is_some(),
            "a drag must not re-decode the wallpaper"
        );
    }

    #[test]
    fn drag_snapshot_uses_capture_outside_mask_and_wallpaper_inside() {
        let mut state = State {
            source: Some(WallpaperSource::Solid([9, 9, 9])),
            ..State::default()
        };
        let captured: Vec<u8> = (0..4 * 4).flat_map(|i| [i as u8, 0, 0, 255]).collect();
        // Mask the 2x2 block at (1,1) of a 4x4 root; it must read as wallpaper colour.
        let drag = build_drag(&mut state, 7, (4, 4), Some(captured.clone()), (1, 1, 2, 2)).unwrap();
        assert_eq!(drag.snapshot.crop(0, 0, 1, 1), captured[0..4].to_vec());
        assert_eq!(drag.snapshot.crop(1, 1, 1, 1), vec![9, 9, 9, 255]);
        assert_eq!(drag.snapshot.crop(2, 2, 1, 1), vec![9, 9, 9, 255]);
        assert_eq!(drag.snapshot.crop(3, 3, 1, 1), captured[60..64].to_vec());
    }

    #[test]
    fn drag_mask_band_matches_full_frame_mask_for_titlebar_reads() {
        // A titlebar blur reads only the band; a full-frame mask must give identical titlebars.
        let root = (1280u32, 900u32);
        let captured: Vec<u8> = (0..root.0 * root.1)
            .flat_map(|i| {
                [(i * 7 % 251) as u8, (i * 13 % 239) as u8, (i * 3 % 233) as u8, 255]
            })
            .collect();
        let frame = (100i32, 200i32, 1152u32, 726u32);
        let titlebar = |mask: (i32, i32, u32, u32), h: u32, blur: u8, origin: (i32, i32)| {
            let mut state = State {
                source: Some(WallpaperSource::Solid([10, 20, 30])),
                ..State::default()
            };
            state.drag = build_drag(&mut state, 1, root, Some(captured.clone()), mask);
            titlebar_in(&mut state, 1, origin, (frame.2, h), (0, 0, 0), 40, blur)
        };
        let origins = [
            (frame.0, frame.1),
            (frame.0 + 3, frame.1 + 17),
            (frame.0 - 5, frame.1 + 31),
        ];
        for h in [20u32, 40, 72] {
            for blur in [30u8, 100] {
                for origin in origins {
                    let full = titlebar(frame, h, blur, origin);
                    let band = titlebar(band_mask(frame), h, blur, origin);
                    assert_eq!(band, full, "h {h} blur {blur} origin {origin:?}");
                }
            }
        }
        assert_eq!(band_mask(frame).3, DRAG_MASK_BAND_Y);
    }

    #[test]
    fn drag_blur_keeps_uniform_colour_and_zero_radius_is_plain_crop() {
        let mut state = State {
            source: Some(WallpaperSource::Solid([0, 0, 0])),
            ..State::default()
        };
        let uniform: Vec<u8> = [40, 80, 120, 255].repeat(8 * 8);
        let drag = build_drag(&mut state, 1, (8, 8), Some(uniform), (0, 0, 0, 0)).unwrap();
        let blurred = blur_crop(&mut BlurScratch::new(), &drag.snapshot, (2, 2), 3, 3, 2.0);
        assert!(blurred.chunks_exact(4).all(|px| px == [40, 80, 120, 255]));
        assert_eq!(
            blur_crop(&mut BlurScratch::new(), &drag.snapshot, (2, 2), 3, 3, 0.0),
            drag.snapshot.crop(2, 2, 3, 3)
        );
    }

    #[test]
    fn drag_moves_match_fresh_blur_and_reuse_cell_region() {
        let captured: Vec<u8> = (0..96 * 80u32)
            .flat_map(|i| {
                [
                    (i * 37 % 251) as u8,
                    (i * 91 % 253) as u8,
                    (i * 13 % 247) as u8,
                    255,
                ]
            })
            .collect();
        let mut state = State {
            source: Some(WallpaperSource::Solid([0; 3])),
            ..State::default()
        };
        state.drag = build_drag(&mut state, 5, (96, 80), Some(captured), (0, 0, 0, 0));
        let snapshot = state.drag.as_ref().unwrap().snapshot.clone();
        let (w, h, rgb, alpha) = (20u32, 12u32, (10u8, 20u8, 30u8), 60u8);
        let mut saw_large_radius = false;
        for blur_percent in [30u8, 100] {
            let radius = blur_radius_px(opacity_percent(alpha), blur_percent);
            saw_large_radius |= radius > DRAG_SLACK_Y as f32 / 2.0;
            for origin in [
                (3, 3),
                (10, 3),
                (20, 3),
                (31, 3),
                (33, 3),
                (40, 20),
                (63, 40),
                (64, 41),
                (70, 50),
                (-5, -7),
                (70, 66),
            ] {
                let got =
                    titlebar_in(&mut state, 5, origin, (w, h), rgb, alpha, blur_percent).unwrap();
                let fresh = blur_crop(&mut BlurScratch::new(), &snapshot, origin, w, h, radius);
                assert_eq!(
                    got,
                    compose_rgba(&fresh, rgb, alpha),
                    "origin {origin:?} blur {blur_percent}"
                );
            }
        }
        assert!(saw_large_radius, "exercise the downscaled blur path too");

        // Moves inside one cell reuse the cached region; crossing the cell edge re-blurs.
        titlebar_in(&mut state, 5, (2, 2), (w, h), rgb, alpha, 30).unwrap();
        let before = state.drag.as_ref().unwrap().blurred.clone();
        titlebar_in(&mut state, 5, (9, 17), (w, h), rgb, alpha, 30).unwrap();
        assert_eq!(
            state
                .drag
                .as_ref()
                .unwrap()
                .blurred
                .as_ref()
                .map(|b| &b.region),
            before.as_ref().map(|b| &b.region)
        );
        // Horizontal cells are DRAG_SLACK_X wide: x=33 is still the first cell.
        titlebar_in(&mut state, 5, (33, 17), (w, h), rgb, alpha, 30).unwrap();
        assert_eq!(
            state.drag.as_ref().unwrap().blurred.as_ref().unwrap().cell,
            (0, 0)
        );
        titlebar_in(&mut state, 5, (DRAG_SLACK_X as i32 + 1, 17), (w, h), rgb, alpha, 30).unwrap();
        assert_eq!(
            state.drag.as_ref().unwrap().blurred.as_ref().unwrap().cell,
            (DRAG_SLACK_X as i32, 0)
        );
    }

    #[test]
    fn released_resize_serves_kept_blur_without_wallpaper_fallback() {
        let mut state = State {
            source: Some(WallpaperSource::Solid([9, 9, 9])),
            ..State::default()
        };
        state.drag = build_drag(
            &mut state,
            3,
            (8, 8),
            Some([200, 0, 0, 255].repeat(8 * 8)),
            (0, 0, 0, 0),
        );
        titlebar_in(&mut state, 3, (2, 2), (3, 2), (0, 0, 0), 0, 0).unwrap();
        end_drag_in(&mut state, 3);
        // Size changed at the same origin: kept blur served, kept crop survives.
        let resized = titlebar_in(&mut state, 3, (2, 2), (5, 2), (0, 0, 0), 0, 0).unwrap();
        assert_eq!(resized.len(), 5 * 2 * 4);
        assert!(state.retained.is_some(), "kept crop survives a resized paint");
        // Moved origin still falls back (wallpaper path).
        assert!(titlebar_in(&mut state, 3, (5, 2), (3, 2), (0, 0, 0), 0, 0).is_none());
    }

    #[test]
    fn frozen_resize_serves_the_drag_blur_without_reblurring() {
        let captured: Vec<u8> = (0..96 * 80u32)
            .flat_map(|i| [(i * 37 % 251) as u8, (i * 91 % 253) as u8, (i * 13 % 247) as u8, 255])
            .collect();
        let mut state = State {
            source: Some(WallpaperSource::Solid([0; 3])),
            ..State::default()
        };
        state.drag = build_drag(&mut state, 5, (96, 80), Some(captured), (0, 0, 0, 0));
        let (rgb, alpha) = ((10u8, 20u8, 30u8), 60u8);
        let origin = (10, 3);
        let first = titlebar_in(&mut state, 5, origin, (20, 12), rgb, alpha, 30).unwrap();
        set_drag_frozen_in(&mut state, 5, true);
        let blurred_before = state.drag.as_ref().unwrap().blurred.clone();

        // Growing and shrinking the frame during the resize never touches the blur.
        let grown = titlebar_in(&mut state, 5, origin, (30, 20), rgb, alpha, 30).unwrap();
        let shrunk = titlebar_in(&mut state, 5, origin, (8, 6), rgb, alpha, 30).unwrap();
        assert_eq!(grown.len(), 30 * 20 * 4);
        assert_eq!(shrunk.len(), 8 * 6 * 4);
        assert_eq!(state.drag.as_ref().unwrap().blurred, blurred_before);
        // The overlapping pixels are the same as before the resize started.
        assert_eq!(&grown[..20 * 4], &first[..20 * 4]);
        assert_eq!(&shrunk[..8 * 4], &first[..8 * 4]);

        // Thaw: the next paint at a new key blurs again for the size it paints.
        set_drag_frozen_in(&mut state, 5, false);
        titlebar_in(&mut state, 5, origin, (30, 20), rgb, alpha, 30).unwrap();
        assert_ne!(state.drag.as_ref().unwrap().blurred, blurred_before);
    }

    #[test]
    fn clamped_slice_repeats_region_edges_and_never_fails() {
        let blurred = Blurred {
            key: (0, 0, 0, 0, 0, 0),
            cell: (0, 0),
            // 2x2 region: pixel values 1..=4 in the red channel.
            region: vec![1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0],
            region_w: 2,
            region_h: 2,
        };
        // Origin left/up of the region clamps to its first pixel; beyond clamps to the last.
        let crop = slice_region_clamped(&blurred, (-1, -1), 3, 3);
        let reds: Vec<u8> = crop.chunks_exact(4).map(|px| px[0]).collect();
        assert_eq!(reds, vec![1, 1, 2, 1, 1, 2, 3, 3, 4]);
    }

    #[test]
    fn kept_crop_survives_release_and_drops_on_move_wallpaper_or_forget() {
        let mut state = State {
            source: Some(WallpaperSource::Solid([9, 9, 9])),
            ..State::default()
        };
        let captured: Vec<u8> = [200, 0, 0, 255].repeat(8 * 8);
        state.drag = build_drag(&mut state, 3, (8, 8), Some(captured), (0, 0, 0, 0));
        let live = titlebar_in(&mut state, 3, (2, 2), (3, 2), (0, 0, 0), 0, 0).unwrap();
        assert_eq!(&live[..4], &[200, 0, 0, 255]);
        end_drag_in(&mut state, 3);
        assert!(
            state.drag.is_none() && state.retained.is_some(),
            "snapshot dropped, crop kept"
        );
        assert_eq!(
            titlebar_in(&mut state, 3, (2, 2), (3, 2), (0, 0, 0), 0, 0),
            Some(live.clone())
        );
        assert_eq!(
            titlebar_in(&mut state, 4, (2, 2), (3, 2), (0, 0, 0), 0, 0),
            None,
            "other owner"
        );
        assert!(
            titlebar_in(&mut state, 3, (5, 2), (3, 2), (0, 0, 0), 0, 0).is_none(),
            "moved"
        );
        assert!(state.retained.is_none(), "moved frame drops the kept crop");
        state.drag = build_drag(
            &mut state,
            3,
            (8, 8),
            Some([200, 0, 0, 255].repeat(64)),
            (0, 0, 0, 0),
        );
        end_drag_in(&mut state, 3);
        state.generation += 1;
        assert!(
            titlebar_in(&mut state, 3, (2, 2), (3, 2), (0, 0, 0), 0, 0).is_none(),
            "wallpaper change"
        );
        state.drag = build_drag(
            &mut state,
            3,
            (8, 8),
            Some([200, 0, 0, 255].repeat(64)),
            (0, 0, 0, 0),
        );
        end_drag_in(&mut state, 3);
        forget_in(&mut state, 3);
        assert!(
            state.retained.is_none(),
            "closed window drops the kept crop"
        );
    }

    #[test]
    fn drag_snapshot_falls_back_to_wallpaper_without_capture() {
        let mut state = State {
            source: Some(WallpaperSource::Solid([5, 6, 7])),
            ..State::default()
        };
        let drag = build_drag(&mut state, 1, (2, 2), None, (0, 0, 1, 1)).unwrap();
        assert_eq!(drag.snapshot.crop(1, 1, 1, 1), vec![5, 6, 7, 255]);
    }

    #[test]
    fn drag_snapshot_rejects_wrong_capture_length_and_uses_wallpaper() {
        let mut state = State {
            source: Some(WallpaperSource::Solid([5, 6, 7])),
            ..State::default()
        };
        let drag = build_drag(&mut state, 1, (2, 2), Some(vec![0; 4]), (0, 0, 0, 0)).unwrap();
        assert_eq!(drag.snapshot.crop(1, 1, 1, 1), vec![5, 6, 7, 255]);
    }

    #[test]
    fn canvas_kept_across_misses_released_by_hook_and_rebuilt_lazily() {
        let mut state = State {
            source: Some(WallpaperSource::Image {
                path: "/nonexistent/flamewm-test.png".into(),
                fit: WallpaperFit::Fill,
            }),
            ..State::default()
        };
        let root = (16, 8);
        let fill = |state: &mut State, origin| {
            titlebar_fill_in(state, origin, root, (4, 2), (0, 0, 0), 0, 0.0)
        };
        fill(&mut state, (0, 0));
        assert!(state.canvas.is_some(), "kept after first miss");
        assert_eq!(state.cache.misses, 1);
        fill(&mut state, (1, 0));
        assert!(state.canvas.is_some(), "kept across consecutive miss");
        assert_eq!(state.cache.misses, 2);
        fill(&mut state, (1, 0));
        assert!(state.canvas.is_none(), "hit with no drag drops canvas");
        assert_eq!((state.cache.hits, state.cache.misses), (1, 2));
        fill(&mut state, (2, 0));
        assert!(state.canvas.is_some(), "rebuilt lazily on miss");
        release_canvas_in(&mut state);
        assert!(state.canvas.is_none(), "released by hook");
        fill(&mut state, (3, 0));
        assert!(
            state.canvas.is_some(),
            "rebuilt lazily after hook release on miss"
        );
        assert_eq!(state.cache.misses, 4);
    }

    #[test]
    fn canvas_bytes_tracked_and_released_after_build_when_hit_needs_none() {
        let (mut state, path) = ppm_state("bytes");
        let root = (16, 8);
        let fill = |state: &mut State, origin| {
            titlebar_fill_in(state, origin, root, (4, 2), (0, 0, 0), 0, 0.0)
        };
        fill(&mut state, (0, 0));
        let canvas = root.0 as usize * root.1 as usize * 4;
        assert_eq!(canvas_bytes(&state), canvas, "miss builds the root canvas");
        assert_eq!(
            state.cache.bytes(),
            4 * 2 * 4,
            "one titlebar backdrop cached"
        );
        fill(&mut state, (0, 0));
        assert_eq!(canvas_bytes(&state), 0, "hit releases resident canvas");
        assert_eq!(state.cache.bytes(), 4 * 2 * 4, "cache untouched by release");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn canvas_retained_through_drag_and_rebuilt_on_miss() {
        let (mut state, path) = ppm_state("drag-keep");
        let root = (16, 8);
        state.drag = build_drag(&mut state, 9, root, None, (0, 0, 2, 2));
        assert!(state.drag.is_some());
        let fill = |state: &mut State, origin| {
            titlebar_fill_in(state, origin, root, (4, 2), (0, 0, 0), 0, 0.0)
        };
        fill(&mut state, (0, 0));
        assert!(state.canvas.is_some(), "drag keeps canvas for the miss");
        fill(&mut state, (0, 0));
        assert_eq!(
            canvas_bytes(&state),
            16 * 8 * 4,
            "hit during drag keeps canvas resident"
        );
        fill(&mut state, (5, 0));
        assert_eq!((state.cache.hits, state.cache.misses), (1, 2));
        assert_eq!(
            canvas_bytes(&state),
            16 * 8 * 4,
            "miss during drag rebuilds without re-decode loss"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn solid_source_blends_over_wallpaper_colour_and_tracks_generation() {
        let g1 = set_wallpaper_source(WallpaperSource::Solid([100, 100, 100]));
        assert_eq!(
            set_wallpaper_source(WallpaperSource::Solid([100, 100, 100])),
            g1
        );
        let px = titlebar_pixels((-10, -5), (64, 64), 4, 2, (0, 0, 0), 255, 80);
        assert_eq!(&px[..4], &[0, 0, 0, 255]);
        let half = titlebar_pixels((0, 0), (64, 64), 4, 2, (0, 0, 0), 0, 80);
        assert_eq!(&half[..4], &[100, 100, 100, 255]);
        assert_ne!(set_wallpaper_source(WallpaperSource::Solid([1, 2, 3])), g1);
    }

    const ALL_FITS: [WallpaperFit; 5] = [
        WallpaperFit::Fill,
        WallpaperFit::Fit,
        WallpaperFit::Stretch,
        WallpaperFit::Center,
        WallpaperFit::Tile,
    ];

    /// Opaque, non-uniform 7x5 image so every channel and sampling path is exercised.
    fn synthetic_natural() -> RgbaImage {
        let pixels = (0..7u32 * 5)
            .flat_map(|i| {
                [
                    (i * 37 % 251) as u8,
                    (i * 91 % 253) as u8,
                    (i * 13 % 247) as u8,
                    255,
                ]
            })
            .collect();
        RgbaImage {
            width: 7,
            height: 5,
            pixels,
        }
    }

    const MASKS: [(i32, i32, u32, u32); 7] = [
        (0, 0, 16, 9),
        (-3, -2, 6, 5),
        (14, 8, 4, 4),
        (-10, -10, 3, 2),
        (2, 1, 5, 3),
        (1, 0, 16, 1),
        (0, 0, 1, 1),
    ];

    #[test]
    fn masked_from_natural_matches_root_canvas_pipeline() {
        let natural = synthetic_natural();
        for root in [(16, 9), (3, 11), (1, 1)] {
            for fit in ALL_FITS {
                let canvas_image = wallpaper::compose(&natural, root.0, root.1, fit).unwrap();
                let source = WallpaperSource::Image {
                    path: "unused".to_owned(),
                    fit,
                };
                for mask in MASKS {
                    let mut canvas = Some((root, Some(canvas_image.clone())));
                    let old = wallpaper_rect(Some(&source), &mut canvas, mask, root);
                    let new = masked_from_natural(&natural, fit, mask, root);
                    assert_eq!(new, Some(old), "root {root:?} {fit:?} mask {mask:?}");
                }
            }
        }
    }

    #[test]
    fn mask_pixels_from_file_matches_canvas_pipeline_and_builds_no_canvas() {
        let natural = synthetic_natural();
        let path = std::env::temp_dir().join(format!("flamewm-mask-{}.ppm", std::process::id()));
        let mut bytes = b"P6\n7 5\n255\n".to_vec();
        bytes.extend(
            natural
                .pixels
                .chunks_exact(4)
                .flat_map(|px| [px[0], px[1], px[2]]),
        );
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            wallpaper::decode_file(&path).is_ok(),
            "synthetic PPM must decode"
        );
        let file = path.to_string_lossy().into_owned();
        for root in [(16, 9), (3, 11), (1, 1)] {
            for fit in ALL_FITS {
                let source = WallpaperSource::Image {
                    path: file.clone(),
                    fit,
                };
                for mask in MASKS {
                    let mut old_canvas = None;
                    let old = wallpaper_rect(Some(&source), &mut old_canvas, mask, root);
                    let mut new_canvas = None;
                    let mut natural = None;
                    let new =
                        mask_pixels(Some(&source), &mut new_canvas, &mut natural, mask, root, true);
                    assert_eq!(new, old, "root {root:?} {fit:?} mask {mask:?}");
                    assert!(
                        new_canvas.is_none(),
                        "captured drag must not build the root canvas"
                    );
                }
            }
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn drag_with_capture_keeps_no_root_canvas() {
        let (mut state, path) = ppm_state("drag");
        let captured = [1u8, 2, 3, 255].repeat(16 * 9);
        state.drag = build_drag(&mut state, 9, (16, 9), Some(captured), (-2, 3, 5, 4));
        assert!(state.drag.is_some());
        assert!(
            state.canvas.is_none(),
            "captured drag composes only the mask"
        );
        std::fs::remove_file(&path).ok();
    }
}

#[cfg(test)]
mod drag_cell_tests {
    use super::*;

    #[test]
    fn sideways_drag_stays_in_one_blur_cell() {
        // A 300 px horizontal move from x=100 keeps one cell: no blur recompute per step.
        let cell = drag_cell((100, 40));
        for x in (100..=400).step_by(10) {
            assert_eq!(drag_cell((x, 40)), cell, "x={x}");
        }
        assert_eq!(cell, (0, 32));
    }

    #[test]
    fn cell_changes_past_horizontal_slack_and_vertical_slack() {
        assert_ne!(drag_cell((511, 0)), drag_cell((512, 0)));
        assert_ne!(drag_cell((0, 31)), drag_cell((0, 32)));
    }

    #[test]
    fn negative_origins_map_to_floor_cells() {
        assert_eq!(drag_cell((-1, -1)), (-512, -32));
        assert_eq!(drag_cell((-512, -32)), (-512, -32));
    }
}

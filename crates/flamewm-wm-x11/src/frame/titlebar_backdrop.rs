//! Titlebar surface material: `surfaceColor` at `surfaceOpacity` over the desktop layer behind the
//! frame, blurred per `surfaceBlurPercent`.
//!
//! There is no compositor, so the WM bakes `blend(color @ opacity, desktop layer)` into the frame's
//! opaque titlebar pixels. The desktop layer is the same blurred wallpaper canvas every translucent
//! `FlameWM` surface blends over (`flamewm_render_x11::backdrop::desktop_canvas`, shared on disk with
//! the panel and app windows). The root is never read: a root read holds other windows as they
//! were at that instant and baked them into the titlebar as stale ghosts and black marks. A paint
//! at any origin, including every drag step, is one crop of the canvas.
//!
//! Memory: one canvas at 1/16 of the root (about half a megabyte at 1080p) for blurs of at least
//! 8px; nothing per window.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use flamewm_api::background::WallpaperFit;
use flamewm_image_core::wallpaper::DEFAULT_WALLPAPER_PATH;
use flamewm_render_x11::backdrop::{BlurredCanvas, blur_radius_px, desktop_canvas};
use flamewm_ui_core::surface::saturate_rgb;

/// Backdrop used when no wallpaper pixels are available (frame background colour).
pub const FALLBACK_BACKDROP: [u8; 3] = [0x1b, 0x1e, 0x20];

/// Wallpaper the desktop owner paints, as named by the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WallpaperSource {
    Solid([u8; 3]),
    /// Empty `path` is the bundled default wallpaper. `outputs` (root `x, y, w, h`) lays the
    /// wallpaper out per output as the desktop does; empty is one wallpaper over the whole root.
    Image {
        path: String,
        fit: WallpaperFit,
        outputs: Vec<(i32, i32, u32, u32)>,
    },
}

/// What a built canvas was made for; any change rebuilds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CanvasKey {
    generation: u32,
    root: (u32, u32),
    blur_percent: u8,
}

#[derive(Default)]
struct State {
    source: Option<WallpaperSource>,
    generation: u32,
    /// Desktop canvas for its key; `None` image means the wallpaper could not be composed.
    canvas: Option<(CanvasKey, Option<BlurredCanvas>)>,
    /// Output rects (root `x, y, w, h`) published by the WM.
    outputs: Vec<(i32, i32, u32, u32)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Layout the wallpaper is composed for: one copy per WM output, as the desktop draws it, or one
/// wallpaper over the root when `shareWallpaper` is on (read from the live settings sync).
fn effective_layout(state: &State) -> Vec<(i32, i32, u32, u32)> {
    if crate::live_settings::share_wallpaper() {
        Vec::new()
    } else {
        state.outputs.clone()
    }
}

/// The layout titlebars compose the wallpaper for (root `x, y, w, h` per output); empty is one
/// wallpaper over the root.
#[must_use]
pub fn wallpaper_layout() -> Vec<(i32, i32, u32, u32)> {
    STATE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(effective_layout))
        .unwrap_or_default()
}

/// Publish the WM output rects (root coordinates). The canvas is rebuilt only when the layout
/// changes.
pub fn set_wallpaper_outputs(outputs: &[(i32, i32, u32, u32)]) {
    let Ok(mut guard) = STATE.lock() else {
        return;
    };
    let state = guard.get_or_insert_with(State::default);
    if state.outputs == outputs {
        return;
    }
    state.outputs = outputs.to_vec();
    let layout = effective_layout(state);
    if let Some(WallpaperSource::Image {
        outputs: current, ..
    }) = state.source.as_mut()
    {
        if *current != layout {
            *current = layout;
            state.generation = state.generation.wrapping_add(1);
            state.canvas = None;
        }
    }
}

/// Publish the wallpaper source; returns the backdrop generation (bumped on change).
pub fn set_wallpaper_source(source: WallpaperSource) -> u32 {
    let Ok(mut guard) = STATE.lock() else {
        return 0;
    };
    set_source_in(guard.get_or_insert_with(State::default), source)
}

fn set_source_in(state: &mut State, mut source: WallpaperSource) -> u32 {
    if let WallpaperSource::Image { outputs, .. } = &mut source {
        *outputs = effective_layout(state);
    }
    if state.source.as_ref() != Some(&source) {
        state.source = Some(source);
        state.generation = state.generation.wrapping_add(1);
        state.canvas = None;
    }
    state.generation
}

/// One channel of `src @ alpha` over `dst`, rounded.
#[must_use]
pub fn blend_over(src: u8, alpha: u8, dst: u8) -> u8 {
    let a = u32::from(alpha);
    ((u32::from(src) * a + u32::from(dst) * (255 - a) + 127) / 255) as u8
}

/// Scales each opaque ARGB32 word's colour saturation by `percent` (100 is identity). The per-pixel
/// op commutes with the blur that built the canvas, so saturating the crop matches saturating it.
fn saturate_words(words: &mut [u32], percent: u32) {
    if percent == 100 {
        return;
    }
    for word in words.iter_mut() {
        *word = (*word & 0xff00_0000) | saturate_rgb(*word & 0x00ff_ffff, percent);
    }
}

/// Opaque RGBA of `rgb @ alpha` over each opaque ARGB32 word of `backdrop`.
fn compose_words(backdrop: &[u32], rgb: (u8, u8, u8), alpha: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(backdrop.len() * 4);
    for &word in backdrop {
        out.extend_from_slice(&[
            blend_over(rgb.0, alpha, (word >> 16) as u8),
            blend_over(rgb.1, alpha, (word >> 8) as u8),
            blend_over(rgb.2, alpha, word as u8),
            255,
        ]);
    }
    out
}

/// `width x height` opaque RGBA of `rgb @ alpha` over one flat colour.
fn compose_flat(
    backdrop: [u8; 3],
    width: u32,
    height: u32,
    rgb: (u8, u8, u8),
    alpha: u8,
) -> Vec<u8> {
    [
        blend_over(rgb.0, alpha, backdrop[0]),
        blend_over(rgb.1, alpha, backdrop[1]),
        blend_over(rgb.2, alpha, backdrop[2]),
        255,
    ]
    .repeat(width as usize * height as usize)
}

/// Opacity percent from the 8-bit token (`percent * 255 / 100` inverse).
#[must_use]
pub fn opacity_percent(alpha: u8) -> u8 {
    ((u32::from(alpha) * 100 + 127) / 255) as u8
}

/// Whether the titlebar blends over the desktop layer at all. An opaque titlebar (255) is its flat
/// colour whatever lies behind it, so no canvas is built.
#[must_use]
pub fn is_translucent(alpha: u8) -> bool {
    alpha < 255
}

/// Flat opaque titlebar over [`FALLBACK_BACKDROP`] (no geometry / no wallpaper).
#[must_use]
pub fn flat_pixels(width: u32, height: u32, rgb: (u8, u8, u8), alpha: u8) -> Vec<u8> {
    compose_flat(FALLBACK_BACKDROP, width, height, rgb, alpha)
}

/// Opaque RGBA titlebar pixels for the frame at root `origin`, `width` x `height`.
/// `corner_radius` is the frame's top corner radius (0 for square frames).
#[must_use]
pub fn titlebar_pixels(
    origin: (i32, i32),
    root: (u32, u32),
    width: u32,
    height: u32,
    rgb: (u8, u8, u8),
    alpha: u8,
    blur_percent: u8,
    corner_radius: u32,
) -> Vec<u8> {
    let Ok(mut guard) = STATE.lock() else {
        return flat_pixels(width, height, rgb, alpha);
    };
    titlebar_in(
        guard.get_or_insert_with(State::default),
        origin,
        root,
        (width, height),
        rgb,
        alpha,
        blur_percent,
        corner_radius,
    )
}

/// Coverage (0..=255) of pixel `(x, y)` inside a top corner arc of radius `radius`, from a 4x4
/// supersample. `None` outside the corner squares. The arc is mirrored for the right corner.
fn corner_coverage(x: u32, y: u32, width: u32, radius: u32) -> Option<u8> {
    let col = if x < radius {
        x
    } else if x + radius >= width {
        width - 1 - x
    } else {
        return None;
    };
    if y >= radius {
        return None;
    }
    let r = radius as f32;
    let mut hits = 0_u32;
    for sy in 0..4 {
        for sx in 0..4 {
            let px = col as f32 + (sx as f32 + 0.5) / 4.0;
            let py = y as f32 + (sy as f32 + 0.5) / 4.0;
            if (r - px).powi(2) + (r - py).powi(2) <= r * r {
                hits += 1;
            }
        }
    }
    Some((hits * 255 / 16) as u8)
}

/// Blends the binary XShape edge of the top corners: each partly covered pixel is mixed with the
/// desktop word behind it, so the arc reads as anti-aliased instead of stair-stepped. Fully
/// covered pixels and the straight edges are untouched.
fn antialias_top_corners(pixels: &mut [u8], backdrop: &[u32], width: u32, height: u32, radius: u32) {
    let radius = radius.min(width / 2);
    for y in 0..height.min(radius) {
        for x in (0..radius).chain(width - radius..width) {
            let Some(cover) = corner_coverage(x, y, width, radius) else {
                continue;
            };
            if cover == 255 {
                continue;
            }
            let index = (y * width + x) as usize;
            let word = backdrop[index];
            let at = index * 4;
            let behind = [(word >> 16) as u8, (word >> 8) as u8, word as u8];
            for (channel, under) in behind.into_iter().enumerate() {
                pixels[at + channel] = blend_over(pixels[at + channel], cover, under);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn titlebar_in(
    state: &mut State,
    origin: (i32, i32),
    root: (u32, u32),
    (width, height): (u32, u32),
    rgb: (u8, u8, u8),
    alpha: u8,
    blur_percent: u8,
    corner_radius: u32,
) -> Vec<u8> {
    if !is_translucent(alpha) {
        return flat_pixels(width, height, rgb, alpha);
    }
    let image = match state.source.as_ref() {
        Some(WallpaperSource::Solid(color)) => {
            return compose_flat(*color, width, height, rgb, alpha);
        }
        Some(WallpaperSource::Image { path, fit, outputs }) => (path, *fit, outputs),
        None => return flat_pixels(width, height, rgb, alpha),
    };
    let blur_percent = if blur_radius_px(opacity_percent(alpha), blur_percent) > 0.0 {
        blur_percent
    } else {
        0
    };
    let key = CanvasKey {
        generation: state.generation,
        root,
        blur_percent,
    };
    if state.canvas.as_ref().is_none_or(|(built, _)| *built != key) {
        let (path, fit, outputs) = image;
        let _guard = flamewm_profiler::start("wm.chrome.backdrop.canvas");
        let canvas = desktop_canvas(&wallpaper_file(path), fit, outputs, root, blur_percent)
            .map_err(|error| {
                eprintln!("flamewm-wm-x11: titlebar desktop layer unavailable: {error}");
            })
            .ok();
        state.canvas = Some((key, canvas));
    }
    match state
        .canvas
        .as_ref()
        .and_then(|(_, canvas)| canvas.as_ref())
    {
        Some(canvas) => {
            let mut backdrop = canvas.crop(origin, (width, height));
            saturate_words(&mut backdrop, crate::live_settings::saturation_percent());
            let mut pixels = compose_words(&backdrop, rgb, alpha);
            antialias_top_corners(&mut pixels, &backdrop, width, height, corner_radius);
            pixels
        }
        None => flat_pixels(width, height, rgb, alpha),
    }
}

/// Wallpaper file for `path`; an empty path is the bundled default.
fn wallpaper_file(path: &str) -> PathBuf {
    if path.trim().is_empty() {
        packaged_root().join(DEFAULT_WALLPAPER_PATH)
    } else {
        PathBuf::from(path)
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

    #[test]
    fn blend_math_at_opacity_0_50_100_over_known_backdrop() {
        let backdrop = [0xffc8_6400u32];
        assert_eq!(
            compose_words(&backdrop, (0, 0, 0), 0),
            vec![200, 100, 0, 255]
        );
        assert_eq!(
            compose_words(&backdrop, (0, 0, 0), 127),
            vec![100, 50, 0, 255]
        );
        assert_eq!(
            compose_words(&backdrop, (10, 20, 30), 255),
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
    fn opaque_titlebar_is_flat_and_builds_no_canvas() {
        let mut state = State::default();
        set_source_in(&mut state, WallpaperSource::Solid([1, 2, 3]));
        let px = titlebar_in(&mut state, (0, 0), (8, 8), (4, 2), (9, 8, 7), 255, 80, 0);
        assert_eq!(px, [9, 8, 7, 255].repeat(8));
        assert!(state.canvas.is_none());
        assert!(!is_translucent(255) && is_translucent(254));
    }

    #[test]
    fn solid_wallpaper_blends_over_its_colour_and_tracks_generation() {
        let mut state = State::default();
        let first = set_source_in(&mut state, WallpaperSource::Solid([200, 100, 0]));
        assert_eq!(
            set_source_in(&mut state, WallpaperSource::Solid([200, 100, 0])),
            first
        );
        let px = titlebar_in(&mut state, (5, 5), (8, 8), (2, 1), (0, 0, 0), 127, 80, 0);
        assert_eq!(px, [100, 50, 0, 255].repeat(2));
        assert_ne!(
            set_source_in(&mut state, WallpaperSource::Solid([0, 0, 0])),
            first
        );
    }

    #[test]
    fn missing_wallpaper_falls_back_to_flat_and_never_errors() {
        let mut state = State::default();
        set_source_in(
            &mut state,
            WallpaperSource::Image {
                path: "/nonexistent/flamewm-test-wallpaper.png".to_owned(),
                fit: WallpaperFit::Fill,
                outputs: Vec::new(),
            },
        );
        let px = titlebar_in(&mut state, (0, 0), (16, 8), (3, 2), (0, 0, 0), 127, 80, 0);
        assert_eq!(px, flat_pixels(3, 2, (0, 0, 0), 127));
    }

    #[test]
    fn image_titlebar_is_a_crop_of_the_built_desktop_canvas() {
        let mut state = State::default();
        set_source_in(
            &mut state,
            WallpaperSource::Image {
                path: String::new(),
                fit: WallpaperFit::Fill,
                outputs: Vec::new(),
            },
        );
        let rgba = (0..64u32 * 32)
            .flat_map(|i| [(i % 64 * 4) as u8, (i / 64 * 8) as u8, 7, 255])
            .collect();
        let canvas = BlurredCanvas::new(64, 32, rgba, 0, 80);
        let mut crop = canvas.crop((10, 4), (20, 6));
        saturate_words(&mut crop, crate::live_settings::saturation_percent());
        let expected = compose_words(&crop, (0, 0, 0), 127);
        let key = CanvasKey {
            generation: state.generation,
            root: (64, 32),
            blur_percent: 80,
        };
        state.canvas = Some((key, Some(canvas)));
        let px = titlebar_in(&mut state, (10, 4), (64, 32), (20, 6), (0, 0, 0), 127, 80, 0);
        assert_eq!(px, expected);
        let moved = titlebar_in(&mut state, (30, 9), (64, 32), (20, 6), (0, 0, 0), 127, 80, 0);
        assert_ne!(
            moved, px,
            "a move recrops the same canvas at the new origin"
        );
        assert!(
            state
                .canvas
                .as_ref()
                .is_some_and(|(built, _)| *built == key)
        );
    }

    #[test]
    fn a_new_output_layout_rebuilds_the_canvas() {
        let mut state = State {
            outputs: vec![(0, 0, 32, 16)],
            ..State::default()
        };
        set_source_in(
            &mut state,
            WallpaperSource::Image {
                path: String::new(),
                fit: WallpaperFit::Fill,
                outputs: Vec::new(),
            },
        );
        assert!(matches!(
            state.source.as_ref(),
            Some(WallpaperSource::Image { outputs, .. }) if outputs == &[(0, 0, 32, 16)]
        ));
    }

    #[test]
    fn saturation_is_identity_at_100_and_desaturates_at_0() {
        let mut words = [0xff20_40c0_u32, 0xff80_8080];
        saturate_words(&mut words, 100);
        assert_eq!(words, [0xff20_40c0, 0xff80_8080]);
        saturate_words(&mut words, 0);
        assert_eq!(words, [0xff44_4444, 0xff80_8080]);
    }

    #[test]
    fn corner_coverage_is_zero_outside_the_arc_and_rises_inward() {
        let (w, r) = (200, 24);
        assert_eq!(corner_coverage(24, 0, w, r), None, "straight top edge");
        assert_eq!(corner_coverage(0, 0, w, r), Some(0), "outer corner pixel");
        assert_eq!(corner_coverage(w - 1, 0, w, r), Some(0), "mirrored right corner");
        let row: Vec<u8> = (0..r).map(|x| corner_coverage(x, 12, w, r).unwrap_or(255)).collect();
        assert!(row.windows(2).all(|p| p[0] <= p[1]), "coverage rises inward: {row:?}");
    }

    #[test]
    fn top_corner_edge_blends_toward_the_desktop_behind_it() {
        let (w, h, r) = (200_u32, 31_u32, 24_u32);
        let backdrop = vec![0x00ff_ffff_u32; (w * h) as usize];
        let mut pixels = vec![0_u8; (w * h * 4) as usize];
        antialias_top_corners(&mut pixels, &backdrop, w, h, r);
        let red = |x: u32, y: u32| pixels[((y * w + x) * 4) as usize];
        assert_eq!(red(0, 0), 255, "outside the arc shows the desktop");
        assert_eq!(red(100, 0), 0, "straight edge is untouched");
        let partial = (0..r).any(|y| (0..r).any(|x| red(x, y) > 0 && red(x, y) < 255));
        assert!(partial, "arc has partially covered pixels");
    }
}

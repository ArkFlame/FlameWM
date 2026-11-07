//! Snap-preview backdrop: root readback under the preview rect.
//! No compositor, no XSHM. Pure conversion + one X11-backed capture helper.
//! Flow (caller-gated on target/rect change): capture under rect via
//! XGetImage on the root, convert pixels with live server masks/byte order
//! (never hardcoded RGB order), retain as preview background, then draw the
//! desktop-selection material over it. Failure -> border-only preview.
//! No capture runs unless the caller reports a geometry change.

mod blur;
mod canvas;
mod capture;
mod decode;
mod desktop;
mod extend;

pub use self::blur::*;
pub use self::canvas::*;
pub use self::capture::*;
pub use self::decode::*;
pub use self::desktop::*;
pub use self::extend::*;

use crate::xlib::*;

/// Accent from the shared `flamewm-skin` selection contract (0xef4048).
pub const BACKDROP_ACCENT_RGB: (u8, u8, u8) = (0xef, 0x40, 0x48);

/// Border alpha family: strong accent edge over the captured backdrop.
pub const BACKDROP_BORDER_ALPHA: u8 = 0xdd;

/// Border width family in device px: matches the compiled snap-preview CSS.
pub const BACKDROP_BORDER_WIDTH_PX: u32 = 2;

/// Re-exported from the dependency-free core so every backend names the same type.
pub use flamewm_render_core::backdrop::WindowBackdrop;

/// Wallpaper file for a `wallpaper` setting value (empty means the packaged default).
#[must_use]
pub fn wallpaper_file(wallpaper: &str, packaged_root: &std::path::Path) -> std::path::PathBuf {
    if wallpaper.trim().is_empty() {
        packaged_root.join(flamewm_image_core::wallpaper::DEFAULT_WALLPAPER_PATH)
    } else {
        std::path::PathBuf::from(wallpaper)
    }
}

/// Whether the window snapshot must be recaptured now.
/// A move keeps the window size, so it recaptures at once. A size change (interactive resize)
/// keeps the stale snapshot out of the scene (paint skips a size mismatch) and waits until no
/// X events are queued, so only the settled size pays for a capture.
#[must_use]
pub fn recapture_due(
    snapshot_size: Option<(u32, u32)>,
    window_size: (u32, u32),
    events_queued: bool,
) -> bool {
    match snapshot_size {
        None => true,
        Some(size) if size == window_size => true,
        Some(_) => !events_queued,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recrop_matches_a_fresh_crop_for_every_shift() {
        let (width, height) = (96, 64);
        let rgba: Vec<u8> = (0..width * height * 4)
            .map(|i| ((i * 37) ^ (i >> 5)) as u8)
            .collect();
        for blur in [0, 100] {
            let canvas = BlurredCanvas::new(width, height, rgba.clone(), 40, blur);
            let size = (31, 23);
            let previous_origin = (10, -4);
            let previous = canvas.crop(previous_origin, size);
            for dx in [-40, -31, -7, -1, 0, 1, 5, 30, 31, 80] {
                for dy in [-30, -23, -3, 0, 2, 22, 23, 70] {
                    let origin = (previous_origin.0 + dx, previous_origin.1 + dy);
                    assert_eq!(
                        canvas.recrop(&previous, previous_origin, origin, size),
                        canvas.crop(origin, size),
                        "blur={blur} shift=({dx}, {dy})"
                    );
                }
            }
        }
    }

    #[test]
    fn wallpaper_output_words_parse_signed_rects_and_drop_empty_or_partial() {
        let left = (-1280_i32) as u32;
        assert_eq!(
            wallpaper_outputs_from_words(&[left, 0, 1280, 800, 0, 0, 1920, 1080, 5, 5, 0, 9, 1]),
            vec![(-1280, 0, 1280, 800), (0, 0, 1920, 1080)]
        );
        assert!(wallpaper_outputs_from_words(&[]).is_empty());
    }

    #[test]
    fn desktop_frame_words_parse_and_reject_malformed() {
        assert_eq!(
            desktop_frame_from_words(&[7, 0, (-3i32) as u32, 1280, 800, 9]),
            Some(DesktopFrame {
                pixmap: 7,
                x: 0,
                y: -3,
            })
        );
        assert_eq!(desktop_frame_from_words(&[0, 0, 0, 1280, 800, 9]), None);
        assert_eq!(desktop_frame_from_words(&[7, 0, 0, 1280, 800]), None);
    }

    #[test]
    fn full_opacity_and_zero_blur_never_blur() {
        assert_eq!(blur_radius_px(100, 80), 0.0);
        assert_eq!(blur_radius_px(80, 0), 0.0);
        assert!((blur_radius_px(80, 100) - 24.0).abs() < f32::EPSILON);
        assert!((blur_radius_px(80, 250) - 24.0).abs() < f32::EPSILON);
    }

    #[test]
    fn unblurred_words_are_opaque_argb_in_rgb_order() {
        let words = blurred_argb_words(
            2,
            1,
            vec![0x11, 0x22, 0x33, 0x00, 0x11, 0x22, 0x33, 0x00],
            100,
            80,
        );
        assert_eq!(words, vec![0xff11_2233, 0xff11_2233]);
    }

    #[test]
    fn blur_of_uniform_snapshot_keeps_colour() {
        let words = blurred_argb_words(8, 8, [0x40, 0x80, 0xc0, 0xff].repeat(64), 40, 100);
        assert!(words.iter().all(|&w| w == 0xff40_80c0));
    }

    #[test]
    fn downscaled_blur_keeps_output_size_and_softens_an_edge() {
        // 66 is not a multiple of the downscale factor, so the last blocks are partial.
        let (width, height) = (66u32, 3u32);
        let rgba: Vec<u8> = (0..width * height)
            .flat_map(|i| {
                let v = if i % width < 33 { 0u8 } else { 255 };
                [v, v, v, 0xff]
            })
            .collect();
        let words = blurred_argb_words(width, height, rgba, 40, 100);
        assert_eq!(words.len(), (width * height) as usize);
        let channel = |x: usize| (words[x] & 0xff) as u8;
        assert!(
            channel(0) < channel(16) && channel(16) < channel(33),
            "dark side ramps up"
        );
        assert!(
            channel(33) < channel(50) && channel(50) < channel(65),
            "light side ramps up"
        );
        assert!(
            (64..192).contains(&channel(33)),
            "edge is soft: {}",
            channel(33)
        );
    }

    /// A scaled canvas never stores the full image, yet every crop (inside, straddling or off the
    /// canvas) equals cropping the full upsampled image that `blurred_argb_words` returns.
    #[test]
    fn scaled_canvas_crops_match_the_full_upsampled_image() {
        let (width, height) = (61u32, 37u32);
        let rgba: Vec<u8> = (0..width * height)
            .flat_map(|i| {
                let (x, y) = (i % width, i / width);
                [
                    (x * 255 / width) as u8,
                    (y * 255 / height) as u8,
                    ((x * y) % 251) as u8,
                    0xff,
                ]
            })
            .collect();
        let full = blurred_argb_words(width, height, rgba.clone(), 0, 80);
        let canvas = BlurredCanvas::new(width, height, rgba, 0, 80);
        assert!(
            canvas.scaled_bytes().is_some(),
            "80% blur keeps only the small image"
        );
        let reference = |origin: (i32, i32), size: (u32, u32)| -> Vec<u32> {
            let mut out = Vec::new();
            for y in 0..size.1 as i32 {
                let sy = (origin.1 + y).clamp(0, height as i32 - 1) as u32;
                for x in 0..size.0 as i32 {
                    let sx = (origin.0 + x).clamp(0, width as i32 - 1) as u32;
                    out.push(full[(sy * width + sx) as usize]);
                }
            }
            out
        };
        for &(origin, size) in &[
            ((0, 0), (61, 37)),
            ((7, 3), (20, 11)),
            ((-5, -2), (12, 9)),
            ((55, 30), (14, 14)),
        ] {
            assert_eq!(
                canvas.crop(origin, size),
                reference(origin, size),
                "origin {origin:?} size {size:?}"
            );
        }
        let stored = canvas.scaled_bytes().expect("scaled").to_vec();
        let reloaded = BlurredCanvas::from_scaled(width, height, stored).expect("same dimensions");
        assert_eq!(
            reloaded.crop((3, 4), (10, 10)),
            canvas.crop((3, 4), (10, 10))
        );
        assert!(BlurredCanvas::from_scaled(width + 9, height, vec![0; 16]).is_none());
    }

    #[test]
    fn a_cached_canvas_round_trips_through_its_file() {
        let dir = std::env::temp_dir().join(format!("flamewm-canvas-{}", std::process::id()));
        let file = dir.join("one.canvas");
        let canvas = BlurredCanvas::new(40, 24, vec![90; 40 * 24 * 4], 0, 80);
        store_canvas(&file, (40, 24), canvas.scaled_bytes().expect("scaled"));
        let loaded = load_canvas(&file, (40, 24)).expect("cached canvas");
        assert_eq!(loaded.crop((0, 0), (40, 24)), canvas.crop((0, 0), (40, 24)));
        assert!(
            load_canvas(&file, (41, 24)).is_none(),
            "another root size is a miss"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn downscaled_blur_tracks_full_resolution_blur() {
        let (width, height) = (96u32, 64u32);
        let rgba: Vec<u8> = (0..width * height)
            .flat_map(|i| {
                let (x, y) = (i % width, i / width);
                [(x * 255 / width) as u8, (y * 255 / height) as u8, 96, 0xff]
            })
            .collect();
        let mut full = rgba.clone();
        flamewm_image_core::blur::gaussian_blur_rgba(&mut full, width, height, 24.0).unwrap();
        let fast = blurred_argb_words(width, height, rgba, 40, 100);
        let total: u32 = full
            .chunks_exact(4)
            .zip(&fast)
            .map(|(p, w)| {
                let got = [(w >> 16) & 0xff, (w >> 8) & 0xff, w & 0xff];
                (0..3)
                    .map(|c| (i32::from(p[c]) - got[c] as i32).unsigned_abs())
                    .sum::<u32>()
            })
            .sum();
        let mean = total as f32 / (width * height * 3) as f32;
        assert!(mean < 3.0, "mean channel error {mean}");
    }

    #[test]
    fn in_place_blur_matches_copying_reference_on_random_pixels() {
        // Reference: the pre-move path (copy, blur the copy, then convert).
        fn reference(width: u32, height: u32, rgba: &[u8], radius: f32) -> Vec<u32> {
            let mut pixels = rgba.to_vec();
            if radius > 0.0 {
                flamewm_image_core::blur::gaussian_blur_rgba(&mut pixels, width, height, radius)
                    .unwrap();
            }
            pixels
                .chunks_exact(4)
                .map(|p| {
                    0xff00_0000 | (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2])
                })
                .collect()
        }
        let mut seed: u32 = 0x1234_5678;
        let mut next = move || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 16) as u8
        };
        for &(width, height) in &[(1u32, 1u32), (7, 5), (31, 17)] {
            let len = (width * height * 4) as usize;
            let rgba: Vec<u8> = (0..len).map(|_| next()).collect();
            // Radii below twice BLUR_DOWNSCALE keep the full-resolution path this reference models.
            for &(opacity, blur) in &[(100u8, 80u8), (80, 0), (95, 25), (60, 33)] {
                let radius = blur_radius_px(opacity, blur);
                let expected = reference(width, height, &rgba, radius);
                let actual = blurred_argb_words(width, height, rgba.clone(), opacity, blur);
                assert_eq!(
                    actual, expected,
                    "{width}x{height} opacity={opacity} blur={blur}"
                );
            }
        }
    }

    #[test]
    fn pixmap_zero_masks_fall_back_to_truecolor_layout() {
        assert_eq!(channel_masks(0, 0, 0), (0xff_0000, 0xff_00, 0xff));
        // A window image keeps the masks its visual reports, even in BGR order.
        assert_eq!(
            channel_masks(0xff, 0xff00, 0xff_0000),
            (0xff, 0xff00, 0xff_0000)
        );
    }

    #[test]
    fn pixmap_decode_with_fallback_masks_is_rgb_and_ignores_alpha_byte() {
        let (r, g, b) = channel_masks(0, 0, 0);
        // Server word 0xAA112233: alpha 0xAA in the top byte must not leak into any channel.
        assert_eq!(decode_pixel(0xAA_11_22_33, r, g, b), (0x11, 0x22, 0x33));
    }

    #[test]
    fn decode_uses_masks_not_hardcoded_order() {
        // BGR-ordered server: red in low byte, blue in high byte.
        assert_eq!(
            decode_pixel(0x00_11_22_33, 0x0000ff, 0x00ff00, 0xff0000),
            (0x33, 0x22, 0x11)
        );
        // Standard RGB order.
        assert_eq!(
            decode_pixel(0x00_11_22_33, 0xff0000, 0x00ff00, 0x0000ff),
            (0x11, 0x22, 0x33)
        );
        // 16-bit 565.
        assert_eq!(decode_pixel(0xf800, 0xf800, 0x07e0, 0x001f).0, 255);
        assert_eq!(decode_pixel(0x001f, 0xf800, 0x07e0, 0x001f).2, 255);
    }

    #[test]
    fn fill_default_matches_twenty_percent_contract() {
        assert_eq!(fill_color(20), (0xef, 0x40, 0x48, 51));
        assert_eq!(BACKDROP_BORDER_ALPHA, 0xdd);
        assert_eq!(BACKDROP_BORDER_WIDTH_PX, 2);
        assert_eq!(
            selection_material(20),
            ((0xef, 0x40, 0x48, 51), (0xef, 0x40, 0x48, 0xdd))
        );
    }

    #[test]
    fn capture_rejects_empty_rect_without_x() {
        assert!(capture_drawable_rect(std::ptr::null_mut(), 0, (0, 0, 0, 10)).is_err());
        assert!(capture_drawable_rect(std::ptr::null_mut(), 0, (0, 0, 10, 10)).is_err());
    }
}

#[cfg(test)]
mod window_backdrop_tests {
    use super::*;

    #[test]
    fn empty_wallpaper_setting_selects_packaged_default() {
        let root = std::path::Path::new("/pkg");
        assert_eq!(
            wallpaper_file("  ", root),
            root.join(flamewm_image_core::wallpaper::DEFAULT_WALLPAPER_PATH)
        );
        assert_eq!(
            wallpaper_file("/home/u/bg.png", root),
            std::path::PathBuf::from("/home/u/bg.png")
        );
    }

    #[test]
    fn only_translucent_opacity_needs_a_backdrop() {
        let mut spec = WindowBackdrop {
            wallpaper: String::new(),
            fit: String::new(),
            packaged_root: std::path::PathBuf::new(),
            opacity_percent: 100,
            blur_percent: 40,
        };
        assert!(!spec.is_translucent());
        spec.opacity_percent = 99;
        assert!(spec.is_translucent());
    }

    #[test]
    fn recapture_policy_moves_immediately_and_freezes_resize_until_settled() {
        assert!(recapture_due(None, (800, 600), true));
        assert!(
            recapture_due(Some((800, 600)), (800, 600), true),
            "move recaptures"
        );
        assert!(
            !recapture_due(Some((800, 600)), (900, 600), true),
            "resize frozen while queued"
        );
        assert!(
            recapture_due(Some((800, 600)), (900, 600), false),
            "settled resize captures"
        );
    }

    #[test]
    fn blurred_window_snapshot_is_opaque_argb() {
        let rgba = [0x40, 0x80, 0xc0, 0xff].repeat(4 * 3);
        let words = blurred_argb_words(4, 3, rgba, 0, 50);
        assert_eq!(words.len(), 12);
        assert!(words.iter().all(|w| w >> 24 == 0xff));
    }
}

#[cfg(test)]
mod mirror_extend_tests {
    use super::*;

    /// 3x2 source: row 0 = 1,2,3 ; row 1 = 4,5,6.
    const SRC: [u32; 6] = [1, 2, 3, 4, 5, 6];

    #[test]
    fn grow_right_mirrors_the_right_edge_outward() {
        // 3x2 -> 5x2, origin 0: column 3 reflects column 2, column 4 reflects column 1.
        let out = mirror_extend_argb(&SRC, 3, 2, (5, 2), (0, 0));
        assert_eq!(out, vec![1, 2, 3, 3, 2, 4, 5, 6, 6, 5]);
    }

    #[test]
    fn grow_bottom_mirrors_the_bottom_edge_outward() {
        // 3x2 -> 3x4: row 2 repeats row 1, row 3 reflects row 0.
        let out = mirror_extend_argb(&SRC, 3, 2, (3, 4), (0, 0));
        assert_eq!(out, vec![1, 2, 3, 4, 5, 6, 4, 5, 6, 1, 2, 3]);
    }

    #[test]
    fn grow_left_places_source_at_offset_and_mirrors_leading_edge() {
        // Window grew 2px to the left: the old content starts at x=2 of a 5-wide canvas.
        let out = mirror_extend_argb(&SRC, 3, 2, (5, 2), (2, 0));
        assert_eq!(out, vec![2, 1, 1, 2, 3, 5, 4, 4, 5, 6]);
    }

    #[test]
    fn source_inside_destination_is_copied_unchanged() {
        let out = mirror_extend_argb(&SRC, 3, 2, (3, 2), (0, 0));
        assert_eq!(out, SRC.to_vec());
    }

    #[test]
    fn no_destination_pixel_is_left_zero_when_source_is_present() {
        let out = mirror_extend_argb(&SRC, 3, 2, (9, 7), (1, 3));
        assert!(out.iter().all(|px| *px != 0));
    }

    #[test]
    fn short_or_empty_source_yields_zeroed_canvas_without_panic() {
        assert_eq!(
            mirror_extend_argb(&[1, 2], 3, 2, (2, 2), (0, 0)),
            vec![0; 4]
        );
        assert_eq!(mirror_extend_argb(&[], 0, 0, (2, 2), (0, 0)), vec![0; 4]);
    }
}

#[cfg(test)]
mod edge_extend_tests {
    use super::*;

    #[test]
    fn anchor_is_intersection_for_partially_outside_rect() {
        assert_eq!(
            root_anchor_rect((-100, 50, 300, 200), (1350, 641)),
            (0, 50, 200, 200)
        );
        assert_eq!(
            root_anchor_rect((1200, 600, 300, 200), (1350, 641)),
            (1200, 600, 150, 41)
        );
    }

    #[test]
    fn anchor_is_full_rect_when_inside_root() {
        assert_eq!(
            root_anchor_rect((10, 20, 30, 40), (1350, 641)),
            (10, 20, 30, 40)
        );
    }

    #[test]
    fn anchor_clamps_fully_outside_rect_to_nearest_root_pixel() {
        assert_eq!(
            root_anchor_rect((-500, 10, 100, 100), (1350, 641)),
            (0, 10, 1, 1)
        );
        assert_eq!(
            root_anchor_rect((2000, 900, 100, 100), (1350, 641)),
            (1349, 640, 1, 1)
        );
        assert_eq!(
            root_anchor_rect((-50, -50, 10, 10), (1350, 641)),
            (0, 0, 1, 1)
        );
    }

    #[test]
    fn anchor_never_empty_for_zero_root() {
        assert_eq!(root_anchor_rect((5, 5, 10, 10), (0, 0)), (0, 0, 1, 1));
    }

    #[test]
    fn edge_extend_replicates_source_edges_outside_region() {
        // 2x2 source, pixel value = (x, y, 0, 255).
        let src: Vec<u8> = (0..4)
            .flat_map(|i: u8| [i % 2 + 10, i / 2 + 20, 0, 255])
            .collect();
        // Window 4x3 whose (0,0) is one pixel left/above the source origin.
        let out = edge_extend_rgba(&src, 2, 2, (-1, -1), (4, 3));
        let px = |x: usize, y: usize| &out[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4];
        assert_eq!(px(0, 0), &[10, 20, 0, 255]); // clamped to source (0,0)
        assert_eq!(px(1, 1), &[10, 20, 0, 255]); // source (0,0)
        assert_eq!(px(3, 2), &[11, 21, 0, 255]); // clamped to source (1,1)
        assert!(out.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn edge_extend_fully_outside_window_is_single_colour_not_black() {
        let src = vec![9u8, 8, 7, 255];
        let anchor = root_anchor_rect((-300, -300, 64, 64), (1350, 641));
        let shift = (-300 - i64::from(anchor.0), -300 - i64::from(anchor.1));
        let out = edge_extend_rgba(&src, 1, 1, shift, (64, 64));
        assert!(out.chunks_exact(4).all(|p| p == [9, 8, 7, 255]));
    }

    #[test]
    fn edge_extend_rejects_short_source_without_panicking() {
        let out = edge_extend_rgba(&[1, 2], 2, 2, (0, 0), (2, 2));
        assert_eq!(out.len(), 16);
    }
}

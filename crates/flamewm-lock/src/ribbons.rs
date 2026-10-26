//! Ribbons screensaver frames for the lock background: the animation is rendered
//! on the CPU and returned as straight RGBA8 for the image replace path.
//!
//! Spans the whole lock surface (the root window), which covers every `RandR` output.
//! Per-output independent ribbons are not implemented.
use std::time::Instant;

use flamewm_ribbons::{Config, Quality, Rasterizer, Rect, Ribbons};
use flamewm_ui_x11::RuntimeImage;

use crate::settings::SaverConfig;

/// Outputs larger than 1080p render at a quarter resolution to keep the frame cost bounded.
const FULL_RES_PIXELS: u64 = 1920 * 1080;
const SEED: u64 = 0x464c_414d;
/// Target frame interval (~30 fps).
pub const FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

pub struct RibbonFrames {
    ribbons: Ribbons,
    rasterizer: Rasterizer,
    width: u32,
    height: u32,
    started: Instant,
}

impl RibbonFrames {
    #[must_use]
    pub fn new(width: u32, height: u32, saver: &SaverConfig, now: Instant) -> Self {
        let quality = if u64::from(width) * u64::from(height) > FULL_RES_PIXELS {
            Quality::Quarter
        } else {
            Quality::Half
        };
        let config = Config {
            num: saver.num_ribbons,
            width_px: f64::from(saver.ribbon_width_tenths) / 10.0,
            span_rects: vec![Rect {
                x: 0,
                y: 0,
                w: width,
                h: height,
            }],
            seed: SEED,
        };
        Self {
            ribbons: Ribbons::new_fluid_style(config, 0.0),
            rasterizer: Rasterizer::new(quality),
            width,
            height,
            started: now,
        }
    }

    /// Renders the animation at `now` as an opaque RGBA image of the lock surface size.
    pub fn frame(&mut self, now: Instant) -> RuntimeImage {
        let t = now.saturating_duration_since(self.started).as_secs_f64();
        self.ribbons.step_to(t);
        let mut rgba = vec![0u8; self.width as usize * self.height as usize * 4];
        self.rasterizer.render_rgba(&self.ribbons, &mut rgba);
        RuntimeImage {
            source: "lock-ribbons".to_string(),
            width: self.width,
            height: self.height,
            pixels: rgba,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn saver() -> SaverConfig {
        SaverConfig::default()
    }

    #[test]
    fn frame_has_surface_size_and_opaque_alpha() {
        let now = Instant::now();
        let mut frames = RibbonFrames::new(160, 90, &saver(), now);
        let image = frames.frame(now + Duration::from_secs(2));
        assert_eq!((image.width, image.height), (160, 90));
        assert_eq!(image.pixels.len(), 160 * 90 * 4);
        assert!(image.pixels.chunks(4).all(|p| p[3] == 255));
    }

    /// The Ribbons frame is the whole background: opaque black with ribbons only. Any
    /// wallpaper, blur or dim would show as a non-black corner or sample on the first frame.
    #[test]
    fn first_frame_is_opaque_black_with_no_wallpaper_or_dim() {
        let now = Instant::now();
        let (w, h) = (160u32, 90u32);
        let mut frames = RibbonFrames::new(w, h, &saver(), now);
        let image = frames.frame(now);
        let at = |x: u32, y: u32| {
            let i = (y as usize * w as usize + x as usize) * 4;
            [
                image.pixels[i],
                image.pixels[i + 1],
                image.pixels[i + 2],
                image.pixels[i + 3],
            ]
        };
        for (x, y) in [
            (0, 0),
            (w - 1, 0),
            (0, h - 1),
            (w - 1, h - 1),
            (w / 2, h / 2),
        ] {
            assert_eq!(
                at(x, y),
                [0, 0, 0, 255],
                "pixel ({x}, {y}) is not opaque black"
            );
        }
    }

    /// Every frame after the first must stay fully opaque: a transparent gap would let the
    /// lock-root colour or a stale wallpaper show through the Ribbons screensaver.
    #[test]
    fn every_animated_frame_pixel_is_opaque() {
        let now = Instant::now();
        let mut frames = RibbonFrames::new(160, 90, &saver(), now);
        for secs in [1u64, 2, 5, 13, 40] {
            let image = frames.frame(now + Duration::from_secs(secs));
            let transparent = image.pixels.chunks(4).filter(|p| p[3] != 255).count();
            assert_eq!(
                transparent, 0,
                "frame at {secs}s has {transparent} non-opaque pixels"
            );
        }
    }

    #[test]
    fn frames_change_as_time_advances() {
        let now = Instant::now();
        let mut frames = RibbonFrames::new(320, 180, &saver(), now);
        let first = frames.frame(now + Duration::from_secs(1));
        let second = frames.frame(now + Duration::from_secs(3));
        assert_ne!(
            first.pixels, second.pixels,
            "ribbons must animate between frames"
        );
    }

    #[test]
    fn big_outputs_use_quarter_resolution() {
        let now = Instant::now();
        assert_eq!(
            RibbonFrames::new(1280, 800, &saver(), now)
                .rasterizer
                .quality(),
            Quality::Half
        );
        assert_eq!(
            RibbonFrames::new(3840, 2160, &saver(), now)
                .rasterizer
                .quality(),
            Quality::Quarter
        );
    }
}

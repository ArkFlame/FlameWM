use super::*;

/// A blurred root-sized image stored at the resolution its blur leaves meaningful.
///
/// A radius of at least twice [`BLUR_DOWNSCALE`] blurs a 1/`BLUR_DOWNSCALE` copy and upsamples it
/// (see [`blurred_argb_words`]); that small copy is all this keeps, 1/16 of the full image, and a
/// crop upsamples only the pixels it returns. A crop is pixel-identical to cropping the full
/// upsampled image. Smaller radii keep full-resolution words.
#[derive(Debug)]
pub struct BlurredCanvas {
    width: u32,
    height: u32,
    pixels: CanvasPixels,
}

#[derive(Debug)]
pub(super) enum CanvasPixels {
    Full(Vec<u32>),
    Scaled {
        small: Vec<u8>,
        small_w: u32,
        small_h: u32,
    },
}

impl BlurredCanvas {
    /// Blurs a `width` x `height` RGBA image per [`blur_radius_px`].
    pub fn new(
        width: u32,
        height: u32,
        mut rgba: Vec<u8>,
        opacity_percent: u8,
        blur_percent: u8,
    ) -> Self {
        let radius = blur_radius_px(opacity_percent, blur_percent);
        let pixels = if radius >= 2.0 * BLUR_DOWNSCALE as f32 {
            let (mut small, small_w, small_h) = box_downscale(&rgba, width, height, BLUR_DOWNSCALE);
            drop(rgba); // Release large input before blur; only the small canvas is retained.
            blur_in_place_slice(&mut small, small_w, small_h, radius / BLUR_DOWNSCALE as f32);
            CanvasPixels::Scaled {
                small,
                small_w,
                small_h,
            }
        } else {
            CanvasPixels::Full(blurred_argb_words(
                width,
                height,
                std::mem::take(&mut rgba),
                opacity_percent,
                blur_percent,
            ))
        };
        Self {
            width,
            height,
            pixels,
        }
    }

    /// A canvas from a small image stored earlier (see [`Self::scaled_bytes`]); `None` when the
    /// bytes do not describe a `width` x `height` canvas.
    pub(crate) fn from_scaled(width: u32, height: u32, small: Vec<u8>) -> Option<Self> {
        let (small_w, small_h) = (
            width.div_ceil(BLUR_DOWNSCALE),
            height.div_ceil(BLUR_DOWNSCALE),
        );
        (width > 0 && height > 0 && small.len() == (small_w * small_h * 4) as usize).then_some(
            Self {
                width,
                height,
                pixels: CanvasPixels::Scaled {
                    small,
                    small_w,
                    small_h,
                },
            },
        )
    }

    /// The stored small image, when this canvas keeps one.
    pub(crate) fn scaled_bytes(&self) -> Option<&[u8]> {
        match &self.pixels {
            CanvasPixels::Scaled { small, .. } => Some(small),
            CanvasPixels::Full(_) => None,
        }
    }

    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The `size` region at `origin` as opaque ARGB32 words. Pixels off the canvas replicate its
    /// nearest edge.
    #[must_use]
    pub fn crop(&self, origin: (i32, i32), size: (u32, u32)) -> Vec<u32> {
        if self.width == 0 || self.height == 0 {
            return Vec::new();
        }
        let clamp = |start: i32, offset: u32, len: u32| {
            (i64::from(start) + i64::from(offset)).clamp(0, i64::from(len) - 1) as usize
        };
        let mut out = Vec::with_capacity(size.0 as usize * size.1 as usize);
        match &self.pixels {
            CanvasPixels::Full(words) => {
                for y in 0..size.1 {
                    let row = &words[clamp(origin.1, y, self.height) * self.width as usize..]
                        [..self.width as usize];
                    out.extend((0..size.0).map(|x| row[clamp(origin.0, x, self.width)]));
                }
            }
            CanvasPixels::Scaled {
                small,
                small_w,
                small_h,
            } => {
                // Separable: each source row is blended horizontally once per crop, and every
                // output row only blends two of those lines. Taps cover the crop, not the root.
                let cols = (0..size.0)
                    .map(|x| {
                        bilinear_tap(*small_w, self.width, clamp(origin.0, x, self.width) as u32)
                    })
                    .collect::<Vec<_>>();
                let stride = *small_w as usize * 4;
                let line_len = size.0 as usize * 3;
                let mut top = (usize::MAX, vec![0u32; line_len]);
                let mut bottom = (usize::MAX, vec![0u32; line_len]);
                for y in 0..size.1 {
                    let (r0, r1, wy) = bilinear_tap(
                        *small_h,
                        self.height,
                        clamp(origin.1, y, self.height) as u32,
                    );
                    if top.0 != r0 {
                        if bottom.0 == r0 {
                            std::mem::swap(&mut top, &mut bottom);
                        } else {
                            horizontal_line(&small[r0 * stride..], &cols, &mut top.1);
                            top.0 = r0;
                        }
                    }
                    if bottom.0 != r1 && r1 != r0 {
                        horizontal_line(&small[r1 * stride..], &cols, &mut bottom.1);
                        bottom.0 = r1;
                    }
                    let lower = if r1 == r0 { &top.1 } else { &bottom.1 };
                    out.extend(top.1.chunks_exact(3).zip(lower.chunks_exact(3)).map(
                        |(upper, lower)| {
                            let mut word = 0xff00_0000u32;
                            for channel in 0..3usize {
                                let value =
                                    (upper[channel] * (256 - wy) + lower[channel] * wy + (1 << 15))
                                        >> 16;
                                word |= value << (16 - 8 * channel);
                            }
                            word
                        },
                    ));
                }
            }
        }
        out
    }

    /// [`Self::crop`] at `origin`, reusing `previous`, the same-sized crop at `previous_origin`:
    /// the overlap is copied and only the newly exposed strips are computed.
    #[must_use]
    pub fn recrop(
        &self,
        previous: &[u32],
        previous_origin: (i32, i32),
        origin: (i32, i32),
        size: (u32, u32),
    ) -> Vec<u32> {
        let (width, height) = (size.0 as usize, size.1 as usize);
        let dx = i64::from(origin.0) - i64::from(previous_origin.0);
        let dy = i64::from(origin.1) - i64::from(previous_origin.1);
        if previous.len() != width * height
            || dx.unsigned_abs() >= width as u64
            || dy.unsigned_abs() >= height as u64
        {
            return self.crop(origin, size);
        }
        let (dx, dy) = (dx as isize, dy as isize);
        let (w, h) = (width as isize, height as isize);
        // New columns x0..x1 and rows y0..y1 were inside the previous crop.
        let (x0, x1) = ((-dx).max(0) as usize, (w - dx).min(w) as usize);
        let (y0, y1) = ((-dy).max(0) as usize, (h - dy).min(h) as usize);
        let mut out = vec![0u32; width * height];
        for y in y0..y1 {
            let source = (y as isize + dy) as usize * width + (x0 as isize + dx) as usize;
            out[y * width + x0..y * width + x1]
                .copy_from_slice(&previous[source..source + (x1 - x0)]);
        }
        let mut fill = |left: usize, top: usize, columns: usize, rows: usize| {
            if columns == 0 || rows == 0 {
                return;
            }
            let block = self.crop(
                (origin.0 + left as i32, origin.1 + top as i32),
                (columns as u32, rows as u32),
            );
            for (row, words) in block.chunks_exact(columns).enumerate() {
                let start = (top + row) * width + left;
                out[start..start + columns].copy_from_slice(words);
            }
        };
        fill(0, 0, width, y0);
        fill(0, y1, width, height - y1);
        fill(0, y0, x0, y1 - y0);
        fill(x1, y0, width - x1, y1 - y0);
        out
    }
}

/// Magic and format version of a cached wallpaper canvas file.
pub(super) const CANVAS_CACHE_MAGIC: &[u8; 8] = b"FWBCNV01";

/// Cached canvases kept; older ones are removed when a new one is written.
pub(super) const CANVAS_CACHE_FILES: usize = 6;

/// The desktop layer every translucent surface blends over: the wallpaper under the whole
/// `root`, laid out per output like the desktop draws it (`outputs` in root coordinates; empty is
/// one wallpaper over the root), blurred once per [`BlurredCanvas::new`].
///
/// The first FlameWM process to need it decodes, composes and blurs the wallpaper and stores the
/// small canvas (about half a megabyte at 1080p) under `$XDG_CACHE_HOME/flamewm/backdrops`; every
/// later surface, process and session loads that file instead of decoding the wallpaper again.
/// The key covers the wallpaper file (path, size, modification time), the fit, the output layout,
/// the root size and the blur, so a changed wallpaper or setting never reads a stale canvas. The
/// decoded wallpaper is dropped as soon as the canvas exists.
pub fn desktop_canvas(
    wallpaper: &std::path::Path,
    fit: flamewm_image_core::wallpaper::WallpaperFit,
    outputs: &[(i32, i32, u32, u32)],
    root: (u32, u32),
    blur_percent: u8,
) -> Result<BlurredCanvas, String> {
    let outputs = if outputs == [(0, 0, root.0, root.1)] {
        &[][..]
    } else {
        outputs
    };
    let cache = canvas_cache_path(wallpaper, fit, outputs, root, blur_percent);
    if let Some(canvas) = cache.as_deref().and_then(|file| load_canvas(file, root)) {
        return Ok(canvas);
    }
    let natural = flamewm_image_core::wallpaper::decode_file(wallpaper)?;
    let composed = flamewm_image_core::wallpaper::compose_outputs_region(
        &natural,
        root,
        outputs,
        fit,
        (0, 0, root.0, root.1),
    )
    .ok_or("desktop canvas: wallpaper region unavailable")?;
    drop(natural);
    let canvas = BlurredCanvas::new(root.0, root.1, composed.pixels, 0, blur_percent);
    if let (Some(file), Some(small)) = (cache.as_deref(), canvas.scaled_bytes()) {
        store_canvas(file, root, small);
    }
    Ok(canvas)
}

/// Cache file for a desktop canvas; `None` without a cache directory or a readable wallpaper.
pub(super) fn canvas_cache_path(
    wallpaper: &std::path::Path,
    fit: flamewm_image_core::wallpaper::WallpaperFit,
    outputs: &[(i32, i32, u32, u32)],
    root: (u32, u32),
    blur_percent: u8,
) -> Option<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    let metadata = std::fs::metadata(wallpaper).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (
        wallpaper,
        metadata.len(),
        modified.as_nanos(),
        format!("{fit:?}"),
        outputs,
        root,
        blur_percent,
        BLUR_DOWNSCALE,
    )
        .hash(&mut hasher);
    let base = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| std::path::PathBuf::from(home).join(".cache"))
        })?;
    Some(
        base.join("flamewm/backdrops")
            .join(format!("{:016x}.canvas", hasher.finish())),
    )
}

pub(super) fn load_canvas(file: &std::path::Path, root: (u32, u32)) -> Option<BlurredCanvas> {
    let bytes = std::fs::read(file).ok()?;
    let header = CANVAS_CACHE_MAGIC.len() + 8;
    if bytes.len() < header || &bytes[..CANVAS_CACHE_MAGIC.len()] != CANVAS_CACHE_MAGIC {
        return None;
    }
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
    let size = (
        word(CANVAS_CACHE_MAGIC.len()),
        word(CANVAS_CACHE_MAGIC.len() + 4),
    );
    if size != root {
        return None;
    }
    BlurredCanvas::from_scaled(root.0, root.1, bytes[header..].to_vec())
}

/// Writes the canvas atomically (temporary file, then rename) and keeps only the newest
/// [`CANVAS_CACHE_FILES`] canvases. Any failure only costs the next window a decode.
pub(super) fn store_canvas(file: &std::path::Path, root: (u32, u32), small: &[u8]) {
    let Some(dir) = file.parent() else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let mut bytes = Vec::with_capacity(CANVAS_CACHE_MAGIC.len() + 8 + small.len());
    bytes.extend_from_slice(CANVAS_CACHE_MAGIC);
    bytes.extend_from_slice(&root.0.to_le_bytes());
    bytes.extend_from_slice(&root.1.to_le_bytes());
    bytes.extend_from_slice(small);
    let temporary = file.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&temporary, &bytes).is_err() || std::fs::rename(&temporary, file).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut canvases: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "canvas"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    canvases.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, stale) in canvases.into_iter().skip(CANVAS_CACHE_FILES) {
        let _ = std::fs::remove_file(stale);
    }
}

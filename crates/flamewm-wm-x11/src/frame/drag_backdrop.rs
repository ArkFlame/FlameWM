//! Drag-start backdrop snapshot for transparent titlebars.
//!
//! Supports a drag-start snapshot used to show the content behind a transparent titlebar while
//! a window moves. The root-sized image is composed once when the drag starts, and crops of it
//! are shown during the drag. Content changing behind the window mid-drag is frozen at drag
//! start; it is not re-read until the next drag.
//!
//! Conventions: every buffer is premultiplied RGBA8. Source-over is
//! `dst = src + dst * (255 - src_a) / 255`, which is correct for premultiplied input and needs no
//! division by alpha. `origin` is the root's top-left in desktop space (may be negative in
//! multi-monitor layouts); `crop` takes desktop coordinates; layers take coordinates relative to
//! the root's top-left.
//!
//! Pure logic: no X calls.

use std::fmt;

/// Upper bound on snapshot bytes (64 MiB; a 4K root is about 33 MB).
pub const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
/// Why a snapshot was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotError {
    /// A buffer length does not equal `w * h * 4` for its dimensions.
    BadLength { expected: usize, actual: usize },
    /// The requested buffer exceeds [`MAX_SNAPSHOT_BYTES`].
    TooLarge { bytes: usize },
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadLength { expected, actual } => {
                write!(f, "buffer length {actual} != expected {expected} bytes")
            }
            Self::TooLarge { bytes } => {
                write!(f, "snapshot {bytes} bytes exceeds cap {MAX_SNAPSHOT_BYTES}")
            }
        }
    }
}

impl std::error::Error for SnapshotError {}

/// A premultiplied RGBA layer composited over the base, positioned relative to the root's
/// top-left (may extend past the root; the overhang is clipped).
#[derive(Debug, Clone, Copy)]
pub struct Layer<'a> {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub rgba: &'a [u8],
}

/// Root-sized RGBA image captured at drag start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragSnapshot {
    origin_x: i32,
    origin_y: i32,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

fn byte_len(w: u32, h: u32) -> Result<usize, SnapshotError> {
    let bytes = usize::try_from(w)
        .ok()
        .and_then(|w| usize::try_from(h).ok().and_then(|h| w.checked_mul(h)))
        .and_then(|px| px.checked_mul(4))
        .unwrap_or(usize::MAX);
    if bytes > MAX_SNAPSHOT_BYTES {
        return Err(SnapshotError::TooLarge { bytes });
    }
    Ok(bytes)
}

impl DragSnapshot {
    /// Like `DragSnapshot::crop`, but desktop coordinates outside the root replicate the
    /// nearest root edge pixel instead of transparent black.
    #[cfg(test)]
    pub fn crop_clamped(&self, x: i32, y: i32, w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(w as usize * h as usize * 4);
        self.crop_clamped_into(x, y, w, h, &mut out);
        out
    }

    /// Same output as `DragSnapshot::crop_clamped`, written into `out`, which is cleared
    /// first. Its allocation is reused: repeated crops of the same size do not reallocate.
    pub fn crop_clamped_into(&self, x: i32, y: i32, w: u32, h: u32, out: &mut Vec<u8>) {
        let len = w as usize * h as usize * 4;
        out.clear();
        if self.width == 0 || self.height == 0 {
            out.resize(len, 0);
            return;
        }
        out.reserve(len);
        let stride = self.width as usize * 4;
        for row in 0..h {
            let sy = (i64::from(y) + i64::from(row) - i64::from(self.origin_y))
                .clamp(0, i64::from(self.height) - 1) as usize;
            let src_row = &self.pixels[sy * stride..(sy + 1) * stride];
            for col in 0..w {
                let sx = (i64::from(x) + i64::from(col) - i64::from(self.origin_x))
                    .clamp(0, i64::from(self.width) - 1) as usize;
                out.extend_from_slice(&src_row[sx * 4..sx * 4 + 4]);
            }
        }
        debug_assert_eq!(out.len(), len);
    }

    /// Frees the pixel storage and leaves an empty (0 x 0) snapshot. Later crops return
    /// transparent black, the same as any empty root.
    pub fn release(&mut self) {
        self.width = 0;
        self.height = 0;
        self.pixels = Vec::new();
    }
}

/// `v` as an index when it lies in `0..len`.
fn index_in(v: i64, len: u32) -> Option<usize> {
    usize::try_from(v).ok().filter(|&i| i < len as usize)
}

/// Source-over of one premultiplied pixel onto `dst`.
fn composite_pixel(dst: &mut [u8], src: &[u8]) {
    let inv = 255 - u32::from(src[3]);
    for c in 0..4 {
        let v = u32::from(src[c]) + (u32::from(dst[c]) * inv + 127) / 255;
        dst[c] = v.min(255) as u8;
    }
}

impl DragSnapshot {
    /// Builds the snapshot from `base` (taken as the snapshot's storage, no copy) then `layers`
    /// in order. Refuses sizes above [`MAX_SNAPSHOT_BYTES`] before reading any buffer.
    pub fn build(
        origin: (i32, i32),
        width: u32,
        height: u32,
        base: Vec<u8>,
        layers: &[Layer<'_>],
    ) -> Result<Self, SnapshotError> {
        let len = byte_len(width, height)?;
        if base.len() != len {
            return Err(SnapshotError::BadLength {
                expected: len,
                actual: base.len(),
            });
        }
        let mut pixels = base;
        for layer in layers {
            let layer_len = byte_len(layer.w, layer.h)?;
            if layer.rgba.len() != layer_len {
                return Err(SnapshotError::BadLength {
                    expected: layer_len,
                    actual: layer.rgba.len(),
                });
            }
            for ly in 0..layer.h {
                let Some(dy) = index_in(i64::from(layer.y) + i64::from(ly), height) else { continue };
                for lx in 0..layer.w {
                    let Some(dx) = index_in(i64::from(layer.x) + i64::from(lx), width) else { continue };
                    let s = (ly as usize * layer.w as usize + lx as usize) * 4;
                    let d = (dy * width as usize + dx) * 4;
                    composite_pixel(&mut pixels[d..d + 4], &layer.rgba[s..s + 4]);
                }
            }
        }
        Ok(Self {
            origin_x: origin.0,
            origin_y: origin.1,
            width,
            height,
            pixels,
        })
    }

    /// Returns the `w x h` region at desktop `(x, y)` as RGBA. Areas outside the root are
    /// transparent black. Test-only: the titlebar path crops via `crop_clamped`.
    #[cfg(test)]
    pub fn crop(&self, x: i32, y: i32, w: u32, h: u32) -> Vec<u8> {
        let mut out = vec![0u8; w as usize * h as usize * 4];
        for row in 0..h {
            let sy = i64::from(y) + i64::from(row) - i64::from(self.origin_y);
            if sy < 0 || sy >= i64::from(self.height) {
                continue;
            }
            for col in 0..w {
                let sx = i64::from(x) + i64::from(col) - i64::from(self.origin_x);
                if sx < 0 || sx >= i64::from(self.width) {
                    continue;
                }
                let s = (sy as usize * self.width as usize + sx as usize) * 4;
                let d = (row as usize * w as usize + col as usize) * 4;
                out[d..d + 4].copy_from_slice(&self.pixels[s..s + 4]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLACK: [u8; 4] = [0, 0, 0, 255];

    fn px(c: [u8; 4]) -> Vec<u8> {
        c.to_vec()
    }

    #[test]
    fn composite_applies_layers_in_order() {
        let base = px(BLACK);
        let red = px(RED);
        let green = px(GREEN);
        let layers = [
            Layer { x: 0, y: 0, w: 1, h: 1, rgba: &red },
            Layer { x: 0, y: 0, w: 1, h: 1, rgba: &green },
        ];
        let snap = DragSnapshot::build((0, 0), 1, 1, base.clone(), &layers).unwrap();
        assert_eq!(snap.crop(0, 0, 1, 1), GREEN.to_vec());

        // Half-coverage premultiplied white over opaque black gives mid grey, not white.
        let half = [128, 128, 128, 128];
        let layers = [Layer { x: 0, y: 0, w: 1, h: 1, rgba: &half }];
        let snap = DragSnapshot::build((0, 0), 1, 1, base, &layers).unwrap();
        assert_eq!(snap.crop(0, 0, 1, 1), vec![128, 128, 128, 255]);
    }

    #[test]
    fn crop_with_negative_origin_maps_desktop_coordinates() {
        // 2x2 root whose top-left sits at desktop (-2, -1).
        let mut base = Vec::new();
        for i in 0..4u8 {
            base.extend_from_slice(&[i * 10 + 1, 0, 0, 255]);
        }
        let snap = DragSnapshot::build((-2, -1), 2, 2, base.clone(), &[]).unwrap();
        // Local (0,0) is pixel index 0.
        assert_eq!(snap.crop(-2, -1, 1, 1), base[0..4].to_vec());
        // Desktop (-1, 0) is local (1, 1) = pixel index 3; (0, 0) is local (2, 1), outside.
        let mut expected = base[12..16].to_vec();
        expected.extend_from_slice(&[0, 0, 0, 0]);
        assert_eq!(snap.crop(-1, 0, 2, 1), expected);
    }

    #[test]
    fn crop_outside_root_is_transparent() {
        let base = [px(RED), px(GREEN), px(BLACK), px(RED)].concat();
        let snap = DragSnapshot::build((0, 0), 2, 2, base, &[]).unwrap();
        assert_eq!(snap.crop(10, 10, 3, 3), vec![0u8; 3 * 3 * 4]);
        // Desktop (1,1) is the last root pixel; the rest of this 2x2 window is outside.
        let mut expected = px(RED);
        expected.extend_from_slice(&[0u8; 12]);
        assert_eq!(snap.crop(1, 1, 2, 2), expected);
    }

    #[test]
    fn crop_clamped_replicates_root_edges() {
        let base = [px(RED), px(GREEN), px(BLACK), px(RED)].concat();
        let snap = DragSnapshot::build((0, 0), 2, 2, base, &[]).unwrap();
        assert_eq!(snap.crop_clamped(-2, -2, 2, 2), [px(RED), px(RED), px(RED), px(RED)].concat());
        assert_eq!(snap.crop_clamped(1, 1, 2, 1), [px(RED), px(RED)].concat());
    }

    /// The pre-optimization `crop_clamped`, kept verbatim as the reference output.
    fn reference_crop_clamped(snap: &DragSnapshot, x: i32, y: i32, w: u32, h: u32) -> Vec<u8> {
        let mut out = vec![0u8; w as usize * h as usize * 4];
        if snap.width == 0 || snap.height == 0 {
            return out;
        }
        for row in 0..h {
            let sy = (i64::from(y) + i64::from(row) - i64::from(snap.origin_y))
                .clamp(0, i64::from(snap.height) - 1) as usize;
            for col in 0..w {
                let sx = (i64::from(x) + i64::from(col) - i64::from(snap.origin_x))
                    .clamp(0, i64::from(snap.width) - 1) as usize;
                let s = (sy * snap.width as usize + sx) * 4;
                let d = (row as usize * w as usize + col as usize) * 4;
                out[d..d + 4].copy_from_slice(&snap.pixels[s..s + 4]);
            }
        }
        out
    }

    fn gradient_snapshot(origin: (i32, i32), w: u32, h: u32) -> DragSnapshot {
        let base: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i * 37 % 256) as u8, (i * 91 % 256) as u8, (i * 13 % 256) as u8, 255])
            .collect();
        DragSnapshot::build(origin, w, h, base, &[]).unwrap()
    }

    #[test]
    fn crop_clamped_into_matches_reference_for_edges_origins_and_zero_sizes() {
        let snaps = [
            gradient_snapshot((0, 0), 5, 4),
            gradient_snapshot((-3, -2), 5, 4),
            gradient_snapshot((100, 40), 1, 1),
            DragSnapshot::build((7, 7), 0, 0, Vec::new(), &[]).unwrap(),
            DragSnapshot::build((0, 0), 0, 3, Vec::new(), &[]).unwrap(),
        ];
        let mut buf = Vec::new();
        for snap in &snaps {
            for &(x, y) in &[(-20, -20), (-3, -2), (-1, 0), (0, 0), (2, 1), (4, 3), (99, 39), (200, 200)] {
                for &(w, h) in &[(0, 0), (0, 5), (5, 0), (1, 1), (3, 2), (8, 7)] {
                    let expected = reference_crop_clamped(snap, x, y, w, h);
                    assert_eq!(snap.crop_clamped(x, y, w, h), expected, "crop_clamped ({x},{y},{w},{h})");
                    snap.crop_clamped_into(x, y, w, h, &mut buf);
                    assert_eq!(buf, expected, "crop_clamped_into ({x},{y},{w},{h})");
                }
            }
        }
    }

    #[test]
    fn crop_clamped_into_reuses_allocation_across_repeated_crops() {
        let snap = gradient_snapshot((-10, -10), 64, 48);
        let mut buf = Vec::new();
        snap.crop_clamped_into(-5, -5, 40, 36, &mut buf);
        let cap = buf.capacity();
        let ptr = buf.as_ptr();
        for step in 0..200 {
            snap.crop_clamped_into(step - 20, step % 7 - 3, 40, 36, &mut buf);
            assert_eq!(buf.len(), 40 * 36 * 4);
            assert_eq!(buf.capacity(), cap);
            assert_eq!(buf.as_ptr(), ptr);
        }
    }

    #[test]
    fn release_frees_pixels_and_crops_become_transparent_fallback() {
        let mut snap = gradient_snapshot((0, 0), 4, 4);
        snap.release();
        assert_eq!(snap.pixels.capacity(), 0);
        assert_eq!(snap.crop_clamped(0, 0, 2, 2), vec![0u8; 16]);
    }

    #[test]
    fn build_refuses_snapshot_above_cap() {
        // 4097 x 4096 x 4 bytes is above the 64 MiB cap; the cap check runs before the base
        // buffer is read, so an empty base is enough to exercise it.
        let err = DragSnapshot::build((0, 0), 4097, 4096, Vec::new(), &[]).unwrap_err();
        assert!(matches!(err, SnapshotError::TooLarge { bytes } if bytes > MAX_SNAPSHOT_BYTES));
    }

    #[test]
    fn build_rejects_wrong_base_length() {
        let err = DragSnapshot::build((0, 0), 2, 2, vec![0u8; 4], &[]).unwrap_err();
        assert_eq!(err, SnapshotError::BadLength { expected: 16, actual: 4 });
    }
}

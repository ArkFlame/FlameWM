//! Paranoid damage mode (`FLAMEWM_PARANOID_DAMAGE=1`, default off): every region repaint is
//! checked against a full repaint of the same frame. The full repaint is the frame that is
//! presented, so a region mismatch is reported and never reaches the screen.
//!
//! Off, the only cost on the paint path is one cached bool read in [`enabled`].

use std::os::raw::c_ulong;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use super::X11App;
use super::damage_plan::PixelRegion;
use crate::xlib::{XDestroyImage, XGetImage, XImageHead, ZPIXMAP};

/// Comparison granularity in device pixels.
pub(crate) const TILE: u32 = 32;
/// Mismatch rects printed per distinct rect-set.
const MAX_LOGGED_RECTS: usize = 8;

static CHECKED: AtomicU64 = AtomicU64::new(0);
static MISMATCHED: AtomicU64 = AtomicU64::new(0);
/// Last logged (label, rect-set); an identical set is not logged again.
static LAST_LOGGED: Mutex<Option<(String, Vec<PixelRegion>)>> = Mutex::new(None);

/// `FLAMEWM_PARANOID_DAMAGE=1`, read once.
pub(crate) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("FLAMEWM_PARANOID_DAMAGE").is_ok_and(|v| v == "1"))
}

/// Tile rects (`TILE` square, clipped at the right and bottom edges) where `expected` and `actual`
/// differ, in row-major tile order. Both buffers are row-major ARGB32 of `w` x `h`. A buffer whose
/// length is not `w * h` mismatches the whole frame.
pub(crate) fn diff_rects(expected: &[u32], actual: &[u32], w: u32, h: u32) -> Vec<PixelRegion> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let stride = w as usize;
    let frame = stride * h as usize;
    if expected.len() != frame || actual.len() != frame {
        return vec![PixelRegion {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }];
    }
    let mut out = Vec::new();
    for ty in (0..h).step_by(TILE as usize) {
        for tx in (0..w).step_by(TILE as usize) {
            let tw = TILE.min(w - tx);
            let th = TILE.min(h - ty);
            let differs = (ty..ty + th).any(|y| {
                let row = y as usize * stride;
                (tx..tx + tw).any(|x| {
                    let i = row + x as usize;
                    expected[i] != actual[i]
                })
            });
            if differs {
                out.push(PixelRegion {
                    x: tx as i32,
                    y: ty as i32,
                    width: tw,
                    height: th,
                });
            }
        }
    }
    out
}

/// First pixel inside `rect` where the buffers differ: `(x, y, expected, actual)`. A missing pixel
/// reads as 0.
pub(crate) fn first_difference(
    expected: &[u32],
    actual: &[u32],
    w: u32,
    rect: PixelRegion,
) -> Option<(u32, u32, u32, u32)> {
    let (x0, y0) = (rect.x as u32, rect.y as u32);
    for y in y0..y0 + rect.height {
        for x in x0..x0 + rect.width {
            let i = y as usize * w as usize + x as usize;
            match (expected.get(i), actual.get(i)) {
                (Some(e), Some(a)) if e == a => {}
                (e, a) => {
                    return Some((x, y, e.copied().unwrap_or(0), a.copied().unwrap_or(0)));
                }
            }
        }
    }
    None
}

/// Compares a full frame (`expected`) with the region frame (`actual`). Returns the mismatching
/// tiles, empty when they are equal. Logs each distinct rect-set once, tagged with `label`.
pub(crate) fn check_frame(
    label: &str,
    expected: &[u32],
    actual: &[u32],
    w: u32,
    h: u32,
) -> Vec<PixelRegion> {
    let checked = CHECKED.fetch_add(1, Ordering::Relaxed) + 1;
    if checked % 100 == 0 {
        eprintln!(
            "FLAMEWM_PARANOID summary checked={checked} mismatched={}",
            MISMATCHED.load(Ordering::Relaxed)
        );
    }
    let rects = diff_rects(expected, actual, w, h);
    if rects.is_empty() {
        return rects;
    }
    let mismatched = MISMATCHED.fetch_add(1, Ordering::Relaxed) + 1;
    let Ok(mut last) = LAST_LOGGED.lock() else {
        return rects;
    };
    let same = last
        .as_ref()
        .is_some_and(|(prev, prev_rects)| prev == label && *prev_rects == rects);
    if !same {
        eprintln!(
            "FLAMEWM_PARANOID mismatch label={label} frame={checked} tiles={} mismatched_frames={mismatched}",
            rects.len()
        );
        for rect in rects.iter().take(MAX_LOGGED_RECTS) {
            if let Some((x, y, e, a)) = first_difference(expected, actual, w, *rect) {
                eprintln!(
                    "FLAMEWM_PARANOID   rect={},{},{},{} first=({x},{y}) expected=0x{e:08x} actual=0x{a:08x}",
                    rect.x, rect.y, rect.width, rect.height
                );
            }
        }
        *last = Some((label.to_string(), rects.clone()));
    }
    rects
}

impl X11App {
    /// The backbuffer as row-major ARGB32 pixels.
    ///
    /// # Safety
    /// Must run on the display thread that owns `self.display`.
    pub(crate) unsafe fn read_backbuffer_pixels(&self) -> Result<Vec<u32>, String> {
        let (w, h) = (self.width, self.height);
        // SAFETY: the backbuffer is a live pixmap of this display; the image is freed below.
        let image = unsafe {
            XGetImage(
                self.display,
                self.backbuffer,
                0,
                0,
                w,
                h,
                c_ulong::MAX,
                ZPIXMAP,
            )
        };
        if image.is_null() {
            return Err("XGetImage of backbuffer failed".to_string());
        }
        // SAFETY: XGetImage returned a valid XImage; XImageHead is its public prefix layout.
        let head = unsafe { &*(image as *const XImageHead) };
        let stride = head.bytes_per_line as usize;
        let row_bytes = w as usize * 4;
        let mut bytes = Vec::with_capacity(row_bytes * h as usize);
        for row in 0..h as usize {
            // SAFETY: each row lies inside the image data (stride * height bytes).
            let line = unsafe {
                std::slice::from_raw_parts(head.data.add(row * stride) as *const u8, row_bytes)
            };
            bytes.extend_from_slice(line);
        }
        // SAFETY: matches the XGetImage above.
        unsafe { XDestroyImage(image) };
        Ok(bytes
            .chunks_exact(4)
            .map(|px| u32::from_ne_bytes([px[0], px[1], px[2], px[3]]))
            .collect())
    }

    /// Paranoid check after a full repaint: `region` is the frame the region repaint produced, and
    /// the backbuffer now holds the full repaint of the same frame.
    ///
    /// # Safety
    /// Same as [`Self::read_backbuffer_pixels`].
    pub(crate) unsafe fn paranoid_compare_full(&self, region: &[u32]) {
        // SAFETY: forwarded from the caller's contract.
        match unsafe { self.read_backbuffer_pixels() } {
            Ok(full) => {
                check_frame(
                    &format!("{:#x}", self.window),
                    &full,
                    region,
                    self.width,
                    self.height,
                );
            }
            Err(error) => eprintln!("FLAMEWM_PARANOID readback failed reason={error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, value: u32) -> Vec<u32> {
        vec![value; (w * h) as usize]
    }

    #[test]
    fn identical_frames_have_no_mismatch() {
        let a = solid(100, 70, 0xff10_2030);
        assert!(diff_rects(&a, &a.clone(), 100, 70).is_empty());
    }

    #[test]
    fn one_pixel_difference_reports_its_tile() {
        let a = solid(64, 64, 1);
        let mut b = a.clone();
        b[40 * 64 + 33] = 2;
        assert_eq!(
            diff_rects(&a, &b, 64, 64),
            vec![PixelRegion {
                x: 32,
                y: 32,
                width: 32,
                height: 32
            }]
        );
    }

    #[test]
    fn edge_tile_is_clipped_to_frame() {
        let a = solid(40, 10, 0);
        let mut b = a.clone();
        b[9 * 40 + 39] = 7;
        assert_eq!(
            diff_rects(&a, &b, 40, 10),
            vec![PixelRegion {
                x: 32,
                y: 0,
                width: 8,
                height: 10
            }]
        );
    }

    #[test]
    fn size_mismatch_reports_whole_frame() {
        let a = solid(4, 4, 0);
        let b = solid(4, 3, 0);
        assert_eq!(
            diff_rects(&a, &b, 4, 4),
            vec![PixelRegion {
                x: 0,
                y: 0,
                width: 4,
                height: 4
            }]
        );
        assert!(diff_rects(&a, &a, 0, 4).is_empty());
    }

    #[test]
    fn first_difference_names_the_pixel() {
        let a = solid(64, 64, 5);
        let mut b = a.clone();
        b[35 * 64 + 40] = 9;
        let rect = PixelRegion {
            x: 32,
            y: 32,
            width: 32,
            height: 32,
        };
        assert_eq!(first_difference(&a, &b, 64, rect), Some((40, 35, 5, 9)));
    }

    #[test]
    fn forced_mismatch_is_reported_and_equal_frames_are_not() {
        let a = solid(64, 32, 3);
        let mut b = a.clone();
        b[0] = 4;
        assert_eq!(check_frame("test", &a, &a, 64, 32).len(), 0);
        assert_eq!(check_frame("test", &a, &b, 64, 32).len(), 1);
    }
}

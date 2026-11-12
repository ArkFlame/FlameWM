//! Alt+Tab thumbnail policy: target size, cache key and a bounded LRU of captured pixels.
//!
//! Pure: no X state, no pixels, no allocation beyond the cache itself. The capture path (a later
//! slice) asks `thumb_size` for the target rect, keys its pixels by `ThumbKey`, and stores them in
//! `ThumbCache`, which evicts the least recently shown entry once `MAX_THUMBS` is reached. Captures
//! are lazy (taken when the switcher opens) and freed on close or eviction; nothing stays resident
//! while idle (see docs/backlog/alttab.md).

use std::collections::VecDeque;

/// Thumbnail box in device pixels; the card renders at most this size.
pub(crate) const THUMB_MAX_W: u32 = 160;
pub(crate) const THUMB_MAX_H: u32 = 100;

/// Bytes per thumbnail pixel (32-bit `ZPixmap` after conversion).
#[cfg(test)]
pub(crate) const THUMB_BYTES_PER_PIXEL: u64 = 4;

/// Upper bound on resident thumbnails. `24 * 160 * 100 * 4` = `1_536_000` bytes (about 1.46 MiB),
/// under the 2 MiB transient-peak budget in `docs/competitors/vela/GAP_PLAN.md`.
pub(crate) const MAX_THUMBS: usize = 24;

/// Aspect-fit a source window size into the thumbnail box. Never upscales: a window smaller than
/// the box keeps its own size. Returns `None` for an empty source (nothing to capture).
pub(crate) fn thumb_size(src_w: u32, src_h: u32) -> Option<(u32, u32)> {
    if src_w == 0 || src_h == 0 {
        return None;
    }
    if src_w <= THUMB_MAX_W && src_h <= THUMB_MAX_H {
        return Some((src_w, src_h));
    }
    // Scale by the tighter axis, rounding down, keeping at least one pixel per axis.
    let scale_w = u64::from(THUMB_MAX_W) * u64::from(src_h);
    let scale_h = u64::from(THUMB_MAX_H) * u64::from(src_w);
    if scale_w <= scale_h {
        // Width is the tighter bound.
        let h = (u64::from(src_h) * u64::from(THUMB_MAX_W) / u64::from(src_w)).max(1);
        Some((THUMB_MAX_W, u32::try_from(h).unwrap_or(THUMB_MAX_H)))
    } else {
        let w = (u64::from(src_w) * u64::from(THUMB_MAX_H) / u64::from(src_h)).max(1);
        Some((u32::try_from(w).unwrap_or(THUMB_MAX_W), THUMB_MAX_H))
    }
}

/// Bytes one thumbnail of `(w, h)` occupies.
#[cfg(test)]
pub(crate) fn thumb_bytes(w: u32, h: u32) -> u64 {
    u64::from(w) * u64::from(h) * THUMB_BYTES_PER_PIXEL
}

/// Box-filter downscale of a 32-bit pixel buffer (BGRA or RGBA; channel order is irrelevant) to
/// `dst_w` x `dst_h`. `stride` is the source row pitch in bytes and may exceed `src_w * 4`; padding
/// is skipped. Returns `None` on any zero size, a stride or buffer too small for the image, or a
/// destination larger than the source (this filter never upscales).
pub(crate) fn downscale_box(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    stride: usize,
    dst_w: u32,
    dst_h: u32,
) -> Option<Vec<u8>> {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return None;
    }
    let (sw, sh) = (src_w as usize, src_h as usize);
    let (dw, dh) = (dst_w as usize, dst_h as usize);
    if dw > sw || dh > sh {
        return None;
    }
    let row_bytes = sw.checked_mul(4)?;
    if stride < row_bytes {
        return None;
    }
    let needed = stride.checked_mul(sh - 1)?.checked_add(row_bytes)?;
    if src.len() < needed {
        return None;
    }
    let mut out = vec![0u8; dw * dh * 4];
    for dy in 0..dh {
        let y0 = dy * sh / dh;
        let y1 = ((dy + 1) * sh / dh).max(y0 + 1);
        for dx in 0..dw {
            let x0 = dx * sw / dw;
            let x1 = ((dx + 1) * sw / dw).max(x0 + 1);
            let mut acc = [0u32; 4];
            for sy in y0..y1 {
                let row = &src[sy * stride..sy * stride + row_bytes];
                for px in row[x0 * 4..x1 * 4].chunks_exact(4) {
                    for (a, c) in acc.iter_mut().zip(px) {
                        *a += u32::from(*c);
                    }
                }
            }
            let n = ((x1 - x0) * (y1 - y0)) as u32;
            let o = (dy * dw + dx) * 4;
            for (c, a) in acc.iter().enumerate() {
                out[o + c] = (a / n) as u8;
            }
        }
    }
    Some(out)
}

/// Capture-to-thumbnail: sizes the source with `thumb_size` and box-downscales it to that size.
/// Returns `(w, h, pixels)` or `None` when the source is empty or malformed.
pub(crate) fn capture_to_thumb(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    stride: usize,
) -> Option<(u32, u32, Vec<u8>)> {
    let (w, h) = thumb_size(src_w, src_h)?;
    let pixels = downscale_box(src, src_w, src_h, stride, w, h)?;
    Some((w, h, pixels))
}

/// Cache key: a window plus its source size. A resize changes the key, so stale pixels miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ThumbKey {
    pub(crate) window: u32,
    pub(crate) src_w: u32,
    pub(crate) src_h: u32,
}

/// Bounded LRU. Front is least recently used, back most recently used.
#[derive(Debug)]
pub(crate) struct ThumbCache<V> {
    entries: VecDeque<(ThumbKey, V)>,
    capacity: usize,
}

impl<V> Default for ThumbCache<V> {
    fn default() -> Self {
        Self::with_capacity(MAX_THUMBS)
    }
}

impl<V> ThumbCache<V> {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Looks up `key` and marks it most recently used.
    pub(crate) fn get(&mut self, key: ThumbKey) -> Option<&V> {
        let index = self.entries.iter().position(|(k, _)| *k == key)?;
        let entry = self.entries.remove(index)?;
        self.entries.push_back(entry);
        self.entries.back().map(|(_, v)| v)
    }

    /// Inserts or replaces `key`; returns the value evicted to stay within capacity, if any.
    pub(crate) fn insert(&mut self, key: ThumbKey, value: V) -> Option<V> {
        if let Some(index) = self.entries.iter().position(|(k, _)| *k == key) {
            self.entries.remove(index);
        }
        self.entries.push_back((key, value));
        if self.entries.len() > self.capacity {
            return self.entries.pop_front().map(|(_, v)| v);
        }
        None
    }

    /// Windows that currently hold at least one entry (may repeat per entry).
    pub(crate) fn windows(&self) -> impl Iterator<Item = u32> + '_ {
        self.entries.iter().map(|(key, _)| key.window)
    }

    /// Drops every entry for `window` (window closed or unmapped); returns how many went.
    pub(crate) fn remove_window(&mut self, window: u32) -> usize {
        let before = self.entries.len();
        self.entries.retain(|(k, _)| k.window != window);
        before - self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_window_keeps_its_size() {
        assert_eq!(thumb_size(80, 60), Some((80, 60)));
        assert_eq!(thumb_size(160, 100), Some((160, 100)));
    }

    #[test]
    fn large_window_fits_the_box_keeping_aspect() {
        // 1280x800 is 16:10: the box is 160x100 exactly.
        assert_eq!(thumb_size(1280, 800), Some((160, 100)));
        // 1920x1080 is 16:9: width bound, height 90.
        assert_eq!(thumb_size(1920, 1080), Some((160, 90)));
        // Tall window: height bound, width 50.
        assert_eq!(thumb_size(500, 1000), Some((50, 100)));
    }

    #[test]
    fn extreme_aspect_keeps_one_pixel() {
        assert_eq!(thumb_size(10_000, 1), Some((160, 1)));
        assert_eq!(thumb_size(1, 10_000), Some((1, 100)));
    }

    #[test]
    fn empty_source_has_no_thumbnail() {
        assert_eq!(thumb_size(0, 600), None);
        assert_eq!(thumb_size(800, 0), None);
    }

    #[test]
    fn resident_budget_stays_under_two_mib() {
        let per = thumb_bytes(THUMB_MAX_W, THUMB_MAX_H);
        assert_eq!(per, 64_000);
        assert!(per * MAX_THUMBS as u64 <= 2 * 1024 * 1024);
    }

    fn key(window: u32) -> ThumbKey {
        ThumbKey {
            window,
            src_w: 1280,
            src_h: 800,
        }
    }

    #[test]
    fn lru_evicts_least_recently_used() {
        let mut cache = ThumbCache::with_capacity(2);
        assert_eq!(cache.insert(key(1), "a"), None);
        assert_eq!(cache.insert(key(2), "b"), None);
        assert_eq!(cache.get(key(1)), Some(&"a")); // 1 is now most recent
        assert_eq!(cache.insert(key(3), "c"), Some("b"));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(key(2)), None);
        assert_eq!(cache.get(key(1)), Some(&"a"));
    }

    #[test]
    fn resize_is_a_new_key() {
        let mut cache = ThumbCache::with_capacity(4);
        cache.insert(key(7), 1);
        let resized = ThumbKey {
            src_w: 640,
            ..key(7)
        };
        assert_eq!(cache.get(resized), None);
        assert_eq!(cache.get(key(7)), Some(&1));
    }

    #[test]
    fn reinsert_replaces_without_growing() {
        let mut cache = ThumbCache::with_capacity(2);
        cache.insert(key(1), 1);
        assert_eq!(cache.insert(key(1), 2), None);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(key(1)), Some(&2));
    }

    #[test]
    fn remove_window_drops_all_its_sizes() {
        let mut cache = ThumbCache::with_capacity(8);
        cache.insert(key(1), 1);
        cache.insert(
            ThumbKey {
                src_w: 640,
                ..key(1)
            },
            2,
        );
        cache.insert(key(2), 3);
        assert_eq!(cache.remove_window(1), 2);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn default_cache_is_bounded_by_max_thumbs() {
        let mut cache: ThumbCache<u32> = ThumbCache::default();
        for w in 0..(MAX_THUMBS as u32 + 5) {
            cache.insert(key(w), w);
        }
        assert_eq!(cache.len(), MAX_THUMBS);
        assert_eq!(cache.get(key(0)), None);
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    #[test]
    fn downscale_2x2_to_1x1_is_exact_channel_mean() {
        // Pixels [0,4,8,255], [4,8,12,255], [8,12,16,255], [12,16,20,255]: means 6, 10, 14, 255.
        let src = [0, 4, 8, 255, 4, 8, 12, 255, 8, 12, 16, 255, 12, 16, 20, 255];
        assert_eq!(
            downscale_box(&src, 2, 2, 8, 1, 1),
            Some(vec![6, 10, 14, 255])
        );
    }

    #[test]
    fn downscale_ignores_stride_padding() {
        // 2x2 image, stride 16 (8 bytes of 0xEE padding per row): same result as the tight case.
        let mut src = Vec::new();
        for row in [
            [0u8, 4, 8, 255, 4, 8, 12, 255],
            [8, 12, 16, 255, 12, 16, 20, 255],
        ] {
            src.extend_from_slice(&row);
            src.extend_from_slice(&[0xEE; 8]);
        }
        assert_eq!(
            downscale_box(&src, 2, 2, 16, 1, 1),
            Some(vec![6, 10, 14, 255])
        );
    }

    #[test]
    fn capture_keeps_aspect_and_fits_the_box() {
        for &(w, h, tw, th) in &[
            (1280u32, 800u32, 160u32, 100u32),
            (1920, 1080, 160, 90),
            (640, 400, 160, 100),
        ] {
            let stride = w as usize * 4;
            let src = vec![0x80u8; stride * h as usize];
            let (ow, oh, px) = capture_to_thumb(&src, w, h, stride).expect("thumb");
            assert_eq!((ow, oh), (tw, th));
            assert_eq!(px.len(), (tw * th * 4) as usize);
            assert!(px.iter().all(|&b| b == 0x80));
        }
    }

    #[test]
    fn small_source_is_copied_not_upscaled() {
        let src: Vec<u8> = (0..(3 * 2 * 4)).map(|i| i as u8).collect();
        let (w, h, px) = capture_to_thumb(&src, 3, 2, 12).expect("thumb");
        assert_eq!((w, h), (3, 2));
        assert_eq!(px, src);
    }

    #[test]
    fn malformed_input_never_panics() {
        assert_eq!(capture_to_thumb(&[], 0, 0, 0), None);
        assert_eq!(capture_to_thumb(&[0; 64], 0, 4, 16), None);
        assert_eq!(capture_to_thumb(&[0; 64], 4, 0, 16), None);
        assert_eq!(
            capture_to_thumb(&[0; 64], 4, 4, 15),
            None,
            "stride below row width"
        );
        assert_eq!(capture_to_thumb(&[0; 63], 4, 4, 16), None, "short buffer");
        assert_eq!(downscale_box(&[0; 16], 2, 2, 8, 0, 1), None);
        assert_eq!(downscale_box(&[0; 16], 2, 2, 8, 3, 1), None, "no upscale");
        assert_eq!(
            downscale_box(&[0; 16], 2, 2, usize::MAX, 1, 1),
            None,
            "stride overflow"
        );
    }

    #[test]
    fn stride_padding_on_last_row_is_not_required() {
        // Buffer ends at the last row's pixel data; no trailing pad needed.
        let src = [1u8, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(downscale_box(&src, 1, 2, 4, 1, 1), Some(vec![3, 4, 5, 6]));
    }

    #[cfg(test)]
    mod bench {
        use super::super::*;
        use std::time::Instant;
        use x11rb::COPY_FROM_PARENT;
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{
            ConnectionExt, CreateGCAux, CreateWindowAux, ImageFormat, WindowClass,
        };

        /// Measures `XGetImage` + downscale + cache insert on a mapped, unobscured window of known
        /// pattern. Run: `DISPLAY=:520 cargo test --release -p flamewm-wm-x11 -- --ignored --nocapture`.
        #[test]
        #[ignore = "needs DISPLAY; measurement only"]
        fn measure_capture_to_thumb_cost() {
            let (conn, screen_num) = x11rb::connect(None).expect("DISPLAY");
            let screen = conn.setup().roots[screen_num].clone();
            let depth = screen.root_depth;
            for &(w, h) in &[(640u16, 400u16), (1280, 800), (1920, 1080)] {
                let win = conn.generate_id().unwrap();
                conn.create_window(
                    depth,
                    win,
                    screen.root,
                    0,
                    0,
                    w,
                    h,
                    0,
                    WindowClass::INPUT_OUTPUT,
                    COPY_FROM_PARENT,
                    &CreateWindowAux::new(),
                )
                .unwrap();
                conn.map_window(win).unwrap();
                let gc = conn.generate_id().unwrap();
                conn.create_gc(gc, win, &CreateGCAux::new()).unwrap();
                let (wu, hu) = (usize::from(w), usize::from(h));
                let mut rows = vec![0u8; wu * 16 * 4];
                for y0 in (0..hu).step_by(16) {
                    let y1 = (y0 + 16).min(hu);
                    for (ry, y) in (y0..y1).enumerate() {
                        for x in 0..wu {
                            let o = (ry * wu + x) * 4;
                            rows[o..o + 4].copy_from_slice(&[
                                (x * 7) as u8,
                                (y * 5) as u8,
                                ((x ^ y) & 0xff) as u8,
                                0,
                            ]);
                        }
                    }
                    let n = (y1 - y0) * wu * 4;
                    conn.put_image(
                        ImageFormat::Z_PIXMAP,
                        win,
                        gc,
                        w,
                        (y1 - y0) as u16,
                        0,
                        y0 as i16,
                        0,
                        depth,
                        &rows[..n],
                    )
                    .unwrap();
                }
                conn.get_input_focus().unwrap().reply().unwrap();

                let key = ThumbKey {
                    window: win,
                    src_w: u32::from(w),
                    src_h: u32::from(h),
                };
                let mut cache: ThumbCache<(u32, u32, Vec<u8>)> = ThumbCache::default();
                let mut times = Vec::new();
                let mut last = (0, 0);
                for i in 0..=50 {
                    let t = Instant::now();
                    let img = conn
                        .get_image(ImageFormat::Z_PIXMAP, win, 0, 0, w, h, !0)
                        .unwrap()
                        .reply()
                        .unwrap();
                    let stride = img.data.len() / usize::from(h);
                    let thumb = capture_to_thumb(&img.data, u32::from(w), u32::from(h), stride)
                        .expect("thumb");
                    last = (thumb.0, thumb.1);
                    cache.insert(key, (thumb.0, thumb.1, thumb.2));
                    let el = t.elapsed().as_secs_f64() * 1e3;
                    if i > 0 {
                        times.push(el);
                    }
                }
                times.sort_by(|a, b| a.partial_cmp(b).unwrap());
                println!(
                    "THUMB_BENCH src={w}x{h} n=50 median_ms={:.3} min_ms={:.3} max_ms={:.3} thumb={}x{} stride={} resident={}",
                    times[25],
                    times[0],
                    times[49],
                    last.0,
                    last.1,
                    usize::from(w) * 4,
                    cache.len()
                );
                conn.free_gc(gc).unwrap();
                conn.destroy_window(win).unwrap();
                conn.get_input_focus().unwrap().reply().unwrap();
            }
        }
    }
}

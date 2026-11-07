/// Root-space rect the wallpaper is actually composed from: the intersection of `rect` with
/// the root bounds, or a 1x1 anchor at the nearest root pixel when `rect` lies fully outside.
/// The result is always non-empty and inside the root, so the compose never misses.
#[must_use]
pub fn root_anchor_rect(rect: (i32, i32, u32, u32), root: (u32, u32)) -> (i32, i32, u32, u32) {
    let (rx, ry, w, h) = (
        i64::from(rect.0),
        i64::from(rect.1),
        i64::from(rect.2),
        i64::from(rect.3),
    );
    let (root_w, root_h) = (i64::from(root.0.max(1)), i64::from(root.1.max(1)));
    let (x0, y0) = (rx.max(0), ry.max(0));
    let (x1, y1) = ((rx + w).min(root_w), (ry + h).min(root_h));
    if x0 < x1 && y0 < y1 {
        return (x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32);
    }
    let (ax, ay) = (rx.clamp(0, root_w - 1), ry.clamp(0, root_h - 1));
    (ax as i32, ay as i32, 1, 1)
}

/// Fills a `win_w`x`win_h` RGBA buffer from `src` (`src_w`x`src_h`). Window pixel `(i, j)` reads
/// source pixel `(i + shift.0, j + shift.1)`, clamped to the source edge, so out-of-source
/// regions replicate the nearest wallpaper edge and never come out as opaque black.
#[must_use]
pub fn edge_extend_rgba(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    shift: (i64, i64),
    win: (u32, u32),
) -> Vec<u8> {
    let (sw, sh) = (i64::from(src_w.max(1)), i64::from(src_h.max(1)));
    let mut out = vec![0u8; win.0 as usize * win.1 as usize * 4];
    if src.len() < (sw * sh * 4) as usize {
        return out;
    }
    for (j, row) in out.chunks_exact_mut(win.0 as usize * 4).enumerate() {
        let sy = (j as i64 + shift.1).clamp(0, sh - 1) as usize;
        let src_row = &src[sy * sw as usize * 4..(sy + 1) * sw as usize * 4];
        for (i, px) in row.chunks_exact_mut(4).enumerate() {
            let sx = (i as i64 + shift.0).clamp(0, sw - 1) as usize;
            px.copy_from_slice(&src_row[sx * 4..sx * 4 + 4]);
        }
    }
    out
}

/// Source index for destination index `i` when a `n`-long source sits at `origin` in the
/// destination. Positions outside the source reflect back across the nearest edge (period `2n`),
/// so a grown strip continues the existing content with no seam and no black.
#[must_use]
pub fn mirror_index(i: i64, origin: i64, n: i64) -> usize {
    if n <= 1 {
        return 0;
    }
    let u = (i - origin).rem_euclid(2 * n);
    let idx = if u < n { u } else { 2 * n - 1 - u };
    idx as usize
}

/// Mirror-extends a `src_w`x`src_h` ARGB snapshot over a `dst_w`x`dst_h` canvas. The snapshot's
/// top-left lands at `origin`; every other pixel is the reflected snapshot, so growing the window
/// right/bottom (origin 0) or left/top (origin = the snapshot's offset) keeps the join seamless.
#[must_use]
pub fn mirror_extend_argb(
    src: &[u32],
    src_w: u32,
    src_h: u32,
    dst: (u32, u32),
    origin: (i64, i64),
) -> Vec<u32> {
    let (sw, sh) = (i64::from(src_w), i64::from(src_h));
    let mut out = vec![0u32; dst.0 as usize * dst.1 as usize];
    if sw == 0 || sh == 0 || src.len() < (sw * sh) as usize {
        return out;
    }
    for (j, row) in out.chunks_exact_mut(dst.0 as usize).enumerate() {
        let sy = mirror_index(j as i64, origin.1, sh);
        let src_row = &src[sy * sw as usize..(sy + 1) * sw as usize];
        for (i, px) in row.iter_mut().enumerate() {
            *px = src_row[mirror_index(i as i64, origin.0, sw)];
        }
    }
    out
}

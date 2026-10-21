//! Pure ARGB32 helpers: straight RGBA8 -> premultiplied ARGB32.
//! The X11 upload path in `app/images.rs` uses [`scale_and_premultiply`] so the
//! cached depth-32 pixmap holds premultiplied content for PictOpOver.

/// Nearest-neighbor scale of straight RGBA8, then premultiply to ARGB32.
pub fn scale_and_premultiply(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) -> Vec<u32> {
    let mut out = Vec::new();
    scale_and_premultiply_into(&mut out, src, src_w, src_h, dst_w, dst_h);
    out
}

/// Same as [`scale_and_premultiply`], reusing `out`'s capacity for hot per-paint callers.
pub fn scale_and_premultiply_into(
    out: &mut Vec<u32>,
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
) {
    let (sw, sh, dw, dh) = (src_w.max(1), src_h.max(1), dst_w.max(1), dst_h.max(1));
    out.clear();
    out.reserve((dw as usize) * (dh as usize));
    for y in 0..dh {
        let sy = ((y as u64 * src_h as u64) / dh as u64).min((sh - 1) as u64) as usize;
        for x in 0..dw {
            let sx = ((x as u64 * src_w as u64) / dw as u64).min((sw - 1) as u64) as usize;
            let off = (sy * sw as usize + sx) * 4;
            let (r, g, b, a) = (
                u32::from(src[off]),
                u32::from(src[off + 1]),
                u32::from(src[off + 2]),
                u32::from(src[off + 3]),
            );
            let prem = |c: u32| ((c * a + 127) / 255) as u32;
            out.push((a << 24) | (prem(r) << 16) | (prem(g) << 8) | prem(b));
        }
    }
    let _ = (sw, sh);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_nearest_neighbor_picks_source_texel() {
        let src = vec![255, 0, 0, 255, 0, 0, 255, 255];
        let out = scale_and_premultiply(&src, 2, 1, 1, 1);
        assert_eq!(out, vec![0xFFFF0000]);
    }

    #[test]
    fn scale_into_reused_buffer_matches_fresh_output() {
        let src = vec![10, 20, 30, 128, 255, 0, 0, 255, 0, 0, 0, 0, 1, 2, 3, 64];
        let fresh = scale_and_premultiply(&src, 2, 2, 3, 1);
        let mut reused = vec![0xDEAD_BEEF; 64];
        scale_and_premultiply_into(&mut reused, &src, 2, 2, 3, 1);
        assert_eq!(reused, fresh);
        scale_and_premultiply_into(&mut reused, &src, 2, 2, 3, 1);
        assert_eq!(reused, fresh);
    }
}

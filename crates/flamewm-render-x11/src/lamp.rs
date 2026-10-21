//! Magic Lamp bend model: pure band geometry for the dock-side cubic deformation.
//!
//! Each source band is a full-width row (bottom/top panel) or full-height column (left/right
//! panel). Boundaries along the band axis are evaluated once and shared by both neighbours,
//! so bands tile without gaps. A band's lateral scale and offset come from `r` at its mid
//! line, so every band is an affine map of its source rows (or columns).

use crate::warp::WarpRect;

/// Edge of the screen the taskbar icon sits on; the window is pulled toward it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelEdge {
    Bottom,
    Top,
    Left,
    Right,
}

/// One source band and its destination. `src` is `(x, y, w, h)` local to the window;
/// `dst` is `(x, y, w, h)` in root coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LampBand {
    pub src: (f64, f64, f64, f64),
    pub dst: (f64, f64, f64, f64),
}

const EPS: f64 = 1e-9;
/// Target source-band thickness in pixels; the band count is `ceil(extent / BAND_PX)`.
const BAND_PX: f64 = 6.0;
const MAX_BANDS: usize = 128;

/// `|n / d|` clamped to `0..=1`; `fallback` (finite) is used when `d` is numerically zero.
fn satabs(n: f64, d: f64, fallback: f64) -> f64 {
    if d.abs() > EPS {
        (n / d).abs().clamp(0.0, 1.0)
    } else {
        fallback.clamp(0.0, 1.0)
    }
}

/// Boundary position `e` (along the band axis, window-local) and lateral factor `r` for the
/// along coordinate `s` (window-local) at progress `p` in `0..=1`.
fn eval(edge: PanelEdge, window: WarpRect, icon: WarpRect, p: f64, s: f64) -> (f64, f64) {
    let (x0, y0) = (window.x, window.y);
    let (w, h) = (window.width, window.height);
    match edge {
        PanelEdge::Bottom => {
            let q = (s + (h - s) * p) / h;
            let pq3 = p * q * q * q;
            let d = (icon.y - y0 + s) * pq3;
            let e = (icon.y - y0).min(s + d);
            (e, satabs(d, icon.y - y0 - s, pq3))
        }
        PanelEdge::Top => {
            let q = (h - s + s * p) / h;
            let pq3 = p * q * q * q;
            let d = (y0 - icon.height + h + s - icon.y) * pq3;
            let r = satabs(d, y0 - icon.height + h - icon.y - (h - s), pq3);
            ((icon.y + icon.height - y0).max(s - d), r)
        }
        PanelEdge::Right => {
            let q = (s + (w - s) * p) / w;
            let pq3 = p * q * q * q;
            let d = (icon.x - x0 + s) * pq3;
            let e = (icon.x - x0).min(s + d);
            (e, satabs(d, icon.x - x0 - s, pq3))
        }
        PanelEdge::Left => {
            let q = (w - s + s * p) / w;
            let pq3 = p * q * q * q;
            let d = (x0 - icon.width + w + s - icon.x) * pq3;
            let r = satabs(d, x0 - icon.width + w - icon.x - (w - s), pq3);
            ((icon.x + icon.width - x0).max(s - d), r)
        }
    }
}

fn is_finite_rect(rect: WarpRect) -> bool {
    [rect.x, rect.y, rect.width, rect.height]
        .iter()
        .all(|v| v.is_finite())
}

/// Chooses the panel edge for an icon: beside the window on that side, otherwise the screen
/// edge nearest the icon (ties and a degenerate screen give [`PanelEdge::Bottom`]).
#[must_use]
pub fn edge_for(window: WarpRect, icon: WarpRect, screen: WarpRect) -> PanelEdge {
    if icon.y >= window.y + window.height {
        PanelEdge::Bottom
    } else if icon.y + icon.height <= window.y {
        PanelEdge::Top
    } else if icon.x >= window.x + window.width {
        PanelEdge::Right
    } else if icon.x + icon.width <= window.x {
        PanelEdge::Left
    } else if !is_finite_rect(screen) || screen.width <= 0.0 || screen.height <= 0.0 {
        PanelEdge::Bottom
    } else {
        let gaps = [
            (
                PanelEdge::Bottom,
                screen.y + screen.height - (icon.y + icon.height),
            ),
            (PanelEdge::Top, icon.y - screen.y),
            (
                PanelEdge::Right,
                screen.x + screen.width - (icon.x + icon.width),
            ),
            (PanelEdge::Left, icon.x - screen.x),
        ];
        gaps.iter()
            .fold(
                gaps[0],
                |best, cur| if cur.1 < best.1 { *cur } else { best },
            )
            .0
    }
}

/// Splits the bend at progress `p` (0 = open, 1 = minimized) into horizontal bands for a
/// bottom/top panel, or vertical bands for a left/right panel.
///
/// `band_extent_px` is the window length along the band axis (height for bottom/top, width
/// for left/right); the band count is `ceil(extent / 6)`, capped at 128.
///
/// Errors on non-finite or empty window rects, a negative icon size, a non-finite `p` or
/// extent. `p` is clamped to `0..=1`. Output is always finite.
pub fn lamp_bands(
    window: WarpRect,
    icon: WarpRect,
    edge: PanelEdge,
    p: f64,
    band_extent_px: f64,
) -> Result<Vec<LampBand>, String> {
    if !is_finite_rect(window) || window.width <= 0.0 || window.height <= 0.0 {
        return Err("lamp: window rect must be finite and non-empty".to_string());
    }
    if !is_finite_rect(icon) || icon.width < 0.0 || icon.height < 0.0 {
        return Err("lamp: icon rect must be finite with a non-negative size".to_string());
    }
    if !p.is_finite() {
        return Err("lamp: progress must be finite".to_string());
    }
    if !band_extent_px.is_finite() || band_extent_px <= 0.0 {
        return Err("lamp: band extent must be finite and positive".to_string());
    }
    let p = p.clamp(0.0, 1.0);
    let vertical = matches!(edge, PanelEdge::Left | PanelEdge::Right);
    // Along axis: bands are rows (bottom/top) or columns (left/right).
    let (along_len, along_origin, lat_len, lat_origin, lat_off, lat_icon) = if vertical {
        (
            window.width,
            window.x,
            window.height,
            window.y,
            icon.y - window.y,
            icon.height,
        )
    } else {
        (
            window.height,
            window.y,
            window.width,
            window.x,
            icon.x - window.x,
            icon.width,
        )
    };
    let count = (band_extent_px / BAND_PX)
        .ceil()
        .clamp(1.0, MAX_BANDS as f64) as usize;

    let mut bands = Vec::with_capacity(count);
    let mut s0 = 0.0_f64;
    let (mut e0, _) = eval(edge, window, icon, p, s0);
    for k in 0..count {
        let s1 = along_len * (k + 1) as f64 / count as f64;
        // Each boundary is evaluated once; the next band reuses `e1` as its start.
        let (e1, _) = eval(edge, window, icon, p, s1);
        let (_, r) = eval(edge, window, icon, p, 0.5 * (s0 + s1));
        let lat_start = lat_origin + r * lat_off;
        let lat_size = lat_len - r * (lat_len - lat_icon);
        let along_start = along_origin + e0;
        let along_size = e1 - e0;
        let band = if vertical {
            LampBand {
                src: (s0, 0.0, s1 - s0, window.height),
                dst: (along_start, lat_start, along_size, lat_size),
            }
        } else {
            LampBand {
                src: (0.0, s0, window.width, s1 - s0),
                dst: (lat_start, along_start, lat_size, along_size),
            }
        };
        bands.push(band);
        s0 = s1;
        e0 = e1;
    }
    Ok(bands)
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: WarpRect = WarpRect {
        x: 300.0,
        y: 100.0,
        width: 800.0,
        height: 600.0,
    };
    const I: WarpRect = WarpRect {
        x: 650.0,
        y: 900.0,
        width: 48.0,
        height: 48.0,
    };
    const SCREEN: WarpRect = WarpRect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };
    const PROGRESS: [f64; 9] = [0.0, 0.01, 0.05, 0.25, 0.5, 0.75, 0.95, 0.99, 1.0];

    fn bands(edge: PanelEdge, p: f64) -> Vec<LampBand> {
        let extent = if matches!(edge, PanelEdge::Left | PanelEdge::Right) {
            G.width
        } else {
            G.height
        };
        lamp_bands(G, I, edge, p, extent).unwrap()
    }

    #[test]
    fn identity_at_zero_progress_for_all_edges() {
        // Icons placed beside the window on each side so the identity holds for every edge.
        let cases = [
            (
                PanelEdge::Bottom,
                WarpRect {
                    x: 650.0,
                    y: 900.0,
                    width: 48.0,
                    height: 48.0,
                },
            ),
            (
                PanelEdge::Top,
                WarpRect {
                    x: 650.0,
                    y: 20.0,
                    width: 48.0,
                    height: 48.0,
                },
            ),
            (
                PanelEdge::Right,
                WarpRect {
                    x: 1200.0,
                    y: 400.0,
                    width: 48.0,
                    height: 48.0,
                },
            ),
            (
                PanelEdge::Left,
                WarpRect {
                    x: 20.0,
                    y: 400.0,
                    width: 48.0,
                    height: 48.0,
                },
            ),
        ];
        for (edge, icon) in cases {
            let extent = if matches!(edge, PanelEdge::Left | PanelEdge::Right) {
                G.width
            } else {
                G.height
            };
            let list = lamp_bands(G, icon, edge, 0.0, extent).unwrap();
            let mut covered = 0.0;
            for band in &list {
                let (sx, sy, sw, sh) = band.src;
                let (dx, dy, dw, dh) = band.dst;
                match edge {
                    PanelEdge::Bottom | PanelEdge::Top => {
                        assert_eq!((dx, sw), (G.x, G.width), "{edge:?}");
                        assert_eq!(sx, 0.0);
                        assert!((dy - (G.y + sy)).abs() < 1e-9, "{edge:?} row y");
                        assert!((dh - sh).abs() < 1e-9, "{edge:?} row h");
                        covered += dh;
                    }
                    PanelEdge::Left | PanelEdge::Right => {
                        assert_eq!((dy, sh), (G.y, G.height), "{edge:?}");
                        assert_eq!(sy, 0.0);
                        assert!((dx - (G.x + sx)).abs() < 1e-9, "{edge:?} col x");
                        assert!((dw - sw).abs() < 1e-9, "{edge:?} col w");
                        covered += dw;
                    }
                }
            }
            let extent_covered = if matches!(edge, PanelEdge::Left | PanelEdge::Right) {
                G.width
            } else {
                G.height
            };
            assert!(
                (covered - extent_covered).abs() < 1e-9,
                "{edge:?} covers the window"
            );
        }
    }

    #[test]
    fn half_progress_bottom_top_row_rises_and_bottom_row_clips_at_icon() {
        let list = bands(PanelEdge::Bottom, 0.5);
        let top = list.first().unwrap();
        // Top row: q = 0.5, d = 50 -> root y = 100 + 50 = 150.
        assert!((top.dst.1 - 150.0).abs() < 1e-6, "top row y {}", top.dst.1);
        let bottom = list.last().unwrap();
        // Bottom row clipped to the icon top: local 800, root 900.
        assert!(
            (bottom.dst.1 + bottom.dst.3 - 900.0).abs() < 1e-6,
            "bottom row edge"
        );
    }

    #[test]
    fn dock_side_rows_contract_more_than_far_rows() {
        let list = bands(PanelEdge::Bottom, 0.5);
        let far = list.first().unwrap().dst.2;
        let dock = list.last().unwrap().dst.2;
        assert!(
            dock < far,
            "dock row {dock} must be narrower than far row {far}"
        );
        assert!(
            G.width - dock > 2.0 * (G.width - far),
            "contraction is not a rigid scale"
        );
    }

    #[test]
    fn neighbouring_band_edges_meet_within_a_millionth_of_a_pixel() {
        for edge in [
            PanelEdge::Bottom,
            PanelEdge::Top,
            PanelEdge::Left,
            PanelEdge::Right,
        ] {
            for p in PROGRESS {
                let list = bands(edge, p);
                for pair in list.windows(2) {
                    let (a, b) = (pair[0].dst, pair[1].dst);
                    // Start + size is one rounding away from the next start; the painter rounds
                    // each shared boundary once, so the tolerance is far below a pixel.
                    let (a_end, b_start) = if matches!(edge, PanelEdge::Left | PanelEdge::Right) {
                        (a.0 + a.2, b.0)
                    } else {
                        (a.1 + a.3, b.1)
                    };
                    assert!(
                        (a_end - b_start).abs() < 1e-9,
                        "{edge:?} p={p}: {a_end} vs {b_start}"
                    );
                }
            }
        }
    }

    #[test]
    fn values_stay_finite_for_sample_progress_and_degenerate_inputs() {
        for edge in [
            PanelEdge::Bottom,
            PanelEdge::Top,
            PanelEdge::Left,
            PanelEdge::Right,
        ] {
            for p in PROGRESS {
                for band in bands(edge, p) {
                    for v in [
                        band.src.0, band.src.1, band.src.2, band.src.3, band.dst.0, band.dst.1,
                        band.dst.2, band.dst.3,
                    ] {
                        assert!(v.is_finite(), "{edge:?} p={p}");
                    }
                }
            }
        }
        let zero_icon = WarpRect {
            width: 0.0,
            height: 0.0,
            ..I
        };
        let overlap = WarpRect {
            x: 400.0,
            y: 500.0,
            width: 200.0,
            height: 200.0,
        };
        let edge_icon = WarpRect {
            x: 1000.0,
            y: 1000.0,
            width: 48.0,
            height: 48.0,
        };
        let fourk = WarpRect {
            x: 0.0,
            y: 0.0,
            width: 3840.0,
            height: 2160.0,
        };
        let fourk_icon = WarpRect {
            x: 3800.0,
            y: 2100.0,
            width: 40.0,
            height: 40.0,
        };
        let cases = [
            (G, zero_icon),
            (G, overlap),
            (G, edge_icon),
            (fourk, fourk_icon),
        ];
        for (window, icon) in cases {
            for edge in [
                PanelEdge::Bottom,
                PanelEdge::Top,
                PanelEdge::Left,
                PanelEdge::Right,
            ] {
                let extent = if matches!(edge, PanelEdge::Left | PanelEdge::Right) {
                    window.width
                } else {
                    window.height
                };
                for p in PROGRESS {
                    let list = lamp_bands(window, icon, edge, p, extent).unwrap();
                    assert!(!list.is_empty() && list.len() <= MAX_BANDS);
                    for band in list {
                        assert!(band.dst.0.is_finite() && band.dst.1.is_finite());
                        assert!(band.dst.2.is_finite() && band.dst.3.is_finite());
                    }
                }
            }
        }
    }

    #[test]
    fn rejects_empty_or_non_finite_inputs() {
        let flat = WarpRect { height: 0.0, ..G };
        assert!(lamp_bands(flat, I, PanelEdge::Bottom, 0.5, 600.0).is_err());
        assert!(lamp_bands(G, I, PanelEdge::Bottom, f64::NAN, 600.0).is_err());
        assert!(lamp_bands(G, I, PanelEdge::Bottom, 0.5, 0.0).is_err());
        let negative = WarpRect { width: -1.0, ..I };
        assert!(lamp_bands(G, negative, PanelEdge::Bottom, 0.5, 600.0).is_err());
    }

    #[test]
    fn band_count_is_adaptive_and_capped() {
        assert_eq!(
            lamp_bands(G, I, PanelEdge::Bottom, 0.5, 600.0)
                .unwrap()
                .len(),
            100
        );
        assert_eq!(
            lamp_bands(G, I, PanelEdge::Bottom, 0.5, 2160.0)
                .unwrap()
                .len(),
            MAX_BANDS
        );
        assert_eq!(
            lamp_bands(G, I, PanelEdge::Bottom, 0.5, 1.0).unwrap().len(),
            1
        );
    }

    #[test]
    fn progress_is_clamped_to_unit_interval() {
        assert_eq!(
            bands(PanelEdge::Bottom, -4.0),
            bands(PanelEdge::Bottom, 0.0)
        );
        assert_eq!(bands(PanelEdge::Bottom, 7.0), bands(PanelEdge::Bottom, 1.0));
    }

    #[test]
    fn edge_for_follows_icon_side_and_falls_back_to_bottom() {
        let below = WarpRect {
            x: 650.0,
            y: 900.0,
            width: 48.0,
            height: 48.0,
        };
        let above = WarpRect {
            x: 650.0,
            y: 20.0,
            width: 48.0,
            height: 48.0,
        };
        let right = WarpRect {
            x: 1200.0,
            y: 400.0,
            width: 48.0,
            height: 48.0,
        };
        let left = WarpRect {
            x: 20.0,
            y: 400.0,
            width: 48.0,
            height: 48.0,
        };
        assert_eq!(edge_for(G, below, SCREEN), PanelEdge::Bottom);
        assert_eq!(edge_for(G, above, SCREEN), PanelEdge::Top);
        assert_eq!(edge_for(G, right, SCREEN), PanelEdge::Right);
        assert_eq!(edge_for(G, left, SCREEN), PanelEdge::Left);
        let overlapping = WarpRect {
            x: 500.0,
            y: 300.0,
            width: 48.0,
            height: 48.0,
        };
        // Overlapping the window: nearest screen edge (top, 300 px away) wins.
        assert_eq!(edge_for(G, overlapping, SCREEN), PanelEdge::Top);
        assert_eq!(
            edge_for(
                G,
                overlapping,
                WarpRect {
                    width: 0.0,
                    ..SCREEN
                }
            ),
            PanelEdge::Bottom
        );
    }
}

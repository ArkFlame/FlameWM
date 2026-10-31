//! Shared radial geometry and cursor-sector highlight for the Convert wheel.
//! UI hit testing and the hover raster use *exactly the same* annulus predicate.
//! The wheel is otherwise a standard surface-coloured `FlameWM` popup.
use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;

pub const WHEEL_SIZE: u32 = 328;
const CENTER: f32 = 164.0;
const INNER_RADIUS: f32 = 54.0;
const OUTER_RADIUS: f32 = 160.0;
const SECTOR_STEP: f32 = PI / 4.0;
const HALF_ARC: f32 = PI / 8.0 - 0.045;

/// Returns the clockwise sector from 12 o'clock. The hub and transparent
/// exterior have no selection, and all eight slices share one predicate.
pub fn sector_at(x: f32, y: f32) -> Option<usize> {
    sector_within(x, y, 0.0)
}

/// Same predicate with the annulus and each wedge grown by `margin` pixels.
/// Margin zero is the exact hit area; a positive margin draws the accent glow.
fn sector_within(x: f32, y: f32, margin: f32) -> Option<usize> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let dx = x - CENTER;
    let dy = y - CENTER;
    let radius_sq = dx * dx + dy * dy;
    let inner = INNER_RADIUS - margin;
    let outer = OUTER_RADIUS + margin;
    if !(inner * inner..=outer * outer).contains(&radius_sq) {
        return None;
    }
    let angle = (dy.atan2(dx) + PI / 2.0).rem_euclid(TAU);
    let sector = ((angle + SECTOR_STEP / 2.0) / SECTOR_STEP).floor() as usize % 8;
    let center = sector as f32 * SECTOR_STEP;
    let distance = (angle - center + PI).rem_euclid(TAU) - PI;
    // Angular margin = arc length / mid-ring radius.
    let half_arc = HALF_ARC + margin / ((INNER_RADIUS + OUTER_RADIUS) / 2.0);
    if distance.abs() > half_arc {
        return None;
    }
    let edge = if distance >= 0.0 { 1.0 } else { -1.0 };
    let r = CORNER_RADIUS + margin;
    let center_angle = sector as f32 * SECTOR_STEP;
    let cut = corner_cut(x, y, center_angle, distance, edge, outer, half_arc, r, true)
        || corner_cut(
            x,
            y,
            center_angle,
            distance,
            edge,
            inner,
            half_arc,
            r,
            false,
        );
    (!cut).then_some(sector)
}

/// Corner radius of each wedge, in pixels. Each corner is a fillet tangent to the
/// radial edge and to its arc, so the wedge keeps its straight edges and gaps.
const CORNER_RADIUS: f32 = 9.0;

/// True when a point lies in the cut-away region of one rounded corner. `outer` selects the
/// outer arc corners; the inner corners use the inner arc.
#[allow(clippy::too_many_arguments)]
fn corner_cut(
    x: f32,
    y: f32,
    center_angle: f32,
    distance: f32,
    edge: f32,
    arc: f32,
    half_arc: f32,
    r: f32,
    outer: bool,
) -> bool {
    let fillet_radius = if outer { arc - r } else { arc + r };
    if fillet_radius <= r {
        return false;
    }
    let offset = half_arc - (r / fillet_radius).asin();
    let radius = ((x - CENTER).powi(2) + (y - CENTER).powi(2)).sqrt();
    let beyond = edge * distance > offset;
    let past_centre = if outer {
        radius > fillet_radius
    } else {
        radius < fillet_radius
    };
    if !beyond || !past_centre {
        return false;
    }
    let angle = center_angle + edge * offset;
    let cx = CENTER + fillet_radius * angle.sin();
    let cy = CENTER - fillet_radius * angle.cos();
    (x - cx).powi(2) + (y - cy).powi(2) > r * r
}

/// Glow width outside the wedge, in pixels, and its straight-alpha value.
const GLOW_MARGIN: f32 = 6.0;
const GLOW_ALPHA: u8 = 34;

/// The centre disc is not a segment; a click there cycles the wheel mode.
#[must_use]
pub fn in_hub(x: f32, y: f32) -> bool {
    if !x.is_finite() || !y.is_finite() {
        return false;
    }
    let dx = x - CENTER;
    let dy = y - CENTER;
    dx * dx + dy * dy < INNER_RADIUS * INNER_RADIUS
}

const NO_SECTOR: u8 = u8::MAX;

/// Per pixel, the sector hit by each of the four antialiasing subsamples (or `NO_SECTOR`).
/// Retaining 4 bytes per pixel instead of eight 8-bit masks halves the idle footprint; the
/// alpha of a sector is recovered exactly as `hits * 23` (= `hits * 92 / 4`).
static SECTOR_SAMPLES: OnceLock<Vec<[u8; 4]>> = OnceLock::new();

fn sector_samples() -> &'static [[u8; 4]] {
    SECTOR_SAMPLES.get_or_init(|| {
        let mut samples = Vec::with_capacity((WHEEL_SIZE * WHEEL_SIZE) as usize);
        for y in 0..WHEEL_SIZE {
            for x in 0..WHEEL_SIZE {
                // Four subsamples give antialiased rounded arc edges.
                samples.push(
                    [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)].map(|(ox, oy)| {
                        sector_at(x as f32 + ox, y as f32 + oy).map_or(NO_SECTOR, |s| s as u8)
                    }),
                );
            }
        }
        samples
    })
}

/// Only the hovered fraction is painted: no permanent triangles or dividers.
/// RGB is the live Settings accent; straight alpha has antialiased edges.
pub fn highlight_pixels(sector: usize, accent: [u8; 3]) -> Vec<u8> {
    let mut pixels = vec![0; (WHEEL_SIZE * WHEEL_SIZE * 4) as usize];
    if sector >= 8 {
        return pixels;
    }
    for (out, hits) in pixels.chunks_exact_mut(4).zip(sector_samples()) {
        let count = hits
            .iter()
            .filter(|hit| usize::from(**hit) == sector)
            .count();
        if count != 0 {
            out[..3].copy_from_slice(&accent);
            out[3] = (count * 23) as u8;
        }
    }
    // Soft accent halo just outside the wedge; never painted under the wedge.
    for (index, out) in pixels.chunks_exact_mut(4).enumerate() {
        if out[3] != 0 {
            continue;
        }
        let x = (index % WHEEL_SIZE as usize) as f32 + 0.5;
        let y = (index / WHEEL_SIZE as usize) as f32 + 0.5;
        if sector_within(x, y, GLOW_MARGIN) == Some(sector) {
            out[..3].copy_from_slice(&accent);
            out[3] = GLOW_ALPHA;
        }
    }
    pixels
}

/// Which set of eight segments the wheel shows. Modifiers pick a mode while
/// the pointer is over the wheel; clicking the centre cycles through them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WheelMode {
    #[default]
    Convert,
    Tools,
    Actions,
}

impl WheelMode {
    /// Shift selects Convert, Ctrl selects Tools and Alt selects Actions.
    /// Shift wins when several are held; no modifier keeps the current mode.
    #[must_use]
    pub const fn from_modifiers(shift: bool, ctrl: bool, alt: bool) -> Option<Self> {
        if shift {
            Some(Self::Convert)
        } else if ctrl {
            Some(Self::Tools)
        } else if alt {
            Some(Self::Actions)
        } else {
            None
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Convert => Self::Tools,
            Self::Tools => Self::Actions,
            Self::Actions => Self::Convert,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Convert => "Convert",
            Self::Tools => "Tools",
            Self::Actions => "Actions",
        }
    }
}

/// Operations offered in Actions mode, in wheel order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelAction {
    Open,
    Reveal,
    Rename,
    Shortcut,
    Trash,
}

impl WheelAction {
    pub const ALL: [Self; 5] = [
        Self::Open,
        Self::Reveal,
        Self::Rename,
        Self::Shortcut,
        Self::Trash,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Reveal => "Reveal",
            Self::Rename => "Rename",
            Self::Shortcut => "Shortcut",
            Self::Trash => "Trash",
        }
    }
}

#[cfg(test)]
mod mode_tests {
    use super::*;
    #[test]
    fn modifiers_select_modes_with_shift_first() {
        assert_eq!(
            WheelMode::from_modifiers(true, false, false),
            Some(WheelMode::Convert)
        );
        assert_eq!(
            WheelMode::from_modifiers(false, true, false),
            Some(WheelMode::Tools)
        );
        assert_eq!(
            WheelMode::from_modifiers(false, false, true),
            Some(WheelMode::Actions)
        );
        assert_eq!(
            WheelMode::from_modifiers(true, true, true),
            Some(WheelMode::Convert)
        );
        assert_eq!(WheelMode::from_modifiers(false, false, false), None);
    }
    #[test]
    fn centre_click_cycles_every_mode_back_to_convert() {
        let mut mode = WheelMode::default();
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(mode.label());
            mode = mode.next();
        }
        assert_eq!(seen, ["Convert", "Tools", "Actions"]);
        assert_eq!(mode, WheelMode::Convert);
    }
    #[test]
    fn actions_fit_eight_segments() {
        assert!(WheelAction::ALL.len() <= 8);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn center_edges_and_octants() {
        assert_eq!(sector_at(164.0, 164.0), None);
        assert_eq!(sector_at(164.0, 0.0), None);
        assert_eq!(sector_at(164.0, 64.0), Some(0));
        assert_eq!(sector_at(235.0, 93.0), Some(1));
        assert_eq!(sector_at(264.0, 164.0), Some(2));
        assert_eq!(sector_at(235.0, 235.0), Some(3));
        assert_eq!(sector_at(164.0, 264.0), Some(4));
        assert_eq!(sector_at(93.0, 235.0), Some(5));
        assert_eq!(sector_at(64.0, 164.0), Some(6));
        assert_eq!(sector_at(93.0, 93.0), Some(7));
    }
    #[test]
    fn hub_disc_is_inside_the_inner_radius_only() {
        assert!(in_hub(164.0, 164.0));
        assert!(!in_hub(164.0, 0.0));
        assert!(!in_hub(164.0, 164.0 + INNER_RADIUS));
    }
    #[test]
    fn wedge_corners_are_rounded_but_the_body_stays_hit() {
        // Sector 2 is centred on 3 o'clock. Its outer corner tip is cut away.
        let half = HALF_ARC;
        let tip_angle = SECTOR_STEP * 2.0 + half - 0.001;
        let tip_radius = OUTER_RADIUS - 0.1;
        let tip_x = CENTER + tip_radius * tip_angle.sin();
        let tip_y = CENTER - tip_radius * tip_angle.cos();
        assert_eq!(sector_at(tip_x, tip_y), None);
        // The middle of the wedge and its straight edge away from the corners stay hit.
        assert_eq!(sector_at(CENTER + 120.0, CENTER), Some(2));
        let mid_edge_angle = SECTOR_STEP * 2.0 + half * 0.5;
        assert_eq!(
            sector_at(
                CENTER + 120.0 * mid_edge_angle.sin(),
                CENTER - 120.0 * mid_edge_angle.cos()
            ),
            Some(2)
        );
    }

    #[test]
    fn glow_surrounds_the_wedge_without_covering_other_sectors() {
        let pixels = highlight_pixels(2, [12, 56, 78]);
        let pixel = |x: usize, y: usize| -> &[u8] { &pixels[(y * 328 + x) * 4..][..4] };
        // Just outside the outer arc, straight out from the selected wedge.
        assert_eq!(pixel(326, 164), &[12, 56, 78, GLOW_ALPHA]);
        // The neighbouring wedge keeps no glow at its own centre.
        assert_eq!(pixel(164, 0)[3], 0);
    }
    #[test]
    fn highlight_only_paints_selected_wedge() {
        let pixels = highlight_pixels(2, [12, 56, 78]);
        let pixel = |x: usize, y: usize| -> &[u8] { &pixels[(y * 328 + x) * 4..][..4] };
        assert_eq!(pixel(264, 164), &[12, 56, 78, 92]);
        assert_eq!(pixel(64, 164)[3], 0);
        assert_eq!(pixel(164, 164)[3], 0);
    }
}

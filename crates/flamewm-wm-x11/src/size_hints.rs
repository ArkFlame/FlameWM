//! Pure `WM_NORMAL_HINTS` parsing and resize-constraint policy (J06).
//!
//! No X calls, no rendering, no live-frame mutation. Product code reads the
//! property once on the manage path and stores the parsed value; these
//! helpers only transform already-read cardinals/sizes.

/// ICCCM `WM_NORMAL_HINTS` flag bits (subset used here).
pub const HINT_P_MIN_SIZE: u32 = 1 << 4;
pub const HINT_P_MAX_SIZE: u32 = 1 << 5;
pub const HINT_P_RESIZE_INC: u32 = 1 << 6;
pub const HINT_P_ASPECT: u32 = 1 << 7;
pub const HINT_P_BASE_SIZE: u32 = 1 << 8;
pub const HINT_P_WIN_GRAVITY: u32 = 1 << 9;

/// Parsed client size hints. All dimensions are client (inner) pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientSizeHints {
    pub min: Option<(u32, u32)>,
    pub max: Option<(u32, u32)>,
    pub base: Option<(u32, u32)>,
    pub inc: Option<(u32, u32)>,
    pub min_aspect: Option<(u32, u32)>,
    pub max_aspect: Option<(u32, u32)>,
    pub gravity: Option<u32>,
    /// False iff exactly one size is allowed (min == max, non-degenerate).
    pub resizable: bool,
}

impl Default for ClientSizeHints {
    fn default() -> Self {
        Self {
            min: None,
            max: None,
            base: None,
            inc: None,
            min_aspect: None,
            max_aspect: None,
            gravity: None,
            resizable: true,
        }
    }
}

impl ClientSizeHints {
    #[must_use]
    pub fn parse(values: &[u32]) -> Self {
        let mut out = Self::default();
        let Some(&flags) = values.first() else {
            return out;
        };
        // XSizeHints layout: flags, pad x4, min x2, max x2, inc x2,
        // aspect x4 (min x/y, max x/y), base x2, gravity.
        if flags & HINT_P_MIN_SIZE != 0 && values.len() >= 7 {
            let size = (values[5], values[6]);
            if size != (0, 0) {
                out.min = Some(size);
            }
        }
        if flags & HINT_P_MAX_SIZE != 0 && values.len() >= 9 {
            let size = (values[7], values[8]);
            if size != (0, 0) {
                out.max = Some(size);
            }
        }
        if flags & HINT_P_RESIZE_INC != 0 && values.len() >= 11 {
            let inc = (values[9], values[10]);
            if inc.0 > 0 && inc.1 > 0 {
                out.inc = Some(inc);
            }
        }
        if flags & HINT_P_ASPECT != 0 && values.len() >= 15 {
            let min_a = (values[11], values[12]);
            let max_a = (values[13], values[14]);
            if min_a.0 > 0 && min_a.1 > 0 {
                out.min_aspect = Some(min_a);
            }
            if max_a.0 > 0 && max_a.1 > 0 {
                out.max_aspect = Some(max_a);
            }
        }
        if flags & HINT_P_BASE_SIZE != 0 && values.len() >= 17 {
            out.base = Some((values[15], values[16]));
        }
        if flags & HINT_P_WIN_GRAVITY != 0 && values.len() >= 18 {
            out.gravity = Some(values[17]);
        }
        out.resizable = match (out.min, out.max) {
            (Some(min), Some(max)) => min != max,
            _ => true,
        };
        out
    }
}

/// X11 window gravity values (`XDefineCursor` numbering: Forget=0,
/// NorthWest=1 .. SouthEast=9, Static=10). Live owner for gravity
/// interpretation; `frame::geometry` consumes this for MapRequest /
/// ConfigureRequest reference-point conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum WinGravity {
    Forget = 0,
    NorthWest = 1,
    North = 2,
    NorthEast = 3,
    West = 4,
    Center = 5,
    East = 6,
    SouthWest = 7,
    South = 8,
    SouthEast = 9,
    Static = 10,
}

impl WinGravity {
    /// Decode a raw gravity cardinal. Unknown values fall back to
    /// `NorthWest` (ICCCM default reference point).
    #[must_use]
    pub fn from_u32(value: u32) -> Self {
        match value {
            0 => Self::Forget,
            1 => Self::NorthWest,
            2 => Self::North,
            3 => Self::NorthEast,
            4 => Self::West,
            5 => Self::Center,
            6 => Self::East,
            7 => Self::SouthWest,
            8 => Self::South,
            9 => Self::SouthEast,
            10 => Self::Static,
            _ => Self::NorthWest,
        }
    }

    /// Decode the parsed `gravity` field, defaulting to `NorthWest`.
    #[must_use]
    pub fn of(hints: &ClientSizeHints) -> Self {
        hints.gravity.map_or(Self::NorthWest, Self::from_u32)
    }

    /// Anchor numerator over denominator 2 along (x, y).
    /// NorthWest=(0,0), Center=(1,1), SouthEast=(2,2).
    /// Static keeps the client origin fixed and is handled separately
    /// by callers; its anchor here matches NorthWest.
    /// Forget has no defined anchor and behaves as NorthWest.
    #[must_use]
    pub const fn anchor_num(self) -> (i32, i32) {
        match self {
            Self::Forget | Self::NorthWest | Self::Static => (0, 0),
            Self::North => (1, 0),
            Self::NorthEast => (2, 0),
            Self::West => (0, 1),
            Self::Center => (1, 1),
            Self::East => (2, 1),
            Self::SouthWest => (0, 2),
            Self::South => (1, 2),
            Self::SouthEast => (2, 2),
        }
    }
}

/// Dragged corner. The opposite corner stays fixed; the constrained size is
/// reported without position so callers keep their own anchor math.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Snap a client size to base/min/max plus integer increment steps.
/// Exact integer arithmetic only; no float drift.
#[must_use]
pub fn snap_client_size(mut w: u32, mut h: u32, hints: &ClientSizeHints) -> (u32, u32) {
    w = w.max(1);
    h = h.max(1);
    let base = hints.base.unwrap_or((0, 0));
    if let Some(min) = hints.min {
        w = w.max(min.0.max(1));
        h = h.max(min.1.max(1));
    }
    if let Some(max) = hints.max {
        w = w.min(max.0.max(1));
        h = h.min(max.1.max(1));
    }
    if let Some(inc) = hints.inc {
        if w > base.0 {
            w = base.0 + ((w - base.0) / inc.0) * inc.0;
        } else {
            w = base.0;
        }
        if h > base.1 {
            h = base.1 + ((h - base.1) / inc.1) * inc.1;
        } else {
            h = base.1;
        }
        w = w.max(1);
        h = h.max(1);
        // Snapping down can dip below min; restore the floor.
        if let Some(min) = hints.min {
            w = w.max(min.0.max(1));
            h = h.max(min.1.max(1));
        }
    }
    // Aspect via integer cross-products (w/h bounded by num/den pairs).
    // Preserve the dragged anchor by shrinking (never growing) the size.
    if let Some((num, den)) = hints.max_aspect {
        // w/h <= num/den  <=>  w*den <= num*h
        if u64::from(w) * u64::from(den) > u64::from(num) * u64::from(h) {
            w = ((u64::from(h) * u64::from(num) / u64::from(den)) as u32).max(1);
        }
    }
    if let Some((num, den)) = hints.min_aspect {
        // w/h >= num/den  <=>  w*den >= num*h
        if u64::from(w) * u64::from(den) < u64::from(num) * u64::from(h) {
            h = ((u64::from(w) * u64::from(den) / u64::from(num)) as u32).max(1);
        }
    }
    // Aspect shrink can dip below min; restore the floor once.
    if let Some(min) = hints.min {
        w = w.max(min.0.max(1));
        h = h.max(min.1.max(1));
    }
    if let Some(max) = hints.max {
        w = w.min(max.0.max(1));
        h = h.min(max.1.max(1));
    }
    (w.max(1), h.max(1))
}

/// Full pure constraint pipeline for a corner drag.
///
/// Order: raw edge/corner size -> min-frame safety -> client domain ->
/// base/min/max -> increment snap -> aspect (integer, anchor-preserving
/// shrink) -> outer -> work-area clamp.
#[cfg(test)]
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn constrain_frame_size(
    raw_w: u32,
    raw_h: u32,
    _corner: ResizeCorner,
    hints: &ClientSizeHints,
    frame_dw: u32,
    frame_dh: u32,
    min_frame: (u32, u32),
    work: (u32, u32),
) -> (u32, u32) {
    let w = raw_w.max(min_frame.0.max(1)).max(frame_dw + 1);
    let h = raw_h.max(min_frame.1.max(1)).max(frame_dh + 1);
    let (cw, ch) = (w - frame_dw, h - frame_dh);
    let (cw, ch) = snap_client_size(cw, ch, hints);
    let (mut ow, mut oh) = (cw + frame_dw, ch + frame_dh);
    ow = ow.min(work.0.max(1)).max(min_frame.0.max(1));
    oh = oh.min(work.1.max(1)).max(min_frame.1.max(1));
    (ow, oh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(values: &mut Vec<u32>, flag: u32) {
        values[0] |= flag;
    }

    fn blank() -> Vec<u32> {
        vec![0; 18]
    }

    #[test]
    fn free_hints_parse_empty() {
        let hints = ClientSizeHints::parse(&[]);
        assert_eq!(hints, ClientSizeHints::default());
        assert!(hints.resizable);
        assert_eq!(snap_client_size(333, 222, &hints), (333, 222));
    }

    #[test]
    fn fixed_single_size_is_not_resizable() {
        let mut values = blank();
        flags(&mut values, HINT_P_MIN_SIZE | HINT_P_MAX_SIZE);
        values[5] = 400;
        values[6] = 300;
        values[7] = 400;
        values[8] = 300;
        let hints = ClientSizeHints::parse(&values);
        assert!(!hints.resizable);
        assert_eq!(hints.min, Some((400, 300)));
        assert_eq!(hints.max, Some((400, 300)));
        assert_eq!(snap_client_size(999, 999, &hints), (400, 300));
        assert_eq!(snap_client_size(10, 10, &hints), (400, 300));
    }

    #[test]
    fn min_max_domain_clamps() {
        let mut values = blank();
        flags(&mut values, HINT_P_MIN_SIZE | HINT_P_MAX_SIZE);
        values[5] = 100;
        values[6] = 100;
        values[7] = 800;
        values[8] = 600;
        let hints = ClientSizeHints::parse(&values);
        assert!(hints.resizable);
        assert_eq!(snap_client_size(50, 50, &hints), (100, 100));
        assert_eq!(snap_client_size(2000, 2000, &hints), (800, 600));
        assert_eq!(snap_client_size(400, 300, &hints), (400, 300));
    }

    #[test]
    fn base_plus_increment_snaps_with_integers() {
        let mut values = blank();
        flags(&mut values, HINT_P_BASE_SIZE | HINT_P_RESIZE_INC);
        values[9] = 8;
        values[10] = 8;
        values[15] = 100;
        values[16] = 100;
        let hints = ClientSizeHints::parse(&values);
        assert_eq!(hints.base, Some((100, 100)));
        assert_eq!(hints.inc, Some((8, 8)));
        // 150 - 100 = 50 -> 48 steps -> 148. Exact, no float drift.
        assert_eq!(snap_client_size(150, 150, &hints), (148, 148));
        assert_eq!(snap_client_size(100, 100, &hints), (100, 100));
        assert_eq!(snap_client_size(10, 10, &hints), (100, 100));
    }

    #[test]
    fn aspect_uses_integer_cross_products() {
        let mut values = blank();
        flags(&mut values, HINT_P_ASPECT);
        values[11] = 1;
        values[12] = 1;
        values[13] = 2;
        values[14] = 1;
        let hints = ClientSizeHints::parse(&values);
        // Too wide (4:1 > 2:1): shrink width to 2*h.
        assert_eq!(snap_client_size(400, 100, &hints), (200, 100));
        // Too tall (1:4 < 1:1): shrink height to w.
        assert_eq!(snap_client_size(100, 400, &hints), (100, 100));
        // Inside range passes through.
        assert_eq!(snap_client_size(300, 200, &hints), (300, 200));
    }

    #[test]
    fn all_four_corners_constrain_identically_for_size() {
        let mut values = blank();
        flags(&mut values, HINT_P_MIN_SIZE | HINT_P_MAX_SIZE);
        values[5] = 100;
        values[6] = 100;
        values[7] = 800;
        values[8] = 600;
        let hints = ClientSizeHints::parse(&values);
        let corners = [
            ResizeCorner::TopLeft,
            ResizeCorner::TopRight,
            ResizeCorner::BottomLeft,
            ResizeCorner::BottomRight,
        ];
        for corner in corners {
            // Below min-frame safety floor: clamped to min frame.
            assert_eq!(
                constrain_frame_size(10, 10, corner, &hints, 8, 30, (50, 50), (1920, 1080)),
                (108, 130),
                "corner {corner:?}"
            );
            // Above work area: clamped to work size.
            assert_eq!(
                constrain_frame_size(5000, 5000, corner, &hints, 8, 30, (50, 50), (1920, 1080)),
                (808, 630),
                "corner {corner:?}"
            );
        }
    }

    #[test]
    fn non_resizable_detection_requires_exact_pair() {
        let mut values = blank();
        flags(&mut values, HINT_P_MIN_SIZE);
        values[5] = 400;
        values[6] = 300;
        assert!(ClientSizeHints::parse(&values).resizable);
        let mut values = blank();
        flags(&mut values, HINT_P_MIN_SIZE | HINT_P_MAX_SIZE);
        values[5] = 100;
        values[6] = 100;
        values[7] = 800;
        values[8] = 600;
        assert!(ClientSizeHints::parse(&values).resizable);
    }

    #[test]
    fn degenerate_zero_sizes_stay_resizable() {
        let mut values = blank();
        flags(&mut values, HINT_P_MIN_SIZE | HINT_P_MAX_SIZE);
        assert!(ClientSizeHints::parse(&values).resizable);
    }
}

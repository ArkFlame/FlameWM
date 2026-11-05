//! Pure window-manager contract predicates (J13/J06) and their unit tests.
/// J13 pure mirrors of frozen product contracts (wm-x11 J04/J05/J06 source
/// wins): Motif no-decoration parse (`decoration/policy.rs`), fixed-size
/// resizable parse (`size_hints.rs`), 6px resize-edge hit test
/// (`client.rs`).
pub(crate) fn j13_motif_no_decorations(values: &[u32]) -> bool {
    if values.len() < 3 {
        return false;
    }
    values[0] & (1 << 1) != 0 && values[2] == 0
}

pub(crate) fn j13_hints_resizable(values: &[u32]) -> bool {
    let Some(&flags) = values.first() else {
        return true;
    };
    let pinned = |bit: u32, at: usize| -> Option<(u32, u32)> {
        if flags & (1 << bit) == 0 {
            return None;
        }
        let size = (*values.get(at)?, *values.get(at + 1)?);
        (size != (0, 0)).then_some(size)
    };
    match (pinned(4, 5), pinned(5, 7)) {
        (Some(a), Some(b)) => a != b,
        _ => true,
    }
}

pub(crate) fn j13_resize_edges(w: u32, h: u32, x: i16, y: i16) -> (bool, bool, bool, bool) {
    let width = w.clamp(1, u32::from(u16::MAX)) as i16;
    let height = h.clamp(1, u32::from(u16::MAX)) as i16;
    (
        x <= 6,
        x >= width.saturating_sub(6),
        y <= 6,
        y >= height.saturating_sub(6),
    )
}

/// J06 post-registration pure helpers (mirrors of frozen product contracts).
pub(crate) fn j06_title_center_offset(frame_w: u32, text_w: u32) -> i16 {
    if text_w >= frame_w {
        return 0;
    }
    ((frame_w - text_w) / 2) as i16
}

pub(crate) fn j06_ledger_pass(sampled: usize, delivered: usize, non_black: usize) -> bool {
    sampled > 0 && delivered == sampled && non_black >= 1
}

#[cfg(test)]
mod j13_tests {
    use super::{j13_hints_resizable, j13_motif_no_decorations, j13_resize_edges};

    #[test]
    pub(crate) fn motif_parse_matches_policy() {
        assert!(j13_motif_no_decorations(&[2, 0, 0, 0, 0]));
        assert!(!j13_motif_no_decorations(&[0, 0, 0, 0, 0]));
        assert!(!j13_motif_no_decorations(&[2, 0, 1, 0, 0]));
        assert!(!j13_motif_no_decorations(&[2, 0]));
    }

    #[test]
    pub(crate) fn hints_fixed_pair_is_not_resizable() {
        let values = vec![(1 << 4) | (1 << 5), 0, 0, 0, 0, 400, 300, 400, 300];
        assert!(!j13_hints_resizable(&values));
        let values = vec![(1 << 4) | (1 << 5), 0, 0, 0, 0, 100, 100, 800, 600];
        assert!(j13_hints_resizable(&values));
        assert!(j13_hints_resizable(&[]));
    }

    #[test]
    pub(crate) fn j06_title_centered() {
        assert_eq!(super::j06_title_center_offset(400, 100), 150);
        assert_eq!(super::j06_title_center_offset(100, 200), 0);
        assert_eq!(super::j06_title_center_offset(400, 400), 0);
    }

    #[test]
    pub(crate) fn j06_ledger_contract() {
        assert!(super::j06_ledger_pass(3, 3, 1));
        assert!(!super::j06_ledger_pass(3, 2, 1));
        assert!(!super::j06_ledger_pass(0, 0, 0));
    }

    #[test]
    pub(crate) fn eight_edges_all_hit() {
        let pts = [
            (200, 2),
            (200, 297),
            (2, 150),
            (397, 150),
            (2, 2),
            (397, 2),
            (2, 297),
            (397, 297),
        ];
        for (x, y) in pts {
            let (l, r, t, b) = j13_resize_edges(400, 300, x, y);
            assert!(l || r || t || b, "edge miss at {x},{y}");
        }
        let (l, r, t, b) = j13_resize_edges(400, 300, 200, 150);
        assert!(!(l || r || t || b));
    }
}

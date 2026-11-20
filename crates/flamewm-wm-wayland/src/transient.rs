//! Pure placement and teardown ordering for xdg toplevels that name a parent
//! (`xdg_toplevel.set_parent`).
//!
//! This module has no Smithay or compositor dependency: it is geometry plus edge traversal, so
//! both crate roots can compile it and the tests run without a seat, a GPU or a running session.
//! The handler reads the parent off the surface and converts to/from `model::Rect` at its boundary.

use flamewm_api::Rect;

/// Frame of `w` x `h` centred over `parent`, kept inside `work`.
///
/// A frame larger than the work area anchors to its top-left edge so its controls stay reachable.
/// This mirrors the X11 transient placement, so both backends agree on where a dialog lands.
#[must_use]
pub fn center_over_parent(parent: Rect, w: i32, h: i32, work: Rect) -> Rect {
    let x = parent.x.saturating_add((parent.width - w) / 2);
    let y = parent.y.saturating_add((parent.height - h) / 2);
    let x = x.clamp(work.x, work.x.saturating_add((work.width - w).max(0)));
    let y = y.clamp(work.y, work.y.saturating_add((work.height - h).max(0)));
    Rect::new(x, y, w, h)
}

/// Ids to close when `root` goes away, deepest descendants first: a dialog never outlives the
/// window that owns it. `edges` holds `(child, parent)` window-id pairs recorded when the client
/// called `set_parent`. `root` itself is not returned; stale and cyclic edges are ignored.
#[must_use]
pub fn descendant_close_order(root: u64, edges: &[(u64, u64)]) -> Vec<u64> {
    fn collect(
        id: u64,
        edges: &[(u64, u64)],
        out: &mut Vec<u64>,
        seen: &mut std::collections::HashSet<u64>,
    ) {
        for &(child, parent) in edges {
            if parent == id && seen.insert(child) {
                collect(child, edges, out, seen);
                out.push(child);
            }
        }
    }

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // A cycle back to the root must not re-add the window already being destroyed.
    seen.insert(root);
    collect(root, edges, &mut out, &mut seen);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect::new(0, 40, 1600, 860);

    #[test]
    fn dialog_is_centered_over_parent() {
        let parent = Rect::new(400, 300, 400, 300);
        assert_eq!(
            center_over_parent(parent, 200, 100, WORK),
            Rect::new(500, 400, 200, 100)
        );
    }

    #[test]
    fn dialog_is_clamped_into_work_area() {
        // Centred would be (-50, -25): pinned to the work-area origin.
        let top_left = Rect::new(0, 0, 100, 100);
        assert_eq!(
            center_over_parent(top_left, 200, 150, WORK),
            Rect::new(0, 40, 200, 150)
        );
        // Centred would be (1400, 750): pinned to the work-area far edge.
        let bottom_right = Rect::new(1500, 800, 100, 100);
        assert_eq!(
            center_over_parent(bottom_right, 300, 200, WORK),
            Rect::new(1300, 700, 300, 200)
        );
    }

    #[test]
    fn oversized_dialog_anchors_to_work_area_origin() {
        let parent = Rect::new(100, 100, 50, 50);
        assert_eq!(
            center_over_parent(parent, 2000, 1000, WORK),
            Rect::new(0, 40, 2000, 1000)
        );
    }

    #[test]
    fn close_order_is_deepest_first() {
        // 1 -> 2 -> 3 chain, plus sibling 4 under 1; 9 belongs to an unrelated parent 8.
        let edges = [(2, 1), (3, 2), (4, 1), (9, 8)];
        assert_eq!(descendant_close_order(1, &edges), vec![3, 2, 4]);
    }

    #[test]
    fn close_order_ignores_cycles_and_unknown_roots() {
        let edges = [(2, 1), (1, 2)];
        assert_eq!(descendant_close_order(1, &edges), vec![2]);
        assert!(descendant_close_order(7, &edges).is_empty());
    }
}

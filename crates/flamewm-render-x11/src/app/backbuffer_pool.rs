//! Capacity policy for the retained backbuffer pixmap.
//!
//! Interactive resize emits a ConfigureNotify per step. Reallocating the pixmap (and the
//! XRender picture bound to it) on every step is the dominant cost, so the backbuffer keeps
//! a capacity at least as large as the window: it grows with headroom and is reused while the
//! window fits. Only a settled window more than 2x smaller than the capacity gives memory back.

/// Extra pixels per axis granted on every grow, so a drag that steps past the current size
/// grows once instead of once per step.
pub(crate) const HEADROOM: u32 = 256;

/// Capacity needed for a window of `need` with `root` as the upper bound.
fn fit_axis(current: u32, need: u32, root: u32) -> u32 {
    if need <= current {
        return current;
    }
    (need + HEADROOM).min(root).max(need)
}

/// Capacity after a window of `need` is presented. Grow-only per axis: an axis already large
/// enough keeps its current size. Returns `current` unchanged when the window fits.
#[must_use]
pub(crate) fn grown_capacity(
    current: (u32, u32),
    need: (u32, u32),
    root: (u32, u32),
) -> (u32, u32) {
    (
        fit_axis(current.0, need.0, root.0),
        fit_axis(current.1, need.1, root.1),
    )
}

/// Shrink target for a settled window. `None` unless an axis is more than 2x the window, so
/// ordinary resizes never churn the pixmap.
#[must_use]
pub(crate) fn settled_capacity(
    current: (u32, u32),
    need: (u32, u32),
    root: (u32, u32),
) -> Option<(u32, u32)> {
    let oversize = current.0 > need.0.saturating_mul(2) || current.1 > need.1.saturating_mul(2);
    oversize.then(|| {
        (
            (need.0 + HEADROOM).min(root.0).max(need.0),
            (need.1 + HEADROOM).min(root.1).max(need.1),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: (u32, u32) = (1920, 1080);

    #[test]
    fn window_within_capacity_reuses_it() {
        assert_eq!(grown_capacity((800, 600), (500, 300), ROOT), (800, 600));
        assert_eq!(grown_capacity((800, 600), (800, 600), ROOT), (800, 600));
    }

    #[test]
    fn grow_adds_headroom_once() {
        assert_eq!(grown_capacity((800, 600), (801, 600), ROOT), (1057, 600));
    }

    #[test]
    fn grow_is_per_axis_and_never_shrinks_the_other_axis() {
        assert_eq!(grown_capacity((800, 900), (900, 300), ROOT), (1156, 900));
    }

    #[test]
    fn grow_clamps_to_root_but_never_below_window() {
        assert_eq!(grown_capacity((800, 600), (1500, 600), ROOT), (1756, 600));
        assert_eq!(grown_capacity((800, 600), (1900, 600), ROOT), (1920, 600));
        assert_eq!(grown_capacity((800, 600), (2000, 600), ROOT), (2000, 600));
    }

    #[test]
    fn settle_shrinks_only_when_more_than_twice_oversize() {
        assert_eq!(
            settled_capacity((1300, 400), (600, 400), ROOT),
            Some((856, 656))
        );
        assert_eq!(
            settled_capacity((600, 1000), (600, 400), ROOT),
            Some((856, 656))
        );
        assert_eq!(
            settled_capacity((1200, 800), (600, 400), ROOT),
            None,
            "exactly 2x is not oversize"
        );
        assert_eq!(settled_capacity((1200, 800), (700, 500), ROOT), None);
        assert_eq!(settled_capacity((1000, 600), (600, 400), ROOT), None);
        assert_eq!(settled_capacity((800, 600), (600, 400), ROOT), None);
    }

    #[test]
    fn repeated_grow_steps_allocate_at_most_once_per_headroom() {
        let mut cap = (100, 100);
        let mut allocations = 0;
        for w in 101..=400 {
            let next = grown_capacity(cap, (w, 100), ROOT);
            if next != cap {
                allocations += 1;
                cap = next;
            }
        }
        assert_eq!(
            allocations, 2,
            "a 300-step drag reallocates twice, not 300 times"
        );
    }
}

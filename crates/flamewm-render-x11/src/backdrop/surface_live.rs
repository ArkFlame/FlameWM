//! Refresh policy for a shell surface's live backdrop. Pure logic: the X calls live in the
//! surface controller, and the policy here decides when they run. See `docs/DOCK_BACKDROP.md`.

/// Result of one live composition of a surface backdrop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveSurfaceFrame {
    /// The composed pixels equal the last composition: the snapshot already on the surface is current.
    Unchanged,
    /// Opaque ARGB32 words of the surface size, blurred per the material.
    Pixels(Vec<u32>),
}

/// Whether a surface with this material needs a backdrop at all. An opaque material (100% opacity,
/// no blur) hides everything beneath it, so no capture is taken and no backdrop is kept.
#[must_use]
pub fn backdrop_needed(opacity_percent: u8, blur_percent: u8) -> bool {
    opacity_percent < 100 || blur_percent > 0
}

/// Whether the backdrop must be composed again. The surface key (rect and material) changed, or
/// root children changed while the live path is in use: what lies beneath may have moved.
/// Without the live path a root change cannot alter the desktop-only backdrop, so it is ignored.
#[must_use]
pub fn recompose_needed<K: PartialEq>(
    previous: Option<&K>,
    next: &K,
    root_changed: bool,
    live_active: bool,
) -> bool {
    previous != Some(next) || (root_changed && live_active)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_material_needs_no_backdrop() {
        assert!(!backdrop_needed(100, 0));
        assert!(backdrop_needed(99, 0));
        assert!(backdrop_needed(100, 1));
        assert!(backdrop_needed(0, 0));
    }

    #[test]
    fn first_compose_and_key_changes_always_recompose() {
        let key = ((0, 0, 10, 10), 80u8, 40u8);
        assert!(recompose_needed(None, &key, false, false));
        assert!(recompose_needed(
            Some(&key),
            &((0, 0, 11, 10), 80, 40),
            false,
            false
        ));
        assert!(recompose_needed(
            Some(&key),
            &((0, 0, 10, 10), 81, 40),
            false,
            false
        ));
        assert!(recompose_needed(
            Some(&key),
            &((0, 0, 10, 10), 80, 41),
            false,
            true
        ));
    }

    #[test]
    fn unchanged_key_without_root_change_is_skipped() {
        let key = ((0, 0, 10, 10), 80u8, 40u8);
        assert!(!recompose_needed(Some(&key), &key, false, true));
        assert!(!recompose_needed(Some(&key), &key, false, false));
    }

    #[test]
    fn root_change_recomposes_only_on_the_live_path() {
        let key = ((5, 5, 10, 10), 80u8, 40u8);
        assert!(recompose_needed(Some(&key), &key, true, true));
        assert!(!recompose_needed(Some(&key), &key, true, false));
    }
}

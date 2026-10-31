//! One scale factor for every taskbar metric, derived from the global `taskbarHeight` setting.
//!
//! The stylesheet and projection constants are authored at the 44 px reference bar. Every
//! size, icon and font the taskbar projects goes through `scaled`, so a smaller bar shrinks
//! its contents and a larger bar grows them by the same factor.

use std::sync::atomic::{AtomicU32, Ordering};

/// Bar height in logical px that the authored taskbar metrics assume.
pub const REFERENCE_HEIGHT: f32 = 44.0;
/// Lower clamp for the factor. The setting is bounded to 34..=72 (about 0.77..1.64).
pub const MIN_SCALE: f32 = 0.75;
/// Upper clamp for the factor.
pub const MAX_SCALE: f32 = 1.65;

static FACTOR_BITS: AtomicU32 = AtomicU32::new(f32::to_bits(1.0));

/// Factor for a logical bar height. A zero height (no panel size) keeps the reference scale.
#[must_use]
pub fn scale_for_height(logical_height: u16) -> f32 {
    if logical_height == 0 {
        return 1.0;
    }
    (f32::from(logical_height) / REFERENCE_HEIGHT).clamp(MIN_SCALE, MAX_SCALE)
}

/// Multiplies an authored px value by `factor`.
#[must_use]
pub fn scale_px(px: f32, factor: f32) -> f32 {
    px * factor
}

/// Border width of `#taskbar` and of the bordered separate-mode nodes, in CSS px.
pub const NODE_BORDER: f32 = 1.0;

/// Border-box height of every taskbar node (capsule, pager, task slots). Every node is
/// stretched or sized to this value, so they share one top edge and one height.
#[must_use]
pub fn node_height(factor: f32) -> f32 {
    scale_px(REFERENCE_HEIGHT, factor) - 2.0 * NODE_BORDER
}

/// Content height inside a bordered node. Children of a capsule or of `#task-slots`
/// and `#tray-group` size to this, so they fit inside the border and stay centred.
#[must_use]
pub fn inner_node_height(factor: f32) -> f32 {
    node_height(factor) - 2.0 * NODE_BORDER
}

/// Start button edge. The start capsule is `node_height` square, so its button is the
/// inner square inside the 1 px border.
#[must_use]
pub fn start_edge(factor: f32) -> f32 {
    inner_node_height(factor)
}

/// Width of the task-slots container for `rendered` slots of authored width `slot_logical`.
/// The container's own border sits outside the slots, so it adds `2 * NODE_BORDER`.
#[must_use]
pub fn task_slots_width(rendered: usize, slot_logical: f32, factor: f32) -> f32 {
    if rendered == 0 {
        return 0.0;
    }
    rendered as f32 * scale_px(slot_logical, factor) + 2.0 * NODE_BORDER
}

/// Stores the factor for the bar height the shell is projecting.
pub fn set_factor(factor: f32) {
    FACTOR_BITS.store(factor.to_bits(), Ordering::Relaxed);
}

/// The factor most recently stored by `set_factor`.
#[must_use]
pub fn factor() -> f32 {
    f32::from_bits(FACTOR_BITS.load(Ordering::Relaxed))
}

/// Scales an authored px value by the current taskbar factor.
#[must_use]
pub fn scaled(px: f32) -> f32 {
    scale_px(px, factor())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_height_keeps_authored_metrics() {
        assert_eq!(scale_for_height(44), 1.0);
        assert_eq!(scale_for_height(0), 1.0);
        assert_eq!(scale_px(25.0, 1.0), 25.0);
    }

    #[test]
    fn smaller_and_larger_bars_scale_proportionally() {
        let small = scale_for_height(34);
        let large = scale_for_height(72);
        assert!((small - 34.0 / 44.0).abs() < 1e-6);
        assert!((large - 72.0 / 44.0).abs() < 1e-6);
        assert!(small < 1.0 && large > 1.0);
        assert!((scale_px(25.0, large) - 25.0 * 72.0 / 44.0).abs() < 1e-4);
    }

    #[test]
    fn factor_is_monotonic_across_the_setting_range() {
        let mut previous = 0.0_f32;
        for height in 34..=72_u16 {
            let factor = scale_for_height(height);
            assert!(factor >= previous, "height {height} shrank");
            previous = factor;
        }
    }

    #[test]
    fn out_of_range_heights_are_clamped() {
        assert_eq!(scale_for_height(1), MIN_SCALE);
        assert_eq!(scale_for_height(u16::MAX), MAX_SCALE);
    }

    const SLOT: f32 = 42.0;
    const ICON: f32 = 25.0;

    #[test]
    fn start_button_is_a_perfect_square_at_every_taskbar_height() {
        for height in 34..=72_u16 {
            let factor = scale_for_height(height);
            let edge = start_edge(factor);
            // The capsule is node_height square; its 1 px border sits outside the button.
            assert_eq!(
                edge + 2.0 * NODE_BORDER,
                node_height(factor),
                "height {height}"
            );
            assert!((edge - (f32::from(height) - 4.0 * NODE_BORDER)).abs() < 1e-3);
        }
    }

    #[test]
    fn task_node_is_exactly_one_icon_plus_symmetric_padding() {
        for height in 34..=72_u16 {
            let factor = scale_for_height(height);
            let node = scale_px(SLOT, factor);
            let icon = scale_px(ICON, factor);
            let pad = (node - icon) / 2.0;
            assert!((icon + 2.0 * pad - node).abs() < 1e-4, "height {height}");
            assert!(pad > 0.0, "height {height}: icon must fit inside the node");
        }
    }

    #[test]
    fn every_node_shares_one_height_across_the_range() {
        for height in 34..=72_u16 {
            let factor = scale_for_height(height);
            let shared = node_height(factor);
            assert!((shared - (f32::from(height) - 2.0 * NODE_BORDER)).abs() < 1e-3);
            assert_eq!(inner_node_height(factor) + 2.0 * NODE_BORDER, shared);
            assert_eq!(start_edge(factor), inner_node_height(factor));
        }
    }

    #[test]
    fn task_slots_container_wraps_its_slots_exactly() {
        let factor = scale_for_height(72);
        assert_eq!(task_slots_width(0, SLOT, factor), 0.0);
        let width = task_slots_width(3, SLOT, factor);
        assert!((width - (3.0 * scale_px(SLOT, factor) + 2.0 * NODE_BORDER)).abs() < 1e-4);
    }
}

/// Serialises tests that write or read the process-wide factor. Each such test resets it to the
/// reference scale under this lock, so results do not depend on test order.
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

//! Overflow policy for the panel tray strip. Pure: decides how many items fit and whether the
//! chevron that opens the overflow list takes a slot.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverflowPlan {
    /// Items shown inline, taken from the front of the registry order.
    pub visible: usize,
    /// Items reachable only through the overflow chevron.
    pub overflow: usize,
    /// True when `overflow > 0`; the chevron occupies one of the `max_slots`.
    pub chevron: bool,
}

/// Fits `total` items into `max_slots` inline slots. When items do not fit, one slot is reserved
/// for the chevron, so `visible + usize::from(chevron) <= max_slots` always holds.
#[must_use]
pub const fn plan_overflow(total: usize, max_slots: usize) -> OverflowPlan {
    if total <= max_slots {
        return OverflowPlan {
            visible: total,
            overflow: 0,
            chevron: false,
        };
    }
    let visible = max_slots.saturating_sub(1);
    OverflowPlan {
        visible,
        overflow: total - visible,
        chevron: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_fits_without_chevron() {
        assert_eq!(
            plan_overflow(3, 4),
            OverflowPlan {
                visible: 3,
                overflow: 0,
                chevron: false
            }
        );
        assert_eq!(
            plan_overflow(4, 4),
            OverflowPlan {
                visible: 4,
                overflow: 0,
                chevron: false
            }
        );
        assert_eq!(
            plan_overflow(0, 4),
            OverflowPlan {
                visible: 0,
                overflow: 0,
                chevron: false
            }
        );
    }

    #[test]
    fn chevron_takes_a_slot_when_items_overflow() {
        let plan = plan_overflow(10, 4);
        assert_eq!(
            plan,
            OverflowPlan {
                visible: 3,
                overflow: 7,
                chevron: true
            }
        );
        assert_eq!(plan.visible + plan.overflow, 10);
        assert!(plan.visible + usize::from(plan.chevron) <= 4);
    }

    #[test]
    fn zero_slots_moves_everything_behind_chevron() {
        assert_eq!(
            plan_overflow(2, 0),
            OverflowPlan {
                visible: 0,
                overflow: 2,
                chevron: true
            }
        );
    }

    #[test]
    fn one_slot_with_overflow_shows_only_chevron() {
        assert_eq!(
            plan_overflow(5, 1),
            OverflowPlan {
                visible: 0,
                overflow: 5,
                chevron: true
            }
        );
    }
}

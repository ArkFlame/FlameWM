//! Screen-corner dwell detection for `hotCorner.*`. Pure state: the caller feeds the pointer's
//! corner and a monotonic millisecond clock, and arms one one-shot timer for [`Dwell::Pending`]'s
//! deadline. There is no polling; leaving the corner cancels the dwell.

/// Pointer must rest in a corner this long before the corner action fires.
pub const DWELL_MS: u64 = 150;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dwell {
    /// Pointer is outside every corner, or the current corner already fired.
    Idle,
    /// Pointer entered `corner`; it must still be there at `deadline_ms`.
    Pending { corner: Corner, deadline_ms: u64 },
    /// Dwell completed; fires once per entry into the corner.
    Fire(Corner),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CornerDwell {
    /// Corner the pointer is in, with its entry time, until it leaves.
    current: Option<(Corner, u64)>,
    /// True once the current entry has fired, so resting there does not re-fire.
    fired: bool,
}

impl CornerDwell {
    /// Reports the pointer state at `now_ms`. `corner` is `None` when the pointer is not in any
    /// corner. Call again when the armed deadline passes (with the same corner) to collect the fire.
    pub fn update(&mut self, corner: Option<Corner>, now_ms: u64) -> Dwell {
        match (corner, self.current) {
            (None, _) => {
                *self = Self::default();
                Dwell::Idle
            }
            (Some(corner), Some((entered, since))) if entered == corner => {
                if self.fired {
                    Dwell::Idle
                } else if now_ms.saturating_sub(since) >= DWELL_MS {
                    self.fired = true;
                    Dwell::Fire(corner)
                } else {
                    Dwell::Pending {
                        corner,
                        deadline_ms: since + DWELL_MS,
                    }
                }
            }
            (Some(corner), _) => {
                self.current = Some((corner, now_ms));
                self.fired = false;
                Dwell::Pending {
                    corner,
                    deadline_ms: now_ms + DWELL_MS,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_only_after_full_dwell() {
        let mut dwell = CornerDwell::default();
        assert_eq!(
            dwell.update(Some(Corner::TopLeft), 1000),
            Dwell::Pending {
                corner: Corner::TopLeft,
                deadline_ms: 1000 + DWELL_MS,
            }
        );
        // Early wake-up before the debounce elapsed does not fire.
        assert!(matches!(
            dwell.update(Some(Corner::TopLeft), 1000 + DWELL_MS - 1),
            Dwell::Pending { .. }
        ));
        assert_eq!(
            dwell.update(Some(Corner::TopLeft), 1000 + DWELL_MS),
            Dwell::Fire(Corner::TopLeft)
        );
    }

    #[test]
    fn leaving_before_deadline_cancels_dwell() {
        let mut dwell = CornerDwell::default();
        dwell.update(Some(Corner::BottomRight), 0);
        assert_eq!(dwell.update(None, 50), Dwell::Idle);
        // Returning restarts the full 150 ms; the old entry time is not reused.
        assert!(matches!(
            dwell.update(Some(Corner::BottomRight), 100),
            Dwell::Pending {
                deadline_ms: 250,
                ..
            }
        ));
        assert_eq!(
            dwell.update(Some(Corner::BottomRight), 249),
            Dwell::Pending {
                corner: Corner::BottomRight,
                deadline_ms: 250,
            }
        );
    }

    #[test]
    fn fires_once_per_entry() {
        let mut dwell = CornerDwell::default();
        dwell.update(Some(Corner::TopRight), 0);
        assert_eq!(
            dwell.update(Some(Corner::TopRight), DWELL_MS),
            Dwell::Fire(Corner::TopRight)
        );
        assert_eq!(
            dwell.update(Some(Corner::TopRight), DWELL_MS + 1000),
            Dwell::Idle
        );
        dwell.update(None, DWELL_MS + 1001);
        assert!(matches!(
            dwell.update(Some(Corner::TopRight), DWELL_MS + 1002),
            Dwell::Pending { .. }
        ));
    }

    #[test]
    fn moving_between_corners_restarts_dwell() {
        let mut dwell = CornerDwell::default();
        dwell.update(Some(Corner::TopLeft), 0);
        assert_eq!(
            dwell.update(Some(Corner::BottomLeft), 120),
            Dwell::Pending {
                corner: Corner::BottomLeft,
                deadline_ms: 120 + DWELL_MS,
            }
        );
        assert_eq!(
            dwell.update(Some(Corner::BottomLeft), 120 + DWELL_MS),
            Dwell::Fire(Corner::BottomLeft)
        );
    }
}

//! Repaint gate for task progress fills. Progress changes are admitted at most once per
//! `MIN_INTERVAL`; a change inside that window waits for the reactor wake deadline instead of
//! polling, so the final value always lands.

use std::time::{Duration, Instant};

/// About 10 Hz: the fastest task-progress fill repaint.
pub(crate) const MIN_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaintDecision {
    /// Repaint now; the gate has recorded this paint.
    Now,
    /// Hold the pending change until this instant.
    Wait(Instant),
}

#[derive(Debug, Default)]
pub(crate) struct ProgressPaintGate {
    last_paint: Option<Instant>,
}

impl ProgressPaintGate {
    pub(crate) fn decide(&mut self, now: Instant) -> PaintDecision {
        match self.last_paint {
            Some(last) if now.saturating_duration_since(last) < MIN_INTERVAL => {
                PaintDecision::Wait(last + MIN_INTERVAL)
            }
            _ => {
                self.last_paint = Some(now);
                PaintDecision::Now
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_change_paints_immediately_and_bursts_wait_for_the_interval() {
        let start = Instant::now();
        let mut gate = ProgressPaintGate::default();
        assert_eq!(gate.decide(start), PaintDecision::Now);
        assert_eq!(
            gate.decide(start + Duration::from_millis(10)),
            PaintDecision::Wait(start + MIN_INTERVAL)
        );
        assert_eq!(
            gate.decide(start + Duration::from_millis(99)),
            PaintDecision::Wait(start + MIN_INTERVAL)
        );
        assert_eq!(gate.decide(start + MIN_INTERVAL), PaintDecision::Now);
    }

    #[test]
    fn repaint_rate_never_exceeds_ten_per_second() {
        let start = Instant::now();
        let mut gate = ProgressPaintGate::default();
        let mut paints = 0;
        // 1 kHz change stream for one second: at most 10 admitted paints.
        for ms in 0..1000 {
            if gate.decide(start + Duration::from_millis(ms)) == PaintDecision::Now {
                paints += 1;
            }
        }
        assert!((1..=10).contains(&paints), "paints={paints}");
    }
}

use crate::frame::model::PlacementMode;

pub(super) fn bump(counter: &std::sync::atomic::AtomicU64) {
    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

pub(super) fn read(counter: &std::sync::atomic::AtomicU64) -> u64 {
    counter.load(std::sync::atomic::Ordering::Relaxed)
}

/// Profiler point plus the running total read by the summary lines.
pub(super) struct TotalledCounter {
    pub(super) point: flamewm_profiler::CounterPoint,
    pub(super) total: std::sync::atomic::AtomicU64,
}

impl TotalledCounter {
    pub(super) fn new(label: &'static str) -> Self {
        Self {
            point: flamewm_profiler::CounterPoint::new(label),
            total: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub(super) fn add(&self, value: u64) {
        self.point.increment_by(value);
        self.total
            .fetch_add(value, std::sync::atomic::Ordering::Relaxed);
    }

    pub(super) fn load(&self) -> u64 {
        self.total.load(std::sync::atomic::Ordering::Relaxed)
    }
}

pub(super) struct ResizeCounterSet {
    pub(super) received: TotalledCounter,
    pub(super) coalesced: TotalledCounter,
    pub(super) committed: TotalledCounter,
    pub(super) max_batch: std::sync::atomic::AtomicU64,
}

pub(super) fn resize_counters() -> &'static ResizeCounterSet {
    use std::sync::OnceLock;
    static SET: OnceLock<ResizeCounterSet> = OnceLock::new();
    SET.get_or_init(|| ResizeCounterSet {
        received: TotalledCounter::new("wm.resize.received"),
        coalesced: TotalledCounter::new("wm.resize.coalesced"),
        committed: TotalledCounter::new("wm.resize.commit"),
        max_batch: std::sync::atomic::AtomicU64::new(0),
    })
}

impl ResizeCounterSet {
    pub(super) fn record_received(&self, value: u64) {
        self.received.add(value);
    }

    pub(super) fn record_coalesced(&self, value: u64) {
        self.coalesced.add(value);
    }

    pub(super) fn record_committed(&self, value: u64) {
        self.committed.add(value);
    }

    pub(super) fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.received.load(),
            self.coalesced.load(),
            self.committed.load(),
        )
    }

    pub(super) fn max_batch(&self) -> usize {
        self.max_batch.load(std::sync::atomic::Ordering::Relaxed) as usize
    }
    pub(super) fn set_max_batch(&self, value: usize) {
        self.max_batch
            .store(value as u64, std::sync::atomic::Ordering::Relaxed);
    }
}

pub(super) fn emit_resize_summary() {
    flamewm_debug::emit(
        flamewm_debug::WM_RESIZE_SUMMARY,
        flamewm_debug::WM_RESIZE_SUMMARY_COOLDOWN,
        || {
            let counters = resize_counters();
            let (received, coalesced, committed) = counters.snapshot();
            let max = counters.max_batch();
            format!("received={received} coalesced={coalesced} commit={committed} max_batch={max}")
        },
    );
}

pub(super) struct MoveCounterSet {
    pub(super) received: TotalledCounter,
    pub(super) coalesced: TotalledCounter,
    pub(super) committed: TotalledCounter,
    pub(super) snap_target: flamewm_profiler::CounterPoint,
    pub(super) preview: flamewm_profiler::CounterPoint,
    pub(super) snap_target_changes: std::sync::atomic::AtomicU64,
    pub(super) preview_updates: std::sync::atomic::AtomicU64,
    pub(super) last_target: std::sync::atomic::AtomicU64,
    pub(super) toggle_max_total: std::sync::atomic::AtomicU64,
    pub(super) client_message_maximize_total: std::sync::atomic::AtomicU64,
    pub(super) state_mutations_total: std::sync::atomic::AtomicU64,
    pub(super) maximized_transitions: std::sync::atomic::AtomicU64,
    pub(super) resize_snap_blocked_total: std::sync::atomic::AtomicU64,
}

pub(super) fn move_counters() -> &'static MoveCounterSet {
    use std::sync::OnceLock;
    static SET: OnceLock<MoveCounterSet> = OnceLock::new();
    SET.get_or_init(|| MoveCounterSet {
        received: TotalledCounter::new("wm.move.received"),
        coalesced: TotalledCounter::new("wm.move.coalesced"),
        committed: TotalledCounter::new("wm.move.commit"),
        snap_target: flamewm_profiler::CounterPoint::new("wm.move.snap_target.count"),
        preview: flamewm_profiler::CounterPoint::new("wm.move.preview.count"),
        snap_target_changes: std::sync::atomic::AtomicU64::new(0),
        preview_updates: std::sync::atomic::AtomicU64::new(0),
        last_target: std::sync::atomic::AtomicU64::new(u64::MAX),
        toggle_max_total: std::sync::atomic::AtomicU64::new(0),
        client_message_maximize_total: std::sync::atomic::AtomicU64::new(0),
        state_mutations_total: std::sync::atomic::AtomicU64::new(0),
        maximized_transitions: std::sync::atomic::AtomicU64::new(0),
        resize_snap_blocked_total: std::sync::atomic::AtomicU64::new(0),
    })
}

impl MoveCounterSet {
    pub(super) fn record_received(&self, value: u64) {
        self.received.add(value);
    }

    pub(super) fn record_coalesced(&self, value: u64) {
        self.coalesced.add(value);
    }

    pub(super) fn record_committed(&self, value: u64) {
        self.committed.add(value);
    }

    pub(super) fn record_snap_target(&self, target_bits: u64) {
        self.snap_target.increment();
        if self
            .last_target
            .swap(target_bits, std::sync::atomic::Ordering::Relaxed)
            != target_bits
        {
            bump(&self.snap_target_changes);
        }
    }

    pub(super) fn record_preview(&self) {
        self.preview.increment();
        bump(&self.preview_updates);
    }

    pub(super) fn record_toggle_max(&self) {
        bump(&self.toggle_max_total);
    }

    pub(super) fn record_client_message_maximize(&self) {
        bump(&self.client_message_maximize_total);
    }

    pub(super) fn record_placement_transition(&self, before: PlacementMode, after: PlacementMode) {
        if before == after {
            return;
        }
        bump(&self.state_mutations_total);
        if matches!(
            (before, after),
            (PlacementMode::Maximized, _) | (_, PlacementMode::Maximized)
        ) {
            bump(&self.maximized_transitions);
        }
    }

    pub(super) fn record_resize_snap_blocked(&self) {
        bump(&self.resize_snap_blocked_total);
    }
}

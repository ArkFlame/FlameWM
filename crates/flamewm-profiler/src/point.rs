//! Public span guards, points, counters, and memory gauges.

use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::span as registry;
use crate::{now_cpu_ns, now_mono_ns};

/// RAII span guard returned by [`start`]. The span is recorded when it is dropped.
pub struct SpanGuard {
    idx: usize,
    start_wall: u64,
    start_cpu: u64,
    on_stack: bool,
}

impl SpanGuard {
    pub(crate) fn begin(label: &'static str) -> Self {
        let idx = registry::slot_for(label);
        let start_cpu = now_cpu_ns();
        let start_wall = now_mono_ns();
        let on_stack = registry::push(idx, start_wall, start_cpu);
        Self {
            idx,
            start_wall,
            start_cpu,
            on_stack,
        }
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        let end_wall = now_mono_ns();
        let end_cpu = now_cpu_ns();
        let wall_inclusive = end_wall.saturating_sub(self.start_wall);
        // Thread CPU clock may be unavailable (0); fall back to wall so the
        // span stays measurable but never fabricates CPU time.
        let cpu_inclusive = if self.start_cpu == 0 || end_cpu == 0 {
            0
        } else {
            end_cpu.saturating_sub(self.start_cpu)
        };
        if self.on_stack {
            let (child_wall, child_cpu) = registry::pop()
                .map(|(_, _, _, cw, cc)| (cw, cc))
                .unwrap_or((0, 0));
            let wall_self = wall_inclusive.saturating_sub(child_wall);
            let cpu_self = cpu_inclusive.saturating_sub(child_cpu);
            registry::record(self.idx, wall_inclusive, wall_self, cpu_inclusive, cpu_self);
            registry::add_child_time(wall_inclusive, cpu_inclusive);
        } else {
            registry::record(
                self.idx,
                wall_inclusive,
                wall_inclusive,
                cpu_inclusive,
                cpu_inclusive,
            );
        }
    }
}

/// Start a timed span under a static label.
pub fn start(label: &'static str) -> SpanGuard {
    SpanGuard::begin(label)
}

/// Reusable static-label timing point.
pub struct ProfilePoint {
    label: &'static str,
}

impl ProfilePoint {
    /// Create a point bound to a static label.
    pub fn new(label: &'static str) -> Self {
        Self { label }
    }

    /// Start one timed occurrence.
    pub fn start(&self) -> SpanGuard {
        SpanGuard::begin(self.label)
    }
}

const MAX_COUNTERS: usize = 128;

struct CounterSlot {
    label: RwLock<Option<&'static str>>,
    value: AtomicU64,
}

impl CounterSlot {
    const fn new() -> Self {
        Self {
            label: RwLock::new(None),
            value: AtomicU64::new(0),
        }
    }
}

macro_rules! counter_slots {
    ($($n:expr),*) => {
        [$( { let _ = $n; CounterSlot::new() } ),*]
    };
}

static COUNTERS: [CounterSlot; MAX_COUNTERS] = counter_slots!(
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49,
    50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73,
    74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97,
    98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116,
    117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127
);

fn counter_slot(label: &'static str) -> usize {
    for (i, slot) in COUNTERS.iter().enumerate() {
        if slot.label.read().map(|g| *g).unwrap_or(None) == Some(label) {
            return i;
        }
    }
    for (i, slot) in COUNTERS.iter().enumerate() {
        let mut guard = slot.label.write().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(label);
            return i;
        }
        if *guard == Some(label) {
            return i;
        }
    }
    MAX_COUNTERS - 1
}

/// Monotonic event counter under a static label.
pub struct CounterPoint {
    idx: usize,
}

impl CounterPoint {
    /// Bind a counter to a static label.
    pub fn new(label: &'static str) -> Self {
        Self {
            idx: counter_slot(label),
        }
    }

    /// Increment by one.
    pub fn increment(&self) {
        self.increment_by(1);
    }

    /// Increment by `n`.
    pub fn increment_by(&self, n: u64) {
        COUNTERS[self.idx].value.fetch_add(n, Ordering::Relaxed);
    }
}

pub(crate) fn counter_snapshot_and_reset() -> Vec<(&'static str, u64)> {
    let mut out = Vec::new();
    for slot in COUNTERS.iter() {
        if let Some(label) = slot.label.read().map(|g| *g).unwrap_or(None) {
            let v = slot.value.swap(0, Ordering::Relaxed);
            if v > 0 {
                out.push((label, v));
            }
        }
    }
    out
}

#[cfg(test)]
pub(crate) fn counter_reset_for_test() {
    for slot in COUNTERS.iter() {
        slot.value.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_reset_drains() {
        let _guard = crate::span::TEST_LOCK.lock().unwrap();
        counter_reset_for_test();
        let c = CounterPoint::new("t_ctr_reset");
        c.increment_by(7);
        let rows = counter_snapshot_and_reset();
        assert!(rows.iter().any(|(l, v)| *l == "t_ctr_reset" && *v == 7));
        let rows2 = counter_snapshot_and_reset();
        assert!(!rows2.iter().any(|(l, _)| *l == "t_ctr_reset"));
    }

    #[test]
    fn concurrent_same_label_registration_shares_counter() {
        let _guard = crate::span::TEST_LOCK.lock().unwrap();
        counter_reset_for_test();
        const THREADS: usize = 8;
        const INCREMENTS: usize = 100;
        std::thread::scope(|scope| {
            for _ in 0..THREADS {
                scope.spawn(|| {
                    let counter = CounterPoint::new("t_ctr_concurrent_same_label");
                    for _ in 0..INCREMENTS {
                        counter.increment();
                    }
                });
            }
        });

        let rows = counter_snapshot_and_reset();
        assert_eq!(
            rows.iter()
                .filter(|(label, _)| *label == "t_ctr_concurrent_same_label")
                .map(|(_, value)| *value)
                .collect::<Vec<_>>(),
            [THREADS as u64 * INCREMENTS as u64]
        );
    }

    fn cap_ctr_label(i: usize) -> &'static str {
        match i {
            0 => "t_cap_ctr_00",
            1 => "t_cap_ctr_01",
            2 => "t_cap_ctr_02",
            3 => "t_cap_ctr_03",
            4 => "t_cap_ctr_04",
            5 => "t_cap_ctr_05",
            6 => "t_cap_ctr_06",
            7 => "t_cap_ctr_07",
            8 => "t_cap_ctr_08",
            9 => "t_cap_ctr_09",
            10 => "t_cap_ctr_10",
            11 => "t_cap_ctr_11",
            12 => "t_cap_ctr_12",
            13 => "t_cap_ctr_13",
            14 => "t_cap_ctr_14",
            15 => "t_cap_ctr_15",
            16 => "t_cap_ctr_16",
            17 => "t_cap_ctr_17",
            18 => "t_cap_ctr_18",
            19 => "t_cap_ctr_19",
            20 => "t_cap_ctr_20",
            21 => "t_cap_ctr_21",
            22 => "t_cap_ctr_22",
            23 => "t_cap_ctr_23",
            24 => "t_cap_ctr_24",
            25 => "t_cap_ctr_25",
            26 => "t_cap_ctr_26",
            27 => "t_cap_ctr_27",
            28 => "t_cap_ctr_28",
            29 => "t_cap_ctr_29",
            30 => "t_cap_ctr_30",
            31 => "t_cap_ctr_31",
            32 => "t_cap_ctr_32",
            33 => "t_cap_ctr_33",
            34 => "t_cap_ctr_34",
            35 => "t_cap_ctr_35",
            36 => "t_cap_ctr_36",
            37 => "t_cap_ctr_37",
            38 => "t_cap_ctr_38",
            39 => "t_cap_ctr_39",
            40 => "t_cap_ctr_40",
            41 => "t_cap_ctr_41",
            42 => "t_cap_ctr_42",
            43 => "t_cap_ctr_43",
            44 => "t_cap_ctr_44",
            45 => "t_cap_ctr_45",
            46 => "t_cap_ctr_46",
            47 => "t_cap_ctr_47",
            48 => "t_cap_ctr_48",
            49 => "t_cap_ctr_49",
            50 => "t_cap_ctr_50",
            51 => "t_cap_ctr_51",
            52 => "t_cap_ctr_52",
            53 => "t_cap_ctr_53",
            54 => "t_cap_ctr_54",
            55 => "t_cap_ctr_55",
            56 => "t_cap_ctr_56",
            57 => "t_cap_ctr_57",
            58 => "t_cap_ctr_58",
            59 => "t_cap_ctr_59",
            60 => "t_cap_ctr_60",
            61 => "t_cap_ctr_61",
            62 => "t_cap_ctr_62",
            63 => "t_cap_ctr_63",
            64 => "t_cap_ctr_64",
            65 => "t_cap_ctr_65",
            66 => "t_cap_ctr_66",
            67 => "t_cap_ctr_67",
            68 => "t_cap_ctr_68",
            69 => "t_cap_ctr_69",
            _ => unreachable!(),
        }
    }

    #[test]
    fn counters_hold_beyond_64_labels() {
        let _guard = crate::span::TEST_LOCK.lock().unwrap();
        counter_reset_for_test();
        const N: usize = 70;
        for i in 0..N {
            CounterPoint::new(cap_ctr_label(i)).increment();
        }
        // Snapshot owner path: counter_snapshot_and_reset drains values.
        let rows = counter_snapshot_and_reset();
        assert!(rows.len() >= N, "rows={}", rows.len());
        for i in 0..N {
            let label = cap_ctr_label(i);
            assert!(rows.iter().any(|(l, _)| *l == label), "{}", label);
        }
        // Labels 65..N (past the old 64 cap) stay distinct.
        assert!(rows.iter().any(|(l, _)| *l == "t_cap_ctr_64"));
        assert!(rows.iter().any(|(l, _)| *l == "t_cap_ctr_69"));
    }
}

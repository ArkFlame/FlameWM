//! Profiler counters for icon and Start view events.
use super::*;

macro_rules! counter {
    ($($name:ident = $id:literal;)*) => {$(
        pub(super) fn $name() -> &'static flamewm_profiler::CounterPoint {
            static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
            C.get_or_init(|| flamewm_profiler::CounterPoint::new($id))
        }
    )*};
}

counter! {
    redundant_hover_counter = "shell.start.redundant_hover";
    request_counter = "shell.icon.request";
    result_counter = "shell.icon.result";
    stale_counter = "shell.icon.stale_drop";
    queue_full_counter = "shell.icon.queue_full";
    high_counter = "shell.icon.high";
    low_counter = "shell.icon.low";
    coalesced_counter = "shell.icon.coalesced";
    dropped_low_counter = "shell.icon.dropped_low";
    fallback_counter = "shell.icon.empty_placeholder";
}

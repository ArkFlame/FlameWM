//! Configure-path counters and the one-shot summary emitter.
use super::*;

pub(super) struct ConfigureCounterSet {
    pub(super) request_received: flamewm_profiler::CounterPoint,
    pub(super) request_accepted: flamewm_profiler::CounterPoint,
    pub(super) request_refused_interactive: flamewm_profiler::CounterPoint,
    pub(super) request_refused_mode: flamewm_profiler::CounterPoint,
    pub(super) notify_root: flamewm_profiler::CounterPoint,
    pub(super) notify_frame: flamewm_profiler::CounterPoint,
    pub(super) notify_client: flamewm_profiler::CounterPoint,
    pub(super) notify_input_child: flamewm_profiler::CounterPoint,
    pub(super) notify_expected: flamewm_profiler::CounterPoint,
    pub(super) notify_mismatch: flamewm_profiler::CounterPoint,
    pub(super) synthetic: flamewm_profiler::CounterPoint,
    pub(super) request_received_total: std::sync::atomic::AtomicU64,
    pub(super) request_accepted_total: std::sync::atomic::AtomicU64,
    pub(super) request_refused_interactive_total: std::sync::atomic::AtomicU64,
    pub(super) request_refused_mode_total: std::sync::atomic::AtomicU64,
    pub(super) notify_root_total: std::sync::atomic::AtomicU64,
    pub(super) notify_frame_total: std::sync::atomic::AtomicU64,
    pub(super) notify_client_total: std::sync::atomic::AtomicU64,
    pub(super) notify_input_child_total: std::sync::atomic::AtomicU64,
    pub(super) notify_expected_total: std::sync::atomic::AtomicU64,
    pub(super) notify_mismatch_total: std::sync::atomic::AtomicU64,
    pub(super) synthetic_total: std::sync::atomic::AtomicU64,
}

pub(super) fn configure_counters() -> &'static ConfigureCounterSet {
    use std::sync::OnceLock;
    static SET: OnceLock<ConfigureCounterSet> = OnceLock::new();
    SET.get_or_init(|| ConfigureCounterSet {
        request_received: flamewm_profiler::CounterPoint::new("wm.configure.request.received"),
        request_accepted: flamewm_profiler::CounterPoint::new("wm.configure.request.accepted"),
        request_refused_interactive: flamewm_profiler::CounterPoint::new(
            "wm.configure.request.refused_interactive",
        ),
        request_refused_mode: flamewm_profiler::CounterPoint::new(
            "wm.configure.request.refused_mode",
        ),
        notify_root: flamewm_profiler::CounterPoint::new("wm.configure.notify.root"),
        notify_frame: flamewm_profiler::CounterPoint::new("wm.configure.notify.frame"),
        notify_client: flamewm_profiler::CounterPoint::new("wm.configure.notify.client"),
        notify_input_child: flamewm_profiler::CounterPoint::new("wm.configure.notify.input_child"),
        notify_expected: flamewm_profiler::CounterPoint::new("wm.configure.notify.expected"),
        notify_mismatch: flamewm_profiler::CounterPoint::new("wm.configure.notify.mismatch"),
        synthetic: flamewm_profiler::CounterPoint::new("wm.configure.synthetic"),
        request_received_total: std::sync::atomic::AtomicU64::new(0),
        request_accepted_total: std::sync::atomic::AtomicU64::new(0),
        request_refused_interactive_total: std::sync::atomic::AtomicU64::new(0),
        request_refused_mode_total: std::sync::atomic::AtomicU64::new(0),
        notify_root_total: std::sync::atomic::AtomicU64::new(0),
        notify_frame_total: std::sync::atomic::AtomicU64::new(0),
        notify_client_total: std::sync::atomic::AtomicU64::new(0),
        notify_input_child_total: std::sync::atomic::AtomicU64::new(0),
        notify_expected_total: std::sync::atomic::AtomicU64::new(0),
        notify_mismatch_total: std::sync::atomic::AtomicU64::new(0),
        synthetic_total: std::sync::atomic::AtomicU64::new(0),
    })
}

macro_rules! configure_bump {
    ($point:ident, $total:ident) => {{
        let counters = configure_counters();
        counters.$point.increment();
        bump(&counters.$total);
    }};
}

pub(super) fn emit_configure_summary() {
    flamewm_debug::emit(
        flamewm_debug::WM_CONFIGURE_SUMMARY,
        std::time::Duration::from_secs(1),
        || {
            let counters = configure_counters();
            format!(
                "requests={} accepted={} refused_interactive={} refused_mode={} notify_frame={} notify_client={} expected={} mismatch={} synthetic={}",
                read(&counters.request_received_total),
                read(&counters.request_accepted_total),
                read(&counters.request_refused_interactive_total),
                read(&counters.request_refused_mode_total),
                read(&counters.notify_frame_total),
                read(&counters.notify_client_total),
                read(&counters.notify_expected_total),
                read(&counters.notify_mismatch_total),
                read(&counters.synthetic_total),
            )
        },
    );
}

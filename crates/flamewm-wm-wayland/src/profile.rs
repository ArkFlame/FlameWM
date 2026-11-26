//! Env-gated composition counters for the compositor's event-loop thread.
//!
//! `FLAMEWM_PROFILE_COMPOSE=1` turns the profiler on and arms a one-second timer that prints
//! `FLAMEWM-WAYLAND-COMPOSE` followed by the profiler's span/counter window to stderr. The window
//! ranks the in-thread subsystems (Wayland dispatch, control/D-Bus, input, render, timers,
//! XWayland) by total wall time, which is what tells the next unit where the idle second goes.
//! Disabled, every helper returns an inert guard: one relaxed atomic load per span.

use std::sync::OnceLock;
use std::time::Duration;

use flamewm_profiler::{ProfilePoint, SpanGuard, report_window};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::state::State;

/// Resolve each label once, then reuse the registry slot instead of rescanning the labels.
fn start(slot: &'static OnceLock<ProfilePoint>, label: &'static str) -> SpanGuard {
    slot.get_or_init(|| ProfilePoint::new(label)).start()
}

/// One `wl_display` client dispatch turn.
pub(crate) fn wayland_dispatch() -> SpanGuard {
    static P: OnceLock<ProfilePoint> = OnceLock::new();
    start(&P, "wmwayland.dispatch.wayland")
}

/// Flushing client events after a loop turn.
pub(crate) fn wayland_flush() -> SpanGuard {
    static P: OnceLock<ProfilePoint> = OnceLock::new();
    start(&P, "wmwayland.dispatch.flush")
}

/// The control service's post-dispatch hooks and D-Bus turns.
pub(crate) fn control() -> SpanGuard {
    static P: OnceLock<ProfilePoint> = OnceLock::new();
    start(&P, "wmwayland.dispatch.control")
}

/// Host input (pointer, keyboard, touch) handed to the compositor.
pub(crate) fn input() -> SpanGuard {
    static P: OnceLock<ProfilePoint> = OnceLock::new();
    start(&P, "wmwayland.dispatch.input")
}

/// A host window resize.
pub(crate) fn resize() -> SpanGuard {
    static P: OnceLock<ProfilePoint> = OnceLock::new();
    start(&P, "wmwayland.dispatch.resize")
}

/// A host-driven frame: scene rebuild, draw and submit.
pub(crate) fn render() -> SpanGuard {
    static P: OnceLock<ProfilePoint> = OnceLock::new();
    start(&P, "wmwayland.dispatch.render")
}

/// A converted `XWayland` event and the Dock property watcher.
pub(crate) fn xwayland() -> SpanGuard {
    static P: OnceLock<ProfilePoint> = OnceLock::new();
    start(&P, "wmwayland.dispatch.xwayland")
}

/// A timer-driven callback (frame pacing, reaping, deadlines).
pub(crate) fn timer() -> SpanGuard {
    static P: OnceLock<ProfilePoint> = OnceLock::new();
    start(&P, "wmwayland.timer")
}

/// Declare the process name so the profiler writes `flamewm-wayland.profile.log`. Cheap and
/// idempotent; called before the loop and inert unless a profiling variable is set.
pub(crate) fn init() {
    if flamewm_profiler::enabled() {
        flamewm_profiler::init_process("flamewm-wayland");
    }
}

/// Arms the one-second composition dump when `FLAMEWM_PROFILE_COMPOSE=1`.
///
/// The timer replaces the profiler's default long window for this process only: each tick drains
/// the spans, prints the ranked composition, and re-arms itself. The timer's own cost is not
/// spanned so it cannot inflate the "timers" share it is meant to report.
pub(crate) fn install_compose_dump(handle: &LoopHandle<'static, State>) {
    let compose = std::env::var("FLAMEWM_PROFILE_COMPOSE").is_ok_and(|value| value == "1");
    if !compose || !flamewm_profiler::enabled() {
        return;
    }
    let mut ticks = 0_u64;
    let inserted = handle.insert_source(
        Timer::from_duration(Duration::from_secs(1)),
        move |_, (), _state| {
            ticks = ticks.saturating_add(1);
            let report = report_window();
            if report.is_empty() {
                // A fully parked loop records no spans: the window is all wait, which is the win.
                eprintln!(
                    "FLAMEWM-WAYLAND-COMPOSE tick={ticks} window=1000ms \
                     share=wait:100.0 (no spans: no subsystem ran)"
                );
            } else {
                eprintln!("FLAMEWM-WAYLAND-COMPOSE tick={ticks} window=1000ms");
                eprint!("{report}");
            }
            TimeoutAction::ToDuration(Duration::from_secs(1))
        },
    );
    if inserted.is_err() {
        eprintln!("flamewm-wayland: compose dump timer unavailable");
    }
}

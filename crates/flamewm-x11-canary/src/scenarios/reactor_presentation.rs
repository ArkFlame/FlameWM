//! T18-T20 admission for reactor-only retained-document presentation.
//!
//! This is a source-boundary canary: the X11 canary cannot honestly observe a
//! private renderer present counter, so it admits the exact owner ordering and
//! retained dirty gate instead of inventing a pixel-only proof.

const EVENTS_SOURCE: &str = include_str!("../../../flamewm-render-x11/src/app/events.rs");
const RENDER_SOURCE: &str = include_str!("../../../flamewm-render-x11/src/app/render.rs");

fn reactor_loop_source() -> Option<&'static str> {
    let start = EVENTS_SOURCE.find("pub(crate) unsafe fn event_loop_with_reactor")?;
    let end = EVENTS_SOURCE[start..]
        .find("pub(crate) fn hit_test")
        .map(|offset| start + offset)?;
    Some(&EVENTS_SOURCE[start..end])
}

fn ordered(source: &str, first: &str, second: &str) -> bool {
    source
        .find(first)
        .zip(source.find(second))
        .is_some_and(|(first, second)| first < second)
}

/// Run the dedicated T18-T20 source-admission scenario.
pub fn run(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let _ = (canary, args);
    let Some(source) = reactor_loop_source() else {
        return (
            false,
            "scenario=T18-T20 classification=RED source_contract=missing-reactor-loop".to_owned(),
        );
    };
    let callback = "on_reactor(document)?";
    let retained_gate = "redraw_if_dirty(document)?";
    let queue_gate = "XEventsQueued";
    let t18 =
        ordered(source, callback, retained_gate) && ordered(source, retained_gate, queue_gate);
    let t19 = source.matches(retained_gate).count() == 1
        && !source.contains("self.redraw(")
        && RENDER_SOURCE.contains("if self.pending_damage.is_empty() && !stale")
        && RENDER_SOURCE.contains("return Ok(false);");
    let t20 = source.matches(callback).count() == 1
        && source.matches(retained_gate).count() == 1
        && ordered(source, callback, retained_gate);
    let pass = t18 && t19 && t20;
    (
        pass,
        format!(
            "scenario=T18-T20 classification=RED admission={} T18_reactor_only_present={} T19_noop_wake_skip={} T20_same_turn_coalesce={} observable=source-control-flow failure_mutation=missing-redraw-if-dirty-before-empty-x-queue path=source-admission",
            pass, t18, t19, t20
        ),
    )
}

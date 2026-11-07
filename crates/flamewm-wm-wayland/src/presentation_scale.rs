//! `wp_fractional_scale_v1` and `wp_presentation` for the nested winit compositor.
//!
//! Fractional scale advertises the output's own scale, so clients render at the size the output
//! actually uses. Presentation feedback is reported right after each frame is submitted, on the
//! monotonic clock; the nested host window has no vblank timestamp to report.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use smithay::delegate_fractional_scale;
use smithay::delegate_presentation;
use smithay::desktop::utils::{OutputPresentationFeedback, take_presentation_feedback_surface_tree};
use smithay::desktop::{PopupManager, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Clock, Monotonic};
use smithay::wayland::compositor::with_states;
use smithay::wayland::fractional_scale::{
    FractionalScaleHandler, FractionalScaleManagerState, with_fractional_scale,
};
use smithay::wayland::presentation::{PresentationState, Refresh};

use crate::state::State;

/// Linux `CLOCK_MONOTONIC`, the clock `wp_presentation` advertises to clients.
pub const PRESENTATION_CLOCK_ID: u32 = 1;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Registers both globals. The managers only own their globals, so they are not kept.
pub fn init(display: &DisplayHandle) {
    let _fractional = FractionalScaleManagerState::new::<State>(display);
    let _presentation = PresentationState::new::<State>(display, PRESENTATION_CLOCK_ID);
}

/// Frame period for an output mode given in millihertz; a zero rate is treated as 1 mHz.
pub fn refresh_period(millihertz: i32) -> Duration {
    Duration::from_nanos(1_000_000_000_000 / u64::from(millihertz.max(1).unsigned_abs()))
}

impl FractionalScaleHandler for State {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let scale = self.output.current_scale().fractional_scale();
        with_states(&surface, |states| {
            with_fractional_scale(states, |fractional| fractional.set_preferred_scale(scale));
        });
    }
}

delegate_fractional_scale!(State);
delegate_presentation!(State);

/// Gathers the presentation feedback of every surface this frame presents on `output`.
///
/// Call before the frame is submitted, so the callbacks belong to the frame being drawn.
pub fn collect_feedback(state: &State, output: &Output) -> OutputPresentationFeedback {
    let mut feedback = OutputPresentationFeedback::new(output);
    let mut surfaces = Vec::new();
    for (_, top) in &state.toplevels {
        let surface = top.wl_surface();
        surfaces.push(surface.clone());
        surfaces.extend(
            PopupManager::popups_for_surface(surface).map(|(popup, _)| popup.wl_surface().clone()),
        );
    }
    surfaces.extend(
        layer_map_for_output(output)
            .layers()
            .map(|layer| layer.layer_surface().wl_surface().clone()),
    );
    for surface in &surfaces {
        take_presentation_feedback_surface_tree(
            surface,
            &mut feedback,
            |_, _| Some(output.clone()),
            |_, _| wp_presentation_feedback::Kind::empty(),
        );
    }
    feedback
}

/// Reports the collected feedback as presented now, at the output's current mode rate.
pub fn presented(mut feedback: OutputPresentationFeedback, output: &Output) {
    let refresh = output
        .current_mode()
        .map_or(Duration::from_millis(16), |mode| refresh_period(mode.refresh));
    let now = Clock::<Monotonic>::new().now();
    let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    feedback.presented::<_, Monotonic>(
        now,
        Refresh::Fixed(refresh),
        seq,
        wp_presentation_feedback::Kind::empty(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_period_matches_sixty_hertz_mode() {
        assert_eq!(refresh_period(60_000), Duration::from_nanos(16_666_666));
    }

    #[test]
    fn zero_refresh_does_not_divide_by_zero() {
        assert_eq!(refresh_period(0), Duration::from_nanos(1_000_000_000_000));
    }

    #[test]
    fn presentation_clock_is_linux_monotonic() {
        assert_eq!(PRESENTATION_CLOCK_ID, 1);
    }
}

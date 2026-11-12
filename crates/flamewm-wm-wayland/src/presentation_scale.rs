//! `wp_fractional_scale_v1` and `wp_presentation` for the nested winit compositor.
//!
//! Fractional scale advertises the output's own scale, so clients render at the size the output
//! actually uses. Presentation feedback is reported right after each frame is submitted, on the
//! monotonic clock; the nested host window has no vblank timestamp to report.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use smithay::delegate_fractional_scale;
use smithay::delegate_presentation;
use smithay::desktop::utils::{
    OutputPresentationFeedback, take_presentation_feedback_surface_tree,
};
use smithay::desktop::{PopupManager, layer_map_for_output};
use smithay::output::{Output, Scale};
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Clock, Monotonic, Transform};
use smithay::wayland::compositor::{send_surface_state, with_states};
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

/// The output scale for a saved percent (100..=200 in 5% steps, validated by the settings parser).
///
/// 100 keeps the integer scale, so the default output is unchanged. Surfaces learn the scale
/// through `new_fractional_scale`, which reads it from the output.
pub fn output_scale(percent: u16) -> Scale {
    if percent == 100 {
        Scale::Integer(1)
    } else {
        Scale::Fractional(f64::from(percent) / 100.0)
    }
}

/// Logical size of a physical mode at the output scale given in percent (see [`output_scale`]).
pub fn logical_output_size(physical: (i32, i32), percent: u16) -> (i32, i32) {
    crate::output_layout::logical_size(physical, percent)
}

/// Frame period for an output mode given in millihertz; a zero rate is treated as 1 mHz.
pub fn refresh_period(millihertz: i32) -> Duration {
    Duration::from_nanos(1_000_000_000_000 / u64::from(millihertz.max(1).unsigned_abs()))
}

impl FractionalScaleHandler for State {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let scale = self.surface_scale(&surface);
        with_states(&surface, |states| {
            with_fractional_scale(states, |fractional| fractional.set_preferred_scale(scale));
        });
    }
}

delegate_fractional_scale!(State);
delegate_presentation!(State);

/// Toplevels and their popups.
fn window_surfaces(state: &State) -> Vec<WlSurface> {
    let mut surfaces = Vec::new();
    for (_, top) in &state.toplevels {
        let surface = top.wl_surface();
        surfaces.push(surface.clone());
        surfaces.extend(
            PopupManager::popups_for_surface(surface).map(|(popup, _)| popup.wl_surface().clone()),
        );
    }
    surfaces
}

/// Every surface whose scale follows an output: windows, popups, and layer surfaces on any output.
/// Layer surfaces are not limited to the primary's layer map, so a panel on a second output counts.
fn scaled_surfaces(state: &State) -> Vec<WlSurface> {
    let mut surfaces = window_surfaces(state);
    surfaces.extend(
        state
            .layer_shell
            .layer_surfaces()
            .map(|layer| layer.wl_surface().clone()),
    );
    surfaces
}

/// Every surface `output` presents: windows, popups, and the output's layer surfaces.
fn mapped_surfaces(state: &State, output: &Output) -> Vec<WlSurface> {
    let mut surfaces = window_surfaces(state);
    surfaces.extend(
        layer_map_for_output(output)
            .layers()
            .map(|layer| layer.layer_surface().wl_surface().clone()),
    );
    surfaces
}

/// Output scales and the output each mapped surface sits on, taken before a scale change.
#[cfg_attr(not(feature = "drm"), allow(dead_code))] // live only where the drm backend sets scale
pub struct ScaleSnapshot {
    outputs: Vec<(String, f64)>,
    surfaces: Vec<(WlSurface, String)>,
}

/// Snapshots the scale and the mapped surfaces of the compositor's output.
#[cfg_attr(not(feature = "drm"), allow(dead_code))] // live only where the drm backend sets scale
pub fn scale_snapshot(state: &State) -> ScaleSnapshot {
    ScaleSnapshot {
        outputs: scale_table(state),
        surfaces: scaled_surfaces(state)
            .into_iter()
            .map(|surface| {
                let name = state.output_name_of(&surface);
                (surface, name)
            })
            .collect(),
    }
}

/// Scale of every output by name. The primary's entry is its live scale, so a change made just
/// before the snapshot is seen; the other outputs come from the last layout sync.
#[cfg_attr(not(feature = "drm"), allow(dead_code))] // live only where the drm backend sets scale
fn scale_table(state: &State) -> Vec<(String, f64)> {
    let primary = state.output.name();
    let mut table = state.output_scales.clone();
    match table.iter_mut().find(|(name, _)| *name == primary) {
        Some(entry) => entry.1 = state.output_scale(),
        None => table.push((primary, state.output_scale())),
    }
    table
}

/// Re-sends the scale to every surface whose output scale changed since `before`.
///
/// Call after the output's scale is set; a surface whose scale is unchanged gets nothing.
#[cfg_attr(not(feature = "drm"), allow(dead_code))] // live only where the drm backend sets scale
pub fn notify_scale_change(state: &State, before: &ScaleSnapshot) {
    let after = scale_snapshot(state);
    for notice in scale_notices(
        &before.outputs,
        &after.outputs,
        &before.surfaces,
        &after.surfaces,
    ) {
        with_states(&notice.surface, |states| {
            with_fractional_scale(states, |fractional| {
                fractional.set_preferred_scale(notice.scale);
            });
            send_surface_state(
                &notice.surface,
                states,
                buffer_scale(notice.scale),
                Transform::Normal,
            );
        });
    }
}

/// A scale to send to one surface.
#[cfg_attr(not(feature = "drm"), allow(dead_code))] // live only where the drm backend sets scale
#[derive(Debug, Clone, PartialEq)]
pub struct ScaleNotice<S> {
    pub surface: S,
    pub scale: f64,
}

/// The surfaces that must learn a new scale, given outputs and surface placement before and after.
///
/// A surface is notified when the scale of its output now differs from the scale of the output it
/// was on before, so a surface moved to another output is included. A surface absent from
/// `surfaces_before` is new and left to the creation path, and an unchanged scale yields nothing.
#[cfg_attr(not(feature = "drm"), allow(dead_code))] // live only where the drm backend sets scale
pub fn scale_notices<S, K>(
    outputs_before: &[(K, f64)],
    outputs_after: &[(K, f64)],
    surfaces_before: &[(S, K)],
    surfaces_after: &[(S, K)],
) -> Vec<ScaleNotice<S>>
where
    S: Clone + PartialEq,
    K: PartialEq,
{
    let scale_of = |outputs: &[(K, f64)], key: &K| {
        outputs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, scale)| *scale)
    };
    surfaces_after
        .iter()
        .filter_map(|(surface, now)| {
            let (_, was) = surfaces_before.iter().find(|(s, _)| s == surface)?;
            let scale = scale_of(outputs_after, now)?;
            (scale_of(outputs_before, was) != Some(scale)).then(|| ScaleNotice {
                surface: surface.clone(),
                scale,
            })
        })
        .collect()
}

/// The integer `wl_surface.preferred_buffer_scale` for a fractional output scale.
///
/// Rounded up, so buffers are never shown below the output's density; never below 1.
#[cfg_attr(not(feature = "drm"), allow(dead_code))] // live only where the drm backend sets scale
pub fn buffer_scale(scale: f64) -> i32 {
    (scale.ceil() as i32).max(1)
}

/// Gathers the presentation feedback of every surface this frame presents on `output`.
///
/// Call before the frame is submitted, so the callbacks belong to the frame being drawn.
pub fn collect_feedback(state: &State, output: &Output) -> OutputPresentationFeedback {
    let mut feedback = OutputPresentationFeedback::new(output);
    for surface in &mapped_surfaces(state, output) {
        take_presentation_feedback_surface_tree(
            surface,
            &mut feedback,
            |_, _| Some(output.clone()),
            |_, _| wp_presentation_feedback::Kind::empty(),
        );
    }
    feedback
}

/// Reports the collected feedback as presented now (nested host: no hardware timestamp).
pub fn presented(feedback: OutputPresentationFeedback, output: &Output) {
    let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    report(
        feedback,
        output,
        monotonic_now(),
        seq,
        wp_presentation_feedback::Kind::empty(),
    );
}

fn monotonic_now() -> Duration {
    Clock::<Monotonic>::new().now().into()
}

#[cfg(feature = "drm")]
/// Reports the collected feedback as presented at the DRM vblank that completed its flip.
///
/// `time` is the kernel's `CLOCK_MONOTONIC` flip time; a realtime-stamped device has none, so the
/// clock is read instead. `sequence` is the kernel's vblank counter for the CRTC.
pub fn presented_on_vblank(
    feedback: OutputPresentationFeedback,
    output: &Output,
    time: Option<Duration>,
    sequence: u32,
) {
    let time = vblank_time(time, monotonic_now());
    let kind = wp_presentation_feedback::Kind::Vsync
        | wp_presentation_feedback::Kind::HwClock
        | wp_presentation_feedback::Kind::HwCompletion;
    report(feedback, output, time, u64::from(sequence), kind);
}

#[cfg(feature = "drm")]
/// The timestamp a vblank is reported at: the kernel flip time, or `now` when there is none.
fn vblank_time(kernel: Option<Duration>, now: Duration) -> Duration {
    kernel.unwrap_or(now)
}

fn report(
    mut feedback: OutputPresentationFeedback,
    output: &Output,
    time: Duration,
    seq: u64,
    kind: wp_presentation_feedback::Kind,
) {
    let refresh = output
        .current_mode()
        .map_or(Duration::from_millis(16), |mode| {
            refresh_period(mode.refresh)
        });
    feedback.presented::<_, Monotonic>(time, Refresh::Fixed(refresh), seq, kind);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "drm")]
    #[test]
    fn vblank_uses_kernel_flip_time_when_monotonic() {
        let flip = Duration::from_millis(5_000);
        assert_eq!(vblank_time(Some(flip), Duration::from_millis(9_999)), flip);
    }

    #[cfg(feature = "drm")]
    #[test]
    fn vblank_falls_back_to_now_without_kernel_time() {
        let now = Duration::from_millis(42);
        assert_eq!(vblank_time(None, now), now);
    }

    #[test]
    fn refresh_period_matches_sixty_hertz_mode() {
        assert_eq!(refresh_period(60_000), Duration::from_nanos(16_666_666));
    }

    #[test]
    fn zero_refresh_does_not_divide_by_zero() {
        assert_eq!(refresh_period(0), Duration::from_nanos(1_000_000_000_000));
    }

    #[test]
    fn default_scale_stays_integer_one() {
        assert!(matches!(output_scale(100), Scale::Integer(1)));
    }

    #[test]
    fn saved_percent_becomes_fractional_scale() {
        match output_scale(125) {
            Scale::Fractional(scale) => assert!((scale - 1.25).abs() < f64::EPSILON),
            other => panic!("expected fractional scale, got {other:?}"),
        }
    }

    #[test]
    fn presentation_clock_is_linux_monotonic() {
        assert_eq!(PRESENTATION_CLOCK_ID, 1);
    }

    fn notices(
        outputs_before: &[(&'static str, f64)],
        outputs_after: &[(&'static str, f64)],
        surfaces_before: &[(u32, &'static str)],
        surfaces_after: &[(u32, &'static str)],
    ) -> Vec<(u32, f64)> {
        scale_notices(
            outputs_before,
            outputs_after,
            surfaces_before,
            surfaces_after,
        )
        .into_iter()
        .map(|notice| (notice.surface, notice.scale))
        .collect()
    }

    #[test]
    fn unchanged_scale_sends_nothing() {
        let outputs = [("out", 1.25)];
        let surfaces = [(1, "out"), (2, "out")];
        assert!(notices(&outputs, &outputs, &surfaces, &surfaces).is_empty());
    }

    #[test]
    fn scale_change_reaches_every_surface_on_that_output() {
        let before = [("out", 1.0)];
        let after = [("out", 1.25)];
        let surfaces = [(1, "out"), (2, "out")];
        assert_eq!(
            notices(&before, &after, &surfaces, &surfaces),
            vec![(1, 1.25), (2, 1.25)]
        );
    }

    #[test]
    fn scale_change_skips_surfaces_on_other_outputs() {
        let before = [("a", 1.0), ("b", 1.0)];
        let after = [("a", 2.0), ("b", 1.0)];
        let surfaces = [(1, "a"), (2, "b")];
        assert_eq!(
            notices(&before, &after, &surfaces, &surfaces),
            vec![(1, 2.0)]
        );
    }

    #[test]
    fn surface_moved_to_output_with_other_scale_is_notified() {
        let outputs = [("a", 1.0), ("b", 2.0)];
        let before = [(1, "a")];
        let after = [(1, "b")];
        assert_eq!(notices(&outputs, &outputs, &before, &after), vec![(1, 2.0)]);
    }

    #[test]
    fn surface_moved_to_output_with_same_scale_is_not_notified() {
        let outputs = [("a", 1.5), ("b", 1.5)];
        let before = [(1, "a")];
        let after = [(1, "b")];
        assert!(notices(&outputs, &outputs, &before, &after).is_empty());
    }

    #[test]
    fn new_surface_is_left_to_creation_path() {
        let outputs = [("out", 1.0)];
        let before: [(u32, &str); 0] = [];
        let after = [(1, "out")];
        assert!(notices(&outputs, &outputs, &before, &after).is_empty());
    }

    #[test]
    fn buffer_scale_rounds_up_and_stays_positive() {
        assert_eq!(buffer_scale(1.0), 1);
        assert_eq!(buffer_scale(1.25), 2);
        assert_eq!(buffer_scale(2.0), 2);
        assert_eq!(buffer_scale(0.0), 1);
    }
}

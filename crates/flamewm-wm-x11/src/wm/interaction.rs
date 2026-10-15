//! Interaction orchestration: native pointer transaction / grab / effect-commit order.
//!
//! Pure orchestration over the frame reducer. No X calls, no x11rb, no geometry
//! math here: geometry lives in `frame::geometry` via `controller::reduce`.
//! The live `wm` owner applies the returned [`PointerCommit`] against the
//! server; J19 wires this module without signature churn.

use std::collections::HashMap;

use crate::client::ManagedClient;
use crate::frame::controller::{ControllerState, FrameEffect, FrameEvent, reduce, reduce_owned};
use crate::frame::coords::RootRect;
use crate::frame::geometry::FrameExtents;
use crate::frame::model::{FrameRegion, PlacementState};
use crate::size_hints::ClientSizeHints;

/// Native pointer-commit order derived from one reducer effect plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointerCommit {
    /// Grab target + cursor when the plan opens with `Grab`.
    pub grab: Option<(u32, flamewm_render_core::CursorKind)>,
    /// Plan ends with (or contains) `Ungrab`.
    pub ungrab: bool,
    /// Grab failed at commit time: session must cancel to idle.
    pub abort_on_grab_fail: bool,
    /// Ordered effect plan, verbatim from `reduce`.
    pub effects: Vec<FrameEffect>,
}

impl PointerCommit {
    #[must_use]
    pub fn plan(effects: Vec<FrameEffect>) -> Self {
        let mut grab = None;
        let mut ungrab = false;
        for effect in &effects {
            match *effect {
                FrameEffect::Grab { window, cursor } => {
                    if grab.is_none() {
                        grab = Some((window, cursor));
                    }
                }
                FrameEffect::Ungrab => ungrab = true,
                _ => {}
            }
        }
        Self {
            grab,
            ungrab,
            abort_on_grab_fail: grab.is_some(),
            effects,
        }
    }
}

/// Build a controller snapshot from explicit live state. No hidden reads.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn build_controller(
    client: u32,
    grab: u32,
    placement: PlacementState,
    work_area: RootRect,
    screen_rect: RootRect,
    hints: ClientSizeHints,
    extents: FrameExtents,
    session: crate::frame::session::InteractionSession,
    region_map: HashMap<u32, FrameRegion>,
) -> ControllerState {
    let mut controller = ControllerState::new(client, grab, placement, work_area, hints, extents)
        .with_screen_rect(screen_rect)
        .with_registry(region_map);
    controller.session = session;
    controller
}

/// One reducer step plus its pointer-commit order. Pure.
#[must_use]
pub fn step(state: &ControllerState, event: FrameEvent) -> (ControllerState, PointerCommit) {
    let (next, effects) = reduce(state, event);
    let commit = PointerCommit::plan(effects);
    (next, commit)
}

/// Owned variant of [`step`] for the live pointer hot path: no state clone.
#[must_use]
pub fn step_owned(state: ControllerState, event: FrameEvent) -> (ControllerState, PointerCommit) {
    let (next, effects) = reduce_owned(state, event);
    let commit = PointerCommit::plan(effects);
    (next, commit)
}

/// Commit the next placement/session into the session map and client mirror.
pub fn store_controller(
    sessions: &mut HashMap<u32, ControllerState>,
    client_state: Option<&mut ManagedClient>,
    client: u32,
    next: &ControllerState,
) {
    let entry = sessions.entry(client).or_insert_with(|| next.clone());
    entry.session = next.session;
    entry.placement = next.placement;
    entry.last_title_click = next.last_title_click;
    if let Some(state) = client_state {
        state.apply_placement(next.placement);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::coords::RootPoint;
    use crate::frame::model::PlacementMode;

    const WA: RootRect = RootRect::new(0, 0, 1920, 1080);
    const START: RootRect = RootRect::new(100, 100, 400, 300);

    fn floating_placement() -> PlacementState {
        PlacementState {
            mode: PlacementMode::Floating,
            current: START,
            floating_restore: START,
            resume: None,
        }
    }

    fn build(placement: PlacementState) -> ControllerState {
        let mut region_map = HashMap::new();
        region_map.insert(1, FrameRegion::TitleDrag);
        build_controller(
            7,
            71,
            placement,
            WA,
            WA,
            ClientSizeHints::default(),
            FrameExtents::new(31, 1),
            crate::frame::session::InteractionSession::Idle,
            region_map,
        )
    }

    fn title_release(state: &ControllerState, time_ms: u32) -> (ControllerState, PointerCommit) {
        let pointer = RootPoint::new(150, 120);
        let (pressed, _) = step(state, FrameEvent::Press { source: 1, pointer });
        step(&pressed, FrameEvent::Release { pointer, time_ms })
    }

    #[test]
    fn title_double_click_survives_store_and_rebuild() {
        let mut sessions = HashMap::new();
        // Map entry already exists from an earlier event (as in the live wm).
        sessions.insert(7, build(floating_placement()));

        let (first, _) = title_release(&build(floating_placement()), 1000);
        assert!(first.last_title_click.is_some());
        store_controller(&mut sessions, None, 7, &first);
        assert!(
            sessions[&7].last_title_click.is_some(),
            "store_controller must persist last_title_click"
        );

        // Rebuild from stored state, seeding the click as the live wm does.
        let mut rebuilt = build(sessions[&7].placement);
        rebuilt.last_title_click = sessions[&7].last_title_click;
        let (second, _) = title_release(&rebuilt, 1300);
        assert_eq!(second.placement.mode, PlacementMode::Maximized);
        assert!(second.last_title_click.is_none());
    }

    #[test]
    fn store_without_seeded_click_does_not_retain_stale_click() {
        let mut sessions = HashMap::new();
        sessions.insert(7, build(floating_placement()));
        let (first, _) = title_release(&build(floating_placement()), 1000);
        store_controller(&mut sessions, None, 7, &first);
        let (second, _) = title_release(&build(floating_placement()), 1300);
        // Rebuilt without seeding: a click is not carried, so this is a first click.
        assert_eq!(second.placement.mode, PlacementMode::Floating);
    }
}

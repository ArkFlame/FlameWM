//! Move/resize release policy and move-summary trace.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReleaseAction {
    None,
    Maximize,
    Snap(crate::frame::model::SnapTarget),
}

pub(super) fn drag_activated(session: &InteractionSession) -> bool {
    match session {
        InteractionSession::Move(move_session) => move_session.anchor.is_activated(),
        InteractionSession::Resize(_) => true,
        _ => false,
    }
}

pub(super) fn activated_move(session: Option<MoveSession>) -> Option<MoveSession> {
    match session {
        Some(session) if session.anchor.is_activated() => Some(session),
        _ => None,
    }
}

pub(super) fn release_action(target: SnapTarget) -> ReleaseAction {
    match target {
        SnapTarget::None => ReleaseAction::None,
        SnapTarget::Maximize => ReleaseAction::Maximize,
        SnapTarget::LeftHalf => ReleaseAction::Snap(crate::frame::model::SnapTarget::Left),
        SnapTarget::RightHalf => ReleaseAction::Snap(crate::frame::model::SnapTarget::Right),
        SnapTarget::TopHalf => ReleaseAction::Snap(crate::frame::model::SnapTarget::Top),
        SnapTarget::BottomHalf => ReleaseAction::Snap(crate::frame::model::SnapTarget::Bottom),
        SnapTarget::TopLeftQuarter => ReleaseAction::Snap(crate::frame::model::SnapTarget::TopLeft),
        SnapTarget::TopRightQuarter => {
            ReleaseAction::Snap(crate::frame::model::SnapTarget::TopRight)
        }
        SnapTarget::BottomLeftQuarter => {
            ReleaseAction::Snap(crate::frame::model::SnapTarget::BottomLeft)
        }
        SnapTarget::BottomRightQuarter => {
            ReleaseAction::Snap(crate::frame::model::SnapTarget::BottomRight)
        }
    }
}

pub(super) fn release_action_for_move(
    session: Option<MoveSession>,
    placement: Option<PlacementMode>,
    target: SnapTarget,
) -> ReleaseAction {
    if activated_move(session).is_none() || placement != Some(PlacementMode::Floating) {
        return ReleaseAction::None;
    }
    release_action(target)
}

pub(super) fn emit_move_summary() {
    flamewm_debug::emit(
        flamewm_debug::WM_FRAME_MOVE_COMMITS,
        std::time::Duration::from_secs(1),
        || {
            let c = move_counters();
            format!(
                "received={} coalesced={} commit={} snap_target_changes={} snap_target_last={} preview_updates={} toggle_max={} client_message_maximize={} state_mutations={} maximized_transitions={} resize_snap_blocked={}",
                c.received.load(),
                c.coalesced.load(),
                c.committed.load(),
                read(&c.snap_target_changes),
                read(&c.last_target),
                read(&c.preview_updates),
                read(&c.toggle_max_total),
                read(&c.client_message_maximize_total),
                read(&c.state_mutations_total),
                read(&c.maximized_transitions),
                read(&c.resize_snap_blocked_total),
            )
        },
    );
}

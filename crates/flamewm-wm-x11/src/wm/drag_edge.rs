//! Edge-drag workspace switching: dwell, entry, and edge hit-testing.
use super::*;

/// An intentional titlebar drag must dwell at the outer root edge before
/// traversing workspaces. Internal monitor boundaries never count.
pub(super) const DRAG_EDGE_DWELL_MS: u32 = 220;
// Preserve proximity to the opposite boundary without immediately retriggering.
pub(super) const DRAG_EDGE_INSET_PX: i32 = 24;
// Mouse-only workspace transitions: Left/Right share one 500ms cooldown;
// Top/Bottom share a separate cooldown. Keyboard and pager actions are exempt.
pub(super) const MOUSE_WORKSPACE_COOLDOWN_MS: u64 = 500;
pub(super) const DRAG_EDGE_HIT_PX: i32 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DragEdge {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MouseWorkspaceAxis {
    Horizontal,
    Vertical,
}

impl DragEdge {
    pub(super) fn axis(self) -> MouseWorkspaceAxis {
        match self {
            Self::Left | Self::Right => MouseWorkspaceAxis::Horizontal,
            Self::Top | Self::Bottom => MouseWorkspaceAxis::Vertical,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct MouseWorkspaceCooldowns {
    pub(super) horizontal: Option<std::time::Instant>,
    pub(super) vertical: Option<std::time::Instant>,
}

impl MouseWorkspaceCooldowns {
    pub(super) fn remaining(
        self,
        axis: MouseWorkspaceAxis,
        now: std::time::Instant,
    ) -> std::time::Duration {
        let timestamp = match axis {
            MouseWorkspaceAxis::Horizontal => self.horizontal,
            MouseWorkspaceAxis::Vertical => self.vertical,
        };
        timestamp.map_or(std::time::Duration::ZERO, |last| {
            std::time::Duration::from_millis(MOUSE_WORKSPACE_COOLDOWN_MS)
                .saturating_sub(now.saturating_duration_since(last))
        })
    }

    pub(super) fn record(&mut self, axis: MouseWorkspaceAxis, now: std::time::Instant) {
        match axis {
            MouseWorkspaceAxis::Horizontal => self.horizontal = Some(now),
            MouseWorkspaceAxis::Vertical => self.vertical = Some(now),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EdgeDragPending {
    pub(super) client: Window,
    pub(super) side: DragEdge,
    pub(super) since: std::time::Instant,
}

/// This policy sees only pointer moves from an already-activated Move session;
/// normal pointer motion, resize and titlebar clicks never call it.
pub(super) fn drag_edge_at(pointer: RootPoint, root: Rect) -> Option<DragEdge> {
    // Only the outer root boundaries are eligible, never a monitor seam.
    // At a corner choose the closest edge (horizontal wins an exact tie),
    // avoiding two competing dwell timers for the same move session.
    let x = pointer.x;
    let y = pointer.y;
    let right = root.right().saturating_sub(1);
    let bottom = root.bottom().saturating_sub(1);
    if x < root.x || x > right || y < root.y || y > bottom {
        return None;
    }
    let mut candidate: Option<(i32, DragEdge)> = None;
    if root.width >= (DRAG_EDGE_INSET_PX * 3) as u32 {
        if x <= root.x.saturating_add(DRAG_EDGE_HIT_PX) {
            candidate = Some((x.saturating_sub(root.x), DragEdge::Left));
        } else if x >= right.saturating_sub(DRAG_EDGE_HIT_PX) {
            candidate = Some((right.saturating_sub(x), DragEdge::Right));
        }
    }
    if root.height >= (DRAG_EDGE_INSET_PX * 3) as u32 {
        let vertical = if y <= root.y.saturating_add(DRAG_EDGE_HIT_PX) {
            Some((y.saturating_sub(root.y), DragEdge::Top))
        } else if y >= bottom.saturating_sub(DRAG_EDGE_HIT_PX) {
            Some((bottom.saturating_sub(y), DragEdge::Bottom))
        } else {
            None
        };
        if let Some(vertical) = vertical {
            if candidate.is_none_or(|(distance, _)| vertical.0 < distance) {
                candidate = Some(vertical);
            }
        }
    }
    candidate.map(|(_, side)| side)
}

pub(super) fn drag_edge_target(current: usize, count: usize, side: DragEdge) -> Option<usize> {
    // Horizontal mouse traversal wraps ONLY inside the current row; vertical
    // traversal wraps ONLY between the same column in the two-row grid.
    let position = flamewm_api::workspace::grid_position(current, count)?;
    let columns = flamewm_api::workspace::workspace_columns(count);
    let row_start = position.row.checked_mul(columns)?;
    let row_end = row_start.checked_add(columns)?.min(count);
    let target = match side {
        DragEdge::Left if current == row_start => row_end - 1,
        DragEdge::Left => current - 1,
        DragEdge::Right if current + 1 == row_end => row_start,
        DragEdge::Right => current + 1,
        DragEdge::Top | DragEdge::Bottom => {
            let opposite_row = (position.row + 1) % flamewm_api::workspace::WORKSPACE_ROWS;
            return flamewm_api::workspace::grid_index(opposite_row, position.column, count)
                .filter(|target| *target != current);
        }
    };
    (target != current).then_some(target)
}

pub(super) fn drag_edge_destination(
    pointer: RootPoint,
    root: Rect,
    side: DragEdge,
    frame_height: i32,
) -> RootPoint {
    let inset_x = DRAG_EDGE_INSET_PX.min(root.width.saturating_sub(1) as i32 / 3);
    let inset_y = DRAG_EDGE_INSET_PX.min(root.height.saturating_sub(1) as i32 / 3);
    let x = match side {
        DragEdge::Left => root.right().saturating_sub(1).saturating_sub(inset_x),
        DragEdge::Right => root.x.saturating_add(inset_x),
        DragEdge::Top | DragEdge::Bottom => pointer.x.clamp(root.x, root.right().saturating_sub(1)),
    };
    let y = match side {
        DragEdge::Top => {
            // Warping a tall dragged window to the bottom 24 px would place
            // its titlebar off the active frame: keep at least half the frame
            // visible while retaining the cursor on the titlebar grip.
            let visible = (frame_height.max(1) / 2).max(32);
            root.bottom()
                .saturating_sub(1)
                .saturating_sub(inset_y.max(visible.saturating_sub(16)))
                .clamp(
                    root.y.saturating_add(inset_y),
                    root.bottom().saturating_sub(1).saturating_sub(inset_y),
                )
        }
        DragEdge::Bottom => root.y.saturating_add(inset_y),
        DragEdge::Left | DragEdge::Right => {
            pointer.y.clamp(root.y, root.bottom().saturating_sub(1))
        }
    };
    RootPoint::new(x, y)
}

pub(super) fn drag_edge_ready(
    previous: Option<EdgeDragPending>,
    client: Window,
    side: DragEdge,
    now: std::time::Instant,
) -> (Option<EdgeDragPending>, bool) {
    if let Some(pending) = previous {
        if pending.client == client && pending.side == side {
            return (
                Some(pending),
                now.saturating_duration_since(pending.since)
                    >= std::time::Duration::from_millis(u64::from(DRAG_EDGE_DWELL_MS)),
            );
        }
    }
    (
        Some(EdgeDragPending {
            client,
            side,
            since: now,
        }),
        false,
    )
}

/// Arms exactly one reactor deadline while a titlebar move is held at an edge.
/// No busy polling: the ordinary X11 reactor blocks until this deadline or an
/// input event. A still pointer can therefore complete the dwell without a
/// second `MotionNotify`.
pub(super) fn drag_edge_wait(
    pending: Option<EdgeDragPending>,
    cooldowns: MouseWorkspaceCooldowns,
    now: std::time::Instant,
    wait: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    let Some(pending) = pending else {
        return wait;
    };
    let dwell_remaining = std::time::Duration::from_millis(u64::from(DRAG_EDGE_DWELL_MS))
        .saturating_sub(now.saturating_duration_since(pending.since));
    // Wake once BOTH dwell and cooldown have expired (no zero-timeout loop).
    let remaining = dwell_remaining.max(cooldowns.remaining(pending.side.axis(), now));
    Some(wait.map_or(remaining, |other| other.min(remaining)))
}

/// Keep the grabbed titlebar and most of the frame visible on the arrival
/// side. Reusing the original pointer-to-frame offset can leave nearly the
/// entire frame offscreen after the pointer wraps; the controller will rebase
/// the grip at the new edge instead.
pub(super) fn drag_entry_frame_x(pointer: RootPoint, width: i32, side: DragEdge) -> i32 {
    let width = width.max(1);
    let grip = (width / 2).clamp(1, DRAG_EDGE_INSET_PX - 8);
    match side {
        DragEdge::Right => pointer.x.saturating_sub(grip),
        DragEdge::Left => pointer.x.saturating_sub(width.saturating_sub(grip)),
        DragEdge::Top | DragEdge::Bottom => unreachable!("vertical transfers preserve x"),
    }
}

/// Vertical crossings keep the cursor in the titlebar after the warp. The
/// destination frame stays under the pointer rather than abandoning the grab.
pub(super) fn drag_entry_frame_y(pointer: RootPoint, side: DragEdge) -> i32 {
    match side {
        DragEdge::Top | DragEdge::Bottom => pointer.y.saturating_sub(16),
        DragEdge::Left | DragEdge::Right => unreachable!("horizontal transfers preserve y"),
    }
}

/// Separate renderer work from the atomic X11 server-grab phase. The move
/// reducer owns geometry; only its X11-native position/notify effects may run
/// while other X clients are paused. In particular, `PaintChrome` uses another
/// connection and must run after `UngrabServer` has been flushed.
pub(super) fn defer_drag_arrival_paint(
    mut commit: interaction::PointerCommit,
) -> Option<(interaction::PointerCommit, bool)> {
    if commit.grab.is_some()
        || commit.ungrab
        || !commit.effects.iter().all(|effect| {
            matches!(
                effect,
                FrameEffect::MoveFrameRoot { .. }
                    | FrameEffect::NotifyClientRoot { .. }
                    | FrameEffect::PaintChrome
                    | FrameEffect::Noop
            )
        })
    {
        return None;
    }
    let paint = commit
        .effects
        .iter()
        .any(|effect| matches!(effect, FrameEffect::PaintChrome));
    commit
        .effects
        .retain(|effect| !matches!(effect, FrameEffect::PaintChrome));
    Some((commit, paint))
}

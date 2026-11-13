//! Super+arrow keyboard tiling. `flamewm_window_core::keyboard_tile::next_tile` owns the
//! transition; this module applies it through the pointer snap, maximize and restore paths.

use flamewm_window_core::SnapTarget as CoreTile;
use flamewm_window_core::edge_resize::{Axis, move_shared_edge};
use flamewm_window_core::keyboard_tile::{Direction, next_tile};

use super::*;
use crate::client::SnapResize;
use crate::frame::model::SnapTarget as FrameSnap;

/// Shared-edge move per Super+Ctrl+Alt+arrow press, in frame pixels.
const RESIZE_STEP: i32 = 24;
/// Smallest tile edge an edge-tile resize may leave, as on the Wayland engine.
const MIN_TILE_EXTENT: i32 = 120;

/// Un-margined tile of a snapped client: its stored resize while the placement still matches
/// it, otherwise the canonical snap tile for `target` in `work`.
fn tile_for(
    stored: Option<SnapResize>,
    target: FrameSnap,
    current: RootRect,
    canonical: RootRect,
) -> RootRect {
    match stored {
        Some(s) if s.target == target && s.rect == current => s.tile,
        _ => canonical,
    }
}

/// Canonical un-margined snap tile for `target` in `work`, from the shared window-core geometry.
fn canonical_tile(work: RootRect, target: FrameSnap) -> RootRect {
    let work = flamewm_api::Rect::new(work.x, work.y, work.w, work.h);
    let rect = flamewm_window_core::snap_geometry(
        target.core(),
        work,
        flamewm_api::Size::new(work.width, work.height),
    );
    RootRect::new(rect.x, rect.y, rect.width, rect.height)
}

/// Tile state the window currently occupies. Fullscreen is not tileable and yields `None`.
fn tile_state(mode: PlacementMode) -> Option<CoreTile> {
    Some(match mode {
        PlacementMode::Floating => CoreTile::None,
        PlacementMode::Maximized => CoreTile::Maximize,
        PlacementMode::Snapped(target) => target.core(),
        PlacementMode::Fullscreen => return None,
    })
}

/// Snap target for a tile state. `None` and `Maximize` are not snap targets.
fn snap_for(tile: CoreTile) -> Option<FrameSnap> {
    Some(match tile {
        CoreTile::LeftHalf => FrameSnap::Left,
        CoreTile::RightHalf => FrameSnap::Right,
        CoreTile::TopHalf => FrameSnap::Top,
        CoreTile::BottomHalf => FrameSnap::Bottom,
        CoreTile::TopLeftQuarter => FrameSnap::TopLeft,
        CoreTile::TopRightQuarter => FrameSnap::TopRight,
        CoreTile::BottomLeftQuarter => FrameSnap::BottomLeft,
        CoreTile::BottomRightQuarter => FrameSnap::BottomRight,
        CoreTile::None | CoreTile::Maximize => return None,
    })
}

impl<C: Connection> Wm<'_, C> {
    /// Un-margined tile of a snapped client with its work area and workspace; `None` when the
    /// client is not snapped.
    fn snapped_tile_of(
        &mut self,
        client: Window,
    ) -> Option<(FrameSnap, RootRect, RootRect, usize)> {
        let managed = self.clients.get(&client)?;
        let PlacementMode::Snapped(target) = managed.placement.mode else {
            return None;
        };
        let (stored, current, workspace) = (
            managed.snap_resize,
            managed.placement.current,
            managed.workspace,
        );
        let work = self.controller_for(client)?.work_area;
        let tile = tile_for(stored, target, current, canonical_tile(work, target));
        Some((target, tile, work, workspace))
    }

    /// Moves the shared edge of the active snapped tile one step toward `direction`. The snapped
    /// neighbour on that seam moves by the same amount. No neighbour is a no-op.
    pub(super) fn keyboard_tile_resize(&mut self, direction: Direction) -> Result<(), ReplyError> {
        let Some(client) = self.active else {
            return Ok(());
        };
        let Some((target, tile, work, workspace)) = self.snapped_tile_of(client) else {
            return Ok(());
        };
        let vertical = matches!(direction, Direction::Left | Direction::Right);
        let forward = matches!(direction, Direction::Right | Direction::Down);
        let axis = if vertical {
            Axis::Vertical
        } else {
            Axis::Horizontal
        };
        let shared = match (vertical, forward) {
            (true, true) => tile.x + tile.w,
            (true, false) => tile.x,
            (false, true) => tile.y + tile.h,
            (false, false) => tile.y,
        };
        let edge = shared + if forward { RESIZE_STEP } else { -RESIZE_STEP };
        let mut partners: Vec<Window> = Vec::new();
        for (&other, managed) in &self.clients {
            if other != client
                && managed.workspace == workspace
                && matches!(managed.placement.mode, PlacementMode::Snapped(_))
            {
                partners.push(other);
            }
        }
        partners.sort_unstable();
        let to_api = |r: RootRect| flamewm_api::Rect::new(r.x, r.y, r.w, r.h);
        for other in partners {
            let Some((other_target, other_tile, other_work, _)) = self.snapped_tile_of(other)
            else {
                continue;
            };
            if other_work != work {
                continue;
            }
            let (first, second) = if forward {
                (tile, other_tile)
            } else {
                (other_tile, tile)
            };
            let Some((a, b)) =
                move_shared_edge(to_api(first), to_api(second), axis, edge, MIN_TILE_EXTENT)
            else {
                continue;
            };
            let (a_client, a_target, b_client, b_target) = if forward {
                (client, target, other, other_target)
            } else {
                (other, other_target, client, target)
            };
            let a = RootRect::new(a.x, a.y, a.width, a.height);
            let b = RootRect::new(b.x, b.y, b.width, b.height);
            self.apply_snap_tile(a_client, a_target, a)?;
            self.apply_snap_tile(b_client, b_target, b)?;
            return Ok(());
        }
        Ok(())
    }

    /// Applies one edge-tile resize result and records it for the placement it produced.
    fn apply_snap_tile(
        &mut self,
        client: Window,
        target: FrameSnap,
        tile: RootRect,
    ) -> Result<(), ReplyError> {
        self.reduce_and_execute(client, FrameEvent::SnapTile { tile })?;
        if let Some(managed) = self.clients.get_mut(&client) {
            managed.snap_resize = Some(SnapResize {
                target,
                tile,
                rect: managed.placement.current,
            });
        }
        self.mark_windows();
        self.publish_window_state(client)
    }

    /// Moves the active window one tile step in `direction`. A missing active window is a no-op.
    pub(super) fn keyboard_tile(&mut self, direction: Direction) -> Result<(), ReplyError> {
        let Some(client) = self.active else {
            return Ok(());
        };
        let Some(mode) = self.clients.get(&client).map(|state| state.placement.mode) else {
            return Ok(());
        };
        let Some(current) = tile_state(mode) else {
            return Ok(());
        };
        match next_tile(current, direction) {
            None => self.return_to_floating(client),
            Some(CoreTile::Maximize) if mode == PlacementMode::Maximized => Ok(()),
            Some(CoreTile::Maximize) => self.commit_maximize(client),
            Some(tile) => {
                let Some(target) = snap_for(tile) else {
                    return Ok(());
                };
                self.reduce_and_execute(client, FrameEvent::Snap { target })?;
                self.mark_windows();
                self.publish_window_state(client)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_modes_map_to_tile_states() {
        assert_eq!(tile_state(PlacementMode::Floating), Some(CoreTile::None));
        assert_eq!(
            tile_state(PlacementMode::Maximized),
            Some(CoreTile::Maximize)
        );
        assert_eq!(
            tile_state(PlacementMode::Snapped(FrameSnap::BottomRight)),
            Some(CoreTile::BottomRightQuarter)
        );
        assert_eq!(tile_state(PlacementMode::Fullscreen), None);
    }

    #[test]
    fn every_tile_snap_round_trips_to_the_frame_target() {
        let tiles = [
            CoreTile::LeftHalf,
            CoreTile::RightHalf,
            CoreTile::TopHalf,
            CoreTile::BottomHalf,
            CoreTile::TopLeftQuarter,
            CoreTile::TopRightQuarter,
            CoreTile::BottomLeftQuarter,
            CoreTile::BottomRightQuarter,
        ];
        for tile in tiles {
            let target = snap_for(tile).expect("snap tile");
            assert_eq!(tile_state(PlacementMode::Snapped(target)), Some(tile));
        }
        assert_eq!(snap_for(CoreTile::None), None);
        assert_eq!(snap_for(CoreTile::Maximize), None);
    }

    #[test]
    fn stored_resize_is_used_only_while_the_placement_still_matches() {
        let canonical = RootRect::new(0, 0, 500, 700);
        let resized = RootRect::new(0, 0, 600, 700);
        let stored = Some(SnapResize {
            target: FrameSnap::Left,
            tile: resized,
            rect: RootRect::new(0, 0, 580, 680),
        });
        let current = RootRect::new(0, 0, 580, 680);
        assert_eq!(
            tile_for(stored, FrameSnap::Left, current, canonical),
            resized
        );
        // A later snap or move changes the placement, so the canonical tile wins again.
        assert_eq!(
            tile_for(
                stored,
                FrameSnap::Left,
                RootRect::new(0, 0, 500, 700),
                canonical
            ),
            canonical
        );
        assert_eq!(
            tile_for(stored, FrameSnap::Right, current, canonical),
            canonical
        );
        assert_eq!(
            canonical_tile(RootRect::new(0, 0, 1000, 700), FrameSnap::Left),
            canonical
        );
    }

    #[test]
    fn left_from_a_floating_window_snaps_to_the_left_half() {
        let tile = next_tile(
            tile_state(PlacementMode::Floating).unwrap(),
            Direction::Left,
        )
        .and_then(snap_for);
        assert_eq!(tile, Some(FrameSnap::Left));
    }
}

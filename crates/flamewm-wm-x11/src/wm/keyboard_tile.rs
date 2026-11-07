//! Super+arrow keyboard tiling. `flamewm_window_core::keyboard_tile::next_tile` owns the
//! transition; this module applies it through the pointer snap, maximize and restore paths.

use flamewm_window_core::SnapTarget as CoreTile;
use flamewm_window_core::keyboard_tile::{Direction, next_tile};

use super::*;
use crate::frame::model::SnapTarget as FrameSnap;

/// Tile state the window currently occupies. Fullscreen is not tileable and yields `None`.
fn tile_state(mode: PlacementMode) -> Option<CoreTile> {
    Some(match mode {
        PlacementMode::Floating => CoreTile::None,
        PlacementMode::Maximized => CoreTile::Maximize,
        PlacementMode::Snapped(target) => match target {
            FrameSnap::Left => CoreTile::LeftHalf,
            FrameSnap::Right => CoreTile::RightHalf,
            FrameSnap::Top => CoreTile::TopHalf,
            FrameSnap::Bottom => CoreTile::BottomHalf,
            FrameSnap::TopLeft => CoreTile::TopLeftQuarter,
            FrameSnap::TopRight => CoreTile::TopRightQuarter,
            FrameSnap::BottomLeft => CoreTile::BottomLeftQuarter,
            FrameSnap::BottomRight => CoreTile::BottomRightQuarter,
        },
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
    fn left_from_a_floating_window_snaps_to_the_left_half() {
        let tile = next_tile(
            tile_state(PlacementMode::Floating).unwrap(),
            Direction::Left,
        )
        .and_then(snap_for);
        assert_eq!(tile, Some(FrameSnap::Left));
    }
}

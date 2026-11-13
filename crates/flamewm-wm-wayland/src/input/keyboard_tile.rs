//! Super+arrow keyboard tiling. `flamewm_window_core::keyboard_tile::next_tile` owns the
//! transition; this module applies it through the model's snap, maximize and restore paths. The
//! `Tile*` actions carry the Super+arrow bindings, as on the X11 engine.

use flamewm_api::shortcuts::{
    ACTION_TILE_DOWN, ACTION_TILE_LEFT, ACTION_TILE_RESIZE_DOWN, ACTION_TILE_RESIZE_LEFT,
    ACTION_TILE_RESIZE_RIGHT, ACTION_TILE_RESIZE_UP, ACTION_TILE_RIGHT, ACTION_TILE_UP,
};
use flamewm_window_core::SnapTarget;
use flamewm_window_core::keyboard_tile::{Direction, next_tile};

use super::*;
use crate::model::Mode;

/// Tile direction for a shortcut action, or `None` for any other action.
pub(super) fn tile_action(action: &str) -> Option<Direction> {
    match action {
        ACTION_TILE_LEFT => Some(Direction::Left),
        ACTION_TILE_RIGHT => Some(Direction::Right),
        ACTION_TILE_UP => Some(Direction::Up),
        ACTION_TILE_DOWN => Some(Direction::Down),
        _ => None,
    }
}

/// The floating geometry a snap restores to: a floating window is its own rect, a tiled or
/// maximized one keeps the rect it had before it left floating.
pub(super) fn floating_source(mode: Mode, rect: Rect, restore: Rect) -> Rect {
    if mode == Mode::Normal { rect } else { restore }
}

/// Tile state a window occupies. Fullscreen is not tileable and yields `None`.
pub(super) fn tile_state(mode: Mode) -> Option<SnapTarget> {
    match mode {
        Mode::Normal => Some(SnapTarget::None),
        Mode::Snapped(target) => Some(target),
        Mode::Maximized => Some(SnapTarget::Maximize),
        Mode::Fullscreen => None,
    }
}

impl State {
    /// Moves the focused window one tile step in `direction`. No focused window is a no-op.
    pub(super) fn keyboard_tile(&mut self, direction: Direction) {
        let Some(id) = self.focused else {
            return;
        };
        let Some(window) = self.model.get(id) else {
            return;
        };
        let (mode, rect, restore) = (window.mode, window.rect, window.restore);
        let Some(current) = tile_state(mode) else {
            return;
        };
        self.refresh_dock_struts();
        let work = self.work_area_of(id);
        match next_tile(current, direction) {
            // Down from a bottom tile, or from maximized, restores the floating geometry.
            None => match mode {
                Mode::Maximized => self.model.toggle_maximize(id, work),
                Mode::Snapped(_) => self.model.restore_floating(id),
                Mode::Normal | Mode::Fullscreen => return,
            },
            Some(SnapTarget::None) => return,
            Some(SnapTarget::Maximize) => {
                if mode == Mode::Maximized {
                    return;
                }
                self.model.toggle_maximize(id, work);
            }
            Some(target) => {
                self.model
                    .snap_to(id, target, work, floating_source(mode, rect, restore));
            }
        }
        self.apply_geometry(id);
    }
}

/// Shared-edge move per Super+Ctrl+Alt+arrow press, in frame pixels.
const RESIZE_STEP: i32 = 24;

/// Edge-tile resize direction for a shortcut action, or `None` for any other action.
pub(super) fn resize_action(action: &str) -> Option<Direction> {
    match action {
        ACTION_TILE_RESIZE_LEFT => Some(Direction::Left),
        ACTION_TILE_RESIZE_RIGHT => Some(Direction::Right),
        ACTION_TILE_RESIZE_UP => Some(Direction::Up),
        ACTION_TILE_RESIZE_DOWN => Some(Direction::Down),
        _ => None,
    }
}

impl State {
    /// Moves the shared edge of the focused snapped tile. No snapped neighbour is a no-op.
    pub(super) fn keyboard_tile_resize(&mut self, direction: Direction) {
        let Some(id) = self.focused else {
            return;
        };
        let work = self.work_area_of(id);
        if let Some(partner) = self
            .model
            .resize_snapped_edge(id, direction, RESIZE_STEP, work)
        {
            self.apply_geometry(id);
            self.apply_geometry(partner);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Rect;
    use flamewm_api::shortcuts::{ACTION_FOCUS_LEFT, ACTION_FOCUS_UP};
    use flamewm_window_core::keyboard_tile::Direction as D;

    #[test]
    fn only_tile_actions_map_to_tile_directions() {
        assert_eq!(tile_action(ACTION_TILE_LEFT), Some(D::Left));
        assert_eq!(tile_action(ACTION_TILE_RIGHT), Some(D::Right));
        assert_eq!(tile_action(ACTION_TILE_UP), Some(D::Up));
        assert_eq!(tile_action(ACTION_TILE_DOWN), Some(D::Down));
        assert_eq!(tile_action(ACTION_FOCUS_LEFT), None);
        assert_eq!(tile_action(ACTION_FOCUS_UP), None);
        assert_eq!(tile_action("MoveWindowLeft"), None);
        assert_eq!(tile_action("WorkspaceLeft"), None);
    }

    #[test]
    fn only_resize_actions_map_to_edge_resize_directions() {
        assert_eq!(resize_action(ACTION_TILE_RESIZE_LEFT), Some(D::Left));
        assert_eq!(resize_action(ACTION_TILE_RESIZE_DOWN), Some(D::Down));
        assert_eq!(resize_action(ACTION_TILE_LEFT), None);
        assert_eq!(resize_action(ACTION_FOCUS_LEFT), None);
    }

    #[test]
    fn placement_modes_map_to_tile_states() {
        assert_eq!(tile_state(Mode::Normal), Some(SnapTarget::None));
        assert_eq!(tile_state(Mode::Maximized), Some(SnapTarget::Maximize));
        assert_eq!(
            tile_state(Mode::Snapped(SnapTarget::BottomRightQuarter)),
            Some(SnapTarget::BottomRightQuarter)
        );
        assert_eq!(tile_state(Mode::Fullscreen), None);
    }

    #[test]
    fn floating_window_steps_left_then_down_reaches_bottom_left_quarter() {
        let left = next_tile(tile_state(Mode::Normal).unwrap(), D::Left);
        assert_eq!(left, Some(SnapTarget::LeftHalf));
        let down = next_tile(
            tile_state(Mode::Snapped(SnapTarget::LeftHalf)).unwrap(),
            D::Down,
        );
        assert_eq!(down, Some(SnapTarget::BottomLeftQuarter));
    }

    #[test]
    fn restore_source_is_the_floating_rect_only_for_floating_windows() {
        let rect = Rect::new(5, 6, 700, 500);
        let restore = Rect::new(1, 2, 300, 200);
        assert_eq!(floating_source(Mode::Normal, rect, restore), rect);
        assert_eq!(
            floating_source(Mode::Snapped(SnapTarget::TopHalf), rect, restore),
            restore
        );
        assert_eq!(floating_source(Mode::Maximized, rect, restore), restore);
    }
}

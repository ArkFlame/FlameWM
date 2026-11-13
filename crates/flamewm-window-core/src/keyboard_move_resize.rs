//! Keyboard move (`Alt+F7`) and resize (`Alt+F8`) grab, after xfwm4's keyboard move/resize.
//! Arrows step the window (move: 16 px, resize: 10 px per press), Return commits, and Escape
//! restores the rect the grab started from. The rect stays inside the work area. Pure: the WM owns
//! the keyboard grab and the pointer warp; this owns the geometry.

use flamewm_api::{Point, Rect, Size};

/// Move step per arrow press, the xfwm4 `key_move` default.
pub const MOVE_STEP: i32 = 16;
/// Resize step per arrow press.
pub const RESIZE_STEP: i32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Move,
    Resize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Commit,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Keep the grab open with this rect.
    Continue(Rect),
    /// Close the grab and keep this rect.
    Commit(Rect),
    /// Close the grab and restore the rect it started from.
    Cancel(Rect),
}

/// Maps an X keysym to a grab key (`XK_Left`, `XK_Return`, `XK_KP_Enter`, `XK_Escape`, ...).
#[must_use]
pub fn key_for_keysym(keysym: u32) -> Option<Key> {
    Some(match keysym {
        0xff51 => Key::Left,
        0xff52 => Key::Up,
        0xff53 => Key::Right,
        0xff54 => Key::Down,
        0xff0d | 0xff8d => Key::Commit,
        0xff1b => Key::Cancel,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardGrab {
    mode: Mode,
    origin: Rect,
    rect: Rect,
    work: Rect,
    min: Size,
}

impl KeyboardGrab {
    /// Starts a grab on `rect`, kept inside `work` and never below `min`.
    #[must_use]
    pub fn begin(mode: Mode, rect: Rect, work: Rect, min: Size) -> Self {
        Self {
            mode,
            origin: rect,
            rect,
            work,
            min,
        }
    }

    /// Window centre, where the WM warps the pointer on entry.
    #[must_use]
    pub fn pointer(&self) -> Point {
        Point::new(
            self.rect.x + self.rect.width / 2,
            self.rect.y + self.rect.height / 2,
        )
    }

    pub fn step(&mut self, key: Key) -> Step {
        let (dx, dy) = match key {
            Key::Left => (-1, 0),
            Key::Right => (1, 0),
            Key::Up => (0, -1),
            Key::Down => (0, 1),
            Key::Commit => return Step::Commit(self.rect),
            Key::Cancel => return Step::Cancel(self.origin),
        };
        self.rect = match self.mode {
            Mode::Move => self.moved(dx, dy),
            Mode::Resize => self.resized(dx, dy),
        };
        Step::Continue(self.rect)
    }

    fn moved(&self, dx: i32, dy: i32) -> Rect {
        let r = self.rect;
        let x = clamp_axis(r.x + dx * MOVE_STEP, r.width, self.work.x, self.work.width);
        let y = clamp_axis(
            r.y + dy * MOVE_STEP,
            r.height,
            self.work.y,
            self.work.height,
        );
        Rect::new(x, y, r.width, r.height)
    }

    /// Arrows grow or shrink the right and bottom edges; the top-left corner stays put.
    fn resized(&self, dx: i32, dy: i32) -> Rect {
        let r = self.rect;
        let max_w = (self.work.x + self.work.width - r.x).max(self.min.width);
        let max_h = (self.work.y + self.work.height - r.y).max(self.min.height);
        let width = (r.width + dx * RESIZE_STEP).clamp(self.min.width, max_w);
        let height = (r.height + dy * RESIZE_STEP).clamp(self.min.height, max_h);
        Rect::new(r.x, r.y, width, height)
    }
}

/// Keeps `[pos, pos + len)` inside `[lo, lo + span)`; a window wider than the area pins to `lo`.
fn clamp_axis(pos: i32, len: i32, lo: i32, span: i32) -> i32 {
    if len >= span {
        lo
    } else {
        pos.clamp(lo, lo + span - len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect::new(0, 0, 1280, 752);
    const MIN: Size = Size::new(40, 40);

    fn grab(mode: Mode, rect: Rect) -> KeyboardGrab {
        KeyboardGrab::begin(mode, rect, WORK, MIN)
    }

    fn rect_after(mut g: KeyboardGrab, keys: &[Key]) -> Rect {
        for key in keys {
            g.step(*key);
        }
        match g.step(Key::Commit) {
            Step::Commit(r) => r,
            other => panic!("commit expected, got {other:?}"),
        }
    }

    #[test]
    fn arrows_move_in_16px_steps() {
        let start = Rect::new(200, 120, 402, 332);
        let got = rect_after(
            grab(Mode::Move, start),
            &[Key::Right, Key::Right, Key::Down],
        );
        assert_eq!(got, Rect::new(232, 136, 402, 332));
    }

    #[test]
    fn move_clamps_to_work_area_edges() {
        let start = Rect::new(20, 300, 402, 332);
        let left = rect_after(grab(Mode::Move, start), &[Key::Left, Key::Left]);
        assert_eq!(left.x, 0);
        let bottom = rect_after(grab(Mode::Move, start), &[Key::Down; 10]);
        assert_eq!(bottom.y, WORK.height - 332);
    }

    #[test]
    fn resize_steps_right_and_bottom_and_stops_at_work_area() {
        let start = Rect::new(200, 120, 402, 332);
        let grown = rect_after(grab(Mode::Resize, start), &[Key::Right, Key::Down]);
        assert_eq!(grown, Rect::new(200, 120, 412, 342));
        let capped = rect_after(grab(Mode::Resize, start), &[Key::Right; 200]);
        assert_eq!(capped.x + capped.width, WORK.width);
    }

    #[test]
    fn resize_never_goes_below_minimum() {
        let start = Rect::new(0, 0, 60, 60);
        let shrunk = rect_after(grab(Mode::Resize, start), &[Key::Left; 10]);
        assert_eq!((shrunk.width, shrunk.height), (40, 60));
    }

    #[test]
    fn cancel_restores_origin_after_steps() {
        let start = Rect::new(200, 120, 402, 332);
        let mut g = grab(Mode::Move, start);
        g.step(Key::Right);
        g.step(Key::Down);
        assert_eq!(g.step(Key::Cancel), Step::Cancel(start));
    }

    #[test]
    fn pointer_warps_to_window_centre() {
        let g = grab(Mode::Move, Rect::new(200, 120, 402, 332));
        assert_eq!(g.pointer(), Point::new(401, 286));
    }

    #[test]
    fn keysyms_map_to_grab_keys() {
        assert_eq!(key_for_keysym(0xff51), Some(Key::Left));
        assert_eq!(key_for_keysym(0xff54), Some(Key::Down));
        assert_eq!(key_for_keysym(0xff0d), Some(Key::Commit));
        assert_eq!(key_for_keysym(0xff8d), Some(Key::Commit));
        assert_eq!(key_for_keysym(0xff1b), Some(Key::Cancel));
        assert_eq!(key_for_keysym(0x61), None);
    }
}

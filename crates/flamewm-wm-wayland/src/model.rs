//! Pure window-management model for the Wayland compositor.
//!
//! No smithay types live here, so stacking, workspaces, hit testing, shortcut parsing and
//! maximize/fullscreen geometry are unit-testable without a display. Workspace topology and
//! the shortcut action names come from `flamewm-api` / `flamewm-platform` (shared with X11).

use std::collections::BTreeMap;

use flamewm_api::shortcuts::KeyBinding;
use flamewm_api::workspace::{GridDirection, MAX_WORKSPACES, grid_neighbor};

pub const TITLEBAR_HEIGHT: i32 = 28;
pub const BUTTON_SIZE: i32 = 16;
pub const BUTTON_GAP: i32 = 6;
pub const RESIZE_GRIP: i32 = 12;
pub const MIN_CLIENT_W: i32 = 120;
pub const MIN_CLIENT_H: i32 = 80;
const DEFAULT_W: i32 = 640;
const DEFAULT_H: i32 = 420;

pub type WindowId = u64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Half-open containment test, so adjacent rectangles never both claim a pixel.
    pub fn contains(self, px: f64, py: f64) -> bool {
        px >= f64::from(self.x)
            && px < f64::from(self.x + self.w)
            && py >= f64::from(self.y)
            && py < f64::from(self.y + self.h)
    }

    /// The decorated frame: the client rectangle plus the titlebar above it.
    pub const fn frame(self) -> Self {
        Self {
            x: self.x,
            y: self.y - TITLEBAR_HEIGHT,
            w: self.w,
            h: self.h + TITLEBAR_HEIGHT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Minimize,
    Maximize,
    Close,
}

/// What a pointer position hits, topmost window first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Button(WindowId, Button),
    Titlebar(WindowId),
    Resize(WindowId),
    Client(WindowId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Maximized,
    Fullscreen,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Managed {
    pub id: WindowId,
    /// Client rectangle in compositor coordinates (titlebar excluded unless fullscreen).
    pub rect: Rect,
    pub workspace: usize,
    pub minimized: bool,
    pub mode: Mode,
    /// Client rectangle to return to when leaving maximized or fullscreen.
    pub restore: Rect,
}

impl Managed {
    /// Frame rectangle as drawn: fullscreen windows have no server-side titlebar.
    pub fn frame(&self) -> Rect {
        if self.mode == Mode::Fullscreen {
            self.rect
        } else {
            self.rect.frame()
        }
    }

    pub fn decorated(&self) -> bool {
        self.mode != Mode::Fullscreen
    }
}

/// Stacking order is `windows` bottom to top; `current` is the visible workspace.
#[derive(Debug)]
pub struct WindowModel {
    windows: Vec<Managed>,
    current: usize,
    count: usize,
    show_desktop_hidden: Vec<WindowId>,
}

impl Default for WindowModel {
    fn default() -> Self {
        Self::new(2)
    }
}

impl WindowModel {
    pub fn new(workspace_count: usize) -> Self {
        Self {
            windows: Vec::new(),
            current: 0,
            count: workspace_count.clamp(1, MAX_WORKSPACES),
            show_desktop_hidden: Vec::new(),
        }
    }

    pub fn current_workspace(&self) -> usize {
        self.current
    }

    pub fn workspace_count(&self) -> usize {
        self.count
    }

    /// Reassigns a window to another workspace; out-of-range targets are ignored.
    pub fn move_to_workspace(&mut self, id: WindowId, target: usize) {
        if target >= self.count {
            return;
        }
        if let Some(window) = self.get_mut(id) {
            window.workspace = target;
        }
    }

    /// Every managed window, bottom to top.
    pub fn managed(&self) -> &[Managed] {
        &self.windows
    }

    pub fn get(&self, id: WindowId) -> Option<&Managed> {
        self.windows.iter().find(|w| w.id == id)
    }

    fn get_mut(&mut self, id: WindowId) -> Option<&mut Managed> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    /// Default placement: cascade per workspace so new windows do not stack exactly.
    pub fn cascade_rect(&self, workspace: usize) -> Rect {
        let n = self
            .windows
            .iter()
            .filter(|w| w.workspace == workspace)
            .count() as i32;
        let step = 30 * (n % 10);
        Rect::new(60 + step, 60 + step + TITLEBAR_HEIGHT, DEFAULT_W, DEFAULT_H)
    }

    pub fn add(&mut self, id: WindowId, rect: Rect, workspace: usize) {
        let workspace = workspace.min(self.count - 1);
        self.windows.push(Managed {
            id,
            rect,
            workspace,
            minimized: false,
            mode: Mode::Normal,
            restore: rect,
        });
    }

    pub fn remove(&mut self, id: WindowId) -> Option<Managed> {
        let index = self.windows.iter().position(|w| w.id == id)?;
        self.show_desktop_hidden.retain(|hidden| *hidden != id);
        Some(self.windows.remove(index))
    }

    pub fn raise(&mut self, id: WindowId) {
        if let Some(index) = self.windows.iter().position(|w| w.id == id) {
            let window = self.windows.remove(index);
            self.windows.push(window);
        }
    }

    /// Visible (current workspace, not minimized) windows, topmost first.
    pub fn visible_top_first(&self) -> Vec<WindowId> {
        self.windows
            .iter()
            .rev()
            .filter(|w| w.workspace == self.current && !w.minimized)
            .map(|w| w.id)
            .collect()
    }

    pub fn hit(&self, px: f64, py: f64) -> Option<Hit> {
        for window in self.windows.iter().rev() {
            if window.workspace != self.current || window.minimized {
                continue;
            }
            if !window.frame().contains(px, py) {
                continue;
            }
            if window.decorated() && !window.rect.contains(px, py) {
                let frame = window.frame();
                return Some(match button_at(frame, px, py) {
                    Some(button) => Hit::Button(window.id, button),
                    None => Hit::Titlebar(window.id),
                });
            }
            if in_resize_grip(window.rect, px, py) {
                return Some(Hit::Resize(window.id));
            }
            return Some(Hit::Client(window.id));
        }
        None
    }

    /// Alt+Tab: raise the second-topmost visible window and return it.
    pub fn cycle_focus(&mut self) -> Option<WindowId> {
        let target = *self.visible_top_first().get(1)?;
        self.raise(target);
        Some(target)
    }

    /// Workspace neighbour in the shared two-row grid; `None` is a deliberate no-op.
    pub fn workspace_step(&self, direction: GridDirection) -> Option<usize> {
        grid_neighbor(self.current, self.count, direction)
    }

    /// Switch workspace; returns `false` when the target is invalid or already current.
    pub fn switch_workspace(&mut self, target: usize) -> bool {
        if target >= self.count || target == self.current {
            return false;
        }
        self.current = target;
        true
    }

    pub fn minimize(&mut self, id: WindowId) {
        if let Some(window) = self.get_mut(id) {
            window.minimized = true;
        }
    }

    pub fn restore(&mut self, id: WindowId) {
        if let Some(window) = self.get_mut(id) {
            window.minimized = false;
        }
        self.raise(id);
    }

    /// Show desktop: minimize every visible window, or restore exactly those it hid.
    pub fn toggle_show_desktop(&mut self) {
        if self.show_desktop_hidden.is_empty() {
            let visible = self.visible_top_first();
            for id in &visible {
                self.minimize(*id);
            }
            self.show_desktop_hidden = visible;
        } else {
            let hidden = std::mem::take(&mut self.show_desktop_hidden);
            for id in hidden {
                self.restore(id);
            }
        }
    }

    pub fn move_to(&mut self, id: WindowId, x: i32, y: i32) {
        if let Some(window) = self.get_mut(id) {
            window.rect.x = x;
            window.rect.y = y;
        }
    }

    /// Sets the client rect (origin and size) of a managed window.
    pub fn set_rect(&mut self, id: WindowId, rect: Rect) {
        if let Some(window) = self.get_mut(id) {
            window.rect = rect;
        }
    }

    #[cfg(test)]
    pub fn resize_to(&mut self, id: WindowId, w: i32, h: i32) {
        if let Some(window) = self.get_mut(id) {
            window.rect.w = w.max(MIN_CLIENT_W);
            window.rect.h = h.max(MIN_CLIENT_H);
        }
    }

    /// Toggle maximize into `work` (the usable area, titlebar included in the frame).
    pub fn toggle_maximize(&mut self, id: WindowId, work: Rect) {
        let Some(window) = self.get_mut(id) else {
            return;
        };
        match window.mode {
            Mode::Maximized => {
                window.mode = Mode::Normal;
                window.rect = window.restore;
            }
            Mode::Normal => {
                window.restore = window.rect;
                window.mode = Mode::Maximized;
                window.rect = Rect::new(
                    work.x,
                    work.y + TITLEBAR_HEIGHT,
                    work.w,
                    work.h - TITLEBAR_HEIGHT,
                );
            }
            Mode::Fullscreen => {}
        }
    }

    /// Explicit fullscreen on/off; idempotent.
    pub fn set_fullscreen(&mut self, id: WindowId, output: Rect, on: bool) {
        let Some(window) = self.get_mut(id) else {
            return;
        };
        match (window.mode, on) {
            (Mode::Fullscreen, false) => {
                window.mode = Mode::Normal;
                window.rect = window.restore;
            }
            (Mode::Normal | Mode::Maximized, true) => {
                if window.mode == Mode::Normal {
                    window.restore = window.rect;
                }
                window.mode = Mode::Fullscreen;
                window.rect = output;
            }
            _ => {}
        }
    }
}

/// Titlebar button rectangle, right-aligned inside the frame: close, maximize, minimize.
pub fn button_rect(frame: Rect, button: Button) -> Rect {
    let slot = BUTTON_SIZE + BUTTON_GAP;
    let right = frame.x + frame.w - BUTTON_GAP;
    let x = match button {
        Button::Close => right - slot,
        Button::Maximize => right - 2 * slot,
        Button::Minimize => right - 3 * slot,
    };
    Rect::new(
        x,
        frame.y + (TITLEBAR_HEIGHT - BUTTON_SIZE) / 2,
        BUTTON_SIZE,
        BUTTON_SIZE,
    )
}

fn button_at(frame: Rect, px: f64, py: f64) -> Option<Button> {
    [Button::Close, Button::Maximize, Button::Minimize]
        .into_iter()
        .find(|button| button_rect(frame, *button).contains(px, py))
}

/// Client rect inside a grip band along any edge (corners included) starts a resize.
fn in_resize_grip(rect: Rect, px: f64, py: f64) -> bool {
    if !rect.contains(px, py) {
        return false;
    }
    let grip = f64::from(RESIZE_GRIP);
    px < f64::from(rect.x) + grip
        || px >= f64::from(rect.x + rect.w) - grip
        || py < f64::from(rect.y) + grip
        || py >= f64::from(rect.y + rect.h) - grip
}

/// Edges a resize grab moves: `left`/`top` move the origin, `right`/`bottom` move the far side.
/// A corner sets two flags. X11 parity: edge handles and Ctrl+Super+right use the same edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResizeEdge {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl ResizeEdge {
    /// Edges under `(px, py)` inside the grip band of `rect`.
    pub fn at(rect: Rect, px: f64, py: f64) -> Self {
        let grip = f64::from(RESIZE_GRIP);
        Self {
            left: px < f64::from(rect.x) + grip,
            right: px >= f64::from(rect.x + rect.w) - grip,
            top: py < f64::from(rect.y) + grip,
            bottom: py >= f64::from(rect.y + rect.h) - grip,
        }
    }

    /// Quadrant of `rect` holding `(px, py)`: Ctrl+Super+right resizes the corner nearest the grab.
    pub fn quadrant(rect: Rect, px: f64, py: f64) -> Self {
        let left = px < f64::from(rect.x) + f64::from(rect.w) / 2.0;
        let top = py < f64::from(rect.y) + f64::from(rect.h) / 2.0;
        Self {
            left,
            right: !left,
            top,
            bottom: !top,
        }
    }

    /// Geometry after moving this edge by `(dx, dy)` from `start`, never below the minimum size.
    /// A left or top edge that hits the minimum stays on the opposite side (the origin follows).
    pub fn resized(self, start: Rect, dx: i32, dy: i32) -> Rect {
        let (mut x, mut y, mut w, mut h) = (start.x, start.y, start.w, start.h);
        if self.left {
            x = start.x + dx;
            w = start.w - dx;
        } else if self.right {
            w = start.w + dx;
        }
        if self.top {
            y = start.y + dy;
            h = start.h - dy;
        } else if self.bottom {
            h = start.h + dy;
        }
        if w < MIN_CLIENT_W {
            if self.left {
                x = start.x + start.w - MIN_CLIENT_W;
            }
            w = MIN_CLIENT_W;
        }
        if h < MIN_CLIENT_H {
            if self.top {
                y = start.y + start.h - MIN_CLIENT_H;
            }
            h = MIN_CLIENT_H;
        }
        Rect::new(x, y, w, h)
    }
}

/// Modifier state for chord matching.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

/// A parsed binding such as `Ctrl+Super+Left`. `key` is the lowercase keysym name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    pub mods: Mods,
    pub key: String,
}

/// True for a chord key that is one printable character (`d`, `5`, `/`). Binding such a key
/// without a command modifier would fire while the user types.
fn is_printable_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(only) if only.is_ascii_graphic()) && chars.next().is_none()
}

/// Whether a chord on `key` may fire with `mods`. A single printable character always needs
/// Ctrl, Alt or Super (Shift alone is not enough: `Shift+d` is how you type `D`), and an empty
/// key never matches anything.
fn chord_can_fire(mods: Mods, key: &str) -> bool {
    !key.is_empty() && (!is_printable_key(key) || mods.ctrl || mods.alt || mods.logo)
}

/// Parses a normalized binding from `flamewm_platform::shortcuts`.
pub fn parse_chord(raw: &str) -> Option<Chord> {
    let mut mods = Mods::default();
    let mut key = None;
    for part in raw
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" => mods.alt = true,
            "shift" => mods.shift = true,
            "super" | "logo" | "win" | "meta" => mods.logo = true,
            other => key = Some(other.to_owned()),
        }
    }
    key.map(|key| Chord { mods, key })
}

/// Action lookup built from the shared shortcut bindings (action name -> chord).
#[derive(Debug, Default)]
pub struct Shortcuts {
    entries: Vec<(String, Chord)>,
}

impl Shortcuts {
    pub fn from_bindings(bindings: &BTreeMap<String, KeyBinding>) -> Self {
        let entries = bindings
            .iter()
            .filter_map(|(action, binding)| {
                let chord = parse_chord(binding.0.as_deref()?)?;
                // A chord that could fire on ordinary typing never enters the table.
                chord_can_fire(chord.mods, &chord.key).then(|| (action.clone(), chord))
            })
            .collect();
        Self { entries }
    }

    /// Action for an exact modifier+key match; `key` is the keysym name (any case).
    ///
    /// Caps Lock and Num Lock are not part of [`Mods`], so they never affect the match. A chord
    /// on a single printable key also needs Ctrl, Alt or Super, so a stray `d` binding left in a
    /// registry can never fire while the user types.
    pub fn action_for(&self, mods: Mods, key: &str) -> Option<&str> {
        let key = key.to_ascii_lowercase();
        if !chord_can_fire(mods, &key) {
            return None;
        }
        self.entries
            .iter()
            .find(|(_, chord)| chord.mods == mods && chord.key == key)
            .map(|(action, _)| action.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::shortcuts::{ACTION_LAUNCHER, ACTION_WINDOW_SWITCH};
    use flamewm_api::workspace::GridDirection as D;
    use flamewm_platform::shortcuts::ShortcutRegistry;

    const WORK: Rect = Rect::new(0, 0, 1000, 700);

    fn model_with(ids: &[WindowId]) -> WindowModel {
        let mut model = WindowModel::new(4);
        for id in ids {
            let rect = model.cascade_rect(0);
            model.add(*id, rect, 0);
        }
        model
    }

    #[test]
    fn stacking_is_bottom_to_top_and_raise_moves_to_top() {
        let mut model = model_with(&[1, 2, 3]);
        assert_eq!(model.visible_top_first(), vec![3, 2, 1]);
        model.raise(1);
        assert_eq!(model.visible_top_first(), vec![1, 3, 2]);
    }

    #[test]
    fn alt_tab_raises_second_window_and_toggles() {
        let mut model = model_with(&[1, 2, 3]);
        assert_eq!(model.cycle_focus(), Some(2));
        assert_eq!(model.visible_top_first()[0], 2);
        assert_eq!(model.cycle_focus(), Some(3));
        assert_eq!(WindowModel::new(1).cycle_focus(), None);
    }

    #[test]
    fn workspaces_hide_other_windows_and_respect_grid_bounds() {
        let mut model = model_with(&[1]);
        model.add(9, Rect::new(0, 0, 200, 200), 1);
        assert_eq!(model.visible_top_first(), vec![1]);
        assert!(model.switch_workspace(1));
        assert_eq!(model.visible_top_first(), vec![9]);
        assert!(!model.switch_workspace(1), "same workspace is a no-op");
        assert!(!model.switch_workspace(4), "out of range is rejected");
        // 4 workspaces form a 2x2 grid: right of 0 is 1, down of 1 is 3, nothing right of 1.
        assert_eq!(model.workspace_step(D::Right), None);
        assert_eq!(model.workspace_step(D::Down), Some(3));
    }

    #[test]
    fn resize_edges_pick_sides_and_corners_in_the_grip_band() {
        let rect = Rect::new(100, 100, 300, 200);
        let edge = ResizeEdge::at(rect, 101.0, 150.0);
        assert!(edge.left && !edge.right && !edge.top && !edge.bottom);
        let corner = ResizeEdge::at(rect, 398.0, 298.0);
        assert!(corner.right && corner.bottom && !corner.left && !corner.top);
        assert!(in_resize_grip(rect, 150.0, 101.0));
        assert!(!in_resize_grip(rect, 200.0, 200.0));
    }

    #[test]
    fn left_and_top_edges_move_the_origin_and_clamp_to_minimum() {
        let start = Rect::new(100, 100, 300, 200);
        let left = ResizeEdge {
            left: true,
            right: false,
            top: false,
            bottom: false,
        };
        assert_eq!(left.resized(start, -40, 0), Rect::new(60, 100, 340, 200));
        let top = ResizeEdge {
            left: false,
            right: false,
            top: true,
            bottom: false,
        };
        assert_eq!(top.resized(start, 0, 20), Rect::new(100, 120, 300, 180));
        // Dragging past the minimum keeps the opposite side fixed.
        let clamped = left.resized(start, 500, 0);
        assert_eq!(clamped.x + clamped.w, start.x + start.w);
        assert_eq!(clamped.w, MIN_CLIENT_W);
    }

    #[test]
    fn quadrant_picks_the_corner_nearest_the_grab() {
        let rect = Rect::new(0, 0, 200, 100);
        let q = ResizeEdge::quadrant(rect, 150.0, 80.0);
        assert!(q.right && q.bottom && !q.left && !q.top);
        assert_eq!(q.resized(rect, 10, 10), Rect::new(0, 0, 210, 110));
    }

    #[test]
    fn hit_test_prefers_topmost_and_classifies_titlebar_and_buttons() {
        let mut model = WindowModel::new(1);
        model.add(1, Rect::new(100, 100, 300, 300), 0);
        model.add(2, Rect::new(150, 150, 300, 300), 0);
        // Window 2 covers (200,200) client area; window 1 is below it.
        assert_eq!(model.hit(200.0, 200.0), Some(Hit::Client(2)));
        // Titlebar of window 1 above its client rect, away from buttons.
        assert_eq!(model.hit(110.0, 100.0 - 10.0), Some(Hit::Titlebar(1)));
        // Close button sits at the right edge of the frame.
        let frame = model.get(1).unwrap().frame();
        let close = button_rect(frame, Button::Close);
        let (close_x, close_y) = (f64::from(close.x + 2), f64::from(close.y + 2));
        assert_eq!(
            model.hit(close_x, close_y),
            Some(Hit::Button(1, Button::Close))
        );
    }

    #[test]
    fn resize_grip_is_bottom_right_corner_of_client() {
        let mut model = WindowModel::new(1);
        model.add(7, Rect::new(0, 28, 200, 200), 0);
        assert_eq!(model.hit(195.0, 225.0), Some(Hit::Resize(7)));
        assert_eq!(model.hit(50.0, 50.0), Some(Hit::Client(7)));
    }

    #[test]
    fn minimize_and_show_desktop_round_trip() {
        let mut model = model_with(&[1, 2]);
        model.minimize(2);
        assert_eq!(model.visible_top_first(), vec![1]);
        model.restore(2);
        assert_eq!(model.visible_top_first(), vec![2, 1]);

        model.toggle_show_desktop();
        assert!(model.visible_top_first().is_empty());
        model.toggle_show_desktop();
        assert_eq!(model.visible_top_first().len(), 2);
    }

    #[test]
    fn maximize_restores_previous_rect_and_drops_titlebar_offset() {
        let mut model = model_with(&[1]);
        let before = model.get(1).unwrap().rect;
        model.toggle_maximize(1, WORK);
        let maximized = model.get(1).unwrap().clone();
        assert_eq!(maximized.mode, Mode::Maximized);
        assert_eq!(maximized.frame(), Rect::new(0, 0, 1000, 700));
        model.toggle_maximize(1, WORK);
        assert_eq!(model.get(1).unwrap().rect, before);
        assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
    }

    #[test]
    fn set_fullscreen_is_idempotent() {
        let mut model = model_with(&[1]);
        let before = model.get(1).unwrap().rect;
        model.set_fullscreen(1, WORK, true);
        model.set_fullscreen(1, WORK, true);
        model.set_fullscreen(1, WORK, false);
        model.set_fullscreen(1, WORK, false);
        assert_eq!(model.get(1).unwrap().rect, before);
    }

    #[test]
    fn fullscreen_has_no_frame_and_restores() {
        let mut model = model_with(&[1]);
        let before = model.get(1).unwrap().rect;
        model.set_fullscreen(1, WORK, true);
        assert_eq!(model.get(1).unwrap().frame(), WORK);
        assert!(!model.get(1).unwrap().decorated());
        model.set_fullscreen(1, WORK, false);
        assert_eq!(model.get(1).unwrap().rect, before);
    }

    #[test]
    fn resize_is_clamped_to_minimum_client_size() {
        let mut model = model_with(&[1]);
        model.resize_to(1, 1, 1);
        let rect = model.get(1).unwrap().rect;
        assert_eq!((rect.w, rect.h), (MIN_CLIENT_W, MIN_CLIENT_H));
    }

    #[test]
    fn parses_shared_default_bindings_into_chords() {
        assert_eq!(
            parse_chord("Ctrl+Super+Left"),
            Some(Chord {
                mods: Mods {
                    ctrl: true,
                    logo: true,
                    ..Mods::default()
                },
                key: "left".into(),
            })
        );
        assert_eq!(
            parse_chord("Super_L").map(|c| c.key),
            Some("super_l".into())
        );
        assert_eq!(parse_chord("   "), None);
    }

    #[test]
    fn bare_letters_never_trigger_show_desktop_and_both_chords_do() {
        use flamewm_api::shortcuts::{ACTION_SHOW_DESKTOP, ACTION_SHOW_DESKTOP_CTRL_ALT};

        let registry = ShortcutRegistry::default();
        let shortcuts = Shortcuts::from_bindings(registry.bindings());

        // Regression: plain `d` (and every other bare letter) must never toggle show-desktop.
        assert_eq!(shortcuts.action_for(Mods::default(), "d"), None);
        for letter in 'a'..='z' {
            assert_eq!(
                shortcuts.action_for(Mods::default(), &letter.to_string()),
                None,
                "bare {letter} must not match any chord"
            );
        }

        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        let ctrl_alt = Mods {
            ctrl: true,
            alt: true,
            ..Mods::default()
        };
        assert_eq!(shortcuts.action_for(logo, "d"), Some(ACTION_SHOW_DESKTOP));
        assert_eq!(
            shortcuts.action_for(ctrl_alt, "d"),
            Some(ACTION_SHOW_DESKTOP_CTRL_ALT)
        );
        // Super+d must not be reachable through a stale bare-`d` entry either.
        assert_eq!(shortcuts.action_for(logo, "D"), Some(ACTION_SHOW_DESKTOP));
    }

    #[test]
    fn stray_bare_letter_binding_can_never_fire_while_typing() {
        use flamewm_api::shortcuts::{ACTION_SHOW_DESKTOP, ACTION_WINDOW_CLOSE};

        // A legacy or hand-edited registry entry that lost its modifier.
        let stale = BTreeMap::from([
            (ACTION_SHOW_DESKTOP.to_owned(), KeyBinding::assigned("d")),
            (
                ACTION_WINDOW_CLOSE.to_owned(),
                KeyBinding::assigned("Shift+q"),
            ),
        ]);
        let shortcuts = Shortcuts::from_bindings(&stale);
        assert_eq!(shortcuts.action_for(Mods::default(), "d"), None);
        assert_eq!(
            shortcuts.action_for(
                Mods {
                    shift: true,
                    ..Mods::default()
                },
                "q"
            ),
            None,
            "Shift alone must not make a typing key a shortcut"
        );
        // Media keys and named keys stay bindable without a modifier.
        let named = BTreeMap::from([(
            "VolumeUp".to_owned(),
            KeyBinding::assigned("XF86AudioRaiseVolume"),
        )]);
        let shortcuts = Shortcuts::from_bindings(&named);
        assert_eq!(
            shortcuts.action_for(Mods::default(), "xf86audioraisevolume"),
            Some("VolumeUp")
        );
        // An empty key never matches, and neither does an empty query.
        assert_eq!(shortcuts.action_for(Mods::default(), ""), None);
    }

    #[test]
    fn shared_registry_resolves_actions_for_wayland_keys() {
        let registry = ShortcutRegistry::default();
        let shortcuts = Shortcuts::from_bindings(registry.bindings());
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        let alt = Mods {
            alt: true,
            ..Mods::default()
        };
        assert_eq!(shortcuts.action_for(logo, "space"), Some(ACTION_LAUNCHER));
        assert_eq!(shortcuts.action_for(alt, "Tab"), Some(ACTION_WINDOW_SWITCH));
        assert_eq!(shortcuts.action_for(Mods::default(), "Tab"), None);
    }
}

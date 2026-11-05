//! Pure window-management model for the Wayland compositor.
//!
//! No smithay types live here, so stacking, workspaces, hit testing, shortcut parsing and
//! maximize/fullscreen geometry are unit-testable without a display. Workspace topology and
//! the shortcut action names come from `flamewm-api` / `flamewm-platform` (shared with X11).

use std::collections::BTreeMap;

use flamewm_api::shortcuts::KeyBinding;
use flamewm_api::workspace::{GridDirection, MAX_WORKSPACES, grid_neighbor};
use flamewm_window_core::{SnapTarget, apply_window_margin, snap_geometry};

mod geometry;
mod shortcuts;
pub use geometry::{ResizeEdge, resize_edge_at};
pub use shortcuts::{Mods, Shortcuts};
use geometry::*;
#[cfg(test)]
use shortcuts::{Chord, parse_chord};

pub const TITLEBAR_HEIGHT: i32 = flamewm_skin::recipes::window_chrome::WINDOW_CHROME
    .metrics
    .titlebar as i32;
pub const BUTTON_SIZE: i32 = flamewm_skin::recipes::window_chrome::WINDOW_CHROME
    .metrics
    .button_width as i32;
pub const BUTTON_GAP: i32 = 0;
pub const RESIZE_GRIP: i32 = 12;
/// Logical square at each decorated-frame corner that starts a two-axis resize. It covers the
/// titlebar band, where [`RESIZE_GRIP`] cannot reach. X11 mirrors this number for its own corner
/// handles, so both sessions agree on where a corner begins; the close button is wider and taller
/// than the square, so its centre stays a button press.
pub const CORNER_GRIP: i32 = 16;
pub const MIN_CLIENT_W: i32 = 120;
pub const MIN_CLIENT_H: i32 = 80;
const DEFAULT_W: i32 = 640;
const DEFAULT_H: i32 = 420;

/// `titlebars` default, matching `flamewm_platform::settings::ProductSettings::default()`.
pub const DEFAULT_TITLEBARS: bool = true;
/// Margin a model built by [`WindowModel::new`] starts with: none, so an unconfigured model lays
/// windows out exactly as it did before `windowMargin` was honoured (the regression baseline).
///
/// The saved product default is `ProductSettings::default().window_margin` (2 px); the caller
/// delivers it through [`WindowModel::configure`], which is what makes a maximized frame sit in the
/// work area inset by the margin like X11's `apply_window_margin`.
pub const DEFAULT_WINDOW_MARGIN: i32 = 0;

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
    Snapped(SnapTarget),
    Maximized,
    Fullscreen,
}

impl Mode {
    /// Whether the window takes the configured corner radius: only a floating (`Normal`) window
    /// does. Every layout state (snapped, maximized, fullscreen) is square. Single owner of that
    /// policy for the compositor: the titlebar arc, the client clip radii and the outline ring all
    /// derive their radius from it.
    #[must_use]
    pub const fn rounded_frame(self) -> bool {
        matches!(self, Self::Normal)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Managed {
    pub id: WindowId,
    /// Client rectangle in compositor coordinates (titlebar excluded unless fullscreen).
    pub rect: Rect,
    pub workspace: usize,
    pub minimized: bool,
    pub mode: Mode,
    /// `titlebars` as last applied by [`WindowModel::configure`]; false removes the band.
    pub titlebars: bool,
    /// Floating client rectangle retained across snapped/maximized/fullscreen placements.
    pub restore: Rect,
    /// Placement under maximize; keeping mode and rect separate preserves snapped resume state.
    pub maximize_restore_mode: Mode,
    pub maximize_restore_rect: Rect,
    /// Preserve the placement under fullscreen, so exiting restores the prior layout.
    pub fullscreen_restore_mode: Mode,
    /// Geometry before fullscreen, independently of normal/maximized restore geometry.
    pub fullscreen_restore_rect: Rect,
}

impl Managed {
    /// Frame rectangle as drawn: no band when the window is fullscreen or titlebars are off.
    pub fn frame(&self) -> Rect {
        if self.decorated() {
            self.rect.frame()
        } else {
            self.rect
        }
    }

    /// Server-side titlebar band present: `titlebars` is on and the window is not fullscreen.
    pub const fn decorated(&self) -> bool {
        self.titlebars && !matches!(self.mode, Mode::Fullscreen)
    }
}

/// Stacking order is `windows` bottom to top; `current` is the visible workspace.
#[derive(Debug)]
pub struct WindowModel {
    windows: Vec<Managed>,
    current: usize,
    count: usize,
    show_desktop_hidden: Vec<WindowId>,
    /// `titlebars` (`DEFAULT_TITLEBARS` until [`WindowModel::configure`] says otherwise).
    titlebars: bool,
    /// `windowMargin` (`DEFAULT_WINDOW_MARGIN` until [`WindowModel::configure`] says otherwise).
    window_margin: i32,
    /// Work area of the last maximize or refit, so a settings change can reflow without the caller
    /// repeating it. `None` until the shell first reports one.
    work_area: Option<Rect>,
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
            titlebars: DEFAULT_TITLEBARS,
            window_margin: DEFAULT_WINDOW_MARGIN,
            work_area: None,
        }
    }

    /// Applies the `titlebars` and `windowMargin` settings, the two the compositor reads instead
    /// of guessing. Call it once with the saved product values:
    /// `model.configure(settings.titlebars, i32::from(settings.window_margin))`.
    ///
    /// A negative margin means "no margin", exactly as
    /// [`flamewm_window_core::apply_window_margin`] treats it. Every window picks the titlebar mode
    /// up immediately because [`Managed::frame`] and [`WindowModel::hit`] derive the band from it.
    ///
    /// Floating windows keep their client rect; only the band above them appears or disappears,
    /// which is X11 `reflow_titlebars`. A maximized window is reflowed at once into the last known
    /// work area, so a setting change does not wait for the next maximize keypress the way X11's
    /// `reconcile_work_area` would.
    pub fn configure(&mut self, titlebars: bool, margin: i32) {
        self.titlebars = titlebars;
        self.window_margin = margin.max(0);
        let work = self.work_area;
        for window in &mut self.windows {
            window.titlebars = titlebars;
            if let Some(work) = work {
                match window.mode {
                    Mode::Maximized => {
                        window.rect = maximized_client(work, titlebars, self.window_margin);
                    }
                    Mode::Snapped(target) => {
                        window.rect = snapped_client(
                            target,
                            work,
                            window.restore,
                            titlebars,
                            self.window_margin,
                        );
                    }
                    Mode::Normal | Mode::Fullscreen => {}
                }
                if window.mode == Mode::Maximized
                    || (window.mode == Mode::Fullscreen
                        && window.fullscreen_restore_mode == Mode::Maximized)
                {
                    window.maximize_restore_rect = layout_rect(
                        window.maximize_restore_mode,
                        window.maximize_restore_rect,
                        window.restore,
                        work,
                        titlebars,
                        self.window_margin,
                    );
                }
                if window.mode == Mode::Fullscreen {
                    window.fullscreen_restore_rect = layout_rect(
                        window.fullscreen_restore_mode,
                        window.fullscreen_restore_rect,
                        window.restore,
                        work,
                        titlebars,
                        self.window_margin,
                    );
                }
            }
        }
    }

    /// Whether server-side titlebars are currently drawn.
    pub const fn titlebars(&self) -> bool {
        self.titlebars
    }

    /// The margin currently inset from a maximized work area.
    pub const fn window_margin(&self) -> i32 {
        self.window_margin
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

    /// Applies a workspace topology change from a validated [`flamewm_api::workspace::WorkspacePlan`].
    ///
    /// `old_to_new` is the plan's own mapping and is what carries windows across an insertion or a
    /// removal, so a window on a removed workspace lands where the plan says instead of being
    /// orphaned on an index that no longer exists. Returns whether the visible workspace moved, so
    /// the caller can refocus.
    pub fn apply_workspace_plan(&mut self, plan: &flamewm_api::workspace::WorkspacePlan) -> bool {
        self.count = plan.new_count.clamp(1, MAX_WORKSPACES);
        for window in &mut self.windows {
            if let Some(target) = plan.old_to_new.get(window.workspace).copied() {
                window.workspace = target.min(self.count - 1);
            } else {
                // A window on a removed workspace migrates to the plan's new active one, which is
                // where the X11 side parks it too.
                window.workspace = plan.new_active.min(self.count - 1);
            }
        }
        let previous = self.current;
        self.current = plan.new_active.min(self.count - 1);
        self.current != previous
    }

    /// Applies a live workspace count from `workspaces.count` without a topology plan.
    ///
    /// Clamps exactly like [`Self::apply_workspace_plan`]: the new count stays inside
    /// `1..=MAX_WORKSPACES`, windows on a workspace that disappears land on the new last one, and the
    /// visible workspace follows the same clamp so it can never point outside the topology.
    /// Returns whether the visible workspace moved, so the caller can refocus.
    pub fn set_count(&mut self, requested: usize) -> bool {
        let count = requested.clamp(1, MAX_WORKSPACES);
        if count == self.count {
            return false;
        }
        self.count = count;
        for window in &mut self.windows {
            window.workspace = window.workspace.min(count - 1);
        }
        let previous = self.current;
        self.current = self.current.min(count - 1);
        self.current != previous
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
    ///
    /// The cascade step is a frame origin; the client rect sits below it by the titlebar band, so
    /// the band is only reserved when the model draws one.
    pub fn cascade_rect(&self, workspace: usize) -> Rect {
        let n = self
            .windows
            .iter()
            .filter(|w| w.workspace == workspace)
            .count() as i32;
        let step = 30 * (n % 10);
        let band = if self.titlebars { TITLEBAR_HEIGHT } else { 0 };
        Rect::new(60 + step, 60 + step + band, DEFAULT_W, DEFAULT_H)
    }

    pub fn add(&mut self, id: WindowId, rect: Rect, workspace: usize) {
        let workspace = workspace.min(self.count - 1);
        self.windows.push(Managed {
            id,
            rect,
            workspace,
            minimized: false,
            mode: Mode::Normal,
            titlebars: self.titlebars,
            restore: rect,
            maximize_restore_mode: Mode::Normal,
            maximize_restore_rect: rect,
            fullscreen_restore_mode: Mode::Normal,
            fullscreen_restore_rect: rect,
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
            let frame = window.frame();
            if !frame.contains(px, py) {
                continue;
            }
            // A frame corner resizes: it wins over the titlebar drag and, in the square it shares
            // with the close button, over the button.
            if corner_at(frame, px, py).is_some() {
                return Some(Hit::Resize(window.id));
            }
            if window.decorated() && !window.rect.contains(px, py) {
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

    /// `Super+K`: the step before [`Self::cycle_focus`] in stack order — the focused window is the
    /// topmost one (focus raises), so the previous window is the bottom of the stack. With a single
    /// visible window that window is returned unchanged, as X11 `cycle_window(-1)` does.
    pub fn cycle_focus_prev(&mut self) -> Option<WindowId> {
        let target = *self.visible_top_first().last()?;
        self.raise(target);
        Some(target)
    }

    /// Nearest visible window to `from` on the `direction` side, scored like the X11
    /// `directional_target` search: the distance along the axis costs four times the drift across
    /// it, and windows that do not lie on that side are skipped.
    pub fn focus_direction(&self, from: WindowId, direction: GridDirection) -> Option<WindowId> {
        let origin = self.get(from)?.frame();
        let (origin_x, origin_y) = centre(origin);
        let mut best: Option<(i64, WindowId)> = None;
        for window in &self.windows {
            if window.id == from || window.workspace != self.current || window.minimized {
                continue;
            }
            let (x, y) = centre(window.frame());
            let (dx, dy) = (
                i64::from(x) - i64::from(origin_x),
                i64::from(y) - i64::from(origin_y),
            );
            let (along, across) = match direction {
                GridDirection::Left => (-dx, dy.abs()),
                GridDirection::Right => (dx, dy.abs()),
                GridDirection::Up => (-dy, dx.abs()),
                GridDirection::Down => (dy, dx.abs()),
            };
            if along <= 0 {
                continue;
            }
            let score = along * 4 + across;
            if best.is_none_or(|(best_score, _)| score < best_score) {
                best = Some((score, window.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// `Super+equal` / `Super+minus`: the keyboard resize step of X11 `resize_active`, which only
    /// moves a floating window and floors it at 160x96.
    pub fn resize_step(&mut self, id: WindowId, grow: bool) {
        const STEP_W: i32 = 40;
        const STEP_H: i32 = 30;
        const FLOOR_W: i32 = 160;
        const FLOOR_H: i32 = 96;
        let Some(window) = self
            .get_mut(id)
            .filter(|window| window.mode == Mode::Normal)
        else {
            return;
        };
        window.rect.w = if grow {
            window.rect.w.saturating_add(STEP_W)
        } else {
            window.rect.w.saturating_sub(STEP_W).max(FLOOR_W)
        };
        window.rect.h = if grow {
            window.rect.h.saturating_add(STEP_H)
        } else {
            window.rect.h.saturating_sub(STEP_H).max(FLOOR_H)
        };
    }

    /// `Super+Shift+arrows`: the keyboard nudge of X11 `move_active`, 40 px per press, which only
    /// moves a floating window.
    pub fn nudge(&mut self, id: WindowId, direction: GridDirection) {
        const STEP: i32 = 40;
        let (dx, dy) = match direction {
            GridDirection::Left => (-STEP, 0),
            GridDirection::Right => (STEP, 0),
            GridDirection::Up => (0, -STEP),
            GridDirection::Down => (0, STEP),
        };
        let Some(window) = self
            .get_mut(id)
            .filter(|window| window.mode == Mode::Normal)
        else {
            return;
        };
        window.rect.x = window.rect.x.saturating_add(dx);
        window.rect.y = window.rect.y.saturating_add(dy);
    }

    /// `Super+V`: a floating window fills the work area, anything else returns to the remembered
    /// floating rect. There is no tiling engine to toggle against, so this is X11 `toggle_float`;
    /// leaving fullscreen also drops the maximize the window was fullscreened over.
    pub fn toggle_float(&mut self, id: WindowId, work: Rect, output: Rect) {
        match self.get(id).map(|window| window.mode) {
            Some(Mode::Fullscreen) => {
                self.set_fullscreen(id, output, false);
                if self
                    .get(id)
                    .is_some_and(|window| window.mode == Mode::Maximized)
                {
                    self.toggle_maximize(id, work);
                }
            }
            Some(Mode::Normal | Mode::Snapped(_) | Mode::Maximized) => {
                self.toggle_maximize(id, work)
            }
            None => {}
        }
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

    /// Sets the client rect (origin and size) of a managed window.
    pub fn set_rect(&mut self, id: WindowId, rect: Rect) {
        if let Some(window) = self.get_mut(id) {
            if matches!(window.mode, Mode::Snapped(_)) {
                window.mode = Mode::Normal;
                window.maximize_restore_mode = Mode::Normal;
                window.maximize_restore_rect = rect;
            }
            window.rect = rect;
            if window.mode == Mode::Normal {
                window.restore = rect;
            }
        }
    }

    /// Geometry for a snap preview. This uses the same frame-to-client conversion as commit.
    pub fn snap_client_geometry(
        &self,
        id: WindowId,
        target: SnapTarget,
        work: Rect,
    ) -> Option<Rect> {
        let window = self.get(id)?;
        Some(if target == SnapTarget::Maximize {
            maximized_client(work, window.titlebars, self.window_margin)
        } else {
            snapped_client(
                target,
                work,
                window.restore,
                window.titlebars,
                self.window_margin,
            )
        })
    }

    /// Commits an activated move, restoring floating geometry when it left a layout.
    pub fn commit_move(&mut self, id: WindowId, rect: Rect) {
        if let Some(window) = self.get_mut(id) {
            window.mode = Mode::Normal;
            window.rect = rect;
            window.restore = rect;
            window.maximize_restore_mode = Mode::Normal;
            window.maximize_restore_rect = rect;
        }
    }

    /// Commits one snapped layout while retaining its floating restore geometry.
    pub fn snap_to(&mut self, id: WindowId, target: SnapTarget, work: Rect, restore: Rect) {
        let titlebars = self.titlebars;
        let margin = self.window_margin;
        self.work_area = Some(work);
        let Some(window) = self.get_mut(id) else {
            return;
        };
        window.restore = restore;
        window.mode = Mode::Snapped(target);
        window.rect = snapped_client(target, work, restore, titlebars, margin);
    }

    /// Commits the top-center snap target as maximize, preserving its floating restore.
    pub fn maximize_to(&mut self, id: WindowId, work: Rect, restore: Rect) {
        let titlebars = self.titlebars;
        let margin = self.window_margin;
        self.work_area = Some(work);
        let Some(window) = self.get_mut(id) else {
            return;
        };
        window.restore = restore;
        window.maximize_restore_mode = Mode::Normal;
        window.maximize_restore_rect = restore;
        window.mode = Mode::Maximized;
        window.rect = maximized_client(work, titlebars, margin);
    }

    #[cfg(test)]
    pub fn resize_to(&mut self, id: WindowId, w: i32, h: i32) {
        if let Some(window) = self.get_mut(id) {
            window.rect.w = w.max(MIN_CLIENT_W);
            window.rect.h = h.max(MIN_CLIENT_H);
        }
    }

    /// Toggle maximize into `work` (the usable area; the frame fills it, inset by `windowMargin`).
    pub fn toggle_maximize(&mut self, id: WindowId, work: Rect) {
        let titlebars = self.titlebars;
        let margin = self.window_margin;
        self.work_area = Some(work);
        let Some(window) = self.get_mut(id) else {
            return;
        };
        match window.mode {
            Mode::Maximized => {
                window.mode = window.maximize_restore_mode;
                window.rect = if window.mode == Mode::Normal {
                    window.restore
                } else {
                    window.maximize_restore_rect
                };
            }
            Mode::Normal | Mode::Snapped(_) => {
                window.maximize_restore_mode = window.mode;
                window.maximize_restore_rect = window.rect;
                if window.mode == Mode::Normal {
                    window.restore = window.rect;
                }
                window.mode = Mode::Maximized;
                window.rect = maximized_client(work, titlebars, margin);
            }
            Mode::Fullscreen => {}
        }
    }

    /// Refit output-sized placements without overwriting a floating window's restore geometry.
    pub fn refit_output(&mut self, id: WindowId, output: Rect, work: Rect) {
        let titlebars = self.titlebars;
        let margin = self.window_margin;
        self.work_area = Some(work);
        let Some(window) = self.get_mut(id) else {
            return;
        };
        match window.mode {
            Mode::Fullscreen => {
                window.rect = output;
                if window.fullscreen_restore_mode == Mode::Maximized {
                    window.maximize_restore_rect = layout_rect(
                        window.maximize_restore_mode,
                        window.maximize_restore_rect,
                        window.restore,
                        work,
                        titlebars,
                        margin,
                    );
                }
                window.fullscreen_restore_rect = layout_rect(
                    window.fullscreen_restore_mode,
                    window.fullscreen_restore_rect,
                    window.restore,
                    work,
                    titlebars,
                    margin,
                );
            }
            Mode::Maximized => {
                window.rect = maximized_client(work, titlebars, margin);
                window.maximize_restore_rect = layout_rect(
                    window.maximize_restore_mode,
                    window.maximize_restore_rect,
                    window.restore,
                    work,
                    titlebars,
                    margin,
                );
            }
            Mode::Snapped(target) => {
                window.rect = snapped_client(target, work, window.restore, titlebars, margin);
            }
            Mode::Normal => {}
        }
    }

    /// Explicit fullscreen on/off; idempotent.
    pub fn set_fullscreen(&mut self, id: WindowId, output: Rect, on: bool) {
        let Some(window) = self.get_mut(id) else {
            return;
        };
        match (window.mode, on) {
            (Mode::Fullscreen, false) => {
                window.mode = window.fullscreen_restore_mode;
                window.rect = if window.mode == Mode::Normal {
                    window.restore
                } else {
                    window.fullscreen_restore_rect
                };
            }
            (Mode::Normal | Mode::Snapped(_) | Mode::Maximized, true) => {
                window.fullscreen_restore_mode = window.mode;
                // Floating restore remains independent of the placement under fullscreen.
                window.fullscreen_restore_rect = window.rect;
                window.mode = Mode::Fullscreen;
                window.rect = output;
            }
            // A host/output resize does not change fullscreen state, but must update its rect.
            (Mode::Fullscreen, true) => window.rect = output,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;

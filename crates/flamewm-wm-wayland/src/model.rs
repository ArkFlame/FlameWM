//! Pure window-management model for the Wayland compositor.
//!
//! No smithay types live here, so stacking, workspaces, hit testing, shortcut parsing and
//! maximize/fullscreen geometry are unit-testable without a display. Workspace topology and
//! the shortcut action names come from `flamewm-api` / `flamewm-platform` (shared with X11).

use std::collections::BTreeMap;

use flamewm_api::shortcuts::KeyBinding;
use flamewm_api::workspace::{GridDirection, MAX_WORKSPACES, grid_neighbor};
use flamewm_window_core::{SnapTarget, apply_window_margin, snap_geometry};

pub const TITLEBAR_HEIGHT: i32 = flamewm_skin::recipes::window_chrome::WINDOW_CHROME
    .metrics
    .titlebar as i32;
pub const BUTTON_SIZE: i32 = flamewm_skin::recipes::window_chrome::WINDOW_CHROME
    .metrics
    .button_width as i32;
pub const BUTTON_GAP: i32 = 0;
pub const RESIZE_GRIP: i32 = 12;
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

/// Rectangle centre, the anchor the X11 directional search compares window positions by.
fn centre(rect: Rect) -> (i32, i32) {
    (rect.x + rect.w / 2, rect.y + rect.h / 2)
}

/// Converts one shared frame rectangle to the model's client rectangle exactly once.
fn client_from_frame(frame: flamewm_api::Rect, titlebars: bool) -> Rect {
    let band = if titlebars { TITLEBAR_HEIGHT } else { 0 };
    Rect::new(frame.x, frame.y + band, frame.width, frame.height - band)
}

/// Client rect of a window maximized into `work`: the frame fills the shared-margin work area.
fn maximized_client(work: Rect, titlebars: bool, margin: i32) -> Rect {
    let work = api_rect(work);
    let tile = snap_geometry(
        SnapTarget::Maximize,
        work,
        flamewm_api::Size::new(work.width, work.height),
    );
    client_from_frame(apply_window_margin(tile, work, margin), titlebars)
}

/// Client rect for a snapped placement. Preview and commit both call this one conversion path.
fn snapped_client(
    target: SnapTarget,
    work: Rect,
    floating_restore: Rect,
    titlebars: bool,
    margin: i32,
) -> Rect {
    let work = api_rect(work);
    let tile = snap_geometry(
        target,
        work,
        flamewm_api::Size::new(floating_restore.w, floating_restore.h),
    );
    client_from_frame(apply_window_margin(tile, work, margin), titlebars)
}

fn layout_rect(
    mode: Mode,
    saved_rect: Rect,
    floating_restore: Rect,
    work: Rect,
    titlebars: bool,
    margin: i32,
) -> Rect {
    match mode {
        Mode::Normal | Mode::Fullscreen => saved_rect,
        Mode::Snapped(target) => snapped_client(target, work, floating_restore, titlebars, margin),
        Mode::Maximized => maximized_client(work, titlebars, margin),
    }
}

/// Bridges the model rectangle to the shared geometry type the window-core policy speaks.
const fn api_rect(rect: Rect) -> flamewm_api::Rect {
    flamewm_api::Rect::new(rect.x, rect.y, rect.w, rect.h)
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
    Rect::new(x, frame.y, BUTTON_SIZE, TITLEBAR_HEIGHT)
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

/// Whether a chord on `key` may fire with `mods`. A single printable character always needs
/// Ctrl, Alt or Super (Shift alone is not enough: `Shift+d` is how you type `D`), and an empty
/// key never matches anything.
fn chord_can_fire(mods: Mods, key: &str) -> bool {
    // Binding a printable key without a command modifier would fire while the user types.
    !key.is_empty()
        && (!flamewm_platform::shortcuts::is_typing_key(key) || mods.ctrl || mods.alt || mods.logo)
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

    /// `Super+K` steps backwards: from the topmost window it wraps to the bottom of the stack,
    /// which is where X11 `cycle_window(-1)` lands. Minimized windows and other workspaces are
    /// never candidates, and a lone window stays focused.
    #[test]
    fn cycle_focus_prev_wraps_to_the_bottom_of_the_stack() {
        let mut model = model_with(&[1, 2, 3]);
        assert_eq!(model.cycle_focus_prev(), Some(1));
        assert_eq!(model.visible_top_first(), vec![1, 3, 2]);
        assert_eq!(model.cycle_focus_prev(), Some(2));
        assert_eq!(model.visible_top_first(), vec![2, 1, 3]);

        // One window is its own predecessor, as X11's modular step returns it unchanged.
        assert_eq!(model_with(&[7]).cycle_focus_prev(), Some(7));
        assert_eq!(WindowModel::new(1).cycle_focus_prev(), None);

        // Skipped candidates: a minimized window and a window on another workspace.
        let mut model = model_with(&[1, 2]);
        model.add(9, Rect::new(0, 0, 200, 200), 1);
        model.minimize(2);
        assert_eq!(model.cycle_focus_prev(), Some(1));
    }

    /// The directional search scores distance along the axis four times the drift across it, so the
    /// nearest window on that side wins even when a farther one is better aligned. Anything not on
    /// the requested side is never a target.
    #[test]
    fn focus_direction_picks_the_nearest_window_on_that_side() {
        let mut model = WindowModel::new(2);
        model.add(1, Rect::new(100, 100, 200, 200), 0);
        model.add(2, Rect::new(500, 100, 200, 200), 0);
        model.add(3, Rect::new(100, 500, 200, 200), 0);
        assert_eq!(model.focus_direction(1, D::Right), Some(2));
        assert_eq!(model.focus_direction(1, D::Down), Some(3));
        assert_eq!(model.focus_direction(1, D::Left), None);
        assert_eq!(model.focus_direction(1, D::Up), None);
        // Both windows sit to the left of 2 at the same distance along the axis; the one that also
        // lines up horizontally wins on the across term.
        assert_eq!(model.focus_direction(2, D::Left), Some(1));

        // A minimized window is not a candidate, and neither is one on another workspace.
        model.minimize(2);
        assert_eq!(model.focus_direction(1, D::Right), None);
        model.restore(2);
        assert_eq!(model.focus_direction(1, D::Right), Some(2));
        model.move_to_workspace(2, 1);
        assert_eq!(model.focus_direction(1, D::Right), None);
    }

    /// `Super+equal` / `Super+minus` follow X11 `resize_active`: 40x30 per press, floored at
    /// 160x96, and only for a floating window.
    #[test]
    fn resize_step_grows_shrinks_and_floors_a_floating_window() {
        let mut model = WindowModel::new(1);
        model.add(1, Rect::new(10, 10, 200, 200), 0);
        model.resize_step(1, true);
        assert_eq!(model.get(1).unwrap().rect, Rect::new(10, 10, 240, 230));
        model.resize_step(1, false);
        assert_eq!(model.get(1).unwrap().rect, Rect::new(10, 10, 200, 200));
        for _ in 0..20 {
            model.resize_step(1, false);
        }
        assert_eq!(model.get(1).unwrap().rect, Rect::new(10, 10, 160, 96));

        // A maximized window is not floating: the step leaves its geometry alone.
        model.set_rect(1, Rect::new(10, 10, 500, 400));
        model.toggle_maximize(1, WORK);
        let maximized = model.get(1).unwrap().rect;
        model.resize_step(1, false);
        assert_eq!(model.get(1).unwrap().rect, maximized);
    }

    /// `Super+Shift+arrows` nudge a floating window 40 px and a maximized one never moves.
    #[test]
    fn nudge_moves_only_a_floating_window() {
        let mut model = WindowModel::new(1);
        model.add(1, Rect::new(100, 100, 200, 200), 0);
        model.nudge(1, D::Left);
        model.nudge(1, D::Down);
        assert_eq!(model.get(1).unwrap().rect, Rect::new(60, 140, 200, 200));

        model.toggle_maximize(1, WORK);
        let maximized = model.get(1).unwrap().rect;
        model.nudge(1, D::Right);
        assert_eq!(model.get(1).unwrap().rect, maximized);
    }

    /// `Super+V` is X11 `toggle_float`: a floating window fills the work area, and pressing again
    /// returns it to its floating rect. From fullscreen the first press lands on the floating rect,
    /// dropping the maximize underneath.
    #[test]
    fn toggle_float_fills_the_work_area_and_returns_to_floating() {
        let mut model = model_with(&[1]);
        let floating = model.get(1).unwrap().rect;
        model.toggle_float(1, WORK, WORK);
        assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
        assert_eq!(model.get(1).unwrap().frame(), WORK);
        model.toggle_float(1, WORK, WORK);
        assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
        assert_eq!(model.get(1).unwrap().rect, floating);

        // Fullscreen over a maximized layout: leaving it returns to floating, not to maximized.
        model.toggle_maximize(1, WORK);
        model.set_fullscreen(1, WORK, true);
        model.toggle_float(1, WORK, WORK);
        assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
        assert_eq!(model.get(1).unwrap().rect, floating);
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

    /// A model that is never configured keeps the defaults, which are also the regression baseline:
    /// titlebars on and no margin, i.e. exactly the geometry and hit testing this model had before
    /// the two settings were read.
    #[test]
    fn defaults_are_titlebars_on_and_no_margin() {
        for model in [WindowModel::new(2), WindowModel::default()] {
            assert!(model.titlebars);
            assert_eq!(model.titlebars, DEFAULT_TITLEBARS);
            assert_eq!(model.window_margin, 0);
            assert_eq!(model.window_margin, DEFAULT_WINDOW_MARGIN);
            assert_eq!(model.work_area, None);
        }
    }

    /// `titlebars=false` is X11 `reflow_titlebars(false)`: the frame's top edge moves down by the
    /// band, so the client keeps its on-screen position and the frame equals the client rect. The
    /// band above the client is then outside the frame and hits nothing at all.
    #[test]
    fn titlebars_off_drops_the_band_from_geometry_and_hit_testing() {
        let mut model = WindowModel::new(1);
        model.add(1, Rect::new(100, 100, 300, 300), 0);
        // Baseline, titlebars on: the band is part of the frame and grabs the click.
        assert!(model.get(1).unwrap().decorated());
        assert_eq!(
            model.get(1).unwrap().frame(),
            Rect::new(100, 100 - TITLEBAR_HEIGHT, 300, 300 + TITLEBAR_HEIGHT)
        );
        assert_eq!(model.hit(110.0, 100.0 - 10.0), Some(Hit::Titlebar(1)));

        model.configure(false, 0);
        let window = model.get(1).unwrap().clone();
        assert!(!window.decorated());
        assert_eq!(
            window.rect,
            Rect::new(100, 100, 300, 300),
            "client stays put"
        );
        assert_eq!(
            window.frame(),
            window.rect,
            "frame no longer includes a band"
        );
        assert_eq!(
            model.hit(110.0, 100.0 - 10.0),
            None,
            "band is not hit-testable"
        );
        // Inside the client the hit is a plain client hit: no titlebar and no buttons.
        assert_eq!(model.hit(200.0, 200.0), Some(Hit::Client(1)));
        assert_eq!(model.hit(350.0, 120.0), Some(Hit::Client(1)));
        // The resize grip still belongs to the client rect.
        assert_eq!(model.hit(102.0, 200.0), Some(Hit::Resize(1)));
    }

    /// Turning titlebars back on restores the band above the unchanged client rect, which is the
    /// same reflow X11 applies when the setting flips, so the hit test classifies it again.
    #[test]
    fn titlebars_on_again_restores_the_band_above_the_same_client() {
        let mut model = WindowModel::new(1);
        model.add(1, Rect::new(100, 100, 300, 300), 0);
        model.configure(false, 0);
        model.configure(true, 0);
        let window = model.get(1).unwrap().clone();
        assert!(window.decorated());
        assert_eq!(window.rect, Rect::new(100, 100, 300, 300));
        assert_eq!(
            window.frame(),
            Rect::new(100, 100 - TITLEBAR_HEIGHT, 300, 300 + TITLEBAR_HEIGHT)
        );
        assert_eq!(model.hit(110.0, 100.0 - 10.0), Some(Hit::Titlebar(1)));
        let close = button_rect(window.frame(), Button::Close);
        assert_eq!(
            model.hit(f64::from(close.x + 2), f64::from(close.y + 2)),
            Some(Hit::Button(1, Button::Close))
        );
    }

    /// Windows added after the settings were applied inherit them; a negative margin is "no margin",
    /// as `apply_window_margin` itself treats it.
    #[test]
    fn configure_reaches_new_windows_and_clamps_a_negative_margin() {
        let mut model = WindowModel::new(1);
        model.configure(false, -5);
        assert_eq!(model.window_margin, 0, "a negative margin means none");
        model.add(2, Rect::new(0, 0, 200, 200), 0);
        assert!(!model.get(2).unwrap().decorated());
        assert_eq!(model.get(2).unwrap().frame(), Rect::new(0, 0, 200, 200));
    }

    /// `windowMargin` parity with X11: a maximized frame is the work area inset by
    /// `flamewm_window_core::apply_window_margin`, and the titlebar band comes out of the client.
    #[test]
    fn maximize_insets_the_work_area_by_the_window_margin() {
        let mut model = model_with(&[1]);
        model.configure(true, 2);
        model.toggle_maximize(1, WORK);
        let window = model.get(1).unwrap().clone();
        let frame = Rect::new(2, 2, 996, 696);
        assert_eq!(window.frame(), frame);
        assert_eq!(
            window.rect,
            Rect::new(
                frame.x,
                frame.y + TITLEBAR_HEIGHT,
                frame.w,
                frame.h - TITLEBAR_HEIGHT
            )
        );
        // Same rectangle the shared policy produces for a tile that fills the work area.
        assert_eq!(
            flamewm_window_core::apply_window_margin(
                flamewm_api::Rect::new(0, 0, 1000, 700),
                flamewm_api::Rect::new(0, 0, 1000, 700),
                2
            ),
            flamewm_api::Rect::new(frame.x, frame.y, frame.w, frame.h)
        );
        // Leaving maximized restores the floating rect untouched by the margin.
        model.toggle_maximize(1, WORK);
        assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
    }

    /// Margin 0 is today's geometry byte for byte: the frame fills the work area exactly.
    #[test]
    fn zero_margin_maximize_is_unchanged_from_today() {
        let mut model = model_with(&[1]);
        model.configure(true, 0);
        model.toggle_maximize(1, WORK);
        let frame = model.get(1).unwrap().frame();
        assert_eq!(frame, WORK);
        assert_eq!(
            model.get(1).unwrap().rect,
            Rect::new(0, TITLEBAR_HEIGHT, WORK.w, WORK.h - TITLEBAR_HEIGHT)
        );
        model.configure(true, 0);
        assert_eq!(
            model.get(1).unwrap().frame(),
            frame,
            "re-applying is a no-op"
        );
    }

    /// Without titlebars the maximized client fills the margined work area, as X11 does when
    /// `titlebar_height()` is 0 and the frame equals the client.
    #[test]
    fn maximize_without_titlebars_fills_the_margined_work_area() {
        let mut model = model_with(&[1]);
        model.configure(false, 2);
        model.toggle_maximize(1, WORK);
        let window = model.get(1).unwrap().clone();
        assert!(!window.decorated());
        assert_eq!(window.rect, Rect::new(2, 2, 996, 696));
        assert_eq!(window.frame(), window.rect);
    }

    /// `Super+V` goes through the same maximize path, so it picks the margin up too.
    #[test]
    fn toggle_float_fills_the_margined_work_area() {
        let mut model = model_with(&[1]);
        model.configure(true, 2);
        model.toggle_float(1, WORK, WORK);
        assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
        assert_eq!(model.get(1).unwrap().frame(), Rect::new(2, 2, 996, 696));
    }

    /// An output size change re-applies the margin through `refit_output`, for the live maximized
    /// window and for the maximized rect a fullscreen window restores to.
    #[test]
    fn refit_output_reapplies_the_margin_after_an_output_change() {
        let mut model = model_with(&[1]);
        model.configure(true, 2);
        model.toggle_maximize(1, WORK);

        let resized = Rect::new(0, 0, 1280, 720);
        let usable = Rect::new(0, 0, 1280, 688);
        model.refit_output(1, resized, usable);
        assert_eq!(model.get(1).unwrap().frame(), Rect::new(2, 2, 1276, 684));

        // Fullscreen over the maximized layout: the saved maximize rect tracks the new margin too.
        model.set_fullscreen(1, resized, true);
        let bigger = Rect::new(0, 0, 1600, 900);
        let bigger_work = Rect::new(0, 0, 1600, 868);
        model.refit_output(1, bigger, bigger_work);
        assert_eq!(
            model.get(1).unwrap().rect,
            bigger,
            "fullscreen keeps the output rect"
        );
        model.set_fullscreen(1, bigger, false);
        assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
        assert_eq!(model.get(1).unwrap().frame(), Rect::new(2, 2, 1596, 864));
    }

    /// A live settings change reflows a maximized frame at once, the way X11 `reflow_titlebars`
    /// plus `reconcile_work_area` do, without waiting for the next maximize keypress.
    #[test]
    fn configure_reflows_a_maximized_frame_immediately() {
        let mut model = model_with(&[1]);
        model.configure(true, 2);
        model.toggle_maximize(1, WORK);
        assert_eq!(model.get(1).unwrap().frame(), Rect::new(2, 2, 996, 696));

        // Margin change: same work area, new inset, no second maximize call.
        model.configure(true, 8);
        assert_eq!(model.get(1).unwrap().frame(), Rect::new(8, 8, 984, 684));

        // Titlebars off: the frame still fills the margined work area, no band to subtract.
        model.configure(false, 8);
        let window = model.get(1).unwrap().clone();
        assert!(!window.decorated());
        assert_eq!(window.frame(), Rect::new(8, 8, 984, 684));
        assert_eq!(window.rect, window.frame());

        // Back to the defaults: the unconfigured geometry this model always produced.
        model.configure(true, 0);
        assert_eq!(model.get(1).unwrap().frame(), WORK);
    }

    /// `configure` never touches a floating window and never overwrites its restore rect.
    #[test]
    fn configure_leaves_floating_and_restore_geometry_alone() {
        let mut model = model_with(&[1]);
        let floating = model.get(1).unwrap().rect;
        model.configure(false, 4);
        assert_eq!(model.get(1).unwrap().rect, floating);

        model.configure(true, 0);
        model.toggle_maximize(1, WORK);
        model.configure(true, 6);
        model.toggle_maximize(1, WORK);
        assert_eq!(model.get(1).unwrap().rect, floating);
    }

    /// A new window cascades below the band only when the model draws one.
    #[test]
    fn cascade_reserves_the_band_only_with_titlebars() {
        let mut model = WindowModel::new(1);
        let with_band = model.cascade_rect(0);
        assert_eq!(with_band.y, 60 + TITLEBAR_HEIGHT);
        model.configure(false, 0);
        let without_band = model.cascade_rect(0);
        assert_eq!(without_band.y, 60);
        assert_eq!(
            without_band,
            Rect::new(with_band.x, 60, with_band.w, with_band.h)
        );
    }

    #[test]
    fn fullscreen_resize_and_restore_maximized_window() {
        let mut model = model_with(&[1, 2]);
        let initial = model.get(1).unwrap().rect;
        model.toggle_maximize(1, WORK);
        let maximized = model.get(1).unwrap().rect;
        model.set_fullscreen(1, WORK, true);
        let resized = Rect::new(0, 0, 1280, 720);
        model.set_fullscreen(1, resized, true);
        assert_eq!(model.get(1).unwrap().rect, resized);
        model.set_fullscreen(1, resized, false);
        assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
        assert_eq!(model.get(1).unwrap().rect, maximized);
        model.toggle_maximize(1, WORK);
        assert_eq!(model.get(1).unwrap().rect, initial);
    }

    #[test]
    fn output_resize_refits_fullscreen_maximized_and_inactive_windows() {
        let mut model = model_with(&[1, 2, 3]);
        let original = model.get(1).unwrap().rect;
        model.toggle_maximize(1, WORK);
        model.set_fullscreen(1, WORK, true);
        model.move_to_workspace(1, 1);
        model.toggle_maximize(2, WORK);
        let resized = Rect::new(0, 0, 1280, 720);
        let usable = Rect::new(0, 0, 1280, 688);
        for id in [1, 2, 3] {
            model.refit_output(id, resized, usable);
        }
        assert_eq!(model.get(1).unwrap().rect, resized);
        assert_eq!(model.get(2).unwrap().frame(), usable);
        assert_eq!(model.get(3).unwrap().mode, Mode::Normal);
        model.set_fullscreen(1, resized, false);
        assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
        assert_eq!(model.get(1).unwrap().frame(), usable);
        model.toggle_maximize(1, WORK);
        assert_eq!(model.get(1).unwrap().rect, original);
    }

    #[test]
    fn fullscreen_resized_while_on_other_workspace_is_not_clipped() {
        let mut model = model_with(&[1]);
        model.move_to_workspace(1, 1);
        model.set_fullscreen(1, WORK, true);
        model.set_fullscreen(1, Rect::new(0, 0, 640, 360), true);
        assert_eq!(model.get(1).unwrap().rect, Rect::new(0, 0, 640, 360));
        model.set_fullscreen(1, WORK, false);
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
    fn snapped_modes_commit_every_shared_edge_and_corner_target() {
        let placements = [
            (SnapTarget::LeftHalf, Rect::new(0, 0, 500, 700)),
            (SnapTarget::RightHalf, Rect::new(500, 0, 500, 700)),
            (SnapTarget::TopHalf, Rect::new(0, 0, 1000, 350)),
            (SnapTarget::BottomHalf, Rect::new(0, 350, 1000, 350)),
            (SnapTarget::TopLeftQuarter, Rect::new(0, 0, 500, 350)),
            (SnapTarget::TopRightQuarter, Rect::new(500, 0, 500, 350)),
            (SnapTarget::BottomLeftQuarter, Rect::new(0, 350, 500, 350)),
            (
                SnapTarget::BottomRightQuarter,
                Rect::new(500, 350, 500, 350),
            ),
        ];
        let floating = Rect::new(120, 130, 640, 420);
        for (target, expected_frame) in placements {
            let mut model = WindowModel::new(1);
            model.configure(true, 0);
            model.add(1, floating, 0);
            model.snap_to(1, target, WORK, floating);
            let window = model.get(1).unwrap();
            assert_eq!(window.mode, Mode::Snapped(target));
            assert_eq!(window.frame(), expected_frame, "{target:?}");
            assert_eq!(
                window.restore, floating,
                "{target:?} keeps floating restore"
            );
        }

        let mut model = WindowModel::new(1);
        model.configure(true, 0);
        model.add(1, floating, 0);
        model.maximize_to(1, WORK, floating);
        assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
        assert_eq!(model.get(1).unwrap().frame(), WORK);
        assert_eq!(model.get(1).unwrap().restore, floating);
    }

    #[test]
    fn snap_preview_and_commit_share_margin_and_titlebar_conversion() {
        let work = Rect::new(0, 0, 1000, 700);
        let floating = Rect::new(120, 130, 640, 420);
        let mut model = WindowModel::new(1);
        model.configure(true, 2);
        model.add(1, floating, 0);
        let preview = model
            .snap_client_geometry(1, SnapTarget::LeftHalf, work)
            .unwrap();
        model.snap_to(1, SnapTarget::LeftHalf, work, floating);
        let window = model.get(1).unwrap();
        assert_eq!(window.rect, preview);
        assert_eq!(window.frame(), Rect::new(2, 2, 497, 696));
        assert_eq!(window.rect.y, 2 + TITLEBAR_HEIGHT);
        assert_eq!(window.rect.h, 696 - TITLEBAR_HEIGHT);
        assert_eq!(window.restore, floating);

        model.configure(false, 2);
        model.snap_to(1, SnapTarget::BottomRightQuarter, work, floating);
        assert_eq!(model.get(1).unwrap().frame(), Rect::new(501, 351, 497, 347));
        assert_eq!(model.get(1).unwrap().rect, model.get(1).unwrap().frame());

        let mut model = WindowModel::new(1);
        model.configure(true, 2);
        model.add(1, floating, 0);
        let preview = model
            .snap_client_geometry(1, SnapTarget::Maximize, work)
            .unwrap();
        model.maximize_to(1, work, floating);
        assert_eq!(model.get(1).unwrap().rect, preview);
        assert_eq!(model.get(1).unwrap().frame(), Rect::new(2, 2, 996, 696));
    }

    #[test]
    fn resizing_a_snapped_window_leaves_layout_without_snapping_on_release() {
        let floating = Rect::new(120, 130, 640, 420);
        let resized = Rect::new(118, 158, 644, 390);
        let mut model = WindowModel::new(1);
        model.add(1, floating, 0);
        model.snap_to(1, SnapTarget::LeftHalf, WORK, floating);
        model.set_rect(1, resized);
        assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
        assert_eq!(model.get(1).unwrap().rect, resized);
        assert_eq!(model.get(1).unwrap().restore, resized);
    }

    #[test]
    fn moving_out_of_snapped_or_maximized_restores_floating_size() {
        let floating = Rect::new(120, 130, 640, 420);
        let moved = Rect::new(260, 90, 640, 420);
        for from_maximized in [false, true] {
            let mut model = WindowModel::new(1);
            model.add(1, floating, 0);
            if from_maximized {
                model.maximize_to(1, WORK, floating);
            } else {
                model.snap_to(1, SnapTarget::BottomRightQuarter, WORK, floating);
            }
            model.commit_move(1, moved);
            let window = model.get(1).unwrap();
            assert_eq!(window.mode, Mode::Normal);
            assert_eq!(window.rect, moved);
            assert_eq!(window.restore, moved);
        }
    }

    #[test]
    fn activated_fullscreen_move_commits_free_and_snapped_placements() {
        let floating = Rect::new(120, 130, 640, 420);
        let moved = Rect::new(260, 90, 640, 420);
        let mut model = WindowModel::new(1);
        model.add(1, floating, 0);
        model.set_fullscreen(1, WORK, true);
        model.commit_move(1, moved);
        let window = model.get(1).unwrap();
        assert_eq!(window.mode, Mode::Normal);
        assert_eq!(window.rect, moved);
        assert_eq!(window.restore, moved);

        // A later fullscreen cycle restores the committed floating geometry, not the old rect.
        model.set_fullscreen(1, WORK, true);
        model.set_fullscreen(1, WORK, false);
        assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
        assert_eq!(model.get(1).unwrap().rect, moved);

        model.set_fullscreen(1, WORK, true);
        model.snap_to(1, SnapTarget::LeftHalf, WORK, moved);
        let window = model.get(1).unwrap();
        assert_eq!(window.mode, Mode::Snapped(SnapTarget::LeftHalf));
        assert_eq!(window.frame(), Rect::new(0, 0, 500, 700));
        assert_eq!(window.restore, moved);

        model.set_fullscreen(1, WORK, true);
        model.maximize_to(1, WORK, moved);
        let window = model.get(1).unwrap();
        assert_eq!(window.mode, Mode::Maximized);
        assert_eq!(window.frame(), WORK);
        assert_eq!(window.restore, moved);
    }

    #[test]
    fn snapped_maximized_fullscreen_and_refit_keep_the_floating_restore() {
        let floating = Rect::new(120, 130, 640, 420);
        let mut model = WindowModel::new(1);
        model.configure(true, 2);
        model.add(1, floating, 0);
        model.snap_to(1, SnapTarget::LeftHalf, WORK, floating);
        let snapped = model.get(1).unwrap().frame();
        model.toggle_maximize(1, WORK);
        assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
        model.set_fullscreen(1, WORK, true);

        let output = Rect::new(0, 0, 1280, 800);
        let work = Rect::new(0, 0, 1280, 760);
        model.refit_output(1, output, work);
        assert_eq!(model.get(1).unwrap().rect, output);
        model.set_fullscreen(1, output, false);
        assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
        assert_eq!(model.get(1).unwrap().frame(), Rect::new(2, 2, 1276, 756));

        model.toggle_maximize(1, work);
        assert_eq!(
            model.get(1).unwrap().mode,
            Mode::Snapped(SnapTarget::LeftHalf)
        );
        assert_eq!(model.get(1).unwrap().frame(), Rect::new(2, 2, 637, 756));
        assert_ne!(
            model.get(1).unwrap().frame(),
            snapped,
            "snap refits to new work area"
        );
        assert_eq!(model.get(1).unwrap().restore, floating);

        let moved = Rect::new(240, 90, floating.w, floating.h);
        model.commit_move(1, moved);
        assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
        assert_eq!(model.get(1).unwrap().rect, moved);
        assert_eq!(model.get(1).unwrap().restore, moved);
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
    /// A removal must carry windows off the removed workspace. The plan's own mapping is what does
    /// it, so a window that sat on the removed index follows the mapping instead of ending up on a
    /// workspace that no longer exists.
    #[test]
    fn a_workspace_removal_carries_windows_through_the_plan_mapping() {
        use flamewm_api::workspace::{WorkspacePlan, WorkspacePlanKind};

        let mut model = WindowModel::new(3);
        model.add(1, Rect::new(0, 0, 100, 100), 0);
        model.add(2, Rect::new(0, 0, 100, 100), 2);
        model.switch_workspace(2);

        // Removing workspace 1 (index 1): old 0 stays, old 2 moves down to 1.
        let plan = WorkspacePlan {
            kind: WorkspacePlanKind::Remove,
            expected_revision: 1,
            new_revision: 2,
            old_count: 3,
            new_count: 2,
            requested_index: 1,
            old_to_new: vec![0, 1, 1],
            new_active: 1,
            new_last: None,
            new_names: vec!["1".to_owned(), "2".to_owned()],
        };
        plan.validate().unwrap();
        let moved = model.apply_workspace_plan(&plan);

        assert_eq!(model.workspace_count(), 2);
        assert!(moved, "the visible workspace changed from 2 to 1");
        assert_eq!(model.current_workspace(), 1);
        assert_eq!(
            model.get(1).unwrap().workspace,
            0,
            "a window on a surviving workspace keeps its place"
        );
        assert_eq!(
            model.get(2).unwrap().workspace,
            1,
            "a window above the removed index follows the mapping down"
        );
        // Nothing may point outside the new topology.
        for window in model.managed() {
            assert!(
                window.workspace < model.workspace_count(),
                "window {} landed outside the topology",
                window.id
            );
        }
    }

    /// Inserting a workspace shifts later windows up and leaves the visible one where the plan says.
    #[test]
    fn a_workspace_insert_shifts_later_windows() {
        use flamewm_api::workspace::{WorkspacePlan, WorkspacePlanKind};

        let mut model = WindowModel::new(2);
        model.add(1, Rect::new(0, 0, 100, 100), 0);
        model.add(2, Rect::new(0, 0, 100, 100), 1);

        let plan = WorkspacePlan {
            kind: WorkspacePlanKind::Insert,
            expected_revision: 1,
            new_revision: 2,
            old_count: 2,
            new_count: 3,
            requested_index: 1,
            old_to_new: vec![0, 2],
            new_active: 1,
            new_last: None,
            new_names: vec!["1".to_owned(), "2".to_owned(), "3".to_owned()],
        };
        plan.validate().unwrap();
        model.apply_workspace_plan(&plan);

        assert_eq!(model.workspace_count(), 3);
        assert_eq!(model.get(1).unwrap().workspace, 0);
        assert_eq!(
            model.get(2).unwrap().workspace,
            2,
            "a window after the inserted slot shifts up"
        );
    }
}

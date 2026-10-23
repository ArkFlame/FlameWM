//! Terminal application state: tabs, the active view (terminal or settings), selection, scrollback position, mouse
//! reporting and the commands that persist theme and config changes. Rendering lives in `ui` and `view`.
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use std::sync::mpsc::Sender;

use flamewm_api::settings::SettingsSnapshot;
use flamewm_ui_x11::{UiActionEvent, UiActionPhase, UiKeyEvent, UiKeyPhase};

use crate::keys::{self, Mods, Shortcut};
use crate::pty::{Launch, PtySize};
use crate::screen::MouseMode;
use crate::session::{Session, SessionEvent};
use crate::settings::{self, Command, SettingsView};
use crate::store::{FOLLOW_FLAMEWM, TerminalConfig, ThemeStore};
use crate::theme;
use crate::themes::{Palette, Theme};
use crate::glyphs::{Arm, blend};
use crate::view::{GridOptions, Run, Span, grid_runs, text_run};
use crate::screen::{CursorShape, char_width};
use crate::suggest::{History, LineInput};

/// Cell pitch as a fraction of the font size for the bundled monospace face (advance and line height).
const CELL_WIDTH_RATIO: f32 = 0.6;
const CELL_HEIGHT_RATIO: f32 = 1.25;
const SCROLL_LINES_PER_NOTCH: usize = 3;
/// Rows the UI pool can show: one tab bar plus grid rows.
pub const MAX_GRID_ROWS: usize = crate::pool::POOL_ROWS - 1;
/// Cells between two panes of a split, drawn as a divider.
const DIVIDER: usize = 1;
/// Smallest pane a split may produce. A split that cannot give both halves these cells is refused.
const MIN_PANE_COLS: usize = 2;
const MIN_PANE_ROWS: usize = 1;
/// Share of the split rectangle the first pane keeps when a split is created.
const SPLIT_RATIO: f32 = 0.5;

/// Split direction. `Columns` puts the panes side by side (Split Vertical); `Rows` stacks them (Split Horizontal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Columns,
    Rows,
}

/// A pane's place in the grid, in cells. `y0` counts grid rows below the tab bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x0: usize,
    pub y0: usize,
    pub cols: usize,
    pub rows: usize,
}

impl Rect {
    fn contains(self, row: usize, col: usize) -> bool {
        row >= self.y0 && row < self.y0 + self.rows && col >= self.x0 && col < self.x0 + self.cols
    }
}

/// One shell in a tab: its PTY and screen, plus the view state that belongs to it alone.
pub struct Pane {
    pub session: Session,
    /// The line typed at this pane's prompt, for suggestions and the history record.
    pub input: LineInput,
    /// The line typed at the prompt: Enter on a `/theme` line opens the theme menu instead of running the shell command.
    input_line: String,
    /// Lines scrolled back from the live bottom; 0 shows the live grid.
    pub view_offset: usize,
    pub selection: Option<Span>,
    drag_anchor: Option<(usize, usize)>,
    /// Last primary press in this pane: time, absolute row, column and click count (1 to 3).
    last_click: Option<(Instant, usize, usize, u8)>,
    /// Text of this pane's last mouse selection.
    pub selection_text: String,
    /// Grid size last given to the PTY, so an unchanged layout does not signal the child again.
    size: (usize, usize),
    /// Test seam: pins the prompt check, which otherwise needs a line-editing shell on a live PTY.
    #[cfg(test)]
    prompt: Option<bool>,
}

/// A tab's panes: leaves are shells, splits divide their rectangle between two subtrees.
pub enum PaneNode {
    Leaf(Box<Pane>),
    Split { axis: Axis, ratio: f32, a: Box<PaneNode>, b: Box<PaneNode> },
    /// Set only while a leaf is moved out: the edit that empties a slot fills it before it returns.
    Vacant,
}

pub struct Tab {
    /// Session id of the tab's first leaf, which names the tab.
    pub id: u64,
    /// Name set with Rename Tab; wins over the title the shell sets.
    pub title: Option<String>,
    pub root: PaneNode,
    /// Session id of the leaf that has the keyboard and the cursor.
    pub focused: u64,
}

impl PaneNode {
    /// True when the tab shows a single shell, which is the only case that draws the startup banner.
    pub fn is_single(&self) -> bool {
        matches!(self, PaneNode::Leaf(_))
    }

    /// The leftmost leaf of the subtree.
    fn leaf(&self) -> Option<&Pane> {
        match self {
            PaneNode::Leaf(pane) => Some(pane),
            PaneNode::Split { a, b, .. } => a.leaf().or_else(|| b.leaf()),
            PaneNode::Vacant => None,
        }
    }

    fn first_id(&self) -> Option<u64> {
        self.leaf().map(|pane| pane.session.id)
    }

    fn contains(&self, id: u64) -> bool {
        match self {
            PaneNode::Leaf(pane) => pane.session.id == id,
            PaneNode::Split { a, b, .. } => a.contains(id) || b.contains(id),
            PaneNode::Vacant => false,
        }
    }

    pub fn pane(&self, id: u64) -> Option<&Pane> {
        match self {
            PaneNode::Leaf(pane) => (pane.session.id == id).then_some(pane),
            PaneNode::Split { a, b, .. } => a.pane(id).or_else(|| b.pane(id)),
            PaneNode::Vacant => None,
        }
    }

    pub fn pane_mut(&mut self, id: u64) -> Option<&mut Pane> {
        match self {
            PaneNode::Leaf(pane) => (pane.session.id == id).then(|| pane.as_mut()),
            PaneNode::Split { a, b, .. } => match a.pane_mut(id) {
                Some(pane) => Some(pane),
                None => b.pane_mut(id),
            },
            PaneNode::Vacant => None,
        }
    }

    fn leaves(&self) -> Vec<&Pane> {
        let mut out = Vec::new();
        match self {
            PaneNode::Leaf(pane) => out.push(pane.as_ref()),
            PaneNode::Split { a, b, .. } => {
                out.extend(a.leaves());
                out.extend(b.leaves());
            }
            PaneNode::Vacant => {}
        }
        out
    }

    fn leaves_mut(&mut self) -> Vec<&mut Pane> {
        let mut out = Vec::new();
        match self {
            PaneNode::Leaf(pane) => out.push(pane.as_mut()),
            PaneNode::Split { a, b, .. } => {
                out.extend(a.leaves_mut());
                out.extend(b.leaves_mut());
            }
            PaneNode::Vacant => {}
        }
        out
    }

    /// Rectangles of every leaf, in tree order (left to right, top to bottom).
    pub fn rects(&self, rect: Rect) -> Vec<(u64, Rect)> {
        match self {
            PaneNode::Leaf(pane) => vec![(pane.session.id, rect)],
            PaneNode::Split { axis, ratio, a, b } => {
                let (first, second) = split_rect(rect, *axis, *ratio);
                let mut out = a.rects(first);
                out.extend(b.rects(second));
                out
            }
            PaneNode::Vacant => Vec::new(),
        }
    }

    /// Splits the leaf `id` along `axis`, putting `pane` second. The pane comes back when the leaf is not here.
    fn split_leaf(&mut self, id: u64, axis: Axis, ratio: f32, pane: Box<Pane>) -> Result<(), Box<Pane>> {
        match std::mem::replace(self, PaneNode::Vacant) {
            PaneNode::Leaf(existing) => {
                if existing.session.id == id {
                    *self = PaneNode::Split {
                        axis,
                        ratio,
                        a: Box::new(PaneNode::Leaf(existing)),
                        b: Box::new(PaneNode::Leaf(pane)),
                    };
                    Ok(())
                } else {
                    *self = PaneNode::Leaf(existing);
                    Err(pane)
                }
            }
            PaneNode::Split { axis: at, ratio: at_ratio, mut a, mut b } => {
                let result = match a.split_leaf(id, axis, ratio, pane) {
                    Ok(()) => Ok(()),
                    Err(pane) => match b.split_leaf(id, axis, ratio, pane) {
                        Ok(()) => Ok(()),
                        Err(pane) => Err(pane),
                    },
                };
                *self = PaneNode::Split { axis: at, ratio: at_ratio, a, b };
                result
            }
            PaneNode::Vacant => Err(pane),
        }
    }

    /// Removes the leaf `id`, collapsing its parent onto the surviving sibling. A node that held only that leaf is
    /// left `Vacant` for its own parent to collapse, so the caller must check the root afterwards.
    fn remove_leaf(&mut self, id: u64) -> Option<Removed> {
        match std::mem::replace(self, PaneNode::Vacant) {
            PaneNode::Leaf(pane) => {
                if pane.session.id == id {
                    return Some(Removed { pane, survivor: None });
                }
                *self = PaneNode::Leaf(pane);
                None
            }
            PaneNode::Split { axis, ratio, mut a, mut b } => {
                if let Some(removed) = a.remove_leaf(id) {
                    *self = if matches!(*a, PaneNode::Vacant) {
                        *b
                    } else {
                        PaneNode::Split { axis, ratio, a, b }
                    };
                    return Some(Removed { survivor: removed.survivor.or_else(|| self.first_id()), pane: removed.pane });
                }
                if let Some(removed) = b.remove_leaf(id) {
                    *self = if matches!(*b, PaneNode::Vacant) {
                        *a
                    } else {
                        PaneNode::Split { axis, ratio, a, b }
                    };
                    return Some(Removed { survivor: removed.survivor.or_else(|| self.first_id()), pane: removed.pane });
                }
                *self = PaneNode::Split { axis, ratio, a, b };
                None
            }
            PaneNode::Vacant => None,
        }
    }
}

/// A leaf taken out of a tab, with the leaf that should take its focus.
struct Removed {
    pane: Box<Pane>,
    survivor: Option<u64>,
}

/// Splits `rect` in two, leaving a one-cell divider between the halves. The halves may be smaller than a usable pane;
/// `split_fits` is the gate for that.
fn split_rect(rect: Rect, axis: Axis, ratio: f32) -> (Rect, Rect) {
    match axis {
        Axis::Columns => {
            let usable = rect.cols.saturating_sub(DIVIDER);
            let first = first_part(usable, ratio, MIN_PANE_COLS);
            (Rect { cols: first, ..rect }, Rect { x0: rect.x0 + first + DIVIDER, cols: usable.saturating_sub(first), ..rect })
        }
        Axis::Rows => {
            let usable = rect.rows.saturating_sub(DIVIDER);
            let first = first_part(usable, ratio, MIN_PANE_ROWS);
            (Rect { rows: first, ..rect }, Rect { y0: rect.y0 + first + DIVIDER, rows: usable.saturating_sub(first), ..rect })
        }
    }
}

/// Cells for the first half: `ratio` of `usable`, at least `min` and leaving at least `min` for the second half.
fn first_part(usable: usize, ratio: f32, min: usize) -> usize {
    if usable < 2 * min {
        return usable / 2;
    }
    (((usable as f32) * ratio).round() as usize).clamp(min, usable - min)
}

/// Whether a split at `rect` leaves both halves a usable pane.
fn split_fits(rect: Rect, axis: Axis) -> bool {
    let (first, second) = split_rect(rect, axis, SPLIT_RATIO);
    first.cols >= MIN_PANE_COLS && second.cols >= MIN_PANE_COLS && first.rows >= MIN_PANE_ROWS && second.rows >= MIN_PANE_ROWS
}

/// A pane's runs moved to its column, with the cells past its right edge dropped.
fn place_runs(line: Vec<Run>, rect: Rect) -> Vec<Run> {
    let mut out = Vec::with_capacity(line.len());
    for mut run in line {
        if run.col >= rect.cols {
            continue;
        }
        if run.col + run.cells > rect.cols {
            run = clip(&run, 0, rect.cols - run.col);
        }
        run.col += rect.x0;
        out.push(run);
    }
    out
}

/// Inline tab rename in progress: the tab index and the text typed so far.
pub struct Rename {
    pub tab: usize,
    pub text: String,
}

/// Startup banner: FlameWM art, then the `/theme` hint. Drawn above the shell until its output scrolls.
const BANNER: [&str; 7] = [
    r"     _____   _         _       __  __   _____  __        __  __  __ ",
    r"   |  ___| | |       / \     |  \/  | | ____| \ \      / / |  \/  | ",
    r"  | |_    | |      / _ \    | |\/| | |  _|    \ \ /\ / /  | |\/| |  ",
    r" |  _|   | |___  / ___ \   | |  | | | |___    \ V  V /   | |  | |   ",
    r"|_|     |_____| /_/   \_\ |_|  |_| |_____|    \_/\_/    |_|  |_|    ",
    "",
    "/theme - Change the Terminal theme.",
];
const BANNER_ART_ROWS: usize = 5;

/// Right-click menu items, in display order. Indexes are matched in `menu_action`.
const MENU: [&str; 8] = ["New Tab", "Close Tab", "Rename Tab", "Copy", "Paste", "Split Vertical", "Split Horizontal", "Close Pane"];

#[derive(Debug)]
pub enum View {
    Terminal,
    Settings(SettingsView),
}

/// Tab bar targets, in window coordinates on row 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    Close(usize),
    NewTab,
    Settings,
}

pub struct App {
    pub tabs: Vec<Tab>,
    pub active: usize,
    next_id: u64,
    pub cols: usize,
    pub grid_rows: usize,
    pub cell_w: f32,
    pub cell_h: f32,
    pub config: TerminalConfig,
    pub config_path: PathBuf,
    pub store: ThemeStore,
    pub themes: Vec<Theme>,
    pub snapshot: Option<SettingsSnapshot>,
    pub palette: Palette,
    pub surface_alpha: u8,
    pub view: View,
    /// System CLIPBOARD handle. `None` when no X connection is available; copy and paste then stay in-process.
    pub clipboard: Option<flamewm_clipboard::Clipboard>,
    pub events: Sender<SessionEvent>,
    pub wake: UnixStream,
    pub first_launch: Option<Launch>,
    pub packaged_root: PathBuf,
    /// Last projected rows and font, so only changed rows reach the UI.
    pub ui: crate::ui::UiCache,
    /// Open context menu: window row and column of its top-left cell.
    pub menu: Option<(usize, usize)>,
    pub rename: Option<Rename>,
    /// Global "most used" command store, shared with other terminal windows through its file.
    pub history: History,
    /// Startup banner shown above the first tab until its output scrolls.
    banner: bool,
    /// Window size in logical px, as last reported by the window. Grid and PTY size follow it.
    window: (u32, u32),
}

impl App {
    pub fn new(
        config: TerminalConfig,
        config_path: PathBuf,
        store: ThemeStore,
        snapshot: Option<SettingsSnapshot>,
        packaged_root: PathBuf,
        events: Sender<SessionEvent>,
        wake: UnixStream,
        window: (u32, u32),
        first_launch: Option<Launch>,
    ) -> Self {
        let mut app = Self {
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            cols: 80,
            grid_rows: 24,
            cell_w: 1.0,
            cell_h: 1.0,
            config,
            config_path,
            themes: Vec::new(),
            palette: crate::themes::builtin_themes()[0].palette.clone(),
            store,
            snapshot,
            surface_alpha: 255,
            view: View::Terminal,
            clipboard: None,
            events,
            wake,
            first_launch,
            packaged_root,
            ui: crate::ui::UiCache::default(),
            menu: None,
            rename: None,
            history: History::open_default(),
            banner: false,
            window,
        };
        app.refresh_metrics();
        app.set_window(window.0, window.1);
        app.reload_themes();
        app
    }

    /// Recomputes cell metrics from the font size.
    pub fn refresh_metrics(&mut self) {
        // Whole pixels: the text backend hints advances to integer pixels, so a fractional pitch drifts across a row.
        let size = f32::from(self.config.font_size);
        self.cell_w = (size * CELL_WIDTH_RATIO).round();
        self.cell_h = (size * CELL_HEIGHT_RATIO).round();
    }

    pub fn reload_themes(&mut self) {
        self.themes = self.store.all_themes();
        self.palette = theme::resolve_palette(&self.config.theme, &self.themes, self.snapshot.as_ref());
        self.surface_alpha = theme::surface_alpha(self.config.background_opacity);
    }

    /// Grid size for a window in logical px. Row 0 is the tab bar.
    pub fn grid_for(&self, width: u32, height: u32) -> (usize, usize) {
        let cols = ((width as f32 / self.cell_w).floor() as usize).clamp(10, 400);
        let rows = ((height as f32 / self.cell_h).floor() as usize).saturating_sub(1).clamp(4, MAX_GRID_ROWS);
        (cols, rows)
    }

    /// The whole grid below the tab bar, in cells. Panes are laid out inside it.
    fn grid_rect(&self) -> Rect {
        Rect { x0: 0, y0: 0, cols: self.cols, rows: self.grid_rows }
    }

    pub fn set_window(&mut self, width: u32, height: u32) {
        self.window = (width, height);
        let (cols, rows) = self.grid_for(width, height);
        // A new grid size moves every cell, so a selection no longer describes what is drawn.
        if cols != self.cols || rows != self.grid_rows {
            self.cols = cols;
            self.grid_rows = rows;
            for tab in &mut self.tabs {
                for pane in tab.root.leaves_mut() {
                    pane.selection = None;
                    pane.drag_anchor = None;
                }
            }
        }
        self.relayout();
    }

    /// Gives every pane the size of its place in the tree, so each PTY follows its own rectangle.
    fn relayout(&mut self) {
        let (cell_w, cell_h) = (self.cell_w, self.cell_h);
        let grid = self.grid_rect();
        for tab in &mut self.tabs {
            for (id, rect) in tab.root.rects(grid) {
                let Some(pane) = tab.root.pane_mut(id) else { continue };
                let size = (rect.cols.max(1), rect.rows.max(1));
                if size == pane.size {
                    continue;
                }
                pane.size = size;
                pane.session.resize(size.0, size.1, ((size.0 as f32 * cell_w) as u16, (size.1 as f32 * cell_h) as u16));
            }
        }
    }

    /// PTY size for a pane of `cols` x `rows` cells.
    fn pty_size(&self, cols: usize, rows: usize) -> PtySize {
        PtySize {
            cols: cols as u16,
            rows: rows as u16,
            pixel_width: (cols as f32 * self.cell_w) as u16,
            pixel_height: (rows as f32 * self.cell_h) as u16,
        }
    }

    /// Starts a pane with a fresh session id, so output and exit route back to that leaf alone.
    fn start_pane(&mut self, launch: &Launch, cols: usize, rows: usize) -> Result<Box<Pane>, String> {
        let id = self.next_id;
        self.next_id += 1;
        let wake = self.wake.try_clone().map_err(|error| error.to_string())?;
        let session = Session::start(id, launch, self.pty_size(cols.max(1), rows.max(1)), self.config.scrollback, self.events.clone(), wake)?;
        Ok(Box::new(Pane {
            session,
            input: LineInput::default(),
            input_line: String::new(),
            view_offset: 0,
            selection: None,
            drag_anchor: None,
            last_click: None,
            selection_text: String::new(),
            size: (cols.max(1), rows.max(1)),
            #[cfg(test)]
            prompt: None,
        }))
    }

    /// Starts a tab. `launch` defaults to the user's shell in `cwd`.
    pub fn open_tab(&mut self, launch: Launch) -> Result<(), String> {
        let pane = self.start_pane(&launch, self.cols, self.grid_rows)?;
        let id = pane.session.id;
        // The banner belongs to the first tab only.
        self.banner = self.tabs.is_empty();
        self.tabs.push(Tab { id, title: None, root: PaneNode::Leaf(pane), focused: id });
        self.active = self.tabs.len() - 1;
        Ok(())
    }

    /// Opens a tab in the focused pane's working directory (the shell when the directory is unknown).
    pub fn open_tab_here(&mut self) -> Result<(), String> {
        let cwd = self.focused_pane().and_then(|pane| pane.session.working_directory());
        self.open_tab(Launch::shell(cwd))
    }

    pub fn close_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            let mut tab = self.tabs.remove(index);
            for pane in tab.root.leaves_mut() {
                pane.session.pty.kill();
            }
            self.retarget_active(index);
        }
    }

    /// Keeps `active` on the same tab after the tab at `index` was removed.
    fn retarget_active(&mut self, index: usize) {
        if index < self.active {
            self.active -= 1;
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
    }

    /// Session id of the leaf that owns the keyboard, when the active tab still holds it.
    fn focused_id(&self) -> Option<u64> {
        let tab = self.tabs.get(self.active)?;
        tab.root.contains(tab.focused).then_some(tab.focused)
    }

    pub fn focused_pane(&self) -> Option<&Pane> {
        let tab = self.tabs.get(self.active)?;
        tab.root.pane(tab.focused)
    }

    fn focused_pane_mut(&mut self) -> Option<&mut Pane> {
        let id = self.focused_id()?;
        self.tabs.get_mut(self.active)?.root.pane_mut(id)
    }

    pub fn pane(&self, id: u64) -> Option<&Pane> {
        self.tabs.iter().find_map(|tab| tab.root.pane(id))
    }

    fn pane_mut(&mut self, id: u64) -> Option<&mut Pane> {
        self.tabs.iter_mut().find_map(|tab| tab.root.pane_mut(id))
    }

    /// The pane holding a grid cell, with its rectangle.
    fn pane_at(&self, grid_row: usize, col: usize) -> Option<(u64, Rect)> {
        let tab = self.tabs.get(self.active)?;
        tab.root.rects(self.grid_rect()).into_iter().find(|(_, rect)| rect.contains(grid_row, col))
    }

    fn pane_rect(&self, id: u64) -> Option<Rect> {
        let tab = self.tabs.get(self.active)?;
        tab.root.rects(self.grid_rect()).into_iter().find(|(pane, _)| *pane == id).map(|(_, rect)| rect)
    }

    /// The pane a drag started in: its press set `drag_anchor`, and a motion elsewhere must not extend it.
    fn drag_pane(&self) -> Option<u64> {
        let tab = self.tabs.get(self.active)?;
        tab.root.leaves().into_iter().find(|pane| pane.drag_anchor.is_some()).map(|pane| pane.session.id)
    }

    fn focus_pane(&mut self, id: u64) {
        if let Some(tab) = self.tabs.get_mut(self.active)
            && tab.root.contains(id)
        {
            tab.focused = id;
        }
    }

    /// Splits the focused pane along `axis`, starting a shell in the focused shell's directory. Nothing happens when
    /// the halves would be too small to use.
    pub fn split_focused(&mut self, axis: Axis) -> Result<(), String> {
        let Some(focused) = self.focused_id() else { return Ok(()) };
        let Some(rect) = self.pane_rect(focused) else { return Ok(()) };
        if !split_fits(rect, axis) {
            return Ok(());
        }
        let (_, second) = split_rect(rect, axis, SPLIT_RATIO);
        let pane = self.start_pane(&Launch::shell(self.split_cwd()), second.cols, second.rows)?;
        let id = pane.session.id;
        let tab = &mut self.tabs[self.active];
        if tab.root.split_leaf(focused, axis, SPLIT_RATIO, pane).is_err() {
            // The focused leaf vanished between the two steps: the new pane is dropped, and its PTY with it.
            return Ok(());
        }
        tab.focused = id;
        self.relayout();
        Ok(())
    }

    /// Directory a new pane starts in: the focused shell's when it is a readable directory, else home.
    fn split_cwd(&self) -> Option<PathBuf> {
        self.focused_pane().and_then(|pane| pane.session.working_directory()).filter(|dir| dir.is_dir()).or_else(home_dir)
    }

    /// Closes the focused pane; the last pane of a tab closes the tab.
    pub fn close_pane(&mut self) {
        if let Some(id) = self.focused_id()
            && let Some(mut pane) = self.remove_pane(id)
        {
            pane.session.pty.kill();
        }
    }

    /// Takes the leaf `id` out of the tree, collapsing its parent onto the surviving sibling and giving that sibling
    /// the focus. The tab goes when its last leaf leaves. The removed pane is returned so the caller ends its process.
    fn remove_pane(&mut self, id: u64) -> Option<Box<Pane>> {
        let index = self.tabs.iter().position(|tab| tab.root.contains(id))?;
        let tab = &mut self.tabs[index];
        let removed = tab.root.remove_leaf(id)?;
        if matches!(tab.root, PaneNode::Vacant) {
            self.tabs.remove(index);
            self.retarget_active(index);
            return Some(removed.pane);
        }
        if tab.focused == id {
            tab.focused = removed.survivor.or_else(|| tab.root.first_id()).unwrap_or(id);
        }
        self.relayout();
        Some(removed.pane)
    }

    /// Applies output or exit from a reader thread. Returns true when no tabs remain and the window should close.
    pub fn session_event(&mut self, event: SessionEvent) -> bool {
        match event {
            SessionEvent::Output { id, bytes } => {
                if let Some(pane) = self.pane_mut(id) {
                    pane.session.feed(&bytes);
                }
                self.update_banner();
            }
            SessionEvent::Exited { id } => {
                // Dropping the pane reaps the child that ended the reader thread.
                if let Some(pane) = self.remove_pane(id) {
                    drop(pane);
                }
                if self.tabs.is_empty() {
                    return true;
                }
            }
        }
        false
    }

    fn send_to(&self, id: u64, bytes: Vec<u8>) {
        if let Some(pane) = self.pane(id) {
            pane.session.write(bytes);
        }
    }

    fn send_to_focused(&self, bytes: Vec<u8>) {
        if let Some(id) = self.focused_id() {
            self.send_to(id, bytes);
        }
    }

    /// Text of a pane's selection, also kept on the pane for the in-process paste fallback.
    fn copy_selection_in(&mut self, id: u64) -> String {
        let Some(pane) = self.pane_mut(id) else { return String::new() };
        let Some(span) = pane.selection else { return String::new() };
        let screen = &pane.session.screen;
        let mut out = Vec::new();
        for index in span.start.0..=span.end.0 {
            let Some(line) = screen.line_abs(index) else { continue };
            let from = if index == span.start.0 { span.start.1 } else { 0 };
            let to = if index == span.end.0 { span.end.1 } else { line.len().saturating_sub(1) };
            let mut text = String::new();
            for col in from..=to.min(line.len().saturating_sub(1)) {
                let cell = &line[col];
                if cell.attrs & crate::screen::attr::SPACER == 0 {
                    text.push(cell.ch);
                }
            }
            out.push(text.trim_end().to_owned());
        }
        pane.selection_text = out.join("\n");
        pane.selection_text.clone()
    }

    /// Makes `span` the pane's selection, copies its text, and publishes it on PRIMARY for middle-click.
    fn select_span(&mut self, id: u64, span: Span) {
        if let Some(pane) = self.pane_mut(id) {
            pane.selection = Some(span);
        }
        let text = self.copy_selection_in(id);
        if text.is_empty() {
            return;
        }
        if let Some(clipboard) = &self.clipboard
            && let Err(error) = clipboard.set_primary_text(&text)
        {
            eprintln!("flamewm-terminal: primary selection: {error}");
        }
    }

    /// Counts presses on the same cell within 400 ms: 1, 2 (word), 3 (line), then back to 1.
    fn register_click(&mut self, id: u64, abs_row: usize, col: usize) -> u8 {
        let now = Instant::now();
        let Some(pane) = self.pane_mut(id) else { return 1 };
        let count = next_click_count(pane.last_click, now, abs_row, col);
        pane.last_click = Some((now, abs_row, col, count));
        count
    }

    fn select_word(&mut self, id: u64, abs_row: usize, col: usize) {
        let chars: Vec<char> = match self.pane(id).and_then(|pane| pane.session.screen.line_abs(abs_row)) {
            Some(line) => line.iter().map(|cell| cell.ch).collect(),
            None => return,
        };
        if let Some((start, end)) = word_bounds(&chars, col) {
            self.select_span(id, Span::normalised((abs_row, start), (abs_row, end)));
        }
    }

    fn select_line(&mut self, id: u64, abs_row: usize) {
        let last = self.pane_rect(id).map_or(0, |rect| rect.cols.saturating_sub(1));
        self.select_span(id, Span::normalised((abs_row, 0), (abs_row, last)));
    }

    /// The focused pane's last in-process selection, the fallback when the system clipboard yields nothing.
    fn selection_text(&self) -> String {
        self.focused_pane().map(|pane| pane.selection_text.clone()).unwrap_or_default()
    }

    /// Ctrl+Shift+C: the selection goes to the system CLIPBOARD. Without an X connection it stays in-process.
    pub fn copy_to_clipboard(&mut self) {
        let Some(id) = self.focused_id() else { return };
        let text = self.copy_selection_in(id);
        if text.is_empty() {
            return;
        }
        if let Some(clipboard) = &self.clipboard
            && let Err(error) = clipboard.set_text(&text)
        {
            eprintln!("flamewm-terminal: clipboard: {error}");
        }
    }

    /// Ctrl+Shift+V: the system CLIPBOARD text, or the in-process selection when the system read yields nothing.
    pub fn paste(&mut self) {
        let system = match &self.clipboard {
            Some(clipboard) => match clipboard.get_text() {
                Ok(text) => text,
                Err(error) => {
                    eprintln!("flamewm-terminal: clipboard: {error}");
                    None
                }
            },
            None => None,
        };
        let text = system.unwrap_or_else(|| self.selection_text());
        self.send_paste(text);
    }

    /// Middle-click: the PRIMARY selection, or the in-process selection when PRIMARY yields nothing.
    fn paste_primary(&mut self) {
        let system = match &self.clipboard {
            Some(clipboard) => clipboard.get_primary_text().unwrap_or_else(|error| {
                eprintln!("flamewm-terminal: primary selection: {error}");
                None
            }),
            None => None,
        };
        let text = system.unwrap_or_else(|| self.selection_text());
        self.send_paste(text);
    }

    fn send_paste(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        let Some(id) = self.focused_id() else { return };
        let bracketed = self.pane(id).is_some_and(|pane| pane.session.screen.bracketed_paste);
        if let Some(pane) = self.pane_mut(id) {
            pane.input.paste(&text);
            pane.view_offset = 0;
        }
        let mut bytes = Vec::new();
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.replace('\n', "\r").as_bytes());
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        self.send_to(id, bytes);
    }

    /// Cursor drawn for the terminal, from the `cursor_style` setting (programs cannot change it).
    pub fn cursor_shape(&self) -> CursorShape {
        match self.config.cursor_style.as_str() {
            "underline" => CursorShape::Underline,
            "bar" => CursorShape::Bar,
            _ => CursorShape::Block,
        }
    }

    fn app_cursor(&self) -> bool {
        self.focused_pane().is_some_and(|pane| pane.session.screen.app_cursor)
    }

    /// Keyboard input: shortcuts first, then the settings view, then the child.
    pub fn key(&mut self, event: &UiKeyEvent) {
        if event.phase != UiKeyPhase::Press {
            return;
        }
        if self.menu.take().is_some() && event.key.eq_ignore_ascii_case("escape") {
            return;
        }
        if self.rename.is_some() {
            let text = event.text.as_deref().filter(|t| !t.is_empty() && !t.chars().any(char::is_control));
            match event.key.to_ascii_lowercase().as_str() {
                "return" | "enter" | "kp_enter" => self.finish_rename(true),
                "escape" => self.finish_rename(false),
                "backspace" => {
                    if let Some(rename) = self.rename.as_mut() {
                        rename.text.pop();
                    }
                }
                _ => {
                    if let (Some(rename), Some(text)) = (self.rename.as_mut(), text) {
                        rename.text.push_str(text);
                    }
                }
            }
            return;
        }
        let mods = Mods::from_names(event.modifiers.iter());
        if let Some(shortcut) = keys::shortcut(&event.key, mods) {
            self.shortcut(shortcut);
            return;
        }
        if let View::Settings(settings) = &mut self.view {
            let command = settings.handle_key(event, &self.themes);
            self.settings_command(command);
            return;
        }
        if let Some(bytes) = keys::encode(event, self.app_cursor()) {
            if let Some(pane) = self.focused_pane_mut() {
                pane.view_offset = 0;
                pane.selection = None;
            }
            if self.theme_command_submitted(event) {
                // Ctrl+U clears the typed `/theme` from the shell line, so the shell never runs it.
                self.send_to_focused(vec![0x15]);
                self.open_settings();
                return;
            }
            self.track_input_line(event);
            let bytes = self.track_key(event, bytes);
            self.send_to_focused(bytes);
        }
    }

    /// Enter on a typed `/theme` line, outside full-screen programs.
    fn theme_command_submitted(&self, event: &UiKeyEvent) -> bool {
        let enter = matches!(event.key.to_ascii_lowercase().as_str(), "return" | "enter" | "kp_enter");
        let Some(pane) = self.focused_pane() else { return false };
        enter && pane.input_line == "/theme" && !pane.session.screen.alt_active()
    }

    /// Follows the text sent to the shell. Editing keys the line cannot follow (arrows, Tab, Ctrl) clear it.
    fn track_input_line(&mut self, event: &UiKeyEvent) {
        let mods = Mods::from_names(event.modifiers.iter());
        let Some(pane) = self.focused_pane_mut() else { return };
        match event.key.to_ascii_lowercase().as_str() {
            "backspace" => {
                pane.input_line.pop();
            }
            "return" | "enter" | "kp_enter" | "escape" => pane.input_line.clear(),
            _ => match event.text.as_deref() {
                Some(text) if !mods.ctrl && !mods.alt && !text.chars().any(char::is_control) => pane.input_line.push_str(text),
                _ => pane.input_line.clear(),
            },
        }
    }

    /// Keeps the pane's typed line in step with the keys the child receives. Tab or Right at the end of a suggested
    /// line sends the suggestion's suffix instead of the key. Enter records the line when the shell is at its prompt.
    fn track_key(&mut self, event: &UiKeyEvent, bytes: Vec<u8>) -> Vec<u8> {
        self.history.refresh();
        let name = event.key.to_ascii_lowercase();
        let mods = Mods::from_names(event.modifiers.iter());
        if !mods.ctrl && !mods.alt && !mods.shift && matches!(name.as_str(), "tab" | "right") {
            if let Some(suffix) = self.ghost().map(str::to_owned) {
                if let Some(pane) = self.focused_pane_mut() {
                    pane.input.insert_str(&suffix);
                }
                return suffix.into_bytes();
            }
        }
        let at_prompt = self.at_prompt();
        let Some(pane) = self.focused_pane_mut() else { return bytes };
        if let Some(line) = pane.input.key(&name, mods, event.text.as_deref()) {
            if at_prompt {
                let _ = self.history.record(&line);
            }
        }
        bytes
    }

    /// Suffix of the typed line that the most used command completes. Drawn only at the shell prompt, on the live
    /// grid, with no menu or rename open and the line exact with the cursor at its end.
    fn ghost(&self) -> Option<&str> {
        let pane = self.focused_pane()?;
        if pane.view_offset != 0 || !matches!(self.view, View::Terminal) || self.menu.is_some() || self.rename.is_some() {
            return None;
        }
        if !pane.input.is_ready() || !self.at_prompt() {
            return None;
        }
        self.history.suggest(&pane.input.text())
    }

    /// The shell reads keys at its prompt: no full-screen program, and the line discipline is not canonical.
    fn at_prompt(&self) -> bool {
        let Some(pane) = self.focused_pane() else { return false };
        #[cfg(test)]
        if let Some(prompt) = pane.prompt {
            return prompt;
        }
        !pane.session.screen.alt_active() && !pane.session.pty.canonical()
    }

    /// Draws the suggestion after the cursor cell of the focused pane, 40% of the way from the foreground colour to
    /// the background.
    fn draw_ghost(&self, rows: &mut [Vec<Run>], suffix: &str) {
        let Some(pane) = self.focused_pane() else { return };
        let Some(rect) = self.pane_rect(pane.session.id) else { return };
        let (row, col) = pane.session.screen.cursor();
        let start = col + 1;
        let room = rect.cols.saturating_sub(start);
        let text: String = suffix.chars().take_while(|&c| char_width(c) == 1).take(room).collect();
        if text.is_empty() {
            return;
        }
        let start = rect.x0 + start;
        let Some(line) = rows.get_mut(1 + self.banner_rows() + rect.y0 + row) else { return };
        let fg = blend(self.palette.foreground, self.palette.background, 2, 5);
        let cells = text.chars().count();
        let runs = std::mem::take(line);
        *line = overlay(runs, start, cells, text_run(start, &text, fg, None, false));
    }

    /// The banner goes once the shell output scrolls or reaches the rows the banner covers.
    fn update_banner(&mut self) {
        let gone = match self.focused_pane() {
            Some(pane) => {
                let (cursor_row, _) = pane.session.screen.cursor();
                pane.session.screen.scrollback_len() > 0 || cursor_row + BANNER.len() >= self.grid_rows
            }
            None => false,
        };
        if gone {
            self.banner = false;
        }
    }

    /// Banner rows above the grid, only while the active tab holds one shell. Zero when the banner is gone or the
    /// window is too short to show it.
    fn banner_rows(&self) -> usize {
        let single = self.tabs.get(self.active).is_some_and(|tab| tab.root.is_single());
        if self.banner && single && self.grid_rows >= 2 * BANNER.len() { BANNER.len() } else { 0 }
    }

    fn shortcut(&mut self, shortcut: Shortcut) {
        match shortcut {
            Shortcut::Copy => self.copy_to_clipboard(),
            Shortcut::Paste => self.paste(),
            Shortcut::NewTab => {
                let _ = self.open_tab_here();
            }
            Shortcut::CloseTab => {
                let index = self.active;
                self.close_tab(index);
            }
            Shortcut::NextTab => {
                if !self.tabs.is_empty() {
                    self.active = (self.active + 1) % self.tabs.len();
                }
            }
            Shortcut::PreviousTab => {
                if !self.tabs.is_empty() {
                    self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
                }
            }
            Shortcut::Settings => self.toggle_settings(),
            Shortcut::FontLarger => self.change_font_size(1),
            Shortcut::FontSmaller => self.change_font_size(-1),
        }
    }

    pub fn toggle_settings(&mut self) {
        if matches!(self.view, View::Settings(_)) {
            self.close_settings();
        } else {
            self.open_settings();
        }
    }

    fn open_settings(&mut self) {
        self.view = View::Settings(SettingsView::open(&self.themes, &self.config, settings::mono_fonts()));
    }

    /// Leaves settings. Unsaved previews are dropped: the saved configuration is reloaded from disk.
    fn close_settings(&mut self) {
        self.config = std::fs::read_to_string(&self.config_path).ok().and_then(|text| TerminalConfig::from_toml(&text).ok()).unwrap_or_default();
        self.refresh_metrics();
        let (width, height) = self.window;
        self.set_window(width, height);
        self.reload_themes();
        self.view = View::Terminal;
    }

    /// Font size up or down, applied now. Grid and PTY size follow the window, which keeps its pixel size.
    fn change_font_size(&mut self, delta: i32) {
        let size = (i32::from(self.config.font_size) + delta).clamp(6, 40) as u16;
        if size != self.config.font_size {
            self.config.font_size = size;
            self.refresh_metrics();
            let (width, height) = self.window;
            self.set_window(width, height);
        }
    }

    /// Applies one settings command: config and theme writes persist to disk.
    pub fn settings_command(&mut self, command: Command) {
        match command {
            Command::None => {}
            Command::Close => self.close_settings(),
            Command::PreviewTheme(name) => self.palette = theme::resolve_palette(&name, &self.themes, self.snapshot.as_ref()),
            Command::Font(family) => self.config.font_family = family,
            Command::Opacity(percent) => {
                self.config.background_opacity = percent;
                self.surface_alpha = theme::surface_alpha(percent);
            }
            Command::Persist => {
                self.persist_config();
                self.settings_message("saved".into());
            }
            Command::Apply(name) => {
                self.config.theme = name;
                self.persist_config();
                self.reload_themes();
            }
            Command::Copy(name) => {
                let source = self.themes.iter().find(|t| t.name == name).cloned();
                if let Some(source) = source {
                    match self.store.copy_of(&source) {
                        Ok(copy) => {
                            self.reload_themes();
                            if let View::Settings(settings) = &mut self.view {
                                settings.focus(&self.themes, &copy.name);
                                settings.mode = crate::settings::Mode::Rename(copy.name.clone());
                                settings.message = "Copied. Type a new name and press Enter.".into();
                            }
                        }
                        Err(error) => self.settings_message(format!("copy failed: {error}")),
                    }
                }
            }
            Command::Delete(name) => {
                match self.store.delete(&name) {
                    Ok(()) => {
                        if self.config.theme == name {
                            self.config.theme = FOLLOW_FLAMEWM.into();
                            self.persist_config();
                        }
                        self.reload_themes();
                        self.settings_message(format!("deleted {name}"));
                    }
                    Err(error) => self.settings_message(format!("delete failed: {error}")),
                }
                if let View::Settings(settings) = &mut self.view {
                    settings.selected = settings.selected.min(self.themes.len().saturating_sub(1));
                }
            }
            Command::Rename { old, new } => {
                match self.store.rename(&old, &new) {
                    Ok(()) => {
                        if self.config.theme == old {
                            self.config.theme = new.clone();
                            self.persist_config();
                        }
                        self.reload_themes();
                        if let View::Settings(settings) = &mut self.view {
                            settings.focus(&self.themes, &new);
                        }
                        self.settings_message(format!("renamed to {new}"));
                    }
                    Err(error) => self.settings_message(format!("rename failed: {error}")),
                }
            }
            Command::Save(theme) => {
                if let Err(error) = self.store.save(&theme) {
                    self.settings_message(format!("save failed: {error}"));
                }
                self.reload_themes();
            }
            Command::FontSize(delta) => self.change_font_size(delta),
            Command::CycleCursor => {
                self.config.cursor_style = match self.config.cursor_style.as_str() {
                    "block" => "underline".into(),
                    "underline" => "bar".into(),
                    _ => "block".into(),
                };
                self.persist_config();
            }
        }
    }

    fn settings_message(&mut self, text: String) {
        if let View::Settings(settings) = &mut self.view {
            settings.message = text;
        }
    }

    pub fn persist_config(&self) {
        if let Some(parent) = self.config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&self.config_path, self.config.to_toml());
    }

    /// Title shown for tab `index`: the name set by Rename Tab, else the focused shell's title, else a default.
    fn tab_title(&self, index: usize) -> String {
        let tab = &self.tabs[index];
        let shell = tab.root.pane(tab.focused).or_else(|| tab.root.leaf()).and_then(|pane| pane.session.screen.title.clone());
        let title = tab.title.clone().or(shell).unwrap_or_else(|| format!("Terminal {}", index + 1));
        title.chars().take(24).collect()
    }

    /// Tab bar layout: labels, the new-tab button and their hit areas on row 0. The tab strip shows only with more
    /// than one tab (or while renaming). Settings opens with Ctrl+, or by typing `/theme`.
    pub fn tab_bar(&self) -> (Vec<Run>, Vec<(usize, usize, Hit)>) {
        let fg = self.palette.foreground;
        let muted = self.palette.ansi[8];
        let accent_bg = Some(self.palette.selection);
        let mut runs = Vec::new();
        let mut hits = Vec::new();
        let mut col = 0;
        if let View::Settings(_) = self.view {
            let label = " ← Back ";
            runs.push(text_run(0, label, fg, accent_bg, true));
            hits.push((0, label.chars().count(), Hit::Settings));
            return (runs, hits);
        }
        if self.tabs.len() > 1 || self.rename.is_some() {
            for index in 0..self.tabs.len() {
                let active = index == self.active;
                let title = match &self.rename {
                    Some(rename) if rename.tab == index => format!("{}\u{258f}", rename.text),
                    _ => self.tab_title(index),
                };
                let label = format!(" {title} ");
                let width = label.chars().count() + 2;
                let bg = if active { accent_bg } else { None };
                let colour = if active { fg } else { muted };
                runs.push(text_run(col, &label, colour, bg, active));
                hits.push((col, col + label.chars().count(), Hit::Tab(index)));
                runs.push(text_run(col + label.chars().count(), "× ", colour, bg, false));
                hits.push((col + label.chars().count(), col + width, Hit::Close(index)));
                col += width;
            }
            runs.push(text_run(col, " + ", fg, None, true));
            hits.push((col, col + 3, Hit::NewTab));
        }
        (runs, hits)
    }

    /// Grid rows of runs for the active view, with the tab bar as row 0. Every pane draws into its own rectangle, so
    /// a row holds the runs of all panes that share it, side by side and never overlapping.
    pub fn frame(&self) -> Vec<Vec<Run>> {
        let (bar, _) = self.tab_bar();
        let mut rows = vec![bar];
        match &self.view {
            View::Terminal => {
                // The banner takes the top rows of the grid; the grid's bottom rows give way to it.
                let banner = self.banner_rows();
                rows.extend(self.banner_lines_for(banner));
                let mut grid: Vec<Vec<Run>> = vec![Vec::new(); self.grid_rows.saturating_sub(banner)];
                if let Some(tab) = self.tabs.get(self.active) {
                    for (id, rect) in tab.root.rects(self.grid_rect()) {
                        let Some(pane) = tab.root.pane(id) else { continue };
                        let options = GridOptions {
                            view_offset: pane.view_offset,
                            selection: pane.selection,
                            // The cursor belongs to the pane with the keyboard.
                            cursor: id == tab.focused,
                            cursor_shape: self.cursor_shape(),
                        };
                        for (offset, line) in grid_runs(&pane.session.screen, &self.palette, &options).into_iter().enumerate() {
                            if offset >= rect.rows {
                                break;
                            }
                            if let Some(target) = grid.get_mut(rect.y0 + offset) {
                                target.extend(place_runs(line, rect));
                            }
                        }
                    }
                }
                rows.extend(grid);
                if let Some(suffix) = self.ghost() {
                    self.draw_ghost(&mut rows, suffix);
                }
                if let Some((row, col)) = self.menu {
                    self.draw_menu(&mut rows, row, col);
                }
            }
            View::Settings(settings) => {
                let palette = settings.preview().cloned().unwrap_or_else(|| self.palette.clone());
                let body = settings.render(self.cols, self.grid_rows, &self.themes, &palette, &self.config);
                rows.extend(body);
            }
        }
        rows
    }

    /// Banner rows, or nothing when the banner is off.
    fn banner_lines_for(&self, rows: usize) -> Vec<Vec<Run>> {
        BANNER[..rows]
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let art = index < BANNER_ART_ROWS;
                let colour = if art { self.palette.cursor } else { self.palette.foreground };
                vec![text_run(0, &text.chars().take(self.cols).collect::<String>(), colour, None, art)]
            })
            .collect()
    }

    /// Paints the context menu over window rows starting at `row`, column `col`.
    fn draw_menu(&self, rows: &mut [Vec<Run>], row: usize, col: usize) {
        let width = menu_width();
        for (index, item) in MENU.iter().enumerate() {
            let Some(line) = rows.get_mut(row + index) else { break };
            let label = format!(" {item:<w$} ", w = width - 2);
            let run = text_run(col, &label, self.palette.foreground, Some(self.palette.selection), false);
            *line = overlay(std::mem::take(line), col, width, run);
        }
    }

    fn menu_action(&mut self, index: usize) {
        match index {
            0 => {
                let _ = self.open_tab_here();
            }
            1 => self.close_tab(self.active),
            2 => self.rename = Some(Rename { tab: self.active, text: String::new() }),
            3 => self.copy_to_clipboard(),
            4 => self.paste(),
            5 => {
                let _ = self.split_focused(Axis::Columns);
            }
            6 => {
                let _ = self.split_focused(Axis::Rows);
            }
            _ => self.close_pane(),
        }
    }

    /// Ends an inline rename. A blank name clears the custom title, so the shell's title shows again.
    fn finish_rename(&mut self, commit: bool) {
        let Some(rename) = self.rename.take() else { return };
        if let (true, Some(tab)) = (commit, self.tabs.get_mut(rename.tab)) {
            let name = rename.text.trim();
            tab.title = (!name.is_empty()).then(|| name.to_owned());
        }
    }

    /// Mouse and wheel actions from the `terminal.surface` region.
    pub fn pointer(&mut self, event: &UiActionEvent) {
        if event.action == "scroll" {
            let dy = event.text.as_deref().and_then(|t| t.split_once(',')).and_then(|(_, dy)| dy.parse::<f32>().ok()).unwrap_or(0.0);
            if dy != 0.0 && event.inside {
                self.wheel(dy < 0.0, event);
            }
            return;
        }
        let row = (event.y / self.cell_h).floor().max(0.0) as usize;
        let col = (event.x / self.cell_w).floor().max(0.0) as usize;
        if let (View::Settings(settings), UiActionPhase::Hover | UiActionPhase::Motion) = (&mut self.view, event.phase) {
            let command = settings.hover_theme(row.saturating_sub(1), self.grid_rows, &self.themes);
            self.settings_command(command);
            return;
        }
        if let Some((menu_row, menu_col)) = self.menu {
            if event.phase == UiActionPhase::Press && event.button == 1 {
                self.menu = None;
                let inside = (menu_row..menu_row + MENU.len()).contains(&row) && (menu_col..menu_col + menu_width()).contains(&col);
                if inside {
                    self.menu_action(row - menu_row);
                    return;
                }
            }
        }
        let mouse_captured = !event.shift_down() && self.focused_pane().is_some_and(|pane| pane.session.screen.mouse != MouseMode::Off);
        if event.phase == UiActionPhase::Release && event.button == 3 && matches!(self.view, View::Terminal) && (row == 0 || !mouse_captured) {
            let top = (self.grid_rows + 1).saturating_sub(MENU.len());
            self.menu = Some((row.min(top), col.min(self.cols.saturating_sub(menu_width()))));
            return;
        }
        if row == 0 {
            if event.phase == UiActionPhase::Press && event.button == 1 {
                let (_, hits) = self.tab_bar();
                if let Some((_, _, hit)) = hits.into_iter().find(|(start, end, _)| (*start..*end).contains(&col)) {
                    self.tab_hit(hit);
                }
            }
            return;
        }
        if !matches!(self.view, View::Terminal) {
            return;
        }
        let Some(grid_row) = (row - 1).checked_sub(self.banner_rows()) else { return };
        if grid_row >= self.grid_rows || col >= self.cols {
            return;
        }
        // The pane under the pointer, except during a drag: that press belongs to the pane it started in.
        let drag = self.drag_pane();
        let target = drag.or_else(|| self.pane_at(grid_row, col).map(|(id, _)| id));
        let Some(target) = target else { return };
        let Some(rect) = self.pane_rect(target) else { return };
        let inside = rect.contains(grid_row, col);
        if !inside && drag.is_none() {
            return;
        }
        // A drag that leaves its pane keeps selecting up to the pane's edge.
        let pane_row = grid_row.saturating_sub(rect.y0).min(rect.rows.saturating_sub(1));
        let pane_col = col.saturating_sub(rect.x0).min(rect.cols.saturating_sub(1));
        if inside && event.phase == UiActionPhase::Press && event.button == 1 {
            // Clicking a pane gives it the keyboard and the cursor.
            self.focus_pane(target);
        }
        let shift = event.shift_down();
        let mouse = self.pane(target).map(|pane| (pane.session.screen.mouse, pane.session.screen.mouse_sgr));
        if inside
            && let Some((mode, sgr)) = mouse
            && mode != MouseMode::Off
            && !shift
        {
            let button = event.button.saturating_sub(1).min(2) as u8;
            if let Some(bytes) = mouse_report(sgr, button, event.phase, pane_col, pane_row, mode) {
                self.send_to(target, bytes);
            }
            return;
        }
        let Some(abs_row) = self.pane(target).and_then(|pane| absolute_row(pane, pane_row)) else { return };
        match event.phase {
            UiActionPhase::Press if event.button == 1 => {
                if let Some(pane) = self.pane_mut(target) {
                    pane.selection = None;
                    pane.drag_anchor = None;
                }
                match self.register_click(target, abs_row, pane_col) {
                    2 => self.select_word(target, abs_row, pane_col),
                    3 => self.select_line(target, abs_row),
                    _ => {
                        if let Some(pane) = self.pane_mut(target) {
                            pane.drag_anchor = Some((abs_row, pane_col));
                        }
                    }
                }
            }
            // The renderer only fires Press for button 1; buttons 2 and 3 are delivered on Release.
            UiActionPhase::Release if event.button == 2 => self.paste_primary(),
            UiActionPhase::Motion | UiActionPhase::Hover => {
                if let Some(pane) = self.pane_mut(target)
                    && let Some(anchor) = pane.drag_anchor
                {
                    pane.selection = Some(Span::normalised(anchor, (abs_row, pane_col)));
                }
            }
            UiActionPhase::Release => {
                let anchor = self.pane_mut(target).and_then(|pane| pane.drag_anchor.take());
                if let Some(anchor) = anchor {
                    if anchor == (abs_row, pane_col) {
                        if let Some(pane) = self.pane_mut(target) {
                            pane.selection = None;
                        }
                    } else {
                        self.select_span(target, Span::normalised(anchor, (abs_row, pane_col)));
                    }
                }
            }
            _ => {}
        }
    }

    /// Wheel over a pane: mouse report or arrow keys to that pane, else its scrollback moves. The pane under the
    /// pointer is used, so scrolling a pane never needs a click first.
    fn wheel(&mut self, up: bool, event: &UiActionEvent) {
        let pointer_col = (event.x / self.cell_w).floor().max(0.0) as usize;
        let pointer_row = ((event.y / self.cell_h).floor().max(1.0) as usize).saturating_sub(1).saturating_sub(self.banner_rows());
        let target = self
            .pane_at(pointer_row, pointer_col)
            .or_else(|| self.focused_id().and_then(|id| self.pane_rect(id).map(|rect| (id, rect))));
        let Some((id, rect)) = target else { return };
        let Some(pane) = self.pane(id) else { return };
        let (mode, sgr, alt) = (pane.session.screen.mouse, pane.session.screen.mouse_sgr, pane.session.screen.alt_active());
        let app_cursor = pane.session.screen.app_cursor;
        let max = pane.session.screen.scrollback_len();
        let (col, row) = (pointer_col.saturating_sub(rect.x0), pointer_row.saturating_sub(rect.y0));
        if mode != MouseMode::Off {
            let button = if up { 64 } else { 65 };
            let report = mouse_report_raw(sgr, button, true, col, row);
            self.send_to(id, report);
        } else if alt {
            let arrow = match (up, app_cursor) {
                (true, true) => "\x1bOA",
                (true, false) => "\x1b[A",
                (false, true) => "\x1bOB",
                (false, false) => "\x1b[B",
            };
            let bytes = arrow.repeat(SCROLL_LINES_PER_NOTCH).into_bytes();
            self.send_to(id, bytes);
        } else if let Some(pane) = self.pane_mut(id) {
            pane.view_offset = if up {
                (pane.view_offset + SCROLL_LINES_PER_NOTCH).min(max)
            } else {
                pane.view_offset.saturating_sub(SCROLL_LINES_PER_NOTCH)
            };
        }
    }

    fn tab_hit(&mut self, hit: Hit) {
        match hit {
            Hit::Tab(index) => {
                if index < self.tabs.len() {
                    self.active = index;
                }
            }
            Hit::Close(index) => self.close_tab(index),
            Hit::NewTab => {
                let _ = self.open_tab_here();
            }
            Hit::Settings => self.close_settings(),
        }
    }
}

/// Absolute line index for a pane-local grid row under that pane's scroll position.
fn absolute_row(pane: &Pane, grid_row: usize) -> Option<usize> {
    let screen = &pane.session.screen;
    let top = screen.total_lines().saturating_sub(screen.rows() + pane.view_offset.min(screen.scrollback_len()));
    Some(top + grid_row)
}

/// The user's home directory, when it exists.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|value| !value.is_empty()).map(PathBuf::from).filter(|dir| dir.is_dir())
}

/// SGR (1006) or X10 mouse report for a button event in grid coordinates (0-based).
pub fn mouse_report(sgr: bool, button: u8, phase: UiActionPhase, col: usize, row: usize, mode: MouseMode) -> Option<Vec<u8>> {
    let motion = matches!(phase, UiActionPhase::Motion | UiActionPhase::Hover);
    if motion && !matches!(mode, MouseMode::Drag | MouseMode::Any) {
        return None;
    }
    let (code, release) = match phase {
        UiActionPhase::Press => (u16::from(button), false),
        UiActionPhase::Release => (u16::from(button), true),
        _ if motion => (u16::from(button) + 32, false),
        _ => return None,
    };
    if sgr {
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes());
    }
    let code = if release { 3 } else { code };
    Some(mouse_report_raw_x10(code as u8, col, row))
}

fn mouse_report_raw(sgr: bool, code: u16, press: bool, col: usize, row: usize) -> Vec<u8> {
    if sgr {
        let end = if press { 'M' } else { 'm' };
        format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes()
    } else {
        mouse_report_raw_x10(code as u8, col, row)
    }
}

fn mouse_report_raw_x10(code: u8, col: usize, row: usize) -> Vec<u8> {
    let clamp = |value: usize| (value + 33).min(255) as u8;
    vec![0x1b, b'[', b'M', code.saturating_add(32), clamp(col), clamp(row)]
}

/// Click count for a press at (`row`, `col`) at `now`, given the previous press.
fn next_click_count(previous: Option<(Instant, usize, usize, u8)>, now: Instant, row: usize, col: usize) -> u8 {
    const MULTI_CLICK: Duration = Duration::from_millis(400);
    match previous {
        Some((at, prev_row, prev_col, count))
            if prev_row == row && prev_col == col && now.duration_since(at) <= MULTI_CLICK =>
        {
            count % 3 + 1
        }
        _ => 1,
    }
}

/// Inclusive bounds of the word under `col`: letters, digits and `_`. None for a blank or out-of-range cell.
fn menu_width() -> usize {
    MENU.iter().map(|item| item.chars().count()).max().unwrap_or(0) + 2
}

/// Replaces cells `[col, col + width)` of a row with `menu`, keeping the runs on either side.
fn overlay(row: Vec<Run>, col: usize, width: usize, menu: Run) -> Vec<Run> {
    let end = col + width;
    let (mut out, mut right) = (Vec::new(), Vec::new());
    for run in row {
        let run_end = run.col + run.cells;
        if run.col < col {
            out.push(clip(&run, 0, col.min(run_end) - run.col));
        }
        if run_end > end {
            right.push(clip(&run, end.max(run.col) - run.col, run.cells));
        }
    }
    out.push(menu);
    out.extend(right);
    out
}

/// The part of `run` between cells `from` and `to` (relative to the run). Text is cut by character, one per cell.
fn clip(run: &Run, from: usize, to: usize) -> Run {
    let text = run.text.chars().skip(from).take(to - from).collect();
    let shape = run.shape.map(|arm| Arm { x0: (arm.x0 - from as f32).max(0.0), x1: (arm.x1 - from as f32).min((to - from) as f32), ..arm });
    Run { col: run.col + from, cells: to - from, text, shape, ..run.clone() }
}

fn word_bounds(chars: &[char], col: usize) -> Option<(usize, usize)> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if !chars.get(col).copied().is_some_and(is_word) {
        return None;
    }
    let mut start = col;
    while start > 0 && is_word(chars[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < chars.len() && is_word(chars[end + 1]) {
        end += 1;
    }
    Some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::PtyProcess;
    use crate::themes::builtin_themes;

    /// A pane's shell: `cat` keeps the PTY open and quiet until it is killed.
    fn cat() -> Launch {
        Launch { program: "/bin/cat".into(), args: Vec::new(), cwd: None }
    }

    /// A key press event with no modifiers.
    fn key(name: &str) -> UiKeyEvent {
        UiKeyEvent { phase: UiKeyPhase::Press, modifiers: Vec::new(), key: name.to_owned(), text: None }
    }

    /// An App without a window: no clipboard, an empty history and a theme directory that does not exist.
    fn test_app() -> App {
        let (events, _receiver) = std::sync::mpsc::channel();
        let (_peer, wake) = UnixStream::pair().expect("socket pair");
        let mut app = App {
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            cols: 80,
            grid_rows: 23,
            cell_w: 8.0,
            cell_h: 16.0,
            config: TerminalConfig::default(),
            config_path: PathBuf::from("/nonexistent/flamewm-terminal/terminal.toml"),
            store: ThemeStore::new("/nonexistent/flamewm-terminal/themes"),
            themes: builtin_themes(),
            snapshot: None,
            palette: builtin_themes()[0].palette.clone(),
            surface_alpha: 255,
            view: View::Terminal,
            clipboard: None,
            events,
            wake,
            first_launch: None,
            packaged_root: PathBuf::from("/"),
            ui: crate::ui::UiCache::default(),
            menu: None,
            rename: None,
            history: History::open(None, &[]),
            banner: false,
            window: (640, 384),
        };
        app.refresh_metrics();
        app.set_window(640, 384);
        app
    }

    /// Every line of a pane's screen, for assertions about which pane received output.
    fn pane_text(app: &App, id: u64) -> String {
        let pane = app.tabs.iter().find_map(|tab| tab.root.pane(id)).expect("pane");
        let screen = &pane.session.screen;
        let top = screen.total_lines().saturating_sub(screen.rows());
        (0..screen.rows())
            .filter_map(|row| screen.line_abs(top + row))
            .map(|line| line.iter().map(|cell| cell.ch).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        a.x0 < b.x0 + b.cols && b.x0 < a.x0 + a.cols && a.y0 < b.y0 + b.rows && b.y0 < a.y0 + a.rows
    }

    /// Opens one tab and splits it once, returning (first leaf, second leaf).
    fn split_tab(app: &mut App, axis: Axis) -> (u64, u64) {
        app.open_tab(cat()).expect("open tab");
        let first = app.tabs[0].focused;
        app.split_focused(axis).expect("split");
        let second = app.tabs[0].focused;
        (first, second)
    }

    /// Grid size a pane was last resized to.
    fn pane_size(app: &App, id: u64) -> (usize, usize) {
        app.pane(id).map(|pane| pane.size).expect("pane")
    }

    #[test]
    fn split_rect_gives_both_axes_a_divider() {
        let grid = Rect { x0: 0, y0: 0, cols: 80, rows: 23 };
        let (left, right) = split_rect(grid, Axis::Columns, SPLIT_RATIO);
        assert_eq!(left, Rect { x0: 0, y0: 0, cols: 40, rows: 23 });
        assert_eq!(right, Rect { x0: 41, y0: 0, cols: 39, rows: 23 }, "one divider column between them");
        let (top, bottom) = split_rect(grid, Axis::Rows, SPLIT_RATIO);
        assert_eq!(top, Rect { x0: 0, y0: 0, cols: 80, rows: 11 });
        assert_eq!(bottom, Rect { x0: 0, y0: 12, cols: 80, rows: 11 }, "one divider row between them");
        // The divider never falls outside the rectangle.
        for rect in [left, right, top, bottom] {
            assert!(rect.x0 + rect.cols <= grid.cols && rect.y0 + rect.rows <= grid.rows);
        }
    }

    #[test]
    fn nested_split_rects_stay_inside_their_parent() {
        let grid = Rect { x0: 0, y0: 0, cols: 80, rows: 23 };
        let (left, right) = split_rect(grid, Axis::Columns, SPLIT_RATIO);
        let (top_left, bottom_left) = split_rect(left, Axis::Rows, SPLIT_RATIO);
        assert_eq!(top_left, Rect { x0: 0, y0: 0, cols: 40, rows: 11 });
        assert_eq!(bottom_left, Rect { x0: 0, y0: 12, cols: 40, rows: 11 });
        let all = [top_left, bottom_left, right];
        for (index, a) in all.iter().enumerate() {
            for b in all.iter().skip(index + 1) {
                assert!(!overlap(*a, *b), "{a:?} overlaps {b:?}");
            }
            assert!(a.x0 + a.cols <= grid.cols && a.y0 + a.rows <= grid.rows);
        }
    }

    #[test]
    fn a_split_can_be_refused_when_the_halves_would_be_too_small() {
        let narrow = Rect { x0: 0, y0: 0, cols: 4, rows: 23 };
        assert!(!split_fits(narrow, Axis::Columns), "2+1+2 leaves two 2-column panes; 3 does not");
        assert!(split_fits(Rect { cols: 5, ..narrow }, Axis::Columns));
        let short = Rect { x0: 0, y0: 0, cols: 80, rows: 1 };
        assert!(!split_fits(short, Axis::Rows), "a one-row grid has no room for two panes");
        assert!(split_fits(Rect { rows: 3, ..short }, Axis::Rows));
    }

    #[test]
    fn splitting_the_focused_pane_gives_it_the_focus_and_a_new_session() {
        let mut app = test_app();
        let (first, second) = split_tab(&mut app, Axis::Columns);
        assert_ne!(first, second, "each leaf has its own session id");
        assert_eq!(app.tabs[0].focused, second);
        assert_eq!(app.tabs[0].root.rects(app.grid_rect()).len(), 2);
        assert!(app.pane(first).is_some() && app.pane(second).is_some());
        // The tab keeps the session id of its first leaf.
        assert_eq!(app.tabs[0].id, first);
    }

    #[test]
    fn nested_splits_tile_the_grid_without_overlapping() {
        let mut app = test_app();
        app.open_tab(cat()).expect("tab");
        app.split_focused(Axis::Columns).expect("split columns");
        app.split_focused(Axis::Rows).expect("split rows");
        let rects: Vec<Rect> = app.tabs[0].root.rects(app.grid_rect()).into_iter().map(|(_, rect)| rect).collect();
        assert_eq!(rects.len(), 3);
        for (index, a) in rects.iter().enumerate() {
            for b in rects.iter().skip(index + 1) {
                assert!(!overlap(*a, *b), "{a:?} overlaps {b:?}");
            }
            assert!(a.x0 + a.cols <= app.cols && a.y0 + a.rows <= app.grid_rows, "{a:?} leaves the grid");
        }
        // Panes plus the one-column and one-row dividers fill the grid exactly.
        let panes: usize = rects.iter().map(|rect| rect.cols * rect.rows).sum();
        assert_eq!(panes + app.grid_rows * DIVIDER + rects[1].cols * DIVIDER, app.cols * app.grid_rows);
    }

    #[test]
    fn an_exit_collapses_the_pane_onto_the_survivor() {
        let mut app = test_app();
        let (first, second) = split_tab(&mut app, Axis::Columns);
        app.session_event(SessionEvent::Output { id: second, bytes: b"bye".to_vec() });
        assert!(!app.session_event(SessionEvent::Exited { id: second }), "the tab survives");
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].root.rects(app.grid_rect()).len(), 1, "the parent split is gone");
        assert_eq!(app.tabs[0].focused, first, "focus moves to the survivor");
        let rect = app.tabs[0].root.rects(app.grid_rect())[0].1;
        assert_eq!(rect, app.grid_rect(), "the survivor takes the whole grid back");
        assert_eq!(app.tabs[0].root.leaf().map(|pane| pane.size), Some((app.cols, app.grid_rows)));
    }

    #[test]
    fn the_last_leaf_takes_the_tab_with_it() {
        let mut app = test_app();
        let (first, second) = split_tab(&mut app, Axis::Rows);
        app.session_event(SessionEvent::Exited { id: second });
        assert_eq!(app.tabs.len(), 1);
        assert!(app.session_event(SessionEvent::Exited { id: first }), "the last tab closing ends the window");
        assert!(app.tabs.is_empty());
    }

    #[test]
    fn output_and_exit_route_to_the_leaf_not_the_tab() {
        let mut app = test_app();
        let (first, second) = split_tab(&mut app, Axis::Columns);
        app.session_event(SessionEvent::Output { id: first, bytes: b"alpha".to_vec() });
        assert!(pane_text(&app, first).contains("alpha"));
        assert!(!pane_text(&app, second).contains("alpha"), "the other pane must not see it");
        app.session_event(SessionEvent::Output { id: second, bytes: b"beta".to_vec() });
        assert!(pane_text(&app, second).contains("beta"));
        assert!(!pane_text(&app, first).contains("beta"));
        // An unfocused leaf exiting does not touch the focused one.
        app.session_event(SessionEvent::Exited { id: first });
        assert_eq!(app.tabs[0].focused, second);
        assert!(pane_text(&app, second).contains("beta"));
    }

    #[test]
    fn each_pane_keeps_its_own_grid_and_pty_size() {
        let mut app = test_app();
        let (first, second) = split_tab(&mut app, Axis::Columns);
        let (left, right) = (pane_size(&app, first), pane_size(&app, second));
        assert_ne!(left, right, "side-by-side panes have different column counts");
        assert_eq!(left.0 + right.0 + DIVIDER, app.cols);
        assert_eq!(left.1, app.grid_rows);
        assert_eq!(right.1, app.grid_rows);
        assert_ne!(app.pty_size(left.0, left.1), app.pty_size(right.0, right.1), "each PTY is told its own size");
        assert_eq!((app.pty_size(left.0, left.1).cols as usize, app.pty_size(left.0, left.1).rows as usize), left);
        assert_eq!((app.pty_size(right.0, right.1).cols as usize, app.pty_size(right.0, right.1).rows as usize), right);
        // A window resize reaches both panes, still with different sizes.
        app.set_window(1000, 600);
        assert_ne!(pane_size(&app, first), left);
        assert_eq!(pane_size(&app, first).0 + pane_size(&app, second).0 + DIVIDER, app.cols);
        // A horizontal split turns the sizes into rows instead of columns.
        let mut stacked = test_app();
        let (top, bottom) = split_tab(&mut stacked, Axis::Rows);
        let (top, bottom) = (pane_size(&stacked, top), pane_size(&stacked, bottom));
        assert_eq!(top.0, stacked.cols);
        assert_eq!(bottom.0, stacked.cols);
        assert!(top.1 < stacked.grid_rows && bottom.1 < stacked.grid_rows, "each pane is shorter than the grid");
        assert_eq!(top.1 + bottom.1 + DIVIDER, stacked.grid_rows);
    }

    #[test]
    fn distinct_pane_sizes_reach_two_real_ptys() {
        let grid = Rect { x0: 0, y0: 0, cols: 80, rows: 23 };
        let (first, second) = split_rect(grid, Axis::Columns, SPLIT_RATIO);
        let size = |rect: Rect| PtySize { cols: rect.cols as u16, rows: rect.rows as u16, pixel_width: 0, pixel_height: 0 };
        let left = PtyProcess::spawn(&cat(), size(first)).expect("left pty");
        let right = PtyProcess::spawn(&cat(), size(second)).expect("right pty");
        assert_ne!(left.winsize().expect("left size"), right.winsize().expect("right size"));
        assert_eq!(left.winsize().expect("left size").cols as usize, first.cols);
        assert_eq!(right.winsize().expect("right size").cols as usize, second.cols);
        // The same PTY follows a new layout, which is what splitting and collapsing do.
        let (top, _bottom) = split_rect(grid, Axis::Rows, SPLIT_RATIO);
        left.resize(size(top)).expect("resize");
        let now = left.winsize().expect("resized size");
        assert_eq!((now.cols as usize, now.rows as usize), (top.cols, top.rows));
    }

    #[test]
    fn function_keys_change_tabs_and_never_go_to_the_child() {
        let mut app = test_app();
        app.open_tab(cat()).expect("first");
        app.open_tab(cat()).expect("second");
        assert_eq!(app.active, 1);
        app.key(&key("f4"));
        assert_eq!(app.active, 0, "F4 wraps to the next tab");
        app.key(&key("f3"));
        assert_eq!(app.active, 1, "F3 wraps back to the previous tab");
        // F1 is not a shortcut: it is encoded for the child and changes nothing here.
        app.key(&key("f1"));
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active, 1);
        app.key(&key("f2"));
        assert_eq!(app.tabs.len(), 3, "F2 opens a tab with a shell of its own");
        assert_eq!(app.active, 2);
        let ids: Vec<u64> = app.tabs.iter().map(|tab| tab.id).collect();
        assert_eq!(ids.len(), 3);
        assert!(ids.iter().all(|id| app.pane(*id).is_some()), "every tab has its own session");
        assert_eq!(ids.len(), ids.iter().collect::<std::collections::HashSet<_>>().len(), "session ids are unique");
    }

    #[test]
    fn a_split_frame_keeps_each_pane_in_its_own_columns_with_one_cursor() {
        let mut app = test_app();
        let (first, focused) = split_tab(&mut app, Axis::Columns);
        app.session_event(SessionEvent::Output { id: first, bytes: b"left".to_vec() });
        app.session_event(SessionEvent::Output { id: focused, bytes: b"right".to_vec() });
        let rects = app.tabs[0].root.rects(app.grid_rect());
        let left = rects.iter().find(|(id, _)| *id == first).expect("left pane").1;
        let right = rects.iter().find(|(id, _)| *id == focused).expect("right pane").1;
        let frame = app.frame();
        assert_eq!(frame.len(), app.grid_rows + 1, "tab bar plus the whole grid");
        let row = 1 + left.y0;
        let (mut saw_left, mut saw_right) = (false, false);
        for run in &frame[row] {
            let end = run.col + run.cells;
            if run.col >= left.x0 && end <= left.x0 + left.cols {
                saw_left = true;
            } else if run.col >= right.x0 && end <= right.x0 + right.cols {
                saw_right = true;
            } else {
                panic!("run crosses a pane edge: {run:?}");
            }
        }
        assert!(saw_left && saw_right, "both panes draw on the shared row");
        assert!(frame[row].iter().any(|run| run.text.starts_with("left")));
        assert!(frame[row].iter().any(|run| run.text.starts_with("right")));
        let cursors: Vec<usize> = frame[row].iter().filter(|run| run.bg == Some(app.palette.cursor)).map(|run| run.col).collect();
        assert_eq!(cursors.len(), 1, "only the focused pane draws a cursor");
        assert!(cursors[0] >= right.x0, "the cursor is in the focused pane");
        // A stacked split puts the panes on different rows instead of columns.
        let mut stacked = test_app();
        let (top, bottom) = split_tab(&mut stacked, Axis::Rows);
        stacked.session_event(SessionEvent::Output { id: top, bytes: b"top".to_vec() });
        stacked.session_event(SessionEvent::Output { id: bottom, bytes: b"bottom".to_vec() });
        let frame = stacked.frame();
        let rows: Vec<Rect> = stacked.tabs[0].root.rects(stacked.grid_rect()).into_iter().map(|(_, rect)| rect).collect();
        let (top_rect, bottom_rect) = (rows[0], rows[1]);
        assert!(frame[1 + top_rect.y0].iter().any(|run| run.text.starts_with("top")));
        assert!(frame[1 + bottom_rect.y0].iter().any(|run| run.text.starts_with("bottom")));
        assert!(!frame[1 + top_rect.y0].iter().any(|run| run.text.starts_with("bottom")));
    }

    #[test]
    fn a_visible_suggestion_still_accepts_tab_and_stays_inside_its_pane() {
        let store = std::env::temp_dir().join(format!("flamewm-terminal-ghost-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&store);
        let mut history = History::open(Some(store.clone()), &[]);
        history.record("ls -la").expect("record a command");
        let mut app = test_app();
        app.history = history;
        let (_first, focused) = split_tab(&mut app, Axis::Columns);
        let rect = app.pane_rect(focused).expect("focused rect");
        if let Some(pane) = app.focused_pane_mut() {
            pane.input.paste("ls");
            pane.prompt = Some(true);
        }
        assert_eq!(app.ghost(), Some(" -la"));
        // The ghost is drawn inside the focused pane, never across the divider.
        let frame = app.frame();
        let row = 1 + rect.y0;
        let ghost = frame[row].iter().find(|run| run.text == " -la").expect("ghost run");
        assert!(ghost.col >= rect.x0 && ghost.col + ghost.cells <= rect.x0 + rect.cols);
        // Tab still accepts the suggestion, exactly as it does with a single pane.
        app.key(&key("tab"));
        assert_eq!(app.focused_pane().map(|pane| pane.input.text()), Some("ls -la".to_string()));
        assert_ne!(app.tabs[0].focused, app.tabs[0].id, "the suggestion went to the focused pane");
        let _ = std::fs::remove_file(&store);
    }

    #[test]
    fn closing_a_tab_reaps_every_leaf() {
        let mut app = test_app();
        app.open_tab(cat()).expect("tab");
        app.split_focused(Axis::Columns).expect("split columns");
        app.split_focused(Axis::Rows).expect("split rows");
        let pids: Vec<u32> = app.tabs[0].root.leaves().iter().map(|pane| pane.session.pty.id()).collect();
        assert_eq!(pids.len(), 3);
        app.close_tab(0);
        assert!(app.tabs.is_empty());
        for pid in pids {
            assert!(!PathBuf::from(format!("/proc/{pid}")).exists(), "child {pid} was not reaped");
        }
    }

    #[test]
    fn closing_another_tab_keeps_the_active_one() {
        let mut app = test_app();
        app.open_tab(cat()).expect("first");
        app.open_tab(cat()).expect("second");
        app.open_tab(cat()).expect("third");
        let active = app.tabs[app.active].id;
        app.close_tab(0);
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.tabs[app.active].id, active, "the active tab does not shift");
    }

    #[test]
    fn closing_a_pane_leaves_the_other_pane_and_its_tab() {
        let mut app = test_app();
        let (first, second) = split_tab(&mut app, Axis::Columns);
        app.close_pane();
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.tabs[0].focused, first);
        assert!(app.pane(second).is_none());
        // The last pane closes the tab.
        app.close_pane();
        assert!(app.tabs.is_empty());
        assert!(app.pane(first).is_none());
    }

    #[test]
    fn banner_and_menu_geometry_stay_in_window_coordinates() {
        let mut app = test_app();
        app.open_tab(cat()).expect("tab");
        // The banner only shows for a single-leaf tab.
        assert!(app.banner_rows() > 0);
        app.menu = Some((1, 2));
        let frame = app.frame();
        let item = frame[1].iter().find(|run| run.text.starts_with(" New Tab")).expect("menu item");
        assert_eq!(item.col, 2, "the menu is drawn where the click was");
        app.split_focused(Axis::Columns).expect("split");
        assert_eq!(app.banner_rows(), 0, "a split tab never shows the start banner");
        assert_eq!(app.frame().len(), app.grid_rows + 1);
    }

    #[test]
    fn banner_art_spells_flamewm_with_every_glyph() {
        let art = &BANNER[..BANNER_ART_ROWS];
        let width = art[0].chars().count();
        assert!(art.iter().all(|row| row.chars().count() == width), "art rows must share one width");
        // Seven letters, six distinct glyphs; the E glyph's middle row is the one that is easiest to lose.
        assert!(art[1].contains("| ____|"), "the E glyph is missing from the banner");
        // Slanted: each row down starts one column further left than the row above.
        let indent = |row: &str| row.chars().take_while(|c| *c == ' ').count();
        assert!(art.windows(2).all(|pair| indent(pair[0]) > indent(pair[1])), "the art must lean right");
        assert_eq!(art.iter().filter(|row| row.contains('_')).count(), BANNER_ART_ROWS, "every art row carries glyph strokes");
    }

    #[test]
    fn overlay_replaces_cells_and_keeps_the_runs_on_either_side() {
        let row = vec![text_run(0, "abcdef", 1, None, false)];
        let menu = text_run(2, "XY", 2, Some(3), false);
        let out = overlay(row, 2, 2, menu);
        let texts: Vec<(usize, usize, &str)> = out.iter().map(|run| (run.col, run.cells, run.text.as_str())).collect();
        assert_eq!(texts, vec![(0, 2, "ab"), (2, 2, "XY"), (4, 2, "ef")]);
    }

    #[test]
    fn repeated_presses_on_one_cell_cycle_single_double_triple() {
        let t0 = Instant::now();
        let first = next_click_count(None, t0, 3, 4);
        assert_eq!(first, 1);
        let second = next_click_count(Some((t0, 3, 4, first)), t0 + Duration::from_millis(100), 3, 4);
        assert_eq!(second, 2);
        let third = next_click_count(Some((t0, 3, 4, second)), t0 + Duration::from_millis(200), 3, 4);
        assert_eq!(third, 3);
        let fourth = next_click_count(Some((t0, 3, 4, third)), t0 + Duration::from_millis(300), 3, 4);
        assert_eq!(fourth, 1);
    }

    #[test]
    fn slow_or_moved_presses_start_over() {
        let t0 = Instant::now();
        assert_eq!(next_click_count(Some((t0, 3, 4, 1)), t0 + Duration::from_millis(900), 3, 4), 1);
        assert_eq!(next_click_count(Some((t0, 3, 4, 1)), t0 + Duration::from_millis(50), 3, 5), 1);
        assert_eq!(next_click_count(Some((t0, 3, 4, 1)), t0 + Duration::from_millis(50), 2, 4), 1);
    }

    #[test]
    fn word_bounds_cover_the_word_under_the_column_only() {
        let chars: Vec<char> = "ls -la foo_bar.rs".chars().collect();
        assert_eq!(word_bounds(&chars, 0), Some((0, 1)));
        assert_eq!(word_bounds(&chars, 4), Some((4, 5)));
        assert_eq!(word_bounds(&chars, 9), Some((7, 13)));
        assert_eq!(word_bounds(&chars, 14), None);
        assert_eq!(word_bounds(&chars, 15), Some((15, 16)));
        assert_eq!(word_bounds(&chars, 2), None);
        assert_eq!(word_bounds(&chars, 99), None);
    }

    #[test]
    fn sgr_mouse_press_and_release_are_one_based() {
        let press = mouse_report(true, 0, UiActionPhase::Press, 4, 2, MouseMode::Press).unwrap();
        assert_eq!(press, b"\x1b[<0;5;3M".to_vec());
        let release = mouse_report(true, 0, UiActionPhase::Release, 4, 2, MouseMode::Press).unwrap();
        assert_eq!(release, b"\x1b[<0;5;3m".to_vec());
    }

    #[test]
    fn motion_is_reported_only_in_drag_or_any_modes() {
        assert!(mouse_report(true, 0, UiActionPhase::Motion, 1, 1, MouseMode::Press).is_none());
        assert_eq!(mouse_report(true, 0, UiActionPhase::Motion, 1, 1, MouseMode::Drag).unwrap(), b"\x1b[<32;2;2M".to_vec());
    }

    #[test]
    fn x10_reports_release_as_button_three_and_offsets_coordinates() {
        let release = mouse_report(false, 0, UiActionPhase::Release, 0, 0, MouseMode::Press).unwrap();
        assert_eq!(release, vec![0x1b, b'[', b'M', 35, 33, 33]);
    }

    #[test]
    fn wheel_reports_use_buttons_64_and_65() {
        assert_eq!(mouse_report_raw(true, 64, true, 0, 0), b"\x1b[<64;1;1M".to_vec());
        assert_eq!(mouse_report_raw(true, 65, true, 0, 0), b"\x1b[<65;1;1M".to_vec());
    }
}

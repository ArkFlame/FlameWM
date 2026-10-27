//! Terminal application state: tabs, the active view (terminal or settings), selection, scrollback position, mouse
//! reporting and the commands that persist theme and config changes. Rendering lives in `ui` and `view`.

mod grid;
mod keyboard;
mod layout;
mod overlay;
mod pointer;
mod preferences;
mod selection;
mod tabs;
#[cfg(test)]
mod tests;

pub use layout::{Axis, PaneNode, Rect};
pub use pointer::mouse_report;
pub use tabs::{Hit, Rename};

use self::grid::GridRow;
use crate::pty::Launch;
use crate::pty::PtySize;
use crate::session::Session;
use crate::session::SessionEvent;
use crate::settings::SettingsView;
use crate::store::TerminalConfig;
use crate::store::ThemeStore;
use crate::suggest::History;
use crate::suggest::LineInput;
use crate::theme;
use crate::themes::Palette;
use crate::themes::Theme;
use crate::view::RunCache;
use crate::view::Span;
use flamewm_api::settings::SettingsSnapshot;
use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Instant;

/// Cell pitch as a fraction of the font size for the bundled monospace face (advance and line height).
const CELL_WIDTH_RATIO: f32 = 0.6;

const CELL_HEIGHT_RATIO: f32 = 1.25;

/// Rows the UI pool can show: one tab bar plus grid rows.
pub const MAX_GRID_ROWS: usize = crate::pool::POOL_ROWS - 1;

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

pub struct Tab {
    /// Session id of the tab's first leaf, which names the tab.
    pub id: u64,
    /// Name set with Rename Tab; wins over the title the shell sets.
    pub title: Option<String>,
    pub root: PaneNode,
    /// Session id of the leaf that has the keyboard and the cursor.
    pub focused: u64,
}

#[derive(Debug)]
pub enum View {
    Terminal,
    Settings(SettingsView),
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
    pub surface_stroke: flamewm_ui_x11::UiColor,
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
    /// Grid runs of each pane, rebuilt only for the rows that changed. Panes that closed drop out each frame.
    pane_runs: HashMap<u64, RunCache>,
    /// Grid rows as last composed from the pane rows, reused while those pane rows are unchanged.
    grid_composed: Vec<GridRow>,
    /// Startup banner shown above the first tab until its output scrolls.
    banner: bool,
    /// Window size in logical px, as last reported by the window. Grid and PTY size follow it.
    window: (u32, u32),
}

impl App {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
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
            surface_stroke: flamewm_ui_x11::UiColor {
                r: 255,
                g: 255,
                b: 255,
                a: 63,
            },
            view: View::Terminal,
            clipboard: None,
            events,
            wake,
            first_launch,
            packaged_root,
            ui: crate::ui::UiCache::default(),
            pane_runs: HashMap::new(),
            grid_composed: Vec::new(),
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
        self.palette =
            theme::resolve_palette(&self.config.theme, &self.themes, self.snapshot.as_ref());
        self.surface_alpha = theme::surface_alpha(self.config.background_opacity);
        if let Some(snapshot) = self.snapshot.as_ref() {
            self.surface_stroke = flamewm_ui_x11::surface_style::surface_border(snapshot);
        }
    }

    /// Grid size for a window in logical px. Row 0 is the tab bar.
    pub fn grid_for(&self, width: u32, height: u32) -> (usize, usize) {
        let cols = ((width as f32 / self.cell_w).floor() as usize).clamp(10, 400);
        let rows = ((height as f32 / self.cell_h).floor() as usize)
            .saturating_sub(1)
            .clamp(4, MAX_GRID_ROWS);
        (cols, rows)
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
    fn start_pane(
        &mut self,
        launch: &Launch,
        cols: usize,
        rows: usize,
    ) -> Result<Box<Pane>, String> {
        let id = self.next_id;
        self.next_id += 1;
        let wake = self.wake.try_clone().map_err(|error| error.to_string())?;
        let session = Session::start(
            id,
            launch,
            self.pty_size(cols.max(1), rows.max(1)),
            self.config.scrollback,
            self.events.clone(),
            wake,
        )?;
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
                // Dropping the removed pane reaps the child that ended the reader thread.
                self.remove_pane(id);
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
}

/// The user's home directory, when it exists.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
}

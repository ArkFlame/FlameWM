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
use crate::glyphs::Arm;
use crate::view::{GridOptions, Run, Span, grid_runs, text_run};
use crate::screen::CursorShape;

/// Cell pitch as a fraction of the font size for the bundled monospace face (advance and line height).
const CELL_WIDTH_RATIO: f32 = 0.6;
const CELL_HEIGHT_RATIO: f32 = 1.25;
const SCROLL_LINES_PER_NOTCH: usize = 3;
/// Rows the UI pool can show: one tab bar plus grid rows.
pub const MAX_GRID_ROWS: usize = crate::pool::POOL_ROWS - 1;

pub struct Tab {
    pub id: u64,
    pub session: Session,
    /// Name set with Rename Tab; wins over the title the shell sets.
    pub title: Option<String>,
}

/// Inline tab rename in progress: the tab index and the text typed so far.
pub struct Rename {
    pub tab: usize,
    pub text: String,
}

/// Startup banner: FlameWM art, then the `/theme` hint. Drawn above the shell until its output scrolls.
const BANNER: [&str; 7] = [
    r" _____ _                 __        ____  __ ",
    r"|  ___| | __ _ _ __ ___  \ \      / /  \/  |",
    r"| |_  | |/ _` | '_ ` _ \  \ \ /\ / /| |\/| |",
    r"|  _| | | (_| | | | | | |  \ V  V / | |  | |",
    r"|_|   |_|\__,_|_| |_| |_|   \_/\_/  |_|  |_|",
    "",
    "/theme - Change the Terminal theme.",
];
const BANNER_ART_ROWS: usize = 5;

/// Right-click menu items, in display order. Indexes are matched in `menu_action`.
const MENU: [&str; 5] = ["New Tab", "Close Tab", "Rename Tab", "Copy", "Paste"];

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
    pub selection: Option<Span>,
    drag_anchor: Option<(usize, usize)>,
    /// Last primary press: time, absolute row, column and click count (1 to 3).
    last_click: Option<(Instant, usize, usize, u8)>,
    pub view_offset: usize,
    /// System CLIPBOARD handle. `None` when no X connection is available; copy and paste then stay in-process.
    pub clipboard: Option<flamewm_clipboard::Clipboard>,
    /// Text of the last mouse selection. Mouse selection never writes the system clipboard; Ctrl+Shift+C does.
    pub selection_text: String,
    pub events: Sender<SessionEvent>,
    pub wake: UnixStream,
    pub first_launch: Option<Launch>,
    pub packaged_root: PathBuf,
    /// Last projected rows and font, so only changed rows reach the UI.
    pub ui: crate::ui::UiCache,
    /// Open context menu: window row and column of its top-left cell.
    pub menu: Option<(usize, usize)>,
    pub rename: Option<Rename>,
    /// Startup banner shown above the first tab until its output scrolls.
    banner: bool,
    /// Text typed on the shell line, from the keys sent to the PTY. Enter on `/theme` opens the theme menu instead.
    input_line: String,
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
            selection: None,
            drag_anchor: None,
            last_click: None,
            view_offset: 0,
            clipboard: None,
            selection_text: String::new(),
            events,
            wake,
            first_launch,
            packaged_root,
            ui: crate::ui::UiCache::default(),
            menu: None,
            rename: None,
            banner: false,
            input_line: String::new(),
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

    pub fn set_window(&mut self, width: u32, height: u32) {
        self.window = (width, height);
        let (cols, rows) = self.grid_for(width, height);
        if cols == self.cols && rows == self.grid_rows && !self.tabs.is_empty() {
            return;
        }
        self.cols = cols;
        self.grid_rows = rows;
        let pixel = ((self.cols as f32 * self.cell_w) as u16, (self.grid_rows as f32 * self.cell_h) as u16);
        for tab in &mut self.tabs {
            tab.session.resize(self.cols, self.grid_rows, pixel);
        }
        self.selection = None;
    }

    fn pty_size(&self) -> PtySize {
        PtySize {
            cols: self.cols as u16,
            rows: self.grid_rows as u16,
            pixel_width: (self.cols as f32 * self.cell_w) as u16,
            pixel_height: (self.grid_rows as f32 * self.cell_h) as u16,
        }
    }

    /// Starts a tab. `launch` defaults to the user's shell in `cwd`.
    pub fn open_tab(&mut self, launch: Launch) -> Result<(), String> {
        let id = self.next_id;
        self.next_id += 1;
        let wake = self.wake.try_clone().map_err(|error| error.to_string())?;
        let session = Session::start(id, &launch, self.pty_size(), self.config.scrollback, self.events.clone(), wake)?;
        // The banner belongs to the first tab only.
        self.banner = self.tabs.is_empty();
        self.tabs.push(Tab { id, session, title: None });
        self.active = self.tabs.len() - 1;
        self.view_offset = 0;
        self.selection = None;
        Ok(())
    }

    /// Opens a tab in the active tab's working directory (the shell when the directory is unknown).
    pub fn open_tab_here(&mut self) -> Result<(), String> {
        let cwd = self.tabs.get(self.active).and_then(|tab| tab.session.working_directory());
        self.open_tab(Launch::shell(cwd))
    }

    pub fn close_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            let mut tab = self.tabs.remove(index);
            tab.session.pty.kill();
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len().saturating_sub(1);
            }
            self.selection = None;
            self.view_offset = 0;
        }
    }

    /// Applies output or exit from a reader thread. Returns true when no tabs remain and the window should close.
    pub fn session_event(&mut self, event: SessionEvent) -> bool {
        match event {
            SessionEvent::Output { id, bytes } => {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.session.feed(&bytes);
                }
                self.update_banner();
            }
            SessionEvent::Exited { id } => {
                if let Some(index) = self.tabs.iter().position(|tab| tab.id == id) {
                    self.close_tab(index);
                }
                if self.tabs.is_empty() {
                    return true;
                }
            }
        }
        false
    }

    fn send_to_active(&mut self, bytes: Vec<u8>) {
        if let Some(session) = self.tabs.get(self.active).map(|tab| &tab.session) {
            session.write(bytes);
        }
    }

    pub fn copy_selection(&mut self) {
        let Some(span) = self.selection else { return };
        let Some(tab) = self.tabs.get(self.active) else { return };
        let screen = &tab.session.screen;
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
        self.selection_text = out.join("\n");
    }

    /// Makes `span` the selection, copies its text, and publishes it on PRIMARY for middle-click.
    fn select_span(&mut self, span: Span) {
        self.selection = Some(span);
        self.copy_selection();
        if self.selection_text.is_empty() {
            return;
        }
        if let Some(clipboard) = &self.clipboard
            && let Err(error) = clipboard.set_primary_text(&self.selection_text)
        {
            eprintln!("flamewm-terminal: primary selection: {error}");
        }
    }

    /// Counts presses on the same cell within 400 ms: 1, 2 (word), 3 (line), then back to 1.
    fn register_click(&mut self, abs_row: usize, col: usize) -> u8 {
        let now = Instant::now();
        let count = next_click_count(self.last_click, now, abs_row, col);
        self.last_click = Some((now, abs_row, col, count));
        count
    }

    fn select_word(&mut self, abs_row: usize, col: usize) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let Some(line) = tab.session.screen.line_abs(abs_row) else { return };
        let chars: Vec<char> = line.iter().map(|cell| cell.ch).collect();
        if let Some((start, end)) = word_bounds(&chars, col) {
            self.select_span(Span::normalised((abs_row, start), (abs_row, end)));
        }
    }

    fn select_line(&mut self, abs_row: usize) {
        let last = self.cols.saturating_sub(1);
        self.select_span(Span::normalised((abs_row, 0), (abs_row, last)));
    }

    /// Ctrl+Shift+C: the selection goes to the system CLIPBOARD. Without an X connection it stays in-process.
    pub fn copy_to_clipboard(&mut self) {
        self.copy_selection();
        if self.selection_text.is_empty() {
            return;
        }
        if let Some(clipboard) = &self.clipboard
            && let Err(error) = clipboard.set_text(&self.selection_text)
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
        let text = system.unwrap_or_else(|| self.selection_text.clone());
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
        let text = system.unwrap_or_else(|| self.selection_text.clone());
        self.send_paste(text);
    }

    fn send_paste(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        let bracketed = self.tabs.get(self.active).is_some_and(|tab| tab.session.screen.bracketed_paste);
        let mut bytes = Vec::new();
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.replace('\n', "\r").as_bytes());
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        self.view_offset = 0;
        self.send_to_active(bytes);
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
        self.tabs.get(self.active).is_some_and(|tab| tab.session.screen.app_cursor)
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
            self.view_offset = 0;
            self.selection = None;
            if self.theme_command_submitted(event) {
                // Ctrl+U clears the typed `/theme` from the shell line, so the shell never runs it.
                self.send_to_active(vec![0x15]);
                self.open_settings();
                return;
            }
            self.track_input_line(event);
            self.send_to_active(bytes);
        }
    }

    /// Enter on a typed `/theme` line, outside full-screen programs.
    fn theme_command_submitted(&self, event: &UiKeyEvent) -> bool {
        let enter = matches!(event.key.to_ascii_lowercase().as_str(), "return" | "enter" | "kp_enter");
        enter && self.input_line == "/theme" && self.tabs.get(self.active).is_some_and(|tab| !tab.session.screen.alt_active())
    }

    /// Follows the text sent to the shell. Editing keys the line cannot follow (arrows, Tab, Ctrl) clear it.
    fn track_input_line(&mut self, event: &UiKeyEvent) {
        let mods = Mods::from_names(event.modifiers.iter());
        match event.key.to_ascii_lowercase().as_str() {
            "backspace" => {
                self.input_line.pop();
            }
            "return" | "enter" | "kp_enter" | "escape" => self.input_line.clear(),
            _ => match event.text.as_deref() {
                Some(text) if !mods.ctrl && !mods.alt && !text.chars().any(char::is_control) => self.input_line.push_str(text),
                _ => self.input_line.clear(),
            },
        }
    }

    /// The banner goes once the shell output scrolls or reaches the rows the banner covers.
    fn update_banner(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let (cursor_row, _) = tab.session.screen.cursor();
        if tab.session.screen.scrollback_len() > 0 || cursor_row + BANNER.len() >= self.grid_rows {
            self.banner = false;
        }
    }

    /// Banner rows above the grid. Zero when the banner is gone or the window is too short to show it.
    fn banner_rows(&self) -> usize {
        if self.banner && self.grid_rows >= 2 * BANNER.len() { BANNER.len() } else { 0 }
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

    /// Title shown for tab `index`: the name set by Rename Tab, else the shell's title, else a default.
    fn tab_title(&self, index: usize) -> String {
        let tab = &self.tabs[index];
        let title = tab.title.clone().or_else(|| tab.session.screen.title.clone()).unwrap_or_else(|| format!("Terminal {}", index + 1));
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

    /// Grid rows of runs for the active view, with the tab bar as row 0.
    pub fn frame(&self) -> Vec<Vec<Run>> {
        let (bar, _) = self.tab_bar();
        let mut rows = vec![bar];
        match &self.view {
            View::Terminal => {
                let palette = &self.palette;
                let selection = self.selection;
                let mut grid = match self.tabs.get(self.active) {
                    Some(tab) => grid_runs(
                        &tab.session.screen,
                        palette,
                        &GridOptions { view_offset: self.view_offset, selection, cursor: true, cursor_shape: self.cursor_shape() },
                    ),
                    None => vec![Vec::new(); self.grid_rows],
                };
                // The banner takes the top rows of the grid; the grid's bottom rows give way to it.
                let banner = self.banner_rows();
                grid.truncate(self.grid_rows - banner);
                rows.extend(self.banner_lines_for(banner));
                rows.extend(grid);
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
            _ => self.paste(),
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
        let mouse_captured = !event.shift_down() && self.tabs.get(self.active).is_some_and(|tab| tab.session.screen.mouse != MouseMode::Off);
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
        let Some(abs_row) = self.absolute_row(grid_row) else { return };
        let shift = event.shift_down();
        let mouse = self.tabs.get(self.active).map(|tab| (tab.session.screen.mouse, tab.session.screen.mouse_sgr));
        if let Some((mode, sgr)) = mouse {
            if mode != MouseMode::Off && !shift {
                let button = event.button.saturating_sub(1).min(2) as u8;
                let report = mouse_report(sgr, button, event.phase, col, grid_row, mode);
                if let Some(bytes) = report {
                    self.send_to_active(bytes);
                }
                return;
            }
        }
        match event.phase {
            UiActionPhase::Press if event.button == 1 => {
                self.selection = None;
                self.drag_anchor = None;
                match self.register_click(abs_row, col) {
                    2 => self.select_word(abs_row, col),
                    3 => self.select_line(abs_row),
                    _ => self.drag_anchor = Some((abs_row, col)),
                }
            }
            // The renderer only fires Press for button 1; buttons 2 and 3 are delivered on Release.
            UiActionPhase::Release if event.button == 2 => self.paste_primary(),
            UiActionPhase::Motion | UiActionPhase::Hover if self.drag_anchor.is_some() => {
                if let Some(anchor) = self.drag_anchor {
                    self.selection = Some(Span::normalised(anchor, (abs_row, col)));
                }
            }
            UiActionPhase::Release => {
                if let Some(anchor) = self.drag_anchor.take() {
                    if anchor == (abs_row, col) {
                        self.selection = None;
                    } else {
                        self.select_span(Span::normalised(anchor, (abs_row, col)));
                    }
                }
            }
            _ => {}
        }
    }

    fn wheel(&mut self, up: bool, event: &UiActionEvent) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let (mode, sgr, alt) = (tab.session.screen.mouse, tab.session.screen.mouse_sgr, tab.session.screen.alt_active());
        let app_cursor = tab.session.screen.app_cursor;
        if mode != MouseMode::Off {
            let col = (event.x / self.cell_w).floor().max(0.0) as usize;
            let row = ((event.y / self.cell_h).floor().max(1.0) as usize).saturating_sub(1);
            let button = if up { 64 } else { 65 };
            let report = mouse_report_raw(sgr, button, true, col, row);
            self.send_to_active(report);
        } else if alt {
            let arrow = match (up, app_cursor) {
                (true, true) => "\x1bOA",
                (true, false) => "\x1b[A",
                (false, true) => "\x1bOB",
                (false, false) => "\x1b[B",
            };
            let bytes = arrow.repeat(SCROLL_LINES_PER_NOTCH).into_bytes();
            self.send_to_active(bytes);
        } else {
            let max = tab.session.screen.scrollback_len();
            self.view_offset = if up {
                (self.view_offset + SCROLL_LINES_PER_NOTCH).min(max)
            } else {
                self.view_offset.saturating_sub(SCROLL_LINES_PER_NOTCH)
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

    /// Absolute line index for a grid row under the current scroll position.
    fn absolute_row(&self, grid_row: usize) -> Option<usize> {
        let tab = self.tabs.get(self.active)?;
        let total = tab.session.screen.total_lines();
        let top = total.saturating_sub(tab.session.screen.rows() + self.view_offset.min(tab.session.screen.scrollback_len()));
        Some(top + grid_row)
    }
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

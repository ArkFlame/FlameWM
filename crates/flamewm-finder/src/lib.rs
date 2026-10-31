//! Native `FlameWM` file finder: one glass search window over a bounded filename index.
//!
//! The index is read from the on-disk cache at start, so the first query answers at once. A walk of
//! the home folder runs on one worker thread and replaces the index when it finishes. The window is
//! toggled by the session shortcut; see [`instance`].

mod format;
mod index;
mod instance;
mod keys;
mod kind;
mod palette;
mod preview;
mod search;

use std::cell::{Cell, RefCell};
use std::env;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::ControlClient;
use flamewm_dbus_reactor::BusKind;
use flamewm_ui_core::surface::surface_fill;
use flamewm_ui_x11::{
    UiActionEvent, UiActionPhase, UiColor, UiControllerEvent, UiDocumentAccess, UiKeyEvent,
    UiKeyPhase, UiWakeDrain, UiWindowConfig, UiWindowRole, X11WindowIdentity, decode_document,
    run_with_controller_events_role_and_two_readable_fds,
};

use index::{Entry, Index};
use keys::{Command, Modifiers};
use kind::{KINDS, Kind};
use palette::{CommandSpec, Mode};
use preview::Preview;
use search::{Hits, Query};

const UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-finder.rwr"));
const WIDTH: u32 = 720;
const HEIGHT: u32 = 520;
/// Result rows that exist in the document; the list scrolls through them.
const ROW_SLOTS: usize = 8;
/// Preview lines that exist in the document.
const PREVIEW_SLOTS: usize = 24;
/// Name pieces per row that exist in the document; a name split by more terms merges the tail.
const SEGMENT_SLOTS: usize = 8;
/// Tiles per row in the grid view.
const GRID_COLUMNS: usize = 4;
/// Duplicate groups shown by the duplicates dialog.
const DUPLICATE_SLOTS: usize = 6;
const QUERY_LIMIT: usize = 256;
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const PLACEHOLDER_INTERVAL: Duration = Duration::from_secs(5);
const PLACEHOLDERS: [&str; 4] = [
    "Search everything",
    "report type:pdf",
    "in:~/Projects config",
    "photo.jpg",
];
const TEXT_RGB: u32 = 0x00f5_f5f7;
const MUTED_RGB: u32 = 0x00a1_a1a6;
const SUBTLE_RGB: u32 = 0x0086_868b;
/// Settings `accent` default; keep equal to `DEFAULT_ACCENT` in `flamewm-platform/src/settings.rs`.
const ACCENT_RGB: u32 = 0x0053_9bff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Overlay {
    None,
    Preview,
    Details,
    Help,
    Duplicates,
    Setup,
}

/// One row or tile of the current view.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Entry(usize),
    Command(&'static CommandSpec),
    Scope(palette::Choice),
}

/// Text and accent colours for the current glass tint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Palette {
    text: u32,
    muted: u32,
    subtle: u32,
    accent: u32,
}

impl Palette {
    /// Light text on dark glass, dark text on light glass, and the Settings `accent` colour.
    fn for_theme(theme: &SettingsSnapshot) -> Self {
        let accent = match theme.values.get("accent") {
            Some(SettingValue::Text(hex)) => u32::from_str_radix(hex.trim_start_matches('#'), 16)
                .map_or(ACCENT_RGB, |rgb| rgb & 0x00ff_ffff),
            _ => ACCENT_RGB,
        };
        if luminance(surface_fill(theme).rgb) > 150 {
            Self {
                text: 0x001d_1d1f,
                muted: 0x0051_5154,
                subtle: 0x006e_6e73,
                accent,
            }
        } else {
            Self {
                text: TEXT_RGB,
                muted: MUTED_RGB,
                subtle: SUBTLE_RGB,
                accent,
            }
        }
    }
}

fn luminance(rgb: u32) -> u32 {
    (((rgb >> 16) & 0xff) * 299 + ((rgb >> 8) & 0xff) * 587 + (rgb & 0xff) * 114) / 1000
}

/// Merges pieces past `max` into the last slot, so every name fits the document's segment nodes.
fn fold_pieces(mut pieces: Vec<(String, bool)>, max: usize) -> Vec<(String, bool)> {
    if pieces.len() <= max {
        return pieces;
    }
    let tail: String = pieces
        .split_off(max - 1)
        .into_iter()
        .map(|(text, _)| text)
        .collect();
    pieces.push((tail, false));
    pieces
}

struct Finder {
    home: String,
    index: Index,
    walking: bool,
    query: String,
    parsed: Query,
    hits: Hits,
    selected: usize,
    first: usize,
    navigating: bool,
    placeholder: usize,
    overlay: Overlay,
    preview: Option<Preview>,
    /// The user's list or grid choice for search results. The home view is always tiles.
    grid: bool,
    groups: Vec<Vec<usize>>,
    theme: SettingsSnapshot,
    palette: Palette,
    pid_file: PathBuf,
    last_click: Option<(usize, Instant)>,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|since| i64::try_from(since.as_secs()).ok())
        .unwrap_or(0)
}

fn rgba(rgb: u32, alpha: u8) -> UiColor {
    UiColor {
        r: ((rgb >> 16) & 0xff) as u8,
        g: ((rgb >> 8) & 0xff) as u8,
        b: (rgb & 0xff) as u8,
        a: alpha,
    }
}

/// Short mark for the type column: the extension in capitals, or `DIR` / `FILE`.
fn mark_for(entry: &Entry) -> String {
    if entry.is_dir {
        return "DIR".to_owned();
    }
    let name = entry.name();
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.len() <= 4 => {
            ext.to_uppercase()
        }
        _ => "FILE".to_owned(),
    }
}

fn meta_line(entry: &Entry, now: i64) -> String {
    let age = format::relative_time(entry.modified, now);
    let head = if entry.is_dir {
        "Folder".to_owned()
    } else {
        format::bytes(entry.size)
    };
    if age.is_empty() {
        head
    } else {
        format!("{head} · {age}")
    }
}

impl Finder {
    fn new(home: String, index: Index, theme: SettingsSnapshot, pid_file: PathBuf) -> Self {
        let palette = Palette::for_theme(&theme);
        let mut app = Self {
            home,
            index,
            walking: true,
            query: String::new(),
            parsed: Query::default(),
            hits: Hits::default(),
            selected: 0,
            first: 0,
            navigating: false,
            placeholder: 0,
            overlay: Overlay::None,
            preview: None,
            grid: false,
            groups: Vec::new(),
            theme,
            palette,
            pid_file,
            last_click: None,
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        self.parsed = Query::parse(&self.query, &self.home);
        self.hits = if palette::mode(&self.query) == Mode::Files {
            search::rank(&self.index, &self.parsed, now_secs())
        } else {
            Hits::default()
        };
        self.selected = 0;
        self.first = 0;
    }

    fn set_query(&mut self, query: String) {
        self.query = query;
        self.refresh();
    }

    /// Items of the current view: result files, commands, or folder scopes.
    fn items(&self) -> Vec<Item> {
        match palette::mode(&self.query) {
            Mode::Files => self.hits.indices.iter().map(|&i| Item::Entry(i)).collect(),
            Mode::Commands(filter) => palette::command_matches(&filter)
                .into_iter()
                .map(Item::Command)
                .collect(),
            Mode::Scopes(filter) => palette::scope_choices(&self.home, &filter)
                .into_iter()
                .map(Item::Scope)
                .collect(),
        }
    }

    fn selected_item(&self) -> Option<Item> {
        self.items().into_iter().nth(self.selected)
    }

    /// True when results appear as tiles: always for the home view, and for results once the user
    /// has switched to the grid.
    fn tiles_shown(&self) -> bool {
        palette::mode(&self.query) == Mode::Files
            && !self.hits.indices.is_empty()
            && (self.grid || self.query.is_empty())
    }

    fn selected_entry(&self) -> Option<&Entry> {
        if palette::mode(&self.query) != Mode::Files {
            return None;
        }
        let position = *self.hits.indices.get(self.selected)?;
        self.index.entries.get(position)
    }

    fn select(&mut self, position: usize) {
        let len = self.items().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        self.selected = position.min(len - 1);
        if self.selected < self.first {
            self.first = self.selected;
        } else if self.selected >= self.first + ROW_SLOTS {
            self.first = self.selected + 1 - ROW_SLOTS;
        }
        if self.overlay == Overlay::Preview {
            self.preview = self
                .selected_entry()
                .map(|entry| preview::load(Path::new(&entry.path), entry.is_dir));
        }
    }

    fn step(&mut self, delta: isize) {
        let target = self.selected.saturating_add_signed(delta);
        self.select(target);
    }

    /// Rows moved by Up and Down: one in the list, a grid row of tiles in tile view.
    fn vertical_step(&self) -> isize {
        if self.tiles_shown() {
            isize::try_from(GRID_COLUMNS).unwrap_or(4)
        } else {
            1
        }
    }

    fn accept_index(&mut self, index: Index) {
        self.index = index;
        self.walking = false;
        self.refresh();
    }

    fn toggle(&mut self, overlay: Overlay) {
        if self.overlay == overlay {
            self.overlay = Overlay::None;
            return;
        }
        if overlay == Overlay::Preview {
            self.preview = self
                .selected_entry()
                .map(|entry| preview::load(Path::new(&entry.path), entry.is_dir));
        }
        self.overlay = overlay;
    }

    fn open_path(&self, target: &Path) {
        if let Err(error) = Process::new("xdg-open").arg(target).spawn() {
            eprintln!("flamewm-finder: open failed: {}: {error}", target.display());
        }
    }

    fn open_selected(&self, reveal: bool) {
        let Some(entry) = self.selected_entry() else {
            return;
        };
        let target = if reveal {
            Path::new(&entry.path)
                .parent()
                .map_or_else(|| PathBuf::from("/"), Path::to_path_buf)
        } else {
            PathBuf::from(&entry.path)
        };
        self.open_path(&target);
    }

    /// Enter: open a file, run a command, or write a folder scope into the search box.
    fn activate_selected(&mut self) {
        match self.selected_item() {
            Some(Item::Entry(_)) => self.open_selected(false),
            Some(Item::Command(spec)) => self.run_command(spec.id),
            Some(Item::Scope(choice)) => self.set_query(choice.query),
            None => {}
        }
    }

    fn run_command(&mut self, id: &str) {
        match id {
            "help" => self.overlay = Overlay::Help,
            "duplicates" => {
                self.groups = index::duplicate_groups(&self.index, DUPLICATE_SLOTS);
                self.overlay = Overlay::Duplicates;
            }
            "setup" => self.overlay = Overlay::Setup,
            "grid" => {
                self.grid = !self.grid;
                self.set_query(String::new());
            }
            "home" => {
                let home = PathBuf::from(&self.home);
                self.open_path(&home);
            }
            "clear" => self.set_query(String::new()),
            _ => {}
        }
    }

    fn quit(&self) -> ! {
        instance::release(&self.pid_file);
        std::process::exit(0)
    }

    fn key(&mut self, key: &UiKeyEvent, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        if key.phase != UiKeyPhase::Press {
            return Ok(());
        }
        let has = |name: &str| key.modifiers.iter().any(|m| m.eq_ignore_ascii_case(name));
        let mods = Modifiers {
            ctrl: has("ctrl") || has("control"),
            shift: has("shift"),
            alt: has("alt"),
        };
        let Some(command) = keys::command(&key.key, key.text.as_deref(), mods, self.navigating)
        else {
            return Ok(());
        };
        let allowed = match self.overlay {
            Overlay::None => true,
            Overlay::Preview | Overlay::Details => matches!(
                command,
                Command::Escape
                    | Command::Open
                    | Command::Reveal
                    | Command::Preview
                    | Command::Details
                    | Command::Help
            ),
            Overlay::Help | Overlay::Duplicates | Overlay::Setup => {
                matches!(command, Command::Escape | Command::Help)
            }
        };
        if !allowed {
            return Ok(());
        }
        match command {
            Command::Escape => match self.overlay {
                Overlay::None => self.quit(),
                _ => self.overlay = Overlay::None,
            },
            Command::Help => self.toggle(Overlay::Help),
            Command::Open => self.activate_selected(),
            Command::Reveal => self.open_selected(true),
            Command::Preview => self.toggle(Overlay::Preview),
            Command::Details => self.toggle(Overlay::Details),
            Command::Next => {
                self.navigating = true;
                self.step(1);
            }
            Command::Previous => {
                self.navigating = true;
                self.step(-1);
            }
            Command::Up => {
                self.navigating = true;
                self.step(-self.vertical_step());
            }
            Command::Down => {
                self.navigating = true;
                self.step(self.vertical_step());
            }
            Command::Left if self.tiles_shown() => {
                self.navigating = true;
                self.step(-1);
            }
            Command::Right if self.tiles_shown() => {
                self.navigating = true;
                self.step(1);
            }
            Command::Left | Command::Right => {}
            Command::ToggleView => self.grid = !self.grid,
            Command::PageDown => {
                self.navigating = true;
                self.step(isize::try_from(ROW_SLOTS).unwrap_or(8));
            }
            Command::PageUp => {
                self.navigating = true;
                self.step(-isize::try_from(ROW_SLOTS).unwrap_or(8));
            }
            Command::First => {
                self.navigating = true;
                self.select(0);
            }
            Command::Last => {
                self.navigating = true;
                self.select(usize::MAX);
            }
            Command::Backspace => {
                self.navigating = false;
                let mut query = self.query.clone();
                query.pop();
                self.set_query(query);
            }
            Command::Insert(text) => {
                self.navigating = false;
                let mut query = self.query.clone();
                query.extend(text.chars().filter(|c| !c.is_control()));
                if query.chars().count() > QUERY_LIMIT {
                    query = query.chars().take(QUERY_LIMIT).collect();
                }
                self.set_query(query);
            }
        }
        self.paint(doc)
    }

    fn action(
        &mut self,
        event: &UiActionEvent,
        doc: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if event.phase != UiActionPhase::Press {
            return Ok(());
        }
        match event.action.as_str() {
            "finder.clear" => {
                self.navigating = false;
                self.set_query(String::new());
            }
            "finder.close-overlay" => self.overlay = Overlay::None,
            action => {
                let slot = action
                    .strip_prefix("finder.row.")
                    .or_else(|| action.strip_prefix("finder.tile."))
                    .and_then(|slot| slot.parse::<usize>().ok());
                if let Some(slot) = slot {
                    let position = self.first + slot;
                    if position < self.items().len() {
                        self.navigating = true;
                        let repeat = self.last_click.is_some_and(|(prior, at)| {
                            prior == position && at.elapsed() < DOUBLE_CLICK
                        });
                        self.select(position);
                        self.last_click = Some((position, Instant::now()));
                        if repeat {
                            self.activate_selected();
                        }
                    }
                }
            }
        }
        self.paint(doc)
    }

    fn paint(&mut self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        self.paint_theme(doc)?;
        self.paint_query(doc)?;
        self.paint_section(doc)?;
        self.paint_items(doc)?;
        self.paint_status(doc)?;
        self.paint_overlays(doc)
    }

    fn paint_theme(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let surface = surface_fill(&self.theme);
        doc.background("finder", rgba(surface.rgb, surface.alpha))?;
        flamewm_ui_x11::surface_style::project_surface_border(doc, "finder", &self.theme)?;
        doc.color_variable("--accent", rgba(self.palette.accent, 255))?;
        doc.color_variable("--text", rgba(self.palette.text, 255))?;
        doc.color_variable("--muted", rgba(self.palette.muted, 255))?;
        doc.color_variable("--subtle", rgba(self.palette.subtle, 255))
    }

    fn paint_query(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let typing = !self.query.is_empty();
        if typing {
            doc.text("query", self.query.clone())?;
            doc.foreground("query", rgba(self.palette.text, 255))?;
        } else {
            doc.text("query", PLACEHOLDERS[self.placeholder].to_owned())?;
            doc.foreground("query", rgba(self.palette.subtle, 255))?;
        }
        doc.visible("clear", typing)?;
        let items = self.items().len();
        let count = match palette::mode(&self.query) {
            _ if !typing => String::new(),
            Mode::Files => match self.hits.total {
                0 => "No results".to_owned(),
                1 => "1 result".to_owned(),
                total => format!("{} results", format::count(total)),
            },
            Mode::Commands(_) => match items {
                0 => "No commands".to_owned(),
                n => format!("{n} commands"),
            },
            Mode::Scopes(_) => match items {
                0 => "No folders".to_owned(),
                n => format!("{n} folders"),
            },
        };
        doc.text("count", count)?;
        doc.visible("count", typing)
    }

    fn paint_section(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let label = match palette::mode(&self.query) {
            Mode::Files if self.query.is_empty() => "Recent",
            Mode::Files => "Results",
            Mode::Commands(_) => "Commands",
            Mode::Scopes(_) => "Folders",
        };
        doc.text("section", label.to_owned())?;
        doc.foreground("section", rgba(self.palette.subtle, 255))
    }

    fn paint_items(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let items = self.items();
        let tiles = self.tiles_shown();
        doc.visible("grid", tiles)?;
        doc.visible("list", !tiles)?;
        doc.visible("empty", items.is_empty())?;
        let empty_text = match palette::mode(&self.query) {
            _ if self.walking && self.index.is_empty() => "Indexing files...",
            Mode::Commands(_) => "No commands match",
            Mode::Scopes(_) => "No folders match",
            Mode::Files if self.query.is_empty() => "Nothing to show yet",
            Mode::Files => "No files match",
        };
        doc.text("empty-text", empty_text.to_owned())?;
        let now = now_secs();
        for slot in 0..ROW_SLOTS {
            let position = self.first + slot;
            let item = items.get(position);
            let selected = position == self.selected;
            if tiles {
                self.paint_tile(doc, slot, item, selected, now)?;
            } else {
                self.paint_row(doc, slot, item, selected, now)?;
            }
        }
        Ok(())
    }

    fn paint_row(
        &self,
        doc: &mut impl UiDocumentAccess,
        slot: usize,
        item: Option<&Item>,
        selected: bool,
        now: i64,
    ) -> Result<(), String> {
        let row = format!("row-{slot}");
        let Some(item) = item else {
            return doc.visible(&row, false);
        };
        doc.visible(&row, true)?;
        doc.background(&row, rgba(0x00ff_ffff, if selected { 36 } else { 0 }))?;
        let (kind, pieces, parent, meta) = match item {
            Item::Entry(position) => {
                let entry = &self.index.entries[*position];
                (
                    Kind::of(entry),
                    search::segments(entry.name(), &self.parsed.terms),
                    format::parent_for_display(&entry.path, &self.home),
                    meta_line(entry, now),
                )
            }
            Item::Command(spec) => (
                Kind::Code,
                vec![(spec.label.to_owned(), false)],
                String::new(),
                spec.hint.to_owned(),
            ),
            Item::Scope(choice) => (
                Kind::Folder,
                vec![(choice.label.clone(), false)],
                choice.query.trim().to_owned(),
                String::new(),
            ),
        };
        self.paint_icon(doc, &format!("ico-{slot}"), kind)?;
        self.paint_segments(doc, slot, pieces)?;
        doc.text(&format!("parent-{slot}"), parent)?;
        doc.text(&format!("meta-{slot}"), meta)
    }

    fn paint_tile(
        &self,
        doc: &mut impl UiDocumentAccess,
        slot: usize,
        item: Option<&Item>,
        selected: bool,
        now: i64,
    ) -> Result<(), String> {
        let tile = format!("tile-{slot}");
        let Some(Item::Entry(position)) = item else {
            return doc.visible(&tile, false);
        };
        let entry = &self.index.entries[*position];
        doc.visible(&tile, true)?;
        doc.background(&tile, rgba(0x00ff_ffff, if selected { 36 } else { 0 }))?;
        self.paint_icon(doc, &format!("tile-{slot}-ico"), Kind::of(entry))?;
        doc.text(&format!("tile-name-{slot}"), entry.name().to_owned())?;
        doc.text(&format!("tile-meta-{slot}"), meta_line(entry, now))
    }

    /// Shows the glyph for `kind` under `prefix` and hides the others.
    fn paint_icon(
        &self,
        doc: &mut impl UiDocumentAccess,
        prefix: &str,
        kind: Kind,
    ) -> Result<(), String> {
        for candidate in KINDS {
            doc.visible(&format!("{prefix}-{}", candidate.slug()), candidate == kind)?;
        }
        Ok(())
    }

    fn paint_segments(
        &self,
        doc: &mut impl UiDocumentAccess,
        slot: usize,
        pieces: Vec<(String, bool)>,
    ) -> Result<(), String> {
        let pieces = fold_pieces(pieces, SEGMENT_SLOTS);
        for segment in 0..SEGMENT_SLOTS {
            let id = format!("seg-{slot}-{segment}");
            match pieces.get(segment) {
                Some((text, hit)) => {
                    doc.visible(&id, true)?;
                    doc.text(&id, text.clone())?;
                    let color = if *hit {
                        self.palette.accent
                    } else {
                        self.palette.text
                    };
                    doc.foreground(&id, rgba(color, 255))?;
                }
                None => doc.visible(&id, false)?,
            }
        }
        Ok(())
    }

    fn paint_status(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let text = if self.walking {
            format!("Indexing · {} files", format::count(self.index.len()))
        } else if self.index.truncated {
            format!(
                "{} files · index limit reached",
                format::count(self.index.len())
            )
        } else {
            format!("{} files", format::count(self.index.len()))
        };
        doc.text("status", text)
    }

    fn paint_overlays(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        doc.visible("preview", self.overlay == Overlay::Preview)?;
        doc.visible("details", self.overlay == Overlay::Details)?;
        doc.visible("help", self.overlay == Overlay::Help)?;
        doc.visible("duplicates", self.overlay == Overlay::Duplicates)?;
        doc.visible("setup", self.overlay == Overlay::Setup)?;
        match self.overlay {
            Overlay::None | Overlay::Help => Ok(()),
            Overlay::Preview => self.paint_preview(doc),
            Overlay::Details => self.paint_details(doc),
            Overlay::Duplicates => self.paint_duplicates(doc),
            Overlay::Setup => self.paint_setup(doc),
        }
    }

    fn paint_preview(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let Some(entry) = self.selected_entry() else {
            return Ok(());
        };
        doc.text("preview-name", entry.name().to_owned())?;
        doc.text("preview-meta", meta_line(entry, now_secs()))?;
        let lines = match &self.preview {
            Some(Preview::Text(lines)) => lines.as_slice(),
            _ => &[],
        };
        doc.visible("preview-empty", lines.is_empty())?;
        for slot in 0..PREVIEW_SLOTS {
            let line = lines.get(slot).cloned().unwrap_or_default();
            doc.text(&format!("preview-line-{slot}"), line)?;
        }
        Ok(())
    }

    fn paint_details(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let Some(entry) = self.selected_entry() else {
            return Ok(());
        };
        doc.text("details-name", entry.name().to_owned())?;
        doc.text("details-path", entry.path.clone())?;
        doc.text(
            "details-type",
            if entry.is_dir {
                "Folder".to_owned()
            } else {
                mark_for(entry)
            },
        )?;
        doc.text(
            "details-size",
            if entry.is_dir {
                "-".to_owned()
            } else {
                format::bytes(entry.size)
            },
        )?;
        doc.text(
            "details-modified",
            format::relative_time(entry.modified, now_secs()),
        )
    }

    fn paint_duplicates(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let summary = match self.groups.len() {
            0 => String::new(),
            1 => "1 group".to_owned(),
            n => format!("{n} groups"),
        };
        doc.text("dups-sub", summary)?;
        doc.visible("dups-empty", self.groups.is_empty())?;
        for slot in 0..DUPLICATE_SLOTS {
            let row = format!("dup-{slot}");
            let Some(group) = self.groups.get(slot) else {
                doc.visible(&row, false)?;
                continue;
            };
            doc.visible(&row, true)?;
            let first = &self.index.entries[group[0]];
            doc.text(&format!("dup-name-{slot}"), first.name().to_owned())?;
            doc.text(
                &format!("dup-meta-{slot}"),
                format!("{} copies · {}", group.len(), format::bytes(first.size)),
            )?;
            let places: Vec<String> = group
                .iter()
                .take(3)
                .map(|position| {
                    format::parent_for_display(&self.index.entries[*position].path, &self.home)
                })
                .collect();
            doc.text(&format!("dup-path-{slot}"), places.join("  ·  "))?;
        }
        Ok(())
    }

    fn paint_setup(&self, doc: &mut impl UiDocumentAccess) -> Result<(), String> {
        let roots: Vec<String> = index::default_roots(env::var_os("HOME"))
            .iter()
            .map(|root| format::parent_for_display(&root.to_string_lossy(), &self.home))
            .collect();
        doc.text("setup-0", roots.join(", "))?;
        let truncated = if self.index.truncated {
            ", limit reached"
        } else {
            ""
        };
        doc.text(
            "setup-1",
            format!("{} files{truncated}", format::count(self.index.len())),
        )?;
        doc.text("setup-2", format!("{} folder levels", index::MAX_DEPTH))?;
        doc.text(
            "setup-3",
            format!("{} files at most", format::count(index::MAX_ENTRIES)),
        )?;
        let cache = index::cache_path(env::var_os("XDG_CACHE_HOME"), env::var_os("HOME"))
            .map_or_else(
                || "not available".to_owned(),
                |path| path.display().to_string(),
            );
        doc.text("setup-4", cache)
    }
}

/// Initial settings snapshot for glass and theme; defaults apply when the control service is down.
fn initial_settings() -> SettingsSnapshot {
    ControlClient::connect(BusKind::Session)
        .ok()
        .and_then(|client| {
            client
                .call_with_timeout(&ControlRequest::GetSettings, Duration::from_millis(250))
                .ok()
        })
        .and_then(|reply| match reply {
            ControlResponse::Settings(snapshot) => Some(snapshot),
            _ => None,
        })
        .unwrap_or_else(|| SettingsSnapshot::new(0))
}

fn packaged_root() -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let installed = env::current_exe().ok().and_then(|exe| {
        let bin = exe.parent()?;
        let usr = bin.parent()?;
        (bin.file_name()? == "bin" && usr.file_name()? == "usr").then(|| usr.join("share/flamewm"))
    });
    installed
        .filter(|root| root.join("assets").is_dir())
        .unwrap_or(source)
}

fn window_backdrop(snapshot: &SettingsSnapshot) -> flamewm_ui_x11::backdrop::WindowBackdrop {
    let text = |key: &str| match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => value.clone(),
        _ => String::new(),
    };
    let blur = match snapshot.values.get("surfaceBlurPercent") {
        Some(SettingValue::Percent(value)) => (*value).min(100),
        _ => 80,
    };
    let opacity = match snapshot.values.get("surfaceOpacity") {
        Some(SettingValue::Percent(value)) => (*value).min(100),
        _ => 50,
    };
    let fit = text("wallpaperFit");
    flamewm_ui_x11::backdrop::WindowBackdrop {
        wallpaper: text("wallpaper"),
        fit: if fit.is_empty() { "fill".into() } else { fit },
        packaged_root: packaged_root(),
        opacity_percent: opacity,
        blur_percent: blur,
    }
}

/// Drains a wake pipe. The pipe carries only "something changed" bytes, so the data is discarded.
fn drain(reader: &mut UnixStream) -> UiWakeDrain {
    let mut buffer = [0_u8; 64];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return UiWakeDrain::Retire,
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return UiWakeDrain::Keep;
            }
            Err(_) => return UiWakeDrain::Retire,
        }
    }
}

fn nonblocking_pair() -> Result<(UnixStream, UnixStream), String> {
    let (reader, writer) = UnixStream::pair().map_err(|error| error.to_string())?;
    reader
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    writer
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    Ok((reader, writer))
}

/// Starts the window. Returns `Ok` without a window when this launch toggled an open finder off.
pub fn run() -> Result<(), String> {
    let home = env::var("HOME").unwrap_or_default();
    let runtime_dir = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let cache_file = index::cache_path(env::var_os("XDG_CACHE_HOME"), env::var_os("HOME"));
    let cache_dir = cache_file.as_deref().and_then(Path::parent);
    let pid_file = instance::pid_path(runtime_dir.as_deref(), cache_dir)
        .ok_or_else(|| "no runtime or cache folder for the finder pid file".to_owned())?;
    if instance::claim(&pid_file, Path::new("/proc")) == instance::Claim::Toggled {
        return Ok(());
    }
    let result = show(&home, cache_file.as_deref(), &pid_file);
    instance::release(&pid_file);
    result
}

fn show(home: &str, cache_file: Option<&Path>, pid_file: &Path) -> Result<(), String> {
    let theme = initial_settings();
    let backdrop = window_backdrop(&theme);
    let mut document = decode_document(UI)?.with_window_backdrop(backdrop);
    let cached = cache_file.and_then(index::load_cache).unwrap_or_default();
    let mut app = Finder::new(home.to_owned(), cached, theme, pid_file.to_path_buf());
    app.paint(&mut document)?;

    let (mut index_reader, mut index_writer) = nonblocking_pair()?;
    let (mut tick_reader, mut tick_writer) = nonblocking_pair()?;
    let (sender, receiver) = mpsc::sync_channel::<Index>(1);
    let walk_cache = cache_file.map(Path::to_path_buf);
    let roots = index::default_roots(env::var_os("HOME"));
    std::thread::spawn(move || {
        let fresh = index::walk(
            &roots,
            index::MAX_ENTRIES,
            index::MAX_DEPTH,
            index::WALK_BUDGET,
        );
        if let Some(path) = &walk_cache {
            let _ = index::save_cache(path, &fresh);
        }
        if sender.send(fresh).is_ok() {
            let _ = index_writer.write(&[1]);
        }
    });

    let running = Arc::new(AtomicBool::new(true));
    let ticking = Arc::clone(&running);
    std::thread::spawn(move || {
        while ticking.load(Ordering::Relaxed) {
            std::thread::sleep(PLACEHOLDER_INTERVAL);
            if tick_writer
                .write(&[1])
                .is_err_and(|error| error.kind() == std::io::ErrorKind::BrokenPipe)
            {
                break;
            }
        }
    });

    let ticked = Rc::new(Cell::new(false));
    let ticked_drain = Rc::clone(&ticked);
    let window = Rc::new(RefCell::new(app));
    let event_window = Rc::clone(&window);
    let wake_window = Rc::clone(&window);
    let incoming: Rc<RefCell<Receiver<Index>>> = Rc::new(RefCell::new(receiver));
    let wake_incoming = Rc::clone(&incoming);
    let fd = index_reader.as_raw_fd();
    let tick_fd = tick_reader.as_raw_fd();
    let outcome = run_with_controller_events_role_and_two_readable_fds(
        document,
        UiWindowConfig {
            width: WIDTH,
            height: HEIGHT,
            x: 120,
            y: 90,
            title: "FlameWM Finder".into(),
            identity: Some(X11WindowIdentity {
                instance: "flamewm-finder".into(),
                class: "flamewm-finder".into(),
            }),
        },
        UiWindowRole::Normal,
        fd,
        move || drain(&mut index_reader),
        Some((
            tick_fd,
            Box::new(move || {
                ticked_drain.set(true);
                drain(&mut tick_reader)
            }) as Box<dyn FnMut() -> UiWakeDrain>,
        )),
        move |event, doc| {
            let mut app = event_window.borrow_mut();
            match event {
                UiControllerEvent::Action(action) => app.action(action, doc),
                UiControllerEvent::Key(key) => app.key(key, doc),
                UiControllerEvent::Resized { .. } | UiControllerEvent::Drag(_) | UiControllerEvent::Visibility { .. } => Ok(()),
            }
        },
        move |doc| {
            let mut app = wake_window.borrow_mut();
            let mut changed = false;
            while let Ok(fresh) = wake_incoming.borrow().try_recv() {
                app.accept_index(fresh);
                changed = true;
            }
            if ticked.replace(false) && app.query.is_empty() {
                app.placeholder = (app.placeholder + 1) % PLACEHOLDERS.len();
                changed = true;
            }
            if changed {
                app.paint(doc)?;
            }
            Ok(())
        },
    );
    running.store(false, Ordering::Relaxed);
    outcome
}

#[cfg(test)]
mod tests {
    #[test]
    fn palette_accent_follows_the_settings_accent_key() {
        let mut theme = SettingsSnapshot::new(1);
        assert_eq!(Palette::for_theme(&theme).accent, ACCENT_RGB);
        theme.values.insert("accent".to_owned(), SettingValue::Text("#EF4048".to_owned()));
        assert_eq!(Palette::for_theme(&theme).accent, 0x00ef_4048);
    }

    use super::*;

    #[test]
    fn marks_use_extension_or_type_word() {
        assert_eq!(
            mark_for(&Entry::new("/h/a/report.pdf".into(), false, 0, 0)),
            "PDF"
        );
        assert_eq!(
            mark_for(&Entry::new("/h/a/Makefile".into(), false, 0, 0)),
            "FILE"
        );
        assert_eq!(mark_for(&Entry::new("/h/a/src".into(), true, 0, 0)), "DIR");
        assert_eq!(
            mark_for(&Entry::new("/h/.bashrc".into(), false, 0, 0)),
            "FILE"
        );
    }

    #[test]
    fn meta_line_joins_size_and_age() {
        let file = Entry::new("/h/a.txt".into(), false, 1536, 1_000);
        assert_eq!(meta_line(&file, 1_000 + 120), "1.5 KB · 2m ago");
        let folder = Entry::new("/h/d".into(), true, 0, 0);
        assert_eq!(meta_line(&folder, 5), "Folder");
    }

    #[test]
    fn rgba_splits_channels() {
        assert_eq!(
            rgba(0x0012_3456, 7),
            UiColor {
                r: 0x12,
                g: 0x34,
                b: 0x56,
                a: 7
            }
        );
    }
}

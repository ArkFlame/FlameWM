//! Settings view drawn inside the terminal, with four tabs: Theme, Font type, Font size and Background opacity.
//! Left/Right change tabs. Theme, font and size changes preview live; Enter saves them and Escape restores the saved
//! settings. Theme tab: Up/Down choose a theme, Enter applies it, C copies it, E edits colours, R renames, D deletes.
//! Built-in themes are read-only. `handle_key` returns commands; the app performs the preview, file and config writes.
use flamewm_ui_x11::UiKeyEvent;

use crate::store::{DEFAULT_FONT_FAMILY, FOLLOW_FLAMEWM, TerminalConfig};
use crate::themes::{Palette, Theme, format_hex, parse_hex};
use crate::view::{Run, text_run};

/// Body row of the first list entry: tab strip, hint and a blank row come before it.
const LIST_TOP: usize = 3;
/// Rows below the list: the mode line and the status line.
const FOOTER_ROWS: usize = 2;
/// Rows the theme colour editor takes below the list.
const EDITOR_ROWS: usize = 22;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Theme,
    Font,
    Size,
    Opacity,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Theme, Tab::Font, Tab::Size, Tab::Opacity];

    fn label(self) -> &'static str {
        match self {
            Tab::Theme => "Theme",
            Tab::Font => "Font type",
            Tab::Size => "Font size",
            Tab::Opacity => "Background opacity",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Tab::Theme => "Enter apply  C copy  E edit  R rename  D delete  F follow FlameWM  Left/Right tab  Esc close",
            Tab::Font => "Up/Down preview a font  Enter save  Esc restore  Left/Right tab",
            Tab::Size => "Up/Down or +/- change size  Enter save  Esc restore  Left/Right tab",
            Tab::Opacity => "Up/Down preview opacity  Enter save  Esc restore  Left/Right tab",
        }
    }

    /// Neighbouring tab; the ends do not wrap.
    fn step(self, forward: bool) -> Tab {
        let index = Tab::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        let next = if forward { (index + 1).min(Tab::ALL.len() - 1) } else { index.saturating_sub(1) };
        Tab::ALL[next]
    }
}

/// Background opacity choices, 100 down to 5 in steps of 5.
pub fn opacity_options() -> Vec<u8> {
    (1..=20).rev().map(|step| step * 5).collect()
}

/// Monospace font families from fontconfig. The generic `monospace` alias always comes first.
pub fn mono_fonts() -> Vec<String> {
    let mut fonts = vec![DEFAULT_FONT_FAMILY.to_owned()];
    if let Ok(output) = std::process::Command::new("fc-list").args([":spacing=mono", "family"]).output() {
        fonts.extend(String::from_utf8_lossy(&output.stdout).lines().filter_map(|line| line.split(',').next()).map(str::trim).filter(|name| !name.is_empty()).map(str::to_owned));
    }
    fonts.sort();
    fonts.dedup();
    fonts
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Rename(String),
    Hex(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    None,
    Close,
    Apply(String),
    Copy(String),
    Delete(String),
    Rename { old: String, new: String },
    /// A user theme's working copy changed; persist it.
    Save(Theme),
    /// Shows a theme on the live screen without saving it.
    PreviewTheme(String),
    /// Shows a font family without saving it.
    Font(String),
    /// Changes the font size without saving it.
    FontSize(i32),
    /// Shows a background opacity without saving it.
    Opacity(u8),
    /// Saves the previewed font, size and opacity.
    Persist,
    CycleCursor,
}

#[derive(Clone, Debug)]
pub struct SettingsView {
    pub tab: Tab,
    pub selected: usize,
    pub fonts: Vec<String>,
    pub font_selected: usize,
    pub opacity_selected: usize,
    pub slot: usize,
    pub mode: Mode,
    /// Working copy of the user theme being edited. Its palette is the live preview.
    pub edit: Option<Theme>,
    pub message: String,
}

impl Default for SettingsView {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsView {
    pub fn new() -> Self {
        Self { tab: Tab::Theme, selected: 0, fonts: Vec::new(), font_selected: 0, opacity_selected: 0, slot: 0, mode: Mode::Browse, edit: None, message: String::new() }
    }

    /// The view as it opens: the saved theme, font and opacity are highlighted.
    pub fn open(themes: &[Theme], config: &TerminalConfig, fonts: Vec<String>) -> Self {
        let mut view = Self::new();
        view.focus(themes, &config.theme);
        view.font_selected = fonts.iter().position(|font| *font == config.font_family).unwrap_or(0);
        view.fonts = fonts;
        view.opacity_selected = opacity_options().iter().position(|&percent| percent == config.background_opacity).unwrap_or(0);
        view
    }

    /// Palette the view should preview while editing, if any.
    pub fn preview(&self) -> Option<&Palette> {
        self.edit.as_ref().map(|theme| &theme.palette)
    }

    fn select_by_name(&mut self, themes: &[Theme], name: &str) {
        if let Some(index) = themes.iter().position(|t| t.name == name) {
            self.selected = index;
        }
    }

    /// Selects a theme by name, e.g. after a copy or rename, so the picker follows the change.
    pub fn focus(&mut self, themes: &[Theme], name: &str) {
        self.select_by_name(themes, name);
    }

    /// Top list index and visible list rows for a body of `rows` rows, for the current tab's selection.
    fn list_window(&self, rows: usize) -> (usize, usize) {
        let reserved = LIST_TOP + FOOTER_ROWS + if self.edit.is_some() { EDITOR_ROWS } else { 0 };
        let list_rows = rows.saturating_sub(reserved).max(1);
        let selected = match self.tab {
            Tab::Theme => self.selected,
            Tab::Font => self.font_selected,
            Tab::Opacity => self.opacity_selected,
            Tab::Size => 0,
        };
        (selected.saturating_sub(list_rows - 1), list_rows)
    }

    /// Mouse hover over a theme row (body row, below the tab strip) shows that theme without saving it.
    pub fn hover_theme(&mut self, body_row: usize, rows: usize, themes: &[Theme]) -> Command {
        if self.tab != Tab::Theme || self.edit.is_some() || self.mode != Mode::Browse || body_row < LIST_TOP {
            return Command::None;
        }
        let (top, list_rows) = self.list_window(rows);
        let offset = body_row - LIST_TOP;
        let index = top + offset;
        if offset >= list_rows || index >= themes.len() || index == self.selected {
            return Command::None;
        }
        self.selected = index;
        self.preview_selected(themes)
    }

    fn preview_selected(&self, themes: &[Theme]) -> Command {
        themes.get(self.selected).map_or(Command::None, |theme| Command::PreviewTheme(theme.name.clone()))
    }

    pub fn handle_key(&mut self, event: &UiKeyEvent, themes: &[Theme]) -> Command {
        if event.phase != flamewm_ui_x11::UiKeyPhase::Press {
            return Command::None;
        }
        let name = event.key.to_ascii_lowercase();
        let selected = themes.get(self.selected).cloned();
        match self.mode.clone() {
            Mode::Rename(buffer) => match name.as_str() {
                "escape" => {
                    self.mode = Mode::Browse;
                    Command::None
                }
                "return" | "enter" => {
                    self.mode = Mode::Browse;
                    match selected {
                        Some(theme) if !buffer.trim().is_empty() => Command::Rename { old: theme.name, new: buffer.trim().to_owned() },
                        _ => Command::None,
                    }
                }
                "backspace" => {
                    let mut buffer = buffer;
                    buffer.pop();
                    self.mode = Mode::Rename(buffer);
                    Command::None
                }
                _ => {
                    if let Some(text) = event.text.as_deref().filter(|t| !t.chars().any(char::is_control)) {
                        self.mode = Mode::Rename(format!("{buffer}{text}"));
                    }
                    Command::None
                }
            },
            Mode::Hex(buffer) => match name.as_str() {
                "escape" => {
                    if buffer.is_empty() {
                        self.edit = None;
                        self.mode = Mode::Browse;
                    } else {
                        self.mode = Mode::Hex(String::new());
                    }
                    Command::None
                }
                "return" | "enter" => {
                    let commit = parse_hex(&format!("#{buffer}"));
                    self.mode = Mode::Hex(String::new());
                    match (commit, self.edit.as_mut()) {
                        (Some(value), Some(theme)) => {
                            let slot = theme.palette.slots()[self.slot].0;
                            theme.palette.set_slot(slot, value);
                            self.message = format!("{slot} set to {}", format_hex(value));
                            Command::Save(theme.clone())
                        }
                        _ => {
                            self.message = "enter six hex digits".into();
                            Command::None
                        }
                    }
                }
                "backspace" => {
                    let mut buffer = buffer;
                    buffer.pop();
                    self.mode = Mode::Hex(buffer);
                    Command::None
                }
                _ => {
                    let mut buffer = buffer;
                    for c in event.text.as_deref().unwrap_or("").chars().filter(char::is_ascii_hexdigit) {
                        if buffer.len() < 6 {
                            buffer.push(c.to_ascii_lowercase());
                        }
                    }
                    self.mode = Mode::Hex(buffer);
                    Command::None
                }
            },
            Mode::Browse => self.browse_key(&name, event, themes, selected),
        }
    }

    fn browse_key(&mut self, name: &str, event: &UiKeyEvent, themes: &[Theme], selected: Option<Theme>) -> Command {
        if self.edit.is_some() {
            let count = self.edit.as_ref().map_or(0, |t| t.palette.slots().len());
            match name {
                "escape" => {
                    self.edit = None;
                    self.message.clear();
                }
                "up" => self.slot = self.slot.saturating_sub(1),
                "down" => self.slot = (self.slot + 1).min(count.saturating_sub(1)),
                "return" | "enter" => self.mode = Mode::Hex(String::new()),
                _ => {}
            }
            return Command::None;
        }
        match name {
            "escape" => return Command::Close,
            "left" => self.tab = self.tab.step(false),
            "right" => self.tab = self.tab.step(true),
            "plus" | "=" => return Command::FontSize(1),
            "minus" | "-" => return Command::FontSize(-1),
            _ => return match self.tab {
                Tab::Theme => self.theme_key(name, event, themes, selected),
                Tab::Font => self.font_key(name),
                Tab::Size => match name {
                    "up" => Command::FontSize(1),
                    "down" => Command::FontSize(-1),
                    "return" | "enter" => Command::Persist,
                    _ => Command::None,
                },
                Tab::Opacity => self.opacity_key(name),
            },
        }
        Command::None
    }

    fn theme_key(&mut self, name: &str, event: &UiKeyEvent, themes: &[Theme], selected: Option<Theme>) -> Command {
        match name {
            "up" => {
                self.selected = self.selected.saturating_sub(1);
                return self.preview_selected(themes);
            }
            "down" => {
                self.selected = (self.selected + 1).min(themes.len().saturating_sub(1));
                return self.preview_selected(themes);
            }
            "return" | "enter" => {
                if let Some(theme) = selected {
                    return Command::Apply(theme.name);
                }
            }
            "f" if event.modifiers.is_empty() => return Command::Apply(FOLLOW_FLAMEWM.into()),
            "c" => {
                if let Some(theme) = selected {
                    return Command::Copy(theme.name);
                }
            }
            "e" => match selected {
                Some(theme) if !theme.builtin => {
                    self.slot = 0;
                    self.edit = Some(theme);
                    self.message = "Enter sets the selected colour. Esc leaves editing.".into();
                }
                _ => self.message = "Built-in themes are read-only. Press C to copy one.".into(),
            },
            "r" => match selected {
                Some(theme) if !theme.builtin => self.mode = Mode::Rename(theme.name),
                _ => self.message = "Built-in themes cannot be renamed. Press C to copy one.".into(),
            },
            "delete" | "d" => match selected {
                Some(theme) if !theme.builtin => return Command::Delete(theme.name),
                _ => self.message = "Built-in themes cannot be deleted.".into(),
            },
            "k" => return Command::CycleCursor,
            _ => {}
        }
        Command::None
    }

    fn font_key(&mut self, name: &str) -> Command {
        match name {
            "up" => self.font_selected = self.font_selected.saturating_sub(1),
            "down" => self.font_selected = (self.font_selected + 1).min(self.fonts.len().saturating_sub(1)),
            "return" | "enter" => return Command::Persist,
            _ => return Command::None,
        }
        self.fonts.get(self.font_selected).map_or(Command::None, |font| Command::Font(font.clone()))
    }

    fn opacity_key(&mut self, name: &str) -> Command {
        let options = opacity_options();
        match name {
            "up" => self.opacity_selected = self.opacity_selected.saturating_sub(1),
            "down" => self.opacity_selected = (self.opacity_selected + 1).min(options.len() - 1),
            "return" | "enter" => return Command::Persist,
            _ => return Command::None,
        }
        Command::Opacity(options[self.opacity_selected])
    }

    /// Settings rows: tab strip, hint, blank, the current tab's body and a status line. `rows` is the body height.
    pub fn render(&self, cols: usize, rows: usize, themes: &[Theme], palette: &Palette, config: &TerminalConfig) -> Vec<Vec<Run>> {
        let fg = palette.foreground;
        let muted = mix(fg, palette.background, 60);
        let mut out: Vec<Vec<Run>> = Vec::new();
        let line = |text: String, colour: u32, bg: Option<u32>, bold: bool| vec![text_run(0, &clip(&text, cols), colour, bg, bold)];

        let mut strip = Vec::new();
        let mut col = 0;
        for tab in Tab::ALL {
            let active = tab == self.tab;
            let label = format!(" {} ", tab.label());
            let width = label.chars().count();
            strip.push(text_run(col, &label, fg, active.then_some(palette.selection), active));
            col += width + 1;
        }
        out.push(strip);
        out.push(line(self.tab.hint().into(), muted, None, false));
        out.push(line(String::new(), fg, None, false));

        let (top, list_rows) = self.list_window(rows);
        match self.tab {
            Tab::Theme => {
                for (index, theme) in themes.iter().enumerate().skip(top).take(list_rows) {
                    let marker = if theme.name == config.theme { "●" } else { " " };
                    let origin = if theme.builtin { "built-in" } else { "custom" };
                    let selected = index == self.selected;
                    let text = format!(" {marker} {:<26} {origin}", theme.name);
                    out.push(line(text, fg, selected.then_some(palette.selection), selected));
                }
                if let Some(theme) = &self.edit {
                    out.push(line(format!(" Editing {}", theme.name), fg, None, true));
                    for (index, (slot, value)) in theme.palette.slots().into_iter().enumerate() {
                        let selected = index == self.slot;
                        let text = format!("   {:<16} {}", slot, format_hex(value));
                        out.push(line(text, fg, if selected { Some(palette.selection) } else { None }, selected));
                    }
                }
            }
            Tab::Font => {
                if self.fonts.is_empty() {
                    out.push(line(" No monospace fonts found (fc-list)".into(), muted, None, false));
                }
                for (index, font) in self.fonts.iter().enumerate().skip(top).take(list_rows) {
                    let marker = if *font == config.font_family { "●" } else { " " };
                    let selected = index == self.font_selected;
                    out.push(line(format!(" {marker} {font}"), fg, selected.then_some(palette.selection), selected));
                }
            }
            Tab::Size => out.push(line(format!(" Font size {}  (Up/Down change)", config.font_size), fg, None, true)),
            Tab::Opacity => {
                for (index, percent) in opacity_options().into_iter().enumerate().skip(top).take(list_rows) {
                    let marker = if percent == config.background_opacity { "●" } else { " " };
                    let selected = index == self.opacity_selected;
                    out.push(line(format!(" {marker} {percent:>3}%"), fg, selected.then_some(palette.selection), selected));
                }
            }
        }
        match &self.mode {
            Mode::Rename(buffer) => out.push(line(format!(" Name: {buffer}_"), fg, None, false)),
            Mode::Hex(buffer) => out.push(line(format!(" Hex: #{buffer}_  (six digits, Enter sets)"), fg, None, false)),
            Mode::Browse => out.push(line(String::new(), fg, None, false)),
        }
        out.push(line(format!(" {}", self.message), muted, None, false));
        while out.len() < rows {
            out.push(Vec::new());
        }
        out.truncate(rows);
        out
    }
}

fn clip(text: &str, cols: usize) -> String {
    text.chars().take(cols).collect()
}

fn mix(a: u32, b: u32, weight_a: u32) -> u32 {
    let channel = |shift: u32| (((a >> shift) & 0xff) * weight_a + ((b >> shift) & 0xff) * (100 - weight_a)) / 100;
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themes::builtin_themes;
    use flamewm_ui_x11::{UiKeyEvent, UiKeyPhase};

    fn press(key: &str, text: Option<&str>) -> UiKeyEvent {
        UiKeyEvent { phase: UiKeyPhase::Press, modifiers: Vec::new(), key: key.into(), text: text.map(str::to_owned) }
    }

    fn user_theme(name: &str) -> Theme {
        Theme { name: name.into(), palette: builtin_themes()[0].palette.clone(), builtin: false }
    }

    fn fonts() -> Vec<String> {
        vec!["monospace".into(), "Fira Code".into(), "Hack".into()]
    }

    #[test]
    fn builtin_themes_cannot_be_edited_deleted_or_renamed() {
        let themes = builtin_themes();
        let mut view = SettingsView::new();
        assert_eq!(view.handle_key(&press("e", Some("e")), &themes), Command::None);
        assert!(view.edit.is_none());
        assert_eq!(view.handle_key(&press("delete", None), &themes), Command::None);
        assert_eq!(view.handle_key(&press("r", Some("r")), &themes), Command::None);
        assert_eq!(view.mode, Mode::Browse);
        assert_eq!(view.handle_key(&press("c", Some("c")), &themes), Command::Copy("Dark+".into()));
    }

    #[test]
    fn enter_applies_the_selected_theme() {
        let themes = builtin_themes();
        let mut view = SettingsView::new();
        view.handle_key(&press("down", None), &themes);
        assert_eq!(view.handle_key(&press("return", None), &themes), Command::Apply("Light+".into()));
    }

    #[test]
    fn arrows_and_hover_preview_themes_without_applying() {
        let themes = builtin_themes();
        let mut view = SettingsView::new();
        assert_eq!(view.handle_key(&press("down", None), &themes), Command::PreviewTheme("Light+".into()));
        // Body row 3 is the first list entry; hovering the third entry previews it.
        assert_eq!(view.hover_theme(LIST_TOP + 2, 40, &themes), Command::PreviewTheme(themes[2].name.clone()));
        assert_eq!(view.hover_theme(LIST_TOP + 2, 40, &themes), Command::None, "hovering the same row does nothing");
        assert_eq!(view.hover_theme(0, 40, &themes), Command::None, "the tab strip is not a theme");
    }

    #[test]
    fn left_and_right_move_between_tabs_without_wrapping() {
        let themes = builtin_themes();
        let mut view = SettingsView::new();
        view.handle_key(&press("left", None), &themes);
        assert_eq!(view.tab, Tab::Theme, "left at the first tab stays put");
        for expected in [Tab::Font, Tab::Size, Tab::Opacity, Tab::Opacity] {
            view.handle_key(&press("right", None), &themes);
            assert_eq!(view.tab, expected);
        }
        view.handle_key(&press("left", None), &themes);
        assert_eq!(view.tab, Tab::Size);
    }

    #[test]
    fn font_tab_previews_each_family_and_enter_saves() {
        let themes = builtin_themes();
        let mut view = SettingsView::open(&themes, &TerminalConfig::default(), fonts());
        view.handle_key(&press("right", None), &themes);
        assert_eq!(view.handle_key(&press("down", None), &themes), Command::Font("Fira Code".into()));
        assert_eq!(view.handle_key(&press("return", None), &themes), Command::Persist);
        assert_eq!(view.handle_key(&press("escape", None), &themes), Command::Close);
    }

    #[test]
    fn size_tab_changes_size_live_and_saves_on_enter() {
        let themes = builtin_themes();
        let mut view = SettingsView::new();
        view.handle_key(&press("right", None), &themes);
        view.handle_key(&press("right", None), &themes);
        assert_eq!(view.handle_key(&press("up", None), &themes), Command::FontSize(1));
        assert_eq!(view.handle_key(&press("plus", Some("+")), &themes), Command::FontSize(1));
        assert_eq!(view.handle_key(&press("return", None), &themes), Command::Persist);
    }

    #[test]
    fn opacity_options_run_from_100_to_5_and_default_is_80() {
        let options = opacity_options();
        assert_eq!(options.first(), Some(&100));
        assert_eq!(options.last(), Some(&5));
        assert_eq!(options.len(), 20);
        assert!(options.windows(2).all(|pair| pair[0] - pair[1] == 5));
        assert!(options.contains(&TerminalConfig::default().background_opacity));
        assert_eq!(TerminalConfig::default().background_opacity, 80);
    }

    #[test]
    fn opacity_tab_previews_steps_and_saves() {
        let themes = builtin_themes();
        let mut view = SettingsView::open(&themes, &TerminalConfig::default(), fonts());
        for _ in 0..3 {
            view.handle_key(&press("right", None), &themes);
        }
        assert_eq!(view.tab, Tab::Opacity);
        // The default 80 is the fifth option, so one Down step previews 75.
        assert_eq!(view.handle_key(&press("down", None), &themes), Command::Opacity(75));
        assert_eq!(view.handle_key(&press("up", None), &themes), Command::Opacity(80));
        assert_eq!(view.handle_key(&press("return", None), &themes), Command::Persist);
    }

    #[test]
    fn hex_editing_sets_a_slot_and_saves_the_working_copy() {
        let mut themes = builtin_themes();
        themes.push(user_theme("Mine"));
        let mut view = SettingsView::new();
        view.selected = themes.len() - 1;
        assert_eq!(view.handle_key(&press("e", Some("e")), &themes), Command::None);
        assert!(view.edit.is_some());
        view.handle_key(&press("return", None), &themes);
        for c in "ff8800".chars() {
            view.handle_key(&press(&c.to_string(), Some(&c.to_string())), &themes);
        }
        let command = view.handle_key(&press("return", None), &themes);
        let Command::Save(saved) = command else { panic!("expected save, got {command:?}") };
        assert_eq!(saved.palette.background, 0xff8800);
    }

    #[test]
    fn rename_commits_trimmed_name_and_escape_cancels() {
        let mut themes = builtin_themes();
        themes.push(user_theme("Mine"));
        let mut view = SettingsView::new();
        view.selected = themes.len() - 1;
        view.handle_key(&press("r", Some("r")), &themes);
        view.handle_key(&press("backspace", None), &themes);
        view.handle_key(&press("x", Some("x")), &themes);
        let command = view.handle_key(&press("return", None), &themes);
        assert_eq!(command, Command::Rename { old: "Mine".into(), new: "Minx".into() });
        view.handle_key(&press("r", Some("r")), &themes);
        assert_eq!(view.handle_key(&press("escape", None), &themes), Command::None);
        assert_eq!(view.mode, Mode::Browse);
    }

    #[test]
    fn render_fits_the_requested_rows_and_marks_the_active_theme() {
        let themes = builtin_themes();
        let view = SettingsView::new();
        let config = TerminalConfig { theme: themes[0].name.clone(), ..TerminalConfig::default() };
        let rows = view.render(60, 12, &themes, &themes[0].palette, &config);
        assert_eq!(rows.len(), 12);
        assert!(rows[LIST_TOP][0].text.contains('●'));
        assert!(rows.iter().all(|row| row.iter().all(|run| run.col + run.cells <= 60)));
    }
}

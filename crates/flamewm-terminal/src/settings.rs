//! Settings view drawn inside the terminal. Keyboard driven: Up/Down choose a theme, Enter applies it, C copies it,
//! E edits a user theme's colours (hex input with live preview), R renames, D deletes. Built-in themes are read-only.
//! `handle_key` returns commands; the app performs the file and config writes.
use flamewm_ui_x11::UiKeyEvent;

use crate::store::FOLLOW_FLAMEWM;
use crate::themes::{Palette, Theme, format_hex, parse_hex};
use crate::view::{Run, text_run};

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
    FontSize(i32),
    CycleCursor,
}

#[derive(Clone, Debug)]
pub struct SettingsView {
    pub selected: usize,
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
        Self { selected: 0, slot: 0, mode: Mode::Browse, edit: None, message: String::new() }
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
            "up" => self.selected = self.selected.saturating_sub(1),
            "down" => self.selected = (self.selected + 1).min(themes.len().saturating_sub(1)),
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
            "plus" | "=" => return Command::FontSize(1),
            "minus" | "-" => return Command::FontSize(-1),
            "k" => return Command::CycleCursor,
            _ => {}
        }
        Command::None
    }

    /// Settings rows: header, theme list, edit or rename area, status line. `list_rows` limits the list height.
    pub fn render(&self, cols: usize, rows: usize, themes: &[Theme], active: &str, palette: &Palette, config_line: &str) -> Vec<Vec<Run>> {
        let fg = palette.foreground;
        let muted = mix(fg, palette.background, 60);
        let mut out: Vec<Vec<Run>> = Vec::new();
        let line = |text: String, colour: u32, bg: Option<u32>, bold: bool| vec![text_run(0, &clip(&text, cols), colour, bg, bold)];
        out.push(line("Settings  ·  Enter apply  C copy  E edit  R rename  D delete  F follow FlameWM  Esc close".into(), fg, None, true));
        out.push(line(config_line.to_owned(), muted, None, false));
        out.push(line(String::new(), fg, None, false));

        let reserved = 3 + if self.edit.is_some() { 22 } else { 0 } + 2;
        let list_rows = rows.saturating_sub(reserved).max(1);
        let top = self.selected.saturating_sub(list_rows.saturating_sub(1));
        for (index, theme) in themes.iter().enumerate().skip(top).take(list_rows) {
            let marker = if theme.name == active { "●" } else { " " };
            let origin = if theme.builtin { "built-in" } else { "custom" };
            let text = format!(" {marker} {:<26} {origin}", theme.name);
            let selected = index == self.selected;
            let (colour, bg) = if selected { (fg, Some(palette.selection)) } else { (fg, None) };
            out.push(line(text, colour, bg, selected));
        }
        if let Some(theme) = &self.edit {
            out.push(line(format!(" Editing {}", theme.name), fg, None, true));
            for (index, (slot, value)) in theme.palette.slots().into_iter().enumerate() {
                let selected = index == self.slot;
                let text = format!("   {:<16} {}", slot, format_hex(value));
                out.push(line(text, fg, if selected { Some(palette.selection) } else { None }, selected));
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
        let rows = view.render(60, 12, &themes, &themes[0].name, &themes[0].palette, "font 13");
        assert_eq!(rows.len(), 12);
        assert!(rows[3][0].text.contains('●'));
        assert!(rows.iter().all(|row| row.iter().all(|run| run.col + run.cells <= 60)));
    }
}

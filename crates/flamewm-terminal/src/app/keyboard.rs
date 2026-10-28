//! Keyboard input: shortcuts, inline rename, the typed-line tracker and key encoding for the focused child.

use super::{App, View};
use crate::keys;
use crate::keys::Mods;
use crate::keys::Shortcut;
use flamewm_ui_x11::UiKeyEvent;
use flamewm_ui_x11::UiKeyPhase;

impl App {
    pub(super) fn app_cursor(&self) -> bool {
        self.focused_pane()
            .is_some_and(|pane| pane.session.screen.app_cursor)
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
            let text = event
                .text
                .as_deref()
                .filter(|t| !t.is_empty() && !t.chars().any(char::is_control));
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
        let enter = matches!(
            event.key.to_ascii_lowercase().as_str(),
            "return" | "enter" | "kp_enter"
        );
        let Some(pane) = self.focused_pane() else {
            return false;
        };
        enter && pane.input_line == "/theme" && !pane.session.screen.alt_active()
    }

    /// Follows the text sent to the shell. Editing keys the line cannot follow (arrows, Tab, Ctrl) clear it.
    fn track_input_line(&mut self, event: &UiKeyEvent) {
        let mods = Mods::from_names(event.modifiers.iter());
        let Some(pane) = self.focused_pane_mut() else {
            return;
        };
        match event.key.to_ascii_lowercase().as_str() {
            "backspace" => {
                pane.input_line.pop();
            }
            "return" | "enter" | "kp_enter" | "escape" => pane.input_line.clear(),
            _ => match event.text.as_deref() {
                Some(text) if !mods.ctrl && !mods.alt && !text.chars().any(char::is_control) => {
                    pane.input_line.push_str(text);
                }
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
        if !mods.ctrl
            && !mods.alt
            && !mods.shift
            && matches!(name.as_str(), "right" | "tab")
            && self.at_prompt()
        {
            if let Some(suffix) = self.ghost().map(str::to_owned) {
                if let Some(pane) = self.focused_pane_mut() {
                    pane.input.insert_str(&suffix);
                }
                return suffix.into_bytes();
            }
        }
        let at_prompt = self.at_prompt();
        let Some(pane) = self.focused_pane_mut() else {
            return bytes;
        };
        if let Some(line) = pane.input.key(&name, mods, event.text.as_deref()) {
            if at_prompt {
                let _ = self.history.record(&line);
            }
        }
        bytes
    }

    /// The shell reads keys at its prompt: no full-screen program, the line discipline is not canonical, and the
    /// terminal does not echo. A password prompt (canonical, no echo) and a program that reads keys with echo on are
    /// both excluded, so what is typed there is never recorded.
    pub(super) fn at_prompt(&self) -> bool {
        let Some(pane) = self.focused_pane() else {
            return false;
        };
        #[cfg(test)]
        if let Some(prompt) = pane.prompt {
            return prompt;
        }
        !pane.session.screen.alt_active()
            && !pane.session.pty.canonical()
            && !pane.session.pty.echoing()
    }

    fn shortcut(&mut self, shortcut: Shortcut) {
        match shortcut {
            Shortcut::Copy => self.copy_to_clipboard(),
            Shortcut::Paste => self.paste(),
            Shortcut::NewTab => {
                let _ = self.open_tab_here();
            }
            Shortcut::CloseTab => self.close_tab(self.active),
            Shortcut::NextTab => self.step_tab(1),
            Shortcut::PreviousTab => self.step_tab(self.tabs.len().saturating_sub(1)),
            Shortcut::Settings => self.toggle_settings(),
            Shortcut::FontLarger => self.change_font_size(1),
            Shortcut::FontSmaller => self.change_font_size(-1),
        }
    }

    fn step_tab(&mut self, step: usize) {
        if !self.tabs.is_empty() {
            self.active = (self.active + step) % self.tabs.len();
        }
    }
}

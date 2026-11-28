//! Settings view state and the commands that persist theme, font and cursor changes.

use super::{App, View};
use crate::settings::{self, Command, Mode, SettingsView};
use crate::store::FOLLOW_FLAMEWM;
use crate::store::TerminalConfig;
use crate::theme;
use std::path::Path;

/// Reloads the saved configuration. Corrupt bytes are quarantined by `store::load_config` with one
/// notice and the terminal runs on defaults, the same as at startup.
fn saved_config(path: &Path) -> TerminalConfig {
    crate::store::load_config(path)
}

impl App {
    pub fn toggle_settings(&mut self) {
        if matches!(self.view, View::Settings(_)) {
            self.close_settings();
        } else {
            self.open_settings();
        }
    }

    pub(super) fn open_settings(&mut self) {
        self.view = View::Settings(SettingsView::open(
            &self.themes,
            &self.config,
            settings::mono_fonts(),
        ));
    }

    /// Leaves settings. Unsaved previews are dropped: the saved configuration is reloaded from disk.
    pub(super) fn close_settings(&mut self) {
        self.config = saved_config(&self.config_path);
        self.refresh_metrics();
        let (width, height) = self.window;
        self.set_window(width, height);
        self.reload_themes();
        self.view = View::Terminal;
    }

    /// Font size up or down, applied now. Grid and PTY size follow the window, which keeps its pixel size.
    pub(super) fn change_font_size(&mut self, delta: i32) {
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
            Command::PreviewTheme(name) => {
                self.palette = theme::resolve_palette(&name, &self.themes, self.snapshot.as_ref());
            }
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
                                settings.mode = Mode::Rename(copy.name.clone());
                                settings.message =
                                    "Copied. Type a new name and press Enter.".into();
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
            Command::Rename { old, new } => match self.store.rename(&old, &new) {
                Ok(()) => {
                    if self.config.theme == old {
                        self.config.theme.clone_from(&new);
                        self.persist_config();
                    }
                    self.reload_themes();
                    if let View::Settings(settings) = &mut self.view {
                        settings.focus(&self.themes, &new);
                    }
                    self.settings_message(format!("renamed to {new}"));
                }
                Err(error) => self.settings_message(format!("rename failed: {error}")),
            },
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
}

#[cfg(test)]
mod corrupt_file_tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        std::env::temp_dir().join(format!(
            "flamewm-terminal-prefs-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[test]
    fn corrupt_saved_config_is_quarantined_and_defaults_used() {
        let dir = temp_dir("corrupt");
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("terminal.toml");
        let bad = b"this line has no equals sign\n".to_vec();
        fs::write(&path, &bad).expect("write bad config");

        assert_eq!(saved_config(&path), TerminalConfig::default());
        assert!(!path.exists());
        let copies: Vec<PathBuf> = fs::read_dir(&dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("terminal.corrupt-"))
            })
            .collect();
        assert_eq!(copies.len(), 1);
        assert_eq!(fs::read(&copies[0]).expect("read copy"), bad);
        fs::remove_dir_all(&dir).ok();
    }
}

//! X11 window rules and remembered geometry: file load, inotify hot reload, debounced atomic save.
//!
//! Matching and the geometry memory policy live in `flamewm_window_core`. This owner only touches
//! files and the reactor. Both files sit beside the settings file.

use std::cell::RefCell;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::geometry::Rect;
use flamewm_window_core::geometry_memory::{GeometryMemory, PersistDebounce};
use flamewm_window_core::window_rules::{WindowRule, match_rule, parse_rules};
use inotify::{Inotify, WatchMask};

use crate::atoms::AnyError;

pub const RULES_FILE: &str = "window-rules.conf";
pub const GEOMETRY_FILE: &str = "window-geometry.conf";

/// Shared between the WM loop (rules, memory) and the inotify callback (reload).
pub type SharedWindowConfig = Rc<RefCell<WindowConfig>>;

#[derive(Default)]
pub struct WindowConfig {
    dir: Option<PathBuf>,
    rules: Vec<WindowRule>,
    geometry: GeometryMemory,
    persist: PersistDebounce,
}

impl WindowConfig {
    /// Loads both files from `dir`. Without a directory the config stays empty and never writes.
    pub fn load(dir: Option<PathBuf>) -> Self {
        let mut config = Self {
            dir,
            ..Self::default()
        };
        config.reload_rules();
        if let Some(text) = config
            .dir
            .as_deref()
            .and_then(|dir| std::fs::read_to_string(dir.join(GEOMETRY_FILE)).ok())
        {
            config.geometry = GeometryMemory::parse(&text);
        }
        config
    }

    #[must_use]
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Reparses the rules file. A missing file clears the rules; a malformed file keeps the
    /// previous rules and reports why.
    pub fn reload_rules(&mut self) {
        let Some(dir) = self.dir.as_deref() else {
            return;
        };
        match std::fs::read_to_string(dir.join(RULES_FILE)) {
            Ok(text) => match parse_rules(&text) {
                Ok(rules) => self.rules = rules,
                Err(error) => eprintln!("flamewm: {RULES_FILE} not applied: {error}"),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => self.rules.clear(),
            Err(error) => eprintln!("flamewm: {RULES_FILE} unreadable: {error}"),
        }
    }

    #[must_use]
    pub fn rule_for(&self, app_id: &str) -> Option<WindowRule> {
        match_rule(&self.rules, app_id).cloned()
    }

    #[must_use]
    pub fn recall(&self, app_id: &str) -> Option<Rect> {
        self.geometry.recall(app_id).map(to_rect)
    }

    pub fn remember(&mut self, app_id: &str, rect: Rect, now: Instant) {
        if self.geometry.remember(app_id, from_rect(rect)) {
            self.persist.note_change(now);
        }
    }

    #[must_use]
    pub fn cap_wait(&self, now: Instant, wait: Option<Duration>) -> Option<Duration> {
        self.persist.cap_wait(now, wait)
    }

    /// Writes the remembered geometry once the debounce quiet period has elapsed.
    pub fn flush_due(&mut self, now: Instant) {
        if !self.persist.take_due(now) {
            return;
        }
        let Some(dir) = self.dir.as_deref() else {
            return;
        };
        if let Err(error) = write_atomic(&dir.join(GEOMETRY_FILE), &self.geometry.serialize()) {
            eprintln!("flamewm: {GEOMETRY_FILE} not saved: {error}");
        }
    }
}

/// Keeps a remembered frame rect inside `bounds` (the work area), so a monitor change cannot strand it.
pub fn fit_to(rect: Rect, bounds: Rect) -> Rect {
    to_rect(flamewm_window_core::keep_visible(
        from_rect(rect),
        from_rect(bounds),
    ))
}

fn to_rect(rect: flamewm_api::Rect) -> Rect {
    Rect::new(
        rect.x,
        rect.y,
        rect.width.unsigned_abs(),
        rect.height.unsigned_abs(),
    )
}

fn from_rect(rect: Rect) -> flamewm_api::Rect {
    flamewm_api::Rect::new(
        rect.x,
        rect.y,
        i32::try_from(rect.width).unwrap_or(i32::MAX),
        i32::try_from(rect.height).unwrap_or(i32::MAX),
    )
}

/// Temp file plus rename, so a crash never leaves a half-written memory file.
fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("conf.tmp");
    let mut file = std::fs::File::create(&temp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temp, path)
}

/// Registers an inotify watch on the config directory. Only changes to the rules file reload it,
/// so the WM's own geometry writes never trigger a reload.
pub fn watch_rules(
    reactor: &RefCell<flamewm_reactor::Reactor>,
    config: SharedWindowConfig,
) -> Result<(), AnyError> {
    let Some(dir) = config.borrow().dir().map(Path::to_path_buf) else {
        return Ok(());
    };
    std::fs::create_dir_all(&dir)?;
    let inotify = Inotify::init()?;
    inotify.watches().add(
        &dir,
        WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO | WatchMask::CREATE | WatchMask::DELETE,
    )?;
    reactor.borrow_mut().register_fd_with_source_action(
        inotify,
        calloop::Interest::READ,
        move |_, source, _| {
            let mut buffer = [0_u8; 4096];
            let mut touched = false;
            loop {
                let events = match source.read_events(&mut buffer) {
                    Ok(events) => events,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(_) => return flamewm_reactor::FdAction::Remove,
                };
                let mut any = false;
                for event in events {
                    any = true;
                    touched |= event.name.is_some_and(|name| name == RULES_FILE);
                }
                if !any {
                    break;
                }
            }
            if touched {
                config.borrow_mut().reload_rules();
            }
            flamewm_reactor::FdAction::Continue
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "flamewm-window-config-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn rules_reload_keeps_previous_rules_on_error() {
        let dir = scratch("reload");
        std::fs::write(dir.join(RULES_FILE), "app = xterm\nworkspace = 2\n").expect("write");
        let mut config = WindowConfig::load(Some(dir.clone()));
        assert_eq!(config.rule_for("xterm").and_then(|r| r.workspace), Some(2));

        std::fs::write(dir.join(RULES_FILE), "app = xterm\nworkspace = 0\n").expect("write");
        config.reload_rules();
        assert_eq!(config.rule_for("xterm").and_then(|r| r.workspace), Some(2));

        std::fs::remove_file(dir.join(RULES_FILE)).expect("remove");
        config.reload_rules();
        assert!(config.rule_for("xterm").is_none());
    }

    #[test]
    fn geometry_is_remembered_debounced_and_persisted_atomically() {
        let dir = scratch("geometry");
        let start = Instant::now();
        let mut config = WindowConfig::load(Some(dir.clone()));
        config.remember("org.app", Rect::new(1, 2, 300, 200), start);
        assert_eq!(config.recall("org.app"), Some(Rect::new(1, 2, 300, 200)));
        assert_eq!(config.recall("missing"), None);

        config.flush_due(start);
        assert!(
            !dir.join(GEOMETRY_FILE).exists(),
            "write waits for the quiet period"
        );
        config.flush_due(start + flamewm_window_core::geometry_memory::PERSIST_DELAY);
        assert!(dir.join(GEOMETRY_FILE).exists());
        assert!(!dir.join("window-geometry.conf.tmp").exists());

        let reloaded = WindowConfig::load(Some(dir));
        assert_eq!(reloaded.recall("org.app"), Some(Rect::new(1, 2, 300, 200)));
        assert_eq!(reloaded.recall("missing"), None);
    }
}

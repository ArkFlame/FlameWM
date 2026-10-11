//! Icon and cursor theme picker: the live catalog registry, off-thread scan and
//! install jobs. Filesystem scans and previews never run on the UI thread.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use flamewm_integrations_linux::IconLookupIndex;
use flamewm_integrations_linux::PortalFileChooser;
use flamewm_integrations_linux::icons::{IconSize, Rgba8Raster, rasterize_path};
use flamewm_integrations_linux::themes::cursor::{self, CursorPreview};
use flamewm_integrations_linux::themes::import::{
    ImportError, InstallOptions, InstalledTheme, install_theme,
};
use flamewm_integrations_linux::themes::{ThemeCatalog, ThemeKind};
use flamewm_ui_x11::RuntimeImage;

use crate::file_chooser::publish_background_completion;

/// Theme rows per picker page.
pub const PICKER_SLOTS: usize = 6;
const ICON_PREVIEW_NAME: &str = "folder";
const ICON_PREVIEW_PX: u32 = 24;
const SYSTEM_DATA_DIRS: &str = "/usr/local/share:/usr/share";

/// Pre-registry icon values, kept so stored settings keep their label.
pub const LEGACY_ICON_THEMES: [(&str, &str); 3] = [
    ("*:-HighContrast", "FlameWM Breeze"),
    ("breeze-dark", "Breeze Dark"),
    ("HighContrast", "High Contrast"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKind {
    Icon,
    Cursor,
}

impl PickerKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "icon" => Some(Self::Icon),
            "cursor" => Some(Self::Cursor),
            _ => None,
        }
    }

    /// The persisted setting this picker writes.
    pub fn setting_key(self) -> &'static str {
        match self {
            Self::Icon => "iconTheme",
            Self::Cursor => "cursorTheme",
        }
    }

    /// Markup id of the select box that shows the current value.
    pub fn select_box(self) -> &'static str {
        match self {
            Self::Icon => "icon-theme-value",
            Self::Cursor => "cursor-theme-value",
        }
    }

    /// Markup id stem shared by the picker's container, rows, previews and pager.
    pub fn node(self) -> &'static str {
        match self {
            Self::Icon => "theme-icon",
            Self::Cursor => "theme-cursor",
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::Icon => "icon",
            Self::Cursor => "cursor",
        }
    }

}

#[derive(Clone, Debug)]
pub struct ThemeItem {
    /// Stable theme directory name; this is what the setting stores.
    pub id: String,
    pub name: String,
    pub preview: Option<RuntimeImage>,
}

#[derive(Clone, Debug, Default)]
pub struct ThemeRegistry {
    pub icon: Vec<ThemeItem>,
    pub cursor: Vec<ThemeItem>,
}

impl ThemeRegistry {
    pub fn items(&self, kind: PickerKind) -> &[ThemeItem] {
        match kind {
            PickerKind::Icon => &self.icon,
            PickerKind::Cursor => &self.cursor,
        }
    }

    /// Installed name for `value`, then the legacy icon label, else the value verbatim.
    pub fn label(&self, kind: PickerKind, value: &str) -> String {
        if value.is_empty() {
            return "Default".to_owned();
        }
        if let Some(item) = self.items(kind).iter().find(|item| item.id == value) {
            return item.name.clone();
        }
        if kind == PickerKind::Icon {
            if let Some((_, label)) = LEGACY_ICON_THEMES.iter().find(|(id, _)| *id == value) {
                return (*label).to_owned();
            }
        }
        value.to_owned()
    }
}

/// Scans the catalog and loads one preview per valid theme. Runs off the UI thread.
pub fn scan(data_dirs: &[PathBuf]) -> ThemeRegistry {
    let catalog = ThemeCatalog::discover(data_dirs);
    let mut registry = ThemeRegistry::default();
    for entry in catalog.entries().iter().filter(|entry| entry.valid) {
        match entry.kind {
            ThemeKind::Icon => registry.icon.push(ThemeItem {
                id: entry.id.clone(),
                name: entry.name.clone(),
                preview: icon_preview(&entry.id, data_dirs),
            }),
            ThemeKind::Cursor => registry.cursor.push(ThemeItem {
                id: entry.id.clone(),
                name: entry.name.clone(),
                preview: cursor::load_left_ptr_preview(&entry.root)
                    .ok()
                    .map(cursor_image),
            }),
        }
    }
    registry
}

fn icon_preview(theme: &str, data_dirs: &[PathBuf]) -> Option<RuntimeImage> {
    let index = IconLookupIndex::build(&[theme.to_owned()], data_dirs, 0);
    let (candidates, _) = index.lookup(ICON_PREVIEW_NAME, ICON_PREVIEW_PX);
    let size = IconSize::new(ICON_PREVIEW_PX, ICON_PREVIEW_PX);
    candidates
        .iter()
        .find_map(|path| rasterize_path(path, size).ok())
        .map(raster_image)
}

fn raster_image(raster: Rgba8Raster) -> RuntimeImage {
    RuntimeImage {
        source: raster.source,
        width: raster.width,
        height: raster.height,
        pixels: raster.pixels,
    }
}

fn cursor_image(preview: CursorPreview) -> RuntimeImage {
    RuntimeImage {
        source: "left_ptr".to_owned(),
        width: preview.width,
        height: preview.height,
        pixels: preview.rgba,
    }
}

/// `$XDG_DATA_HOME`, or `$HOME/.local/share`. Installs go here, never to a system prefix.
pub fn user_data_home(home: Option<&str>, xdg_data_home: Option<&str>) -> Option<PathBuf> {
    non_empty(xdg_data_home)
        .map(PathBuf::from)
        .or_else(|| non_empty(home).map(|home| PathBuf::from(home).join(".local/share")))
}

/// Search order for the catalog: the user data home first, then `XDG_DATA_DIRS`.
pub fn theme_data_dirs(
    home: Option<&str>,
    xdg_data_home: Option<&str>,
    xdg_data_dirs: Option<&str>,
) -> Vec<PathBuf> {
    let system = non_empty(xdg_data_dirs).unwrap_or(SYSTEM_DATA_DIRS);
    user_data_home(home, xdg_data_home)
        .into_iter()
        .chain(system.split(':').filter(|dir| !dir.is_empty()).map(PathBuf::from))
        .collect()
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.is_empty())
}

fn env_var(name: &str) -> Option<String> {
    env::var(name).ok()
}

pub fn theme_data_dirs_from_env() -> Vec<PathBuf> {
    theme_data_dirs(
        env_var("HOME").as_deref(),
        env_var("XDG_DATA_HOME").as_deref(),
        env_var("XDG_DATA_DIRS").as_deref(),
    )
}

pub fn user_icons_dir_from_env() -> Option<PathBuf> {
    user_data_home(env_var("HOME").as_deref(), env_var("XDG_DATA_HOME").as_deref())
        .map(|home| home.join("icons"))
}

/// Starts one catalog scan. The result wakes the Settings reactor when it is ready.
pub fn start_scan(data_dirs: Vec<PathBuf>) -> Result<Receiver<ThemeRegistry>, String> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("flamewm-settings-theme-scan".to_owned())
        .spawn(move || publish_background_completion(&sender, scan(&data_dirs)))
        .map_err(|error| format!("could not start theme scan: {error}"))?;
    Ok(receiver)
}

/// Opens the portal source chooser: one theme archive, or one folder when `folder` is set.
/// `Ok(None)` means the user cancelled.
pub fn choose_theme_source(folder: bool) -> Result<Option<PathBuf>, String> {
    PortalFileChooser::connect()
        .and_then(|chooser| chooser.choose_theme_source("", folder))
        .map_err(|error| error.to_string())
}

pub type InstallOutcome = Result<Option<InstalledTheme>, String>;

/// Starts one install job: chooser, then import into `icons_dir`. `Ok(None)` means cancelled.
pub fn start_install(icons_dir: PathBuf, folder: bool) -> Result<Receiver<InstallOutcome>, String> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("flamewm-settings-theme-install".to_owned())
        .spawn(move || {
            let outcome = choose_theme_source(folder).and_then(|source| match source {
                None => Ok(None),
                Some(source) => install_with(install_theme, &source, &icons_dir).map(Some),
            });
            publish_background_completion(&sender, outcome);
        })
        .map_err(|error| format!("could not start theme install: {error}"))?;
    Ok(receiver)
}

/// Runs one import with fresh options (no overwrite) and maps its error to user text.
pub fn install_with<F>(install: F, source: &Path, icons_dir: &Path) -> Result<InstalledTheme, String>
where
    F: FnOnce(&Path, &Path, &InstallOptions, &AtomicBool) -> Result<InstalledTheme, ImportError>,
{
    let cancel = AtomicBool::new(false);
    install(source, icons_dir, &InstallOptions::default(), &cancel)
        .map_err(|error| install_error_message(&error))
}

pub fn install_error_message(error: &ImportError) -> String {
    match error {
        ImportError::UnsupportedFormat(name) => format!(
            "unsupported theme source '{name}': choose a .tar.gz, .tgz, .tar.xz or .txz archive, or a folder"
        ),
        ImportError::IdCollision(id) => {
            format!("theme '{id}' is already installed; remove it first to replace it")
        }
        ImportError::MissingIndex => "theme has no index.theme".to_owned(),
        ImportError::AmbiguousThemes(ids) => {
            format!("archive holds several themes ({}); keep one per archive", ids.join(", "))
        }
        ImportError::Cancelled => "theme install cancelled".to_owned(),
        other => format!("theme install failed: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn item(id: &str, name: &str) -> ThemeItem {
        ThemeItem {
            id: id.to_owned(),
            name: name.to_owned(),
            preview: None,
        }
    }

    #[test]
    fn stored_legacy_icon_values_keep_their_label_and_installed_names_win() {
        let mut registry = ThemeRegistry::default();
        assert_eq!(registry.label(PickerKind::Icon, "*:-HighContrast"), "FlameWM Breeze");
        assert_eq!(registry.label(PickerKind::Icon, "HighContrast"), "High Contrast");
        assert_eq!(registry.label(PickerKind::Icon, "custom"), "custom");
        assert_eq!(registry.label(PickerKind::Cursor, ""), "Default");
        registry.icon.push(item("breeze-dark", "Installed Breeze"));
        assert_eq!(registry.label(PickerKind::Icon, "breeze-dark"), "Installed Breeze");
        assert_eq!(registry.label(PickerKind::Icon, "*:-HighContrast"), "FlameWM Breeze");
    }

    #[test]
    fn install_dir_is_user_data_only_and_search_puts_user_home_first() {
        assert_eq!(
            user_data_home(Some("/home/u"), None),
            Some(PathBuf::from("/home/u/.local/share"))
        );
        assert_eq!(
            user_data_home(Some("/home/u"), Some("/xdg")),
            Some(PathBuf::from("/xdg"))
        );
        assert_eq!(user_data_home(None, Some("")), None, "never install into a system prefix");
        assert_eq!(
            theme_data_dirs(Some("/home/u"), None, Some("/opt/share:")),
            vec![PathBuf::from("/home/u/.local/share"), PathBuf::from("/opt/share")]
        );
    }

    #[test]
    fn install_error_mapping_uses_a_fake_installer() {
        let unsupported = install_with(
            |_, _, _, _| Err(ImportError::UnsupportedFormat("theme.tar.xz".to_owned())),
            Path::new("/src/theme.tar.xz"),
            Path::new("/icons"),
        );
        let message = unsupported.expect_err("xz must be rejected");
        assert!(message.contains("theme.tar.xz") && message.contains(".txz"), "{message}");

        let collision = install_with(
            |_, _, _, _| Err(ImportError::IdCollision("Papirus".to_owned())),
            Path::new("/src/Papirus"),
            Path::new("/icons"),
        );
        assert!(collision.expect_err("collision").contains("already installed"));

        let calls = Cell::new(0);
        let installed = install_with(
            |source, icons, options, _| {
                calls.set(calls.get() + 1);
                assert!(!options.overwrite, "a fresh install never overwrites");
                assert_eq!(source, Path::new("/src/ok.tgz"));
                assert_eq!(icons, Path::new("/icons"));
                Ok(InstalledTheme {
                    id: "ok".to_owned(),
                    kind: ThemeKind::Cursor,
                    root: PathBuf::from("/icons/ok"),
                })
            },
            Path::new("/src/ok.tgz"),
            Path::new("/icons"),
        )
        .expect("fake installer succeeds");
        assert_eq!(calls.get(), 1);
        assert_eq!(installed.id, "ok");
    }

    #[test]
    fn cursor_preview_maps_to_straight_rgba_image() {
        let image = cursor_image(CursorPreview {
            nominal_size: 24,
            width: 2,
            height: 1,
            hotspot: (0, 0),
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 0],
        });
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.pixels, vec![1, 2, 3, 255, 4, 5, 6, 0]);
    }
}

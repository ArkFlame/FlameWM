//! Icon and cursor theme picker: the live catalog registry, off-thread scan and
//! install jobs. Filesystem scans and previews never run on the UI thread.

use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use flamewm_integrations_linux::IconLookupIndex;
use flamewm_integrations_linux::PortalFileChooser;
use flamewm_integrations_linux::icons::{self, IconSize, Rgba8Raster, rasterize_path};
use flamewm_integrations_linux::themes::cursor::{self, CursorPreview};
use flamewm_integrations_linux::themes::import::{
    ImportError, InstallOptions, InstalledTheme, install_theme,
};
use flamewm_integrations_linux::themes::{ThemeCatalog, ThemeKind};
use flamewm_session_core::{CursorThemeFamily, cursor_theme_family};

use crate::file_chooser::publish_background_completion;

/// Theme rows per picker page.
pub const PICKER_SLOTS: usize = 6;
const ICON_PREVIEW_NAME: &str = "folder";
const ICON_PREVIEW_PX: u32 = 24;
/// Agent 1's selected-value image node, to the left of icon-theme-value text.
pub const SELECTED_ICON_PREVIEW_NODE: &str = "icon-theme-selected-preview";

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
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "icon" => Some(Self::Icon),
            "cursor" => Some(Self::Cursor),
            _ => None,
        }
    }

    /// The persisted setting this picker writes.
    #[must_use]
    pub fn setting_key(self) -> &'static str {
        match self {
            Self::Icon => "iconTheme",
            Self::Cursor => "cursorTheme",
        }
    }

    /// Markup id of the select box that shows the current value.
    #[must_use]
    pub fn select_box(self) -> &'static str {
        match self {
            Self::Icon => "icon-theme-value",
            Self::Cursor => "cursor-theme-value",
        }
    }

    /// Markup id stem shared by the picker's container, rows, previews and pager.
    #[must_use]
    pub fn node(self) -> &'static str {
        match self {
            Self::Icon => "theme-icon",
            Self::Cursor => "theme-cursor",
        }
    }

    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Icon => "icon",
            Self::Cursor => "cursor",
        }
    }
}

/// A rasterized thumbnail. Pixels are shared so each document refresh hands
/// the UI a handle instead of copying the buffer.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewImage {
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct ThemeItem {
    /// Stable theme directory name; this is what the setting stores.
    pub id: String,
    pub name: String,
    pub preview: Option<PreviewImage>,
}

#[derive(Clone, Debug, Default)]
pub struct ThemeRegistry {
    pub icon: Vec<ThemeItem>,
    pub cursor: Vec<ThemeItem>,
}

impl ThemeRegistry {
    #[must_use]
    pub fn items(&self, kind: PickerKind) -> &[ThemeItem] {
        match kind {
            PickerKind::Icon => &self.icon,
            PickerKind::Cursor => &self.cursor,
        }
    }

    /// Borrow the already-rasterized thumbnail for the selected directory id.
    /// An unset icon theme uses `FlameWM`'s `PlagueSur` default; absent or legacy
    /// themes return `None` so the UI can show its static fallback placeholder.
    /// Replacing this registry after an off-thread scan also replaces the cache;
    /// changes to the setting require only a new lookup, never a disk read.
    #[must_use]
    pub fn selected_preview(&self, kind: PickerKind, value: &str) -> Option<&PreviewImage> {
        let id = if value.is_empty() && kind == PickerKind::Icon {
            BUNDLED_ICON_THEMES[0]
        } else {
            value
        };
        self.items(kind)
            .iter()
            .find(|item| item.id == id)
            .and_then(|item| item.preview.as_ref())
    }

    /// Installed name for `value`, then the legacy icon label, else the value verbatim.
    #[must_use]
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

/// `FlameWM`'s bundled icon themes stay at the top of the picker.
pub const BUNDLED_ICON_THEMES: [&str; 2] = ["PlagueSur", "Fluent"];

/// Upstream Fluent Yellow directories can use a different id; retain that
/// filesystem id for icon lookup while presenting the product name as Fluent.
fn fluent_yellow(item: &ThemeItem) -> bool {
    let name = item.name.to_ascii_lowercase();
    let id = item.id.to_ascii_lowercase();
    id == "fluent"
        || name == "fluent"
        || ((id.contains("fluent") || name.contains("fluent"))
            && (id.contains("yellow") || name.contains("yellow")))
}

/// Pin bundled defaults first, then keep every discoverable installed theme.
#[must_use]
pub fn with_bundled_icon_themes(mut icon: Vec<ThemeItem>) -> Vec<ThemeItem> {
    let mut chosen = Vec::with_capacity(BUNDLED_ICON_THEMES.len());
    if let Some(mut plague) = icon.iter().find(|item| item.id == "PlagueSur").cloned() {
        "PlagueSur".clone_into(&mut plague.name);
        chosen.push(plague);
    } else {
        chosen.push(ThemeItem {
            id: "PlagueSur".to_owned(),
            name: "PlagueSur".to_owned(),
            preview: None,
        });
    }
    if let Some(mut fluent) = icon.iter().find(|item| fluent_yellow(item)).cloned() {
        "Fluent".clone_into(&mut fluent.name);
        chosen.push(fluent);
    } else {
        chosen.push(ThemeItem {
            id: "Fluent".to_owned(),
            name: "Fluent".to_owned(),
            preview: None,
        });
    }
    icon.retain(|item| !chosen.iter().any(|pinned| pinned.id == item.id));
    icon.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.id.to_ascii_lowercase().cmp(&b.id.to_ascii_lowercase()))
    });
    chosen.extend(icon);
    chosen
}

/// Every installed cursor theme with a decodable pointer preview is selectable.
#[must_use]
pub fn with_installed_cursor_themes(cursor: Vec<ThemeItem>) -> Vec<ThemeItem> {
    let mut themes: Vec<ThemeItem> = cursor
        .into_iter()
        .filter_map(|mut item| {
            // An unrenderable or absent left_ptr must not appear as a selectable
            // cursor with a misleading thumbnail.
            item.preview.as_ref()?;
            if cursor_theme_family(&item.id) == Some(CursorThemeFamily::McMojave)
                && item.id.eq_ignore_ascii_case("McMojave-cursors")
            {
                "McMojave".clone_into(&mut item.name);
            }
            Some(item)
        })
        .collect();
    themes.sort_by(|a, b| {
        let order = |item: &ThemeItem| match cursor_theme_family(&item.id) {
            Some(CursorThemeFamily::McMojave) => 0,
            Some(CursorThemeFamily::Oxygen) => 1,
            None => 2,
        };
        order(a)
            .cmp(&order(b))
            .then_with(|| a.id.to_ascii_lowercase().cmp(&b.id.to_ascii_lowercase()))
    });
    themes
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
            ThemeKind::Cursor => {
                registry.cursor.push(ThemeItem {
                    id: entry.id.clone(),
                    name: entry.name.clone(),
                    preview: cursor::load_left_ptr_preview(&entry.root)
                        .ok()
                        .map(cursor_image),
                });
            }
        }
    }
    registry.icon = with_bundled_icon_themes(registry.icon);
    registry.cursor = with_installed_cursor_themes(registry.cursor);
    registry
}

fn icon_preview(theme: &str, data_dirs: &[PathBuf]) -> Option<PreviewImage> {
    let index = IconLookupIndex::build(&[theme.to_owned()], data_dirs, 0);
    let (candidates, _) = index.lookup(ICON_PREVIEW_NAME, ICON_PREVIEW_PX);
    let size = IconSize::new(ICON_PREVIEW_PX, ICON_PREVIEW_PX);
    candidates
        .iter()
        .find_map(|path| rasterize_path(path, size).ok())
        .map(raster_image)
}

fn raster_image(raster: Rgba8Raster) -> PreviewImage {
    PreviewImage {
        source: raster.source,
        width: raster.width,
        height: raster.height,
        pixels: Arc::new(raster.pixels),
    }
}

fn cursor_image(preview: CursorPreview) -> PreviewImage {
    PreviewImage {
        source: "left_ptr".to_owned(),
        width: preview.width,
        height: preview.height,
        pixels: Arc::new(preview.rgba),
    }
}

/// `$XDG_DATA_HOME`, or `$HOME/.local/share`. Installs go here, never to a system prefix.
pub fn user_data_home(home: Option<&str>, xdg_data_home: Option<&str>) -> Option<PathBuf> {
    icons::user_data_home(home.map(OsStr::new), xdg_data_home.map(OsStr::new))
}

/// Search order for the catalog: the user data home first, then `XDG_DATA_DIRS`.
pub fn theme_data_dirs(
    home: Option<&str>,
    xdg_data_home: Option<&str>,
    xdg_data_dirs: Option<&str>,
) -> Vec<PathBuf> {
    let mut roots = icons::xdg_data_search_dirs(
        home.map(OsStr::new),
        xdg_data_home.map(OsStr::new),
        xdg_data_dirs,
    );
    // XDG_DATA_DIRS may be replaced by a sandbox/session; packaged themes in the
    // standard system locations must remain visible in the installed-theme picker.
    for system in ["/usr/local/share", "/usr/share"] {
        let system = PathBuf::from(system);
        if !roots.contains(&system) {
            roots.push(system);
        }
    }
    roots
}

fn env_var(name: &str) -> Option<String> {
    env::var(name).ok()
}

#[must_use]
pub fn theme_data_dirs_from_env() -> Vec<PathBuf> {
    theme_data_dirs(
        env_var("HOME").as_deref(),
        env_var("XDG_DATA_HOME").as_deref(),
        env_var("XDG_DATA_DIRS").as_deref(),
    )
}

#[must_use]
pub fn user_icons_dir_from_env() -> Option<PathBuf> {
    user_data_home(
        env_var("HOME").as_deref(),
        env_var("XDG_DATA_HOME").as_deref(),
    )
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
pub fn install_with<F>(
    install: F,
    source: &Path,
    icons_dir: &Path,
) -> Result<InstalledTheme, String>
where
    F: FnOnce(&Path, &Path, &InstallOptions, &AtomicBool) -> Result<InstalledTheme, ImportError>,
{
    let cancel = AtomicBool::new(false);
    install(source, icons_dir, &InstallOptions::default(), &cancel)
        .map_err(|error| install_error_message(&error))
}

#[must_use]
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
            format!(
                "archive holds several themes ({}); keep one per archive",
                ids.join(", ")
            )
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

    fn one_pixel_xcursor() -> Vec<u8> {
        let mut bytes = b"Xcur".to_vec();
        for word in [
            16u32,
            0x0001_0000,
            1,
            0xfffd_0002,
            24,
            28,
            36,
            0xfffd_0002,
            24,
            1,
            1,
            1,
            0,
            0,
            0,
        ] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0, 0xff, 0xff]);
        bytes
    }

    #[test]
    fn stored_legacy_icon_values_keep_their_label_and_installed_names_win() {
        let mut registry = ThemeRegistry::default();
        assert_eq!(
            registry.label(PickerKind::Icon, "*:-HighContrast"),
            "FlameWM Breeze"
        );
        assert_eq!(
            registry.label(PickerKind::Icon, "HighContrast"),
            "High Contrast"
        );
        assert_eq!(registry.label(PickerKind::Icon, "custom"), "custom");
        assert_eq!(registry.label(PickerKind::Cursor, ""), "Default");
        registry.icon.push(item("breeze-dark", "Installed Breeze"));
        assert_eq!(
            registry.label(PickerKind::Icon, "breeze-dark"),
            "Installed Breeze"
        );
        assert_eq!(
            registry.label(PickerKind::Icon, "*:-HighContrast"),
            "FlameWM Breeze"
        );
    }

    #[test]
    fn icon_picker_lists_bundled_defaults_and_installed_system_themes() {
        let ids =
            |items: &[ThemeItem]| items.iter().map(|item| item.id.clone()).collect::<Vec<_>>();
        let names = |items: &[ThemeItem]| {
            items
                .iter()
                .map(|item| item.name.clone())
                .collect::<Vec<_>>()
        };
        let empty = with_bundled_icon_themes(Vec::new());
        assert_eq!(ids(&empty), ["PlagueSur", "Fluent"]);
        let found = with_bundled_icon_themes(vec![
            item("Papirus", "Papirus"),
            item("Fluent-Yellow", "Fluent Yellow"),
            item("PlagueSur", "Custom display name"),
            item("candy-icons", "Candy"),
        ]);
        assert_eq!(
            ids(&found),
            ["PlagueSur", "Fluent-Yellow", "candy-icons", "Papirus"]
        );
        assert_eq!(names(&found), ["PlagueSur", "Fluent", "Candy", "Papirus"]);
        assert_eq!(found.len(), 4);
        let only_cursor = item("Bibata", "Bibata");
        let mut registry = ThemeRegistry::default();
        registry.cursor.push(only_cursor);
        assert_eq!(registry.cursor[0].name, "Bibata");
    }

    #[test]
    fn scan_lists_and_previews_system_icon_and_cursor_themes() {
        let data = std::env::temp_dir().join(format!(
            "flamewm-settings-system-themes-{}",
            std::process::id()
        ));
        let theme = data.join("icons/SystemPack");
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(theme.join("16x16/apps")).unwrap();
        std::fs::create_dir_all(theme.join("cursors")).unwrap();
        std::fs::write(
            theme.join("index.theme"),
            "[Icon Theme]\nName=System Pack\nDirectories=16x16/apps\n[16x16/apps]\nSize=16\nType=Fixed\n",
        )
        .unwrap();
        std::fs::write(
            theme.join("16x16/apps/folder.ppm"),
            b"P6\n1 1\n255\n\xff\x00\x00",
        )
        .unwrap();
        std::fs::write(theme.join("cursors/left_ptr"), one_pixel_xcursor()).unwrap();

        let registry = scan(std::slice::from_ref(&data));
        assert!(
            registry
                .icon
                .iter()
                .any(|item| item.id == "SystemPack" && item.preview.is_some())
        );
        assert!(
            registry
                .cursor
                .iter()
                .any(|item| item.id == "SystemPack" && item.preview.is_some())
        );
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn selected_preview_borrows_cached_thumbnail_or_returns_placeholder() {
        let image = PreviewImage {
            source: "folder".to_owned(),
            width: 1,
            height: 1,
            pixels: Arc::new(vec![30, 40, 50, 255]),
        };
        let mut registry = ThemeRegistry {
            icon: vec![ThemeItem {
                preview: Some(image.clone()),
                ..item("PlagueSur", "PlagueSur")
            }],
            cursor: vec![],
        };
        let preview = registry
            .selected_preview(PickerKind::Icon, "PlagueSur")
            .expect("cached");
        assert!(std::ptr::eq(
            preview,
            registry.icon[0].preview.as_ref().unwrap()
        ));
        assert_eq!(
            registry.selected_preview(PickerKind::Icon, ""),
            Some(preview)
        );
        assert_eq!(registry.selected_preview(PickerKind::Icon, "Removed"), None);
        assert_eq!(
            registry.selected_preview(PickerKind::Icon, "*:-HighContrast"),
            None
        );
        registry = ThemeRegistry::default(); // a rescan invalidates the old cache
        assert!(
            registry
                .selected_preview(PickerKind::Icon, "PlagueSur")
                .is_none()
        );
    }

    #[test]
    fn cursor_picker_includes_all_installed_themes_with_previews() {
        let with_preview = |id: &str, name: &str| ThemeItem {
            preview: Some(PreviewImage {
                source: "left_ptr".to_owned(),
                width: 1,
                height: 1,
                pixels: Arc::new(vec![255, 255, 255, 255]),
            }),
            ..item(id, name)
        };
        let picked = with_installed_cursor_themes(vec![
            with_preview("Bibata-Modern-Ice", "Bibata"),
            with_preview("Oxygen_Black", "Oxygen Black"),
            with_preview("McMojave-cursors", "Upstream name"),
            with_preview("oxy-white", "Oxy White"),
            item("Oxygen_Blue", "Oxygen Blue"), // no usable preview
            with_preview("McMojaveX", "Not a McMojave variant"),
        ]);
        assert_eq!(
            picked
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            [
                "McMojave-cursors",
                "oxy-white",
                "Oxygen_Black",
                "Bibata-Modern-Ice",
                "McMojaveX"
            ]
        );
        assert_eq!(picked[0].name, "McMojave");
        assert_eq!(picked[2].name, "Oxygen Black");
        assert_eq!(PICKER_SLOTS, 6);
        assert_eq!(SELECTED_ICON_PREVIEW_NODE, "icon-theme-selected-preview");
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
        assert_eq!(
            user_data_home(None, Some("")),
            None,
            "never install into a system prefix"
        );
        assert_eq!(
            theme_data_dirs(Some("/home/u"), None, Some("/opt/share:")),
            vec![
                PathBuf::from("/home/u/.local/share"),
                PathBuf::from("/opt/share"),
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ]
        );
        assert_eq!(
            theme_data_dirs(Some("/home/u"), None, None),
            vec![
                PathBuf::from("/home/u/.local/share"),
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ]
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
        assert!(
            message.contains("theme.tar.xz") && message.contains(".txz"),
            "{message}"
        );

        let collision = install_with(
            |_, _, _, _| Err(ImportError::IdCollision("Papirus".to_owned())),
            Path::new("/src/Papirus"),
            Path::new("/icons"),
        );
        assert!(
            collision
                .expect_err("collision")
                .contains("already installed")
        );

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
        assert_eq!(*image.pixels, vec![1, 2, 3, 255, 4, 5, 6, 0]);
    }
}

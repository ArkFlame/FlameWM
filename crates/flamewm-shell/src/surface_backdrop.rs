//! Desktop-layer source for translucent shell surfaces (taskbar, Start, popups).
//!
//! The shell never reads the root for a backdrop: every translucent surface blends over the
//! desktop layer the render owner builds from the wallpaper settings, the same layer the WM
//! titlebars and app windows use.

use std::path::{Path, PathBuf};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_image_core::wallpaper::{self, DEFAULT_WALLPAPER_PATH};
pub use flamewm_ui_x11::backdrop::DesktopSource;

/// Solid wallpaper used when `wallpaperColor` is unparsable (the settings default).
const DEFAULT_SOLID: [u8; 3] = [0x23, 0x26, 0x29];

/// The desktop layer named by `wallpaperMode`, `wallpaper`, `wallpaperColor` and `wallpaperFit`,
/// the keys the desktop wallpaper owner reads. Anything but `solid` or `custom` is the bundled
/// default under `packaged_root`.
#[must_use]
pub fn desktop_source(settings: &SettingsSnapshot, packaged_root: &Path) -> DesktopSource {
    let text = |key: &str| match settings.values.get(key) {
        Some(SettingValue::Text(value)) => value.trim(),
        _ => "",
    };
    match text("wallpaperMode") {
        "solid" => DesktopSource::Solid(
            flamewm_platform::chrome::parse_hex_rgb(&format!(
                "#{}",
                text("wallpaperColor").trim_start_matches('#')
            ))
            .map_or(DEFAULT_SOLID, |(r, g, b)| [r, g, b]),
        ),
        mode => DesktopSource::Image {
            path: if mode == "custom" && !text("wallpaper").is_empty() {
                PathBuf::from(text("wallpaper"))
            } else {
                packaged_root.join(DEFAULT_WALLPAPER_PATH)
            },
            fit: wallpaper::parse(text("wallpaperFit")),
        },
    }
}

/// Debug trace for taskbar pseudo-transparency, enabled by `FLAMEWM_DEBUG_BACKDROP=1` (stderr).
pub fn backdrop_trace(message: std::fmt::Arguments<'_>) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("FLAMEWM_DEBUG_BACKDROP").is_some_and(|v| v == "1"))
    {
        eprintln!("debug backdrop {message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::background::WallpaperFit;

    fn settings(values: &[(&str, &str)]) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        for (key, value) in values {
            snapshot
                .values
                .insert((*key).to_owned(), SettingValue::Text((*value).to_owned()));
        }
        snapshot
    }

    #[test]
    fn solid_mode_uses_the_wallpaper_colour_or_the_default() {
        let root = Path::new("/usr/share/flamewm");
        assert_eq!(
            desktop_source(
                &settings(&[("wallpaperMode", "solid"), ("wallpaperColor", "#102030")]),
                root
            ),
            DesktopSource::Solid([0x10, 0x20, 0x30])
        );
        assert_eq!(
            desktop_source(&settings(&[("wallpaperMode", "solid")]), root),
            DesktopSource::Solid(DEFAULT_SOLID)
        );
    }

    #[test]
    fn only_custom_mode_reads_the_saved_path() {
        let root = Path::new("/usr/share/flamewm");
        let custom = settings(&[
            ("wallpaperMode", "custom"),
            ("wallpaper", "/tmp/w.png"),
            ("wallpaperFit", "tile"),
        ]);
        assert_eq!(
            desktop_source(&custom, root),
            DesktopSource::Image {
                path: PathBuf::from("/tmp/w.png"),
                fit: WallpaperFit::Tile,
            }
        );
        let default = settings(&[("wallpaperMode", "default"), ("wallpaper", "/tmp/w.png")]);
        assert_eq!(
            desktop_source(&default, root),
            DesktopSource::Image {
                path: root.join(DEFAULT_WALLPAPER_PATH),
                fit: WallpaperFit::Fill,
            }
        );
    }
}

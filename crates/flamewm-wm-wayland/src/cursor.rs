//! Pointer image, as the X11 session shows it: the cursor surface a client (or `XWayland`, for every
//! X11 client) set with `wl_pointer.set_cursor`, else a named cursor from the session's xcursor theme.
//!
//! The theme follows the X11 session: `flamewm-session-core` resolves `XCURSOR_THEME`,
//! `XCURSOR_SIZE` and `XCURSOR_PATH` (the saved `cursorTheme` wins), and [`export_session_theme`]
//! puts them in this process's environment before `XWayland` and the session clients start, so
//! compositor-drawn and client-drawn cursors come from one theme. Elements are `Kind::Cursor`, which
//! lets the DRM compositor scan them out on the hardware cursor plane.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::surface::render_elements_from_surface_tree;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData};
use smithay::utils::{IsAlive, Logical, Physical, Point, Transform};
use smithay::wayland::compositor::with_states;

use crate::render::Elem;

/// Nominal size when `XCURSOR_SIZE` is unset or invalid; the session default.
const DEFAULT_SIZE: u32 = 24;

/// Last-resort arrow when the theme has no usable default cursor: `B` outline, `W` fill, `.` clear.
const FALLBACK_ARROW: [&str; 19] = [
    "B...........",
    "BB..........",
    "BWB.........",
    "BWWB........",
    "BWWWB.......",
    "BWWWWB......",
    "BWWWWWB.....",
    "BWWWWWWB....",
    "BWWWWWWWB...",
    "BWWWWWWWWB..",
    "BWWWWWWWWWB.",
    "BWWWWWWBBBBB",
    "BWWWBWWB....",
    "BWWBBWWB....",
    "BWB..BWWB...",
    "BB...BWWB...",
    "B.....BWWB..",
    "......BWWB..",
    ".......BB...",
];

/// One decoded cursor image, uploaded once and reused for every frame.
struct Named {
    buffer: MemoryRenderBuffer,
    hotspot: Point<i32, Logical>,
}

/// The session xcursor theme (then the system `default` theme) and the cursors decoded so far.
pub struct Cursors {
    themes: Vec<xcursor::CursorTheme>,
    size: u32,
    /// `None` records a name the theme does not have, so it is not searched for again.
    named: HashMap<CursorIcon, Option<Named>>,
    fallback: Named,
}

impl Cursors {
    /// The theme named by `XCURSOR_THEME` (`default` when unset) at `XCURSOR_SIZE`.
    pub fn from_env() -> Self {
        let name = std::env::var("XCURSOR_THEME")
            .ok()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "default".to_owned());
        let size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|size| size.trim().parse::<u32>().ok())
            .filter(|size| *size > 0)
            .unwrap_or(DEFAULT_SIZE);
        let mut themes = vec![xcursor::CursorTheme::load(&name)];
        if name != "default" {
            themes.push(xcursor::CursorTheme::load("default"));
        }
        Self {
            themes,
            size,
            named: HashMap::new(),
            fallback: fallback_arrow(),
        }
    }

    /// Render elements for `status` with the hotspot at `pointer`. The client's cursor surface is
    /// drawn while it is alive; a named cursor the theme lacks falls back to its default arrow.
    pub fn elements(
        &mut self,
        renderer: &mut GlowRenderer,
        status: &CursorImageStatus,
        pointer: Point<f64, Logical>,
    ) -> Vec<Elem> {
        let icon = match status {
            CursorImageStatus::Hidden => return Vec::new(),
            CursorImageStatus::Surface(surface) if surface.alive() => {
                let hotspot = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<CursorImageSurfaceData>()
                        .and_then(|data| data.lock().ok().map(|attributes| attributes.hotspot))
                        .unwrap_or_default()
                });
                let origin = (pointer.to_i32_round() - hotspot).to_physical(1);
                return render_elements_from_surface_tree(
                    renderer,
                    surface,
                    origin,
                    1.0,
                    1.0,
                    Kind::Cursor,
                )
                .into_iter()
                .map(Elem::Surface)
                .collect();
            }
            CursorImageStatus::Surface(_) => CursorIcon::Default,
            CursorImageStatus::Named(icon) => *icon,
        };
        let named = self.named(icon);
        let origin: Point<f64, Physical> = (pointer.to_i32_round() - named.hotspot)
            .to_f64()
            .to_physical(1.0);
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            origin,
            &named.buffer,
            None,
            None,
            None,
            Kind::Cursor,
        )
        .map(|element| vec![Elem::Memory(element)])
        .unwrap_or_default()
    }

    /// The decoded cursor for `icon`, then the theme's default arrow, then the built-in arrow.
    fn named(&mut self, icon: CursorIcon) -> &Named {
        for wanted in [icon, CursorIcon::Default] {
            if !self.named.contains_key(&wanted) {
                let loaded = self
                    .themes
                    .iter()
                    .find_map(|theme| load(theme, wanted, self.size));
                if loaded.is_none() {
                    eprintln!(
                        "flamewm-wayland: cursor '{}' not in the cursor themes; using a fallback",
                        wanted.name()
                    );
                }
                self.named.insert(wanted, loaded);
            }
            if self.named.get(&wanted).is_some_and(Option::is_some) {
                return self.named[&wanted].as_ref().expect("checked above");
            }
        }
        &self.fallback
    }
}

/// First frame of the image nearest `size` in the theme file for `icon` or one of its aliases.
fn load(theme: &xcursor::CursorTheme, icon: CursorIcon, size: u32) -> Option<Named> {
    let path = std::iter::once(icon.name())
        .chain(icon.alt_names().iter().copied())
        .find_map(|name| theme.load_icon(name))?;
    let data = std::fs::read(path).ok()?;
    let images = xcursor::parser::parse_xcursor(&data)?;
    // Frames of one size are contiguous and in playback order: the first nearest image is frame 0.
    let image = images
        .iter()
        .min_by_key(|image| image.size.abs_diff(size))?;
    let (width, height) = (
        i32::try_from(image.width).ok()?,
        i32::try_from(image.height).ok()?,
    );
    let bytes = image.width as usize * image.height as usize * 4;
    if width == 0 || height == 0 || image.pixels_rgba.len() < bytes {
        return None;
    }
    // xcursor pixels are little-endian ARGB words, which is DRM's Argb8888 byte order.
    Some(Named {
        buffer: MemoryRenderBuffer::from_slice(
            &image.pixels_rgba,
            Fourcc::Argb8888,
            (width, height),
            1,
            Transform::Normal,
            None,
        ),
        hotspot: Point::from((
            i32::try_from(image.xhot).unwrap_or(0),
            i32::try_from(image.yhot).unwrap_or(0),
        )),
    })
}

/// Width and height of [`FALLBACK_ARROW`].
const FALLBACK_SIZE: (i32, i32) = (12, 19);

/// Premultiplied little-endian ARGB pixels of [`FALLBACK_ARROW`].
fn fallback_pixels() -> (Vec<u8>, i32, i32) {
    let mut pixels = Vec::with_capacity(FALLBACK_ARROW[0].len() * FALLBACK_ARROW.len() * 4);
    for row in FALLBACK_ARROW {
        for cell in row.bytes() {
            pixels.extend_from_slice(match cell {
                b'B' => &[0x00, 0x00, 0x00, 0xff],
                b'W' => &[0xff, 0xff, 0xff, 0xff],
                _ => &[0x00, 0x00, 0x00, 0x00],
            });
        }
    }
    (pixels, FALLBACK_SIZE.0, FALLBACK_SIZE.1)
}

fn fallback_arrow() -> Named {
    let (pixels, width, height) = fallback_pixels();
    Named {
        buffer: MemoryRenderBuffer::from_slice(
            &pixels,
            Fourcc::Argb8888,
            (width, height),
            1,
            Transform::Normal,
            None,
        ),
        hotspot: Point::from((0, 0)),
    }
}

/// The `cursorTheme` saved in the `FlameWM` settings file, as the X11 session reads it at login.
pub fn saved_theme() -> Option<String> {
    let text = std::fs::read_to_string(crate::control::settings_path()).ok()?;
    flamewm_platform::settings::ConfigStore::default()
        .parse_snapshot(&text)
        .ok()
        .map(|settings| settings.cursor_theme)
}

/// Resolves the session cursor theme exactly as the X11 session does and exports it, so `XWayland`,
/// the X11 clients and this compositor share it. Must run before any thread or child starts.
pub fn export_session_theme(saved_theme: Option<&str>) {
    let existing: BTreeMap<String, String> = std::env::vars().collect();
    let home = existing
        .get("HOME")
        .filter(|home| !home.is_empty())
        .map_or_else(|| PathBuf::from("/root"), PathBuf::from);
    let bindir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("/usr/bin"));
    let paths = flamewm_session_core::default_paths_from_home(bindir, home);
    let mut environment = flamewm_session_core::cursor_environment(&paths, &existing, None);
    if let Some(saved) = saved_theme.filter(|saved| !saved.trim().is_empty()) {
        let search_path = environment.get("XCURSOR_PATH").cloned().unwrap_or_default();
        environment.insert(
            "XCURSOR_THEME".to_owned(),
            flamewm_session_core::resolve_cursor_theme(saved, &search_path),
        );
    }
    eprintln!(
        "flamewm-wayland: cursor theme={} size={}",
        environment.get("XCURSOR_THEME").map_or("", String::as_str),
        environment.get("XCURSOR_SIZE").map_or("", String::as_str)
    );
    flamewm_session_core::apply_current_process_env(&environment);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_arrow_is_a_complete_premultiplied_image() {
        let (pixels, width, height) = fallback_pixels();
        assert_eq!(FALLBACK_ARROW.len(), height as usize);
        assert!(FALLBACK_ARROW.iter().all(|row| row.len() == width as usize));
        assert_eq!(pixels.len(), (width * height * 4) as usize);
        for pixel in pixels.chunks_exact(4) {
            assert!(pixel[..3].iter().all(|channel| *channel <= pixel[3]));
        }
        assert_eq!(
            &pixels[..4],
            &[0x00, 0x00, 0x00, 0xff],
            "hotspot pixel is the outline"
        );
    }

    #[test]
    fn a_missing_theme_still_yields_a_cursor() {
        let mut cursors = Cursors {
            themes: vec![xcursor::CursorTheme::load("flamewm-no-such-theme")],
            size: DEFAULT_SIZE,
            named: HashMap::new(),
            fallback: fallback_arrow(),
        };
        let _ = cursors.named(CursorIcon::Pointer);
        assert!(
            cursors.named.contains_key(&CursorIcon::Pointer),
            "the lookup is remembered"
        );
    }
}

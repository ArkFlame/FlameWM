//! Proportional titlebar font. IBM Plex Sans is preferred (matching the X11 Xft chrome), with
//! common installed sans fonts as fallbacks. The font file is located and parsed once, glyph
//! coverage bitmaps are cached per (char, pixel size), and nothing here touches X11 or spawns
//! a process, so the frame loop never loads or rasterizes a font it has already seen.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fontdue::{Font, FontSettings};

const CANDIDATES: [&str; 5] = [
    "IBMPlexSans-Regular.ttf",
    "NotoSans-Regular.ttf",
    "DejaVuSans.ttf",
    "LiberationSans-Regular.ttf",
    "Cantarell-Regular.otf",
];
/// Caps the glyph cache so a hostile stream of distinct titles cannot grow memory without bound.
const CACHE_LIMIT: usize = 1024;

pub struct Glyph {
    pub advance: i32,
    pub left: i32,
    /// Pixels above the baseline (bitmap top edge).
    pub top: i32,
    pub width: usize,
    pub coverage: Vec<u8>,
}

struct TitleFont {
    font: Font,
    cache: HashMap<(char, u32), std::sync::Arc<Glyph>>,
}

static FONT: Mutex<Option<Option<TitleFont>>> = Mutex::new(None);

fn search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(Path::new(&home).join(".local/share/fonts"));
    }
    if let Some(dirs) = std::env::var_os("XDG_DATA_DIRS") {
        roots.extend(std::env::split_paths(&dirs).map(|dir| dir.join("fonts")));
    }
    roots.push(PathBuf::from("/usr/share/fonts"));
    roots.push(PathBuf::from("/usr/local/share/fonts"));
    roots
}

fn find(root: &Path, name: &str, depth: u32) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            dirs.push(path);
        } else if path.file_name().is_some_and(|file| file == name) {
            return Some(path);
        }
    }
    if depth == 0 {
        return None;
    }
    dirs.into_iter().find_map(|dir| find(&dir, name, depth - 1))
}

fn load() -> Option<TitleFont> {
    let roots = search_roots();
    for name in CANDIDATES {
        for root in &roots {
            let Some(path) = find(root, name, 4) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if let Ok(font) = Font::from_bytes(bytes, FontSettings::default()) {
                return Some(TitleFont {
                    font,
                    cache: HashMap::new(),
                });
            }
        }
    }
    None
}

/// Runs `f` with a glyph lookup, or returns `None` when no usable font is installed so the
/// caller can fall back to its built-in glyphs.
pub fn with_glyphs<R>(
    px: f32,
    f: impl FnOnce(&mut dyn FnMut(char) -> std::sync::Arc<Glyph>) -> R,
) -> Option<R> {
    let mut guard = FONT.lock().ok()?;
    let font = guard.get_or_insert_with(load).as_mut()?;
    let key_px = (px * 4.0).round() as u32;
    let mut lookup = |c: char| {
        if let Some(hit) = font.cache.get(&(c, key_px)) {
            return hit.clone();
        }
        if font.cache.len() >= CACHE_LIMIT {
            font.cache.clear();
        }
        let (metrics, coverage) = font.font.rasterize(c, px);
        let glyph = std::sync::Arc::new(Glyph {
            advance: metrics.advance_width.round() as i32,
            left: metrics.xmin,
            top: metrics.height as i32 + metrics.ymin,
            width: metrics.width,
            coverage,
        });
        font.cache.insert((c, key_px), glyph.clone());
        glyph
    };
    Some(f(&mut lookup))
}

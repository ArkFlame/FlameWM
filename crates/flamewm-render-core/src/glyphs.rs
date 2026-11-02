//! Software glyph masks for the non-X11 paint path.
//!
//! `flamewm-render-x11` gets coverage masks from Xft. A Wayland presenter has no Xft, so this
//! rasterizes the same shape of mask — one 8-bit coverage per pixel, top-left origin — from a
//! font file with `fontdue`. The result feeds the existing `PixelOp::BlendMask`, so text runs
//! through the same lowering and blending as every other primitive.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fontdue::{Font, FontSettings};

/// Font files tried in order, matching the chrome font the X11 path prefers.
const CANDIDATES: &[&str] = &[
    "IBMPlexSans-Regular.ttf",
    "NotoSans-Regular.ttf",
    "DejaVuSans.ttf",
    "LiberationSans-Regular.ttf",
    "Cantarell-Regular.otf",
];

/// System directories the candidate fonts are looked up in.
const FONT_DIRS: &[&str] = &["/usr/share/fonts", "/usr/local/share/fonts"];

/// The per-user font directory, from `XDG_DATA_HOME` or the `~/.local/share` default.
fn user_font_dir() -> Option<std::path::PathBuf> {
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        return Some(std::path::PathBuf::from(data_home).join("fonts"));
    }
    std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share/fonts"))
}

/// A rasterized glyph and where its bitmap sits relative to the pen.
#[derive(Clone, Debug)]
pub struct GlyphMask {
    /// Horizontal advance from the glyph origin.
    pub advance: i32,
    /// Bitmap left edge relative to the pen.
    pub left: i32,
    /// Bitmap top edge relative to the baseline, positive upward.
    pub top: i32,
    pub width: usize,
    pub height: usize,
    /// Row-major 8-bit coverage.
    pub coverage: Vec<u8>,
}

/// A rasterized run of text: a coverage mask plus where it starts.
#[derive(Clone, Debug)]
pub struct TextMask {
    /// Device x of the mask's top-left corner.
    pub x: i32,
    /// Device y of the mask's top-left corner.
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Row-major 8-bit coverage.
    pub coverage: Vec<u8>,
}

struct FontCache {
    font: Font,
    cache: HashMap<(char, u32), Arc<GlyphMask>>,
}

static FONT: Mutex<Option<Option<FontCache>>> = Mutex::new(None);

/// Caps the cache so a hostile stream of distinct sizes cannot grow memory without bound.
const CACHE_LIMIT: usize = 1024;

fn find_font() -> Option<Font> {
    fn walk(dir: &std::path::Path, wanted: &str, depth: u8) -> Option<std::path::PathBuf> {
        if depth == 0 {
            return None;
        }
        let entries = std::fs::read_dir(dir).ok()?;
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                subdirs.push(path);
            } else if path.file_name().is_some_and(|name| name == wanted) {
                return Some(path);
            }
        }
        subdirs
            .into_iter()
            .find_map(|dir| walk(&dir, wanted, depth - 1))
    }

    let user = user_font_dir();
    let roots: Vec<std::path::PathBuf> = FONT_DIRS
        .iter()
        .map(std::path::PathBuf::from)
        .chain(user)
        .filter(|root| root.exists())
        .collect();

    for wanted in CANDIDATES {
        for root in &roots {
            let Some(path) = walk(root, wanted, 4) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if let Ok(font) = Font::from_bytes(bytes, FontSettings::default()) {
                return Some(font);
            }
        }
    }
    None
}

/// Runs `f` with a glyph lookup at `px` pixels, or returns `None` when no font is installed.
fn with_glyphs<R>(
    px: f32,
    f: impl FnOnce(&mut dyn FnMut(char) -> Arc<GlyphMask>) -> R,
) -> Option<R> {
    let mut guard = FONT.lock().ok()?;
    if guard.is_none() {
        *guard = Some(find_font().map(|font| FontCache {
            font,
            cache: HashMap::new(),
        }));
    }
    let cache = guard.as_mut()?.as_mut()?;
    let key_px = (px * 4.0).round() as u32;
    let mut lookup = |c: char| {
        if let Some(hit) = cache.cache.get(&(c, key_px)) {
            return Arc::clone(hit);
        }
        let (metrics, coverage) = cache.font.rasterize(c, px);
        let glyph = Arc::new(GlyphMask {
            advance: metrics.advance_width.round() as i32,
            left: metrics.xmin,
            top: metrics.height as i32 + metrics.ymin,
            width: metrics.width,
            height: metrics.height,
            coverage,
        });
        if cache.cache.len() >= CACHE_LIMIT {
            cache.cache.clear();
        }
        cache.cache.insert((c, key_px), Arc::clone(&glyph));
        glyph
    };
    Some(f(&mut lookup))
}

/// Rasterizes `text` at `size` pixels with its baseline at (`pen_x`, `baseline_y`).
///
/// Returns `None` when no usable font is installed. White space still advances the pen, so the
/// layout matches a proportional font.
#[must_use]
pub fn text_mask(text: &str, size: f32, pen_x: f32, baseline_y: f32) -> Option<TextMask> {
    if size <= 0.0 || text.is_empty() {
        return None;
    }
    with_glyphs(size, |lookup| {
        let mut glyphs = Vec::new();
        let mut pen = 0i32;
        for character in text.chars() {
            let glyph = lookup(character);
            if glyph.width > 0 && glyph.height > 0 {
                glyphs.push((pen, Arc::clone(&glyph)));
            }
            pen += glyph.advance;
        }
        if glyphs.is_empty() {
            return TextMask {
                x: pen_x.round() as i32,
                y: baseline_y.round() as i32,
                width: 0,
                height: 0,
                coverage: Vec::new(),
            };
        }

        let (mut min_x, mut min_y) = (i32::MAX, i32::MAX);
        let (mut max_x, mut max_y) = (i32::MIN, i32::MIN);
        for (offset, glyph) in &glyphs {
            let left = offset + glyph.left;
            let top = -glyph.top;
            min_x = min_x.min(left);
            min_y = min_y.min(top);
            max_x = max_x.max(left + glyph.width as i32);
            max_y = max_y.max(top + glyph.height as i32);
        }
        let width = (max_x - min_x).max(0) as u32;
        let height = (max_y - min_y).max(0) as u32;
        let mut coverage = vec![0u8; width as usize * height as usize];

        for (offset, glyph) in &glyphs {
            let base_x = offset + glyph.left - min_x;
            let base_y = -glyph.top - min_y;
            for row in 0..glyph.height {
                let y = base_y + row as i32;
                if y < 0 || y >= height as i32 {
                    continue;
                }
                let src = &glyph.coverage[row * glyph.width..(row + 1) * glyph.width];
                let dst_start = y as usize * width as usize + base_x.max(0) as usize;
                let skip = (-base_x).max(0) as usize;
                let span = glyph.width.saturating_sub(skip);
                if span == 0 {
                    continue;
                }
                let dst_end = (dst_start + span).min(coverage.len());
                let take = dst_end - dst_start;
                coverage[dst_start..dst_end].copy_from_slice(&src[skip..skip + take]);
            }
        }

        TextMask {
            x: pen_x.round() as i32 + min_x,
            y: baseline_y.round() as i32 + min_y,
            width,
            height,
            coverage,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn any_font() -> bool {
        find_font().is_some()
    }

    #[test]
    fn empty_text_produces_no_mask() {
        assert!(text_mask("", 14.0, 0.0, 0.0).is_none());
        assert!(text_mask("A", 0.0, 0.0, 0.0).is_none());
    }

    #[test]
    fn a_visible_glyph_covers_pixels() {
        if !any_font() {
            return;
        }
        let mask = text_mask("A", 24.0, 0.0, 24.0).expect("mask");
        assert!(mask.width > 0 && mask.height > 0, "glyph has a bitmap");
        assert_eq!(
            mask.coverage.len(),
            (mask.width * mask.height) as usize,
            "coverage is one byte per pixel"
        );
        assert!(
            mask.coverage.iter().any(|&coverage| coverage > 0),
            "an 'A' must ink some pixels"
        );
    }

    #[test]
    fn a_longer_run_is_wider_than_a_shorter_one() {
        if !any_font() {
            return;
        }
        let short = text_mask("I", 20.0, 0.0, 20.0).expect("short");
        let long = text_mask("IIII", 20.0, 0.0, 20.0).expect("long");
        assert!(
            long.width > short.width,
            "advancing the pen must widen the mask: {} vs {}",
            long.width,
            short.width
        );
    }

    #[test]
    fn spaces_advance_without_inking() {
        if !any_font() {
            return;
        }
        let mask = text_mask("A   A", 20.0, 0.0, 20.0).expect("mask");
        assert!(mask.width > 0);
        // The interior gap between the two glyphs has no ink on its middle row band.
        let row = mask.width as usize * (mask.height as usize / 2);
        let interior = &mask.coverage[row..row + mask.width as usize];
        assert!(interior.contains(&0), "the spaces stay clear");
    }
}

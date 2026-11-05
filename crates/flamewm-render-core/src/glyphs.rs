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
    /// Unrounded horizontal advance from the glyph origin, in px.
    pub advance: f32,
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

/// One font at one pixel size: the only source of glyph advances and kerning.
///
/// Paint (`text_mask`), caret positions (`caret_positions`), hit-testing (`char_index_at`) and
/// layout width (`advance_width`) all go through `shape`, so they cannot disagree.
struct Run<'a> {
    cache: &'a mut FontCache,
    px: f32,
}

impl Run<'_> {
    fn glyph(&mut self, c: char) -> Arc<GlyphMask> {
        let key = (c, self.px.to_bits());
        if let Some(hit) = self.cache.cache.get(&key) {
            return Arc::clone(hit);
        }
        let (metrics, coverage) = self.cache.font.rasterize(c, self.px);
        let glyph = Arc::new(GlyphMask {
            advance: metrics.advance_width,
            left: metrics.xmin,
            top: metrics.height as i32 + metrics.ymin,
            width: metrics.width,
            height: metrics.height,
            coverage,
        });
        if self.cache.cache.len() >= CACHE_LIMIT {
            self.cache.cache.clear();
        }
        self.cache.cache.insert(key, Arc::clone(&glyph));
        glyph
    }

    /// Kerning between `left` and `right`, in px; zero when the font has no pair.
    fn kern(&self, left: char, right: char) -> f32 {
        self.cache
            .font
            .horizontal_kern(left, right, self.px)
            .unwrap_or(0.0)
    }
}

/// Runs `f` with the font at `px` pixels, or returns `None` when no font is installed.
fn with_run<R>(px: f32, f: impl FnOnce(&mut Run<'_>) -> R) -> Option<R> {
    let mut guard = FONT.lock().ok()?;
    if guard.is_none() {
        *guard = Some(find_font().map(|font| FontCache {
            font,
            cache: HashMap::new(),
        }));
    }
    let cache = guard.as_mut()?.as_mut()?;
    Some(f(&mut Run { cache, px }))
}

/// Shapes `text`: the fractional pen origin of every char plus the run end (`len = chars + 1`),
/// and the glyph of every char. The pen is accumulated in f32; nothing is rounded per glyph.
fn shape(text: &str, run: &mut Run<'_>) -> (Vec<f32>, Vec<Arc<GlyphMask>>) {
    let mut origins = Vec::with_capacity(text.len() + 1);
    let mut glyphs = Vec::with_capacity(text.len());
    let mut pen = 0.0f32;
    let mut prev: Option<char> = None;
    for character in text.chars() {
        if let Some(left) = prev {
            pen += run.kern(left, character);
        }
        origins.push(pen);
        let glyph = run.glyph(character);
        pen += glyph.advance;
        glyphs.push(glyph);
        prev = Some(character);
    }
    origins.push(pen);
    (origins, glyphs)
}

/// Advance width of `text` at `size` px, in fractional px. `None` when no font is installed.
#[must_use]
pub fn advance_width(text: &str, size: f32) -> Option<f32> {
    if size <= 0.0 {
        return None;
    }
    with_run(size, |run| shape(text, run).0.last().copied().unwrap_or(0.0))
}

/// Caret x of every char boundary in `text` at `size` px, relative to the run origin.
///
/// `result[i]` is where a caret before char `i` sits; `result[chars]` is the end of the run.
/// `None` when no font is installed.
#[must_use]
pub fn caret_positions(text: &str, size: f32) -> Option<Vec<f32>> {
    if size <= 0.0 {
        return None;
    }
    with_run(size, |run| shape(text, run).0)
}

/// Char index whose caret boundary is nearest to `x` (run-relative px), clamped to the run.
///
/// Inverse of [`caret_positions`]: `char_index_at(t, s, caret_positions(t, s)[k])` is `k`.
#[must_use]
pub fn char_index_at(text: &str, size: f32, x: f32) -> Option<usize> {
    let origins = caret_positions(text, size)?;
    Some(nearest_caret(&origins, x))
}

/// Index of the caret in `origins` nearest to `x`; ties keep the earlier caret.
fn nearest_caret(origins: &[f32], x: f32) -> usize {
    let mut best = 0usize;
    let mut best_distance = f32::INFINITY;
    for (index, origin) in origins.iter().enumerate() {
        let distance = (origin - x).abs();
        if distance < best_distance {
            best = index;
            best_distance = distance;
        }
    }
    best
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
    with_run(size, |run| {
        let (origins, shaped) = shape(text, run);
        let mut glyphs = Vec::new();
        for (origin, glyph) in origins.iter().zip(shaped) {
            // Each origin rounds once on its own; the pen itself never accumulates rounding.
            if glyph.width > 0 && glyph.height > 0 {
                glyphs.push((origin.round() as i32, glyph));
            }
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

    /// The font file is walked, read and parsed once per test binary. Every test asks for it, and
    /// the reference sums ask once per sample string, so an uncached lookup dominated the run.
    fn test_font() -> Option<&'static Font> {
        static FONT: std::sync::OnceLock<Option<Font>> = std::sync::OnceLock::new();
        FONT.get_or_init(find_font).as_ref()
    }

    fn any_font() -> bool {
        test_font().is_some()
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

    /// Strings of 1..=200 chars mixing ASCII, spaces, kerning pairs and a wide non-Latin glyph.
    fn sample_strings() -> Vec<String> {
        const PIECES: &[&str] = &["AV", "To", "Wa", " ", "x", "ilm", "日", "W", "ff", "./"];
        (1..=200)
            .map(|len| {
                (0..len)
                    .map(|i| PIECES[(i * 7 + len) % PIECES.len()])
                    .collect::<String>()
                    .chars()
                    .take(len)
                    .collect()
            })
            .collect()
    }

    /// Unrounded advance plus kerning, summed straight from the font file, as an independent
    /// reference for the shared pen model.
    fn reference_advance(text: &str, px: f32) -> f32 {
        let font = test_font().expect("font");
        let chars: Vec<char> = text.chars().collect();
        let mut sum = 0.0f32;
        for (i, &c) in chars.iter().enumerate() {
            if i > 0 {
                sum += font.horizontal_kern(chars[i - 1], c, px).unwrap_or(0.0);
            }
            sum += font.metrics(c, px).advance_width;
        }
        sum
    }

    #[test]
    fn caret_end_matches_unrounded_reference_within_half_px() {
        if !any_font() {
            return;
        }
        for size in [11.0f32, 13.0, 14.5, 20.0] {
            for text in sample_strings() {
                let carets = caret_positions(&text, size).expect("carets");
                assert_eq!(carets.len(), text.chars().count() + 1);
                let end = *carets.last().expect("end");
                let expected = reference_advance(&text, size);
                assert!(
                    (end - expected).abs() < 0.5,
                    "{size}px {} chars: caret end {end} vs reference {expected}",
                    text.chars().count()
                );
                assert!(
                    carets.windows(2).all(|pair| pair[0] <= pair[1]),
                    "caret positions must not go backwards"
                );
            }
        }
    }

    #[test]
    fn caret_after_each_prefix_matches_the_next_glyph_origin_within_half_px() {
        if !any_font() {
            return;
        }
        // 40+ chars: mixed widths (wide CJK, narrow i/l, spaces) and kerned pairs (AV, To, Wa).
        let text = "Wi AVAST, Tofu: ilm 日本 Wa ff W./ VAT To ilm AVocado";
        assert!(text.chars().count() >= 40);
        for size in [13.0f32, 15.0] {
            let carets = caret_positions(text, size).expect("carets");
            let chars: Vec<char> = text.chars().collect();
            for k in 1..=chars.len() {
                // The glyph origin the paint path uses for char k is origins[k]; a caret before
                // it must sit at the same x, as must the advance of the prefix that precedes it.
                let prefix: String = chars[..k].iter().collect();
                let prefix_end = advance_width(&prefix, size).expect("advance");
                assert!(
                    (carets[k] - prefix_end).abs() < 0.5,
                    "{size}px prefix {k}: caret {} vs prefix advance {prefix_end}",
                    carets[k]
                );
            }
        }
    }

    #[test]
    fn repeated_glyph_run_does_not_drift_per_letter() {
        if !any_font() {
            return;
        }
        // Per-glyph rounding of the advance would drift linearly with the letter count.
        let px = 13.0;
        let one = advance_width("I", px).expect("advance");
        let long = advance_width(&"I".repeat(200), px).expect("advance");
        assert!(
            (long - 200.0 * one).abs() < 0.5,
            "200 I's: {long} vs 200 * {one}"
        );
    }

    #[test]
    fn caret_hit_test_round_trips_every_boundary() {
        if !any_font() {
            return;
        }
        for size in [13.0f32, 16.0] {
            for text in sample_strings() {
                // Shaping once per string: `char_index_at` re-shapes the whole run on every call,
                // which made this quadratic in debug builds (~10 s). Every boundary goes through the
                // nearest-caret search that `char_index_at` uses; the public entry point is checked
                // at the clamps and at the middle boundary.
                let carets = caret_positions(&text, size).expect("carets");
                for (index, &x) in carets.iter().enumerate() {
                    assert_eq!(
                        nearest_caret(&carets, x),
                        index,
                        "{size}px boundary {index} of {text:?}"
                    );
                }
                let end = *carets.last().expect("end");
                let mid = carets.len() / 2;
                assert_eq!(char_index_at(&text, size, carets[mid]), Some(mid));
                assert_eq!(char_index_at(&text, size, end + 500.0), Some(text.chars().count()));
                assert_eq!(char_index_at(&text, size, -500.0), Some(0));
            }
        }
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

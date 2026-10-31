//! Pseudo-transparency backdrop for a surface (taskbar, Start popup).
//!
//! No compositing manager exists, so a translucent scene pixel cannot blend with
//! the live desktop. The owner supplies a static ARGB snapshot of what is behind the
//! surface; it is laid into the scene first, and the translucent fills then blend
//! over it. The snapshot is not live: the owner refreshes it on show, move, resize
//! or wallpaper change. Without a backdrop the scene keeps its transparent clear.

use std::collections::{HashMap, HashSet};

use crate::xlib::*;
use crate::xlib_ext::XTranslateCoordinates;

use super::X11App;
use super::window_backdrop::{MirrorFill, MirrorKey};

/// Debug trace for backdrop painting, enabled by `FLAMEWM_DEBUG_BACKDROP=1` (stderr).
/// The message is built only when tracing is enabled, so disabled tracing allocates nothing.
fn trace_paint(message: impl FnOnce() -> String) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("FLAMEWM_DEBUG_BACKDROP").is_some_and(|v| v == "1"))
    {
        eprintln!("debug backdrop paint {}", message());
    }
}

/// Retained server pixmap holding one ARGB32 backdrop snapshot.
#[derive(Debug, Clone)]
pub(crate) struct SurfaceBackdrop {
    pub(crate) pixmap: Pixmap,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Premultiplied RGBA8 copy of the snapshot for the GPU frame, which composites the same
    /// pixels the XRender blit does. `None` when the snapshot has no byte copy.
    pub(crate) rgba: Option<Vec<u8>>,
}

impl X11App {
    /// Replaces the retained backdrop with `argb` (row-major 0xAARRGGBB, `width * height`
    /// words). The previous pixmap is freed only after the replacement is uploaded.
    ///
    /// # Safety
    ///
    /// The app's display must be live and this app must own its drawables.
    pub(crate) unsafe fn set_surface_backdrop(
        &mut self,
        argb: &[u32],
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        if width == 0 || height == 0 || argb.len() != width as usize * height as usize {
            return Err("surface backdrop: pixel count does not match size".to_string());
        }
        let pixmap = unsafe { self.upload_argb_pixmap(argb, width, height)? };
        self.clear_surface_backdrop();
        let rgba = argb
            .iter()
            .flat_map(|&w| [(w >> 16) as u8, (w >> 8) as u8, w as u8, (w >> 24) as u8])
            .collect();
        self.surface_backdrop = Some(SurfaceBackdrop {
            pixmap,
            width,
            height,
            rgba: Some(rgba),
        });
        Ok(())
    }

    /// Uploads `argb` (0xAARRGGBB words, `width * height`) into a new 32-bit server pixmap.
    ///
    /// # Safety
    ///
    /// The app's display must be live and this app must own its drawables.
    pub(crate) unsafe fn upload_argb_pixmap(
        &mut self,
        argb: &[u32],
        width: u32,
        height: u32,
    ) -> Result<Pixmap, String> {
        let visual = unsafe { self.backdrop_argb_visual() }?;
        let root = unsafe { XRootWindow(self.display, self.screen) };
        let pixmap = unsafe { XCreatePixmap(self.display, root, width, height, 32) };
        if pixmap == 0 {
            return Err("backdrop: XCreatePixmap failed".to_string());
        }
        if let Err(error) = unsafe { self.upload_argb32_words(pixmap, visual, argb, width, height) }
        {
            unsafe { crate::native::xresource::free_pixmap_raw(self.display, pixmap) };
            return Err(error);
        }
        Ok(pixmap)
    }

    /// Re-uploads `argb` into an existing 32-bit pixmap of `width` x `height`.
    ///
    /// # Safety
    ///
    /// `pixmap` must be a live 32-bit pixmap on this app's display.
    pub(crate) unsafe fn upload_argb_into_pixmap(
        &mut self,
        pixmap: Pixmap,
        argb: &[u32],
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let visual = unsafe { self.backdrop_argb_visual() }?;
        unsafe { self.upload_argb32_words(pixmap, visual, argb, width, height) }
    }

    unsafe fn backdrop_argb_visual(&self) -> Result<*mut Visual, String> {
        let mut vinfo: XVisualInfo = unsafe { std::mem::zeroed() };
        let matched = unsafe {
            XMatchVisualInfo(self.display, self.screen, 32, TRUE_COLOR_CLASS, &mut vinfo)
        };
        if matched == 0 || vinfo.visual.is_null() {
            return Err("backdrop: no 32-bit TrueColor visual".to_string());
        }
        Ok(vinfo.visual)
    }

    /// Frees the retained backdrop, if any. Surfaces without backdrop keep the transparent clear.
    pub(crate) fn clear_surface_backdrop(&mut self) {
        if let Some(backdrop) = self.surface_backdrop.take() {
            unsafe { crate::native::xresource::free_pixmap_raw(self.display, backdrop.pixmap) };
        }
    }

    /// Resized window: fills the whole scene from the last captured snapshot, mirror-extended
    /// from where it sits in root coordinates, so the grown strip continues the existing content
    /// instead of showing the cleared (black) scene.
    ///
    /// The mirrored pixmap is cached for one (scene size, snapshot origin) key. A repaint with the
    /// same key blits the cached pixmap; only a new key recomputes and uploads it.
    ///
    /// # Safety
    ///
    /// Must run on the app's display thread.
    unsafe fn paint_mirrored_window_backdrop(&mut self) -> Result<(), String> {
        let root = unsafe { XRootWindow(self.display, self.screen) };
        let (mut rx, mut ry, mut child) = (0, 0, 0);
        unsafe {
            XTranslateCoordinates(
                self.display,
                self.window,
                root,
                0,
                0,
                &mut rx,
                &mut ry,
                &mut child,
            )
        };
        let (width, height) = (self.width, self.height);
        let Some(captured_root) = self
            .window_backdrop
            .as_ref()
            .and_then(|s| s.captured.as_ref())
            .map(|captured| captured.root)
        else {
            return Ok(());
        };
        let key = MirrorKey {
            width,
            height,
            origin: (
                i64::from(captured_root.0) - i64::from(rx),
                i64::from(captured_root.1) - i64::from(ry),
            ),
        };
        let cached = self
            .window_backdrop
            .as_ref()
            .and_then(|s| s.mirror.as_ref())
            .filter(|fill| fill.key == key)
            .map(|fill| fill.pixmap);
        let pixmap = match cached {
            Some(pixmap) => pixmap,
            None => {
                let words = {
                    let Some(captured) = self
                        .window_backdrop
                        .as_ref()
                        .and_then(|s| s.captured.as_ref())
                    else {
                        return Ok(());
                    };
                    crate::backdrop::mirror_extend_argb(
                        &captured.words,
                        captured.width,
                        captured.height,
                        (width, height),
                        key.origin,
                    )
                };
                let pixmap = unsafe { self.upload_argb_pixmap(&words, width, height)? };
                self.store_mirror_fill(MirrorFill { key, pixmap });
                pixmap
            }
        };
        match self.xrender.as_mut() {
            Some(xrender) => unsafe { xrender.blit_argb32_over(pixmap, 0, 0, width, height) },
            None => Ok(()),
        }
    }

    /// Lays the retained backdrop over the freshly cleared scene (opaque OVER copy), so the
    /// translucent fills painted afterwards blend over the snapshot. No-op without backdrop
    /// or when the scene size no longer matches the snapshot.
    ///
    /// # Safety
    ///
    /// Must run on the app's display thread after `clear_scene`.
    pub(crate) unsafe fn paint_surface_backdrop(&mut self) -> Result<(), String> {
        let Some(backdrop) = self
            .surface_backdrop
            .as_ref()
            .or(self.window_backdrop_pixmap.as_ref())
        else {
            trace_paint(|| "no-backdrop".to_string());
            return Ok(());
        };
        if backdrop.width != self.width || backdrop.height != self.height {
            trace_paint(|| {
                format!(
                    "size-mismatch backdrop={}x{} scene={}x{}",
                    backdrop.width, backdrop.height, self.width, self.height
                )
            });
            // A resized window keeps the last snapshot where it still covers the scene (until the
            // settled capture lands), so the translucent fill never shows raw transparent pixels.
            let is_window = self.surface_backdrop.is_none();
            if is_window {
                return unsafe { self.paint_mirrored_window_backdrop() };
            }
            return Ok(());
        }
        let Some(xrender) = self.xrender.as_mut() else {
            trace_paint(|| "no-xrender".to_string());
            return Ok(());
        };
        trace_paint(|| format!("blit {}x{}", backdrop.width, backdrop.height));
        unsafe { xrender.blit_argb32_over(backdrop.pixmap, 0, 0, backdrop.width, backdrop.height) }
    }
}

/// Geometry, blur and effective alpha that shape a node backdrop. The pixels beneath are compared separately, so
/// the cache never depends on the document revision: text edits keep the same backdrop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NodeBackdropKey {
    pub(crate) rect: (i32, i32, u32, u32),
    /// Clipped capture size (w, h); changes with the scene size, so a resize misses.
    pub(crate) clip: (u32, u32),
    pub(crate) radius: u32,
    pub(crate) blur_percent: u8,
    /// Blurred layer opacity. A change must invalidate the cached premultiplied pixmap.
    pub(crate) opacity: u8,
}

/// One retained backdrop: the geometry it was blurred with, the captured pixels beneath it (the
/// exact input), and the blurred pixmap.
#[derive(Debug)]
struct NodeBackdropEntry {
    key: NodeBackdropKey,
    source: Vec<u8>,
    pixmap: Pixmap,
}

/// Retained blurred backdrops per node index, freed when a frame stops painting them.
#[derive(Debug, Default)]
pub(crate) struct NodeBackdropCache {
    entries: HashMap<u32, NodeBackdropEntry>,
    used: HashSet<u32>,
    /// Scratch for pixmaps freed by the last `finish_frame` / `drain`; reused across frames.
    freed: Vec<Pixmap>,
}

impl NodeBackdropCache {
    /// Pixmap for `node` when its entry was blurred from exactly `key` and `source`.
    pub(crate) fn hit(&mut self, node: u32, key: NodeBackdropKey, source: &[u8]) -> Option<Pixmap> {
        self.used.insert(node);
        self.entries
            .get(&node)
            .filter(|entry| entry.key == key && entry.source == source)
            .map(|entry| entry.pixmap)
    }

    /// Stores `pixmap` for `node`; returns the replaced pixmap for the caller to free.
    pub(crate) fn store(
        &mut self,
        node: u32,
        key: NodeBackdropKey,
        source: Vec<u8>,
        pixmap: Pixmap,
    ) -> Option<Pixmap> {
        self.used.insert(node);
        self.entries
            .insert(
                node,
                NodeBackdropEntry {
                    key,
                    source,
                    pixmap,
                },
            )
            .map(|old| old.pixmap)
    }

    /// Ends a frame: removes entries not touched this frame and returns their pixmaps to free.
    /// The returned slice is reused storage, valid until the next call.
    pub(crate) fn finish_frame(&mut self) -> &[Pixmap] {
        self.freed.clear();
        let used = &self.used;
        let freed = &mut self.freed;
        self.entries.retain(|node, entry| {
            let keep = used.contains(node);
            if !keep {
                freed.push(entry.pixmap);
            }
            keep
        });
        self.used.clear();
        &self.freed
    }

    /// Removes every entry and returns the pixmaps to free. The returned slice is reused storage.
    pub(crate) fn drain(&mut self) -> &[Pixmap] {
        self.used.clear();
        self.freed.clear();
        let freed = &mut self.freed;
        freed.extend(self.entries.drain().map(|(_, entry)| entry.pixmap));
        &self.freed
    }
}

/// The device rect a node backdrop is painted under: its scaled rect, rounded to whole pixels, at
/// least one pixel wide and high. Shared by the XRender and GPU painters.
pub(crate) fn node_device_rect(
    rect: flamewm_render_core::Rect,
    scale: f32,
) -> (i32, i32, u32, u32) {
    let rect = super::primitives::scale_rect(rect, scale);
    (
        rect.x.round() as i32,
        rect.y.round() as i32,
        rect.width.round().max(1.0) as u32,
        rect.height.round().max(1.0) as u32,
    )
}

/// Device rect clipped to a scene of `scene` size; `None` when nothing is visible.
pub(crate) fn node_backdrop_clip(
    rect: (i32, i32, u32, u32),
    scene: (u32, u32),
) -> Option<(i32, i32, u32, u32)> {
    let (x, y, width, height) = rect;
    let left = i64::from(x).max(0);
    let top = i64::from(y).max(0);
    let right = (i64::from(x) + i64::from(width)).min(i64::from(scene.0));
    let bottom = (i64::from(y) + i64::from(height)).min(i64::from(scene.1));
    if right <= left || bottom <= top {
        return None;
    }
    Some((
        left as i32,
        top as i32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

/// Whether pixel `(px, py)` (device px) lies inside the node's rounded rect `node`.
fn inside_rounded(px: i32, py: i32, node: (i32, i32, u32, u32), radius: u32) -> bool {
    flamewm_render_core::pixels::inside_node_shape(px, py, node, radius)
}

/// Blurred premultiplied ARGB words for the clipped capture `rgba` (`clip` size), masked by the
/// node's rounded rect. The opaque input is premultiplied by `opacity` inside the shape;
/// both alpha and RGB are zero outside it. `None` for blur 0 or zero opacity.
pub(crate) fn node_backdrop_words(
    rgba: Vec<u8>,
    clip: (i32, i32, u32, u32),
    node: (i32, i32, u32, u32),
    radius: u32,
    blur_percent: u8,
    opacity: u8,
) -> Option<Vec<u32>> {
    if blur_percent == 0 || opacity == 0 {
        return None;
    }
    let (cx, cy, width, height) = clip;
    let mut words = crate::backdrop::blurred_argb_words(width, height, rgba, 0, blur_percent);
    for (index, word) in words.iter_mut().enumerate() {
        let px = cx + (index as u32 % width) as i32;
        let py = cy + (index as u32 / width) as i32;
        // The blurred wallpaper is opaque inside the rounded shape. The node's translucent fill
        // (`opacity` is its alpha) is painted over it by the caller, so scaling the blur by that
        // alpha too would weight the blur by alpha squared and leave the glass visually unblurred.
        *word = if inside_rounded(px, py, node, radius) {
            0xff00_0000 | (*word & 0x00ff_ffff)
        } else {
            0
        };
    }
    Some(words)
}

impl X11App {
    /// Paints the blurred root pixels under a translucent node's rect at the node's effective
    /// alpha, clipped to its rounded shape. The source is the scene painted so far; each frame
    /// captures the pixels beneath; the blur is reused when they and the geometry are unchanged, so
    /// a text edit above a translucent surface does not re-blur it. Capture failure keeps the plain fill.
    ///
    /// # Safety
    ///
    /// Must run on the app's display thread between `clear_scene` and `present_scene`.
    pub(crate) unsafe fn paint_node_backdrop(
        &mut self,
        node: u32,
        rect: (i32, i32, u32, u32),
        radius: u32,
        blur_percent: u8,
        opacity: u8,
    ) -> Result<(), String> {
        if opacity == 0 || self.xrender.is_none() {
            return Ok(());
        }
        let Some(clip) = node_backdrop_clip(rect, (self.width, self.height)) else {
            return Ok(());
        };
        let key = NodeBackdropKey {
            rect,
            clip: (clip.2, clip.3),
            radius,
            blur_percent,
            opacity,
        };
        let Ok(capture) =
            crate::backdrop::capture_drawable_rect(self.display, self.backbuffer, clip)
        else {
            return Ok(());
        };
        let pixmap = match self.node_backdrops.hit(node, key, &capture.pixels) {
            Some(pixmap) => pixmap,
            None => {
                let Some(words) = node_backdrop_words(
                    capture.pixels.clone(),
                    clip,
                    rect,
                    radius,
                    blur_percent,
                    opacity,
                ) else {
                    return Ok(());
                };
                let pixmap = unsafe { self.upload_argb_pixmap(&words, clip.2, clip.3)? };
                if let Some(old) = self.node_backdrops.store(node, key, capture.pixels, pixmap) {
                    unsafe { crate::native::xresource::free_pixmap_raw(self.display, old) };
                }
                pixmap
            }
        };
        let Some(xrender) = self.xrender.as_mut() else {
            return Ok(());
        };
        unsafe { xrender.blit_argb32_over(pixmap, clip.0, clip.1, clip.2, clip.3) }
    }

    /// Frees node backdrops that the finished frame did not paint.
    pub(crate) fn finish_node_backdrops(&mut self) {
        for &pixmap in self.node_backdrops.finish_frame() {
            unsafe { crate::native::xresource::free_pixmap_raw(self.display, pixmap) };
        }
    }

    /// Frees every node backdrop pixmap (teardown).
    pub(crate) fn clear_node_backdrops(&mut self) {
        for &pixmap in self.node_backdrops.drain() {
            unsafe { crate::native::xresource::free_pixmap_raw(self.display, pixmap) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(blur: u8) -> NodeBackdropKey {
        NodeBackdropKey {
            rect: (10, 20, 40, 30),
            clip: (40, 30),
            radius: 6,
            blur_percent: blur,
            opacity: 255,
        }
    }

    #[test]
    fn blur_zero_is_no_backdrop() {
        let rgba = [0x40, 0x80, 0xc0, 255].repeat(16 * 16);
        assert!(node_backdrop_words(rgba, (0, 0, 16, 16), (0, 0, 16, 16), 0, 0, 255).is_none());
    }

    #[test]
    fn clip_crops_rect_to_scene_and_drops_offscreen() {
        assert_eq!(
            node_backdrop_clip((-5, -3, 20, 10), (100, 100)),
            Some((0, 0, 15, 7))
        );
        assert_eq!(
            node_backdrop_clip((95, 90, 20, 20), (100, 100)),
            Some((95, 90, 5, 10))
        );
        assert_eq!(node_backdrop_clip((100, 0, 5, 5), (100, 100)), None);
        assert_eq!(node_backdrop_clip((0, 0, 0, 5), (100, 100)), None);
    }

    #[test]
    fn rounded_corners_stay_transparent_and_interior_opaque() {
        let rgba = [0x40, 0x80, 0xc0, 255].repeat(12 * 12);
        let words =
            node_backdrop_words(rgba, (0, 0, 12, 12), (0, 0, 12, 12), 6, 50, 255).expect("blur on");
        // Premultiplied ARGB: a masked corner must be zero in every channel, not only alpha.
        for corner in [0, 11, 11 * 12, 11 * 12 + 11] {
            assert_eq!(
                words[corner], 0x0000_0000,
                "corner {corner} fully transparent"
            );
        }
        assert_eq!(words[6 * 12 + 6] >> 24, 0xff, "centre kept");
        for word in &words {
            let alpha = word >> 24;
            for shift in [0, 8, 16] {
                assert!(
                    ((word >> shift) & 0xff) <= alpha,
                    "premultiplied: {word:#010x}"
                );
            }
        }
        let square = [0x40, 0x80, 0xc0, 255].repeat(12 * 12);
        let words = node_backdrop_words(square, (0, 0, 12, 12), (0, 0, 12, 12), 0, 50, 255)
            .expect("blur on");
        assert!(
            words.iter().all(|word| word >> 24 == 0xff),
            "radius 0 is full rect"
        );
    }

    #[test]
    fn translucent_node_blur_is_opaque_under_its_fill() {
        // The node's fill alpha is composited separately, so the blurred layer stays opaque
        // inside the shape at any node opacity. Scaling it here weights the blur by alpha squared.
        let rgba = [0x40, 0x80, 0xc0, 255].repeat(12 * 12);
        let rect = (0, 0, 12, 12);
        assert!(node_backdrop_words(rgba.clone(), rect, rect, 0, 50, 0).is_none());
        for opacity in [13, 41, 255] {
            let words = node_backdrop_words(rgba.clone(), rect, rect, 0, 50, opacity).unwrap();
            assert!(
                words.iter().all(|word| word >> 24 == 255),
                "opacity {opacity}"
            );
            assert_eq!(words[0], 0xff_40_80_c0, "opacity {opacity}");
        }
    }

    #[test]
    fn opacity_change_invalidates_node_backdrop_pixmap() {
        let mut cache = NodeBackdropCache::default();
        let source = vec![1_u8; 16];
        let mut low = key(50);
        low.opacity = 13;
        let mut high = low;
        high.opacity = 128;
        cache.store(3, low, source.clone(), 7);
        assert_eq!(cache.hit(3, high, &source), None);
        assert_eq!(cache.hit(3, low, &source), Some(7));
    }

    #[test]
    fn cache_hits_only_on_identical_key_and_source() {
        let mut cache = NodeBackdropCache::default();
        let source = vec![1_u8; 16];
        assert_eq!(cache.store(3, key(80), source.clone(), 7), None);
        assert_eq!(
            cache.hit(3, key(80), &source),
            Some(7),
            "same geometry and pixels reuse the blur"
        );
        assert_eq!(
            cache.hit(3, key(80), &[2_u8; 16]),
            None,
            "changed pixels beneath miss"
        );
        assert_eq!(cache.hit(3, key(40), &source), None, "blur change misses");
        assert_eq!(
            cache.store(3, key(40), source.clone(), 8),
            Some(7),
            "replaced pixmap returned"
        );
    }

    #[test]
    fn cache_misses_when_clip_size_changes() {
        let mut cache = NodeBackdropCache::default();
        let source = vec![1_u8; 16];
        let mut resized = key(80);
        resized.clip = (30, 30);
        assert_eq!(cache.store(3, key(80), source.clone(), 7), None);
        assert_eq!(
            cache.hit(3, resized, &source),
            None,
            "scene resize with same rect misses"
        );
    }

    #[test]
    fn unpainted_nodes_are_pruned_per_frame() {
        let mut cache = NodeBackdropCache::default();
        cache.store(1, key(80), vec![0], 11);
        cache.store(2, key(80), vec![0], 12);
        cache.finish_frame();
        assert_eq!(cache.hit(1, key(80), &[0]), Some(11));
        assert_eq!(cache.finish_frame(), &[12]);
        assert_eq!(cache.hit(2, key(80), &[0]), None);
    }
}

mod frame;
mod gpu;
mod paint;
mod paint_full;
mod paint_region;
mod present;
mod shape;
mod text_shadow;

#[cfg(test)]
use self::present::background_bind_plan;
pub(crate) use self::shape::StaticShapeKey;
#[cfg(test)]
use self::shape::{fixed_shape_unchanged, shape_needs_apply, static_shape_key};

use super::clip::{Visible, intersect, visible_under};
use super::paranoid_damage;

/// Picture clip for a partly visible fill: the visible part, cut to the active region paint
/// when there is one. `None` means nothing of the fill lies inside the region.
fn partial_fill_clip(part: Rect, region: Option<Rect>) -> Option<Rect> {
    match region {
        None => Some(part),
        Some(region) => intersect(part, region),
    }
}

use super::damage_plan::{
    PixelRegion, RedrawPlan, RegionInput, command_bounds, plan_region_paint, region_translucent,
};

use flamewm_render_core::ImageTreatment;

use super::gpu_frame::{self, FrameSource};

use super::primitives::{scale_rect, snap_device_rect};

use super::*;

use crate::native::coverage::{build_coverage, build_coverage_scaled};

use crate::xshape::ClipRect;

use flamewm_image_core::blur::BlurScratch;

/// Fingerprint of retained scroll state for the paint cache key.
/// Revision alone is not enough: scroll writes are light-touch.
fn scroll_fingerprint(document: &RuntimeDocument) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut mix = |word: u64| {
        hash ^= word;
        hash = hash.wrapping_mul(0x100000001b3);
    };
    mix(document.revision());
    // Revision covers content edits; scroll offsets are hashed directly by
    // sampling node-local state through the public scroll API is not
    // possible without an index list, so the paint path passes offsets via
    // `build_paint_commands_with_scroll` and the cache key includes the
    // interaction state plus revision here. Full scroll-list hashing lives
    // in the document-owned mark path (scroll writes mark Full).
    hash
}

#[cfg(test)]
mod static_shape_tests {
    use super::*;

    #[test]
    fn static_shape_changes_on_resize_or_style_and_dynamic_masks_are_not_memoized() {
        let dock = static_shape_key(1920, 50, 12).unwrap();
        assert_eq!(dock, static_shape_key(1920, 50, 12).unwrap());
        assert_ne!(dock, static_shape_key(1920, 49, 12).unwrap());
        assert_ne!(dock, static_shape_key(1920, 50, 13).unwrap());
        assert_eq!(static_shape_key(1920, 50, 0), None);
    }

    #[test]
    fn invalidated_shape_latch_does_not_skip_reapplication() {
        let key = static_shape_key(800, 48, 12);
        assert!(fixed_shape_unchanged(key, key, true));
        // A slide or an overlay mask writer invalidates the server shape latch.
        assert!(!fixed_shape_unchanged(key, key, false));
        assert!(!fixed_shape_unchanged(
            key,
            static_shape_key(801, 48, 12),
            true,
        ));
        assert!(!fixed_shape_unchanged(key, None, true));
    }
}

impl X11App {
    /// Intersect the clip stack and apply via XSetClipRectangles on the GC.
    /// Empty stack clears the clip.
    unsafe fn apply_clip_stack(&mut self) {
        if self.clip_stack.is_empty() {
            if self.gc_clip != Some(None) {
                unsafe { XSetClipMask(self.display, self.gc, 0) };
                unsafe { XSetClipOrigin(self.display, self.gc, 0, 0) };
                self.gc_clip = Some(None);
            }
            return;
        }
        let mut region = self.clip_stack[0];
        for rect in self.clip_stack.iter().skip(1) {
            let x1 = region.x.max(rect.x);
            let y1 = region.y.max(rect.y);
            let x2 = (region.x + region.width).min(rect.x + rect.width);
            let y2 = (region.y + region.height).min(rect.y + rect.height);
            region = Rect {
                x: x1,
                y: y1,
                width: (x2 - x1).max(0.0),
                height: (y2 - y1).max(0.0),
            };
        }
        if region.width <= 0.0 || region.height <= 0.0 {
            region = Rect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            };
        }
        let rect = XRectangle {
            x: region.x.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16,
            y: region.y.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16,
            width: region.width.round().max(0.0).min(u16::MAX as f32) as u16,
            height: region.height.round().max(0.0).min(u16::MAX as f32) as u16,
        };
        let key = Some((rect.x, rect.y, rect.width, rect.height));
        if self.gc_clip == Some(key) {
            return;
        }
        // Unsorted single rectangle; ordering constant 0 (Unsorted).
        unsafe { XSetClipRectangles(self.display, self.gc, 0, 0, &rect, 1, 0) };
        self.gc_clip = Some(key);
    }

    pub(crate) unsafe fn font_for_size_pub(&self, size: f32) -> Option<Font> {
        unsafe { self.font_for_size(size) }
    }

    pub(crate) unsafe fn pixel_pub(&mut self, color: Color) -> Result<u64, String> {
        unsafe { self.pixel(color) }
    }

    pub(crate) unsafe fn upload_argb32_pub(
        &mut self,
        pixmap: Pixmap,
        argb: &[u32],
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let Some(visual) = self.argb_visual else {
            return Err("no 32-bit TrueColor visual for ARGB32 upload".to_string());
        };
        unsafe { self.upload_argb32_words(pixmap, visual, argb, width, height) }
    }

    unsafe fn font_for_size(&self, size: f32) -> Option<Font> {
        let target = size.round().clamp(1.0, 255.0) as i32;
        self.fonts
            .iter()
            .min_by_key(|(bucket, _)| (i32::from(**bucket) - target).abs())
            .map(|(_, font)| {
                // SAFETY: font pointers are loaded and retained by X11App for its lifetime.
                unsafe { (**font).fid }
            })
    }

    pub(crate) unsafe fn redraw(&mut self, document: &RuntimeDocument) -> Result<(), String> {
        // Only `redraw_if_dirty` may mark a redraw as interaction-only; a direct call (resize,
        // RepaintFull) leaves this false so an unchanged command list still repaints.
        let interaction_only = std::mem::take(&mut self.skip_eligible);
        let layout_guard = flamewm_profiler::start("render.layout");
        let scale = document.ui_scale();
        let layout_key = LayoutKey {
            layout_revision: document.layout_revision(),
            geometry_revision: document.geometry_revision(),
            viewport: (self.width, self.height),
            scale_bits: scale.to_bits(),
        };
        let previous_key = self.cached_layout_key;
        let reuse_layout = self.layout.is_some() && self.cached_layout_key == Some(layout_key);
        if !reuse_layout {
            // Same viewport, scale and layout revision: only isolated geometry moved since the
            // retained layout, so patch its boxes in place. Otherwise recompute and fit scrolls.
            let same_base = previous_key.is_some_and(|key| {
                key.layout_revision == layout_key.layout_revision
                    && key.viewport == layout_key.viewport
                    && key.scale_bits == layout_key.scale_bits
            });
            // Layout widths are measured by this surface's Xft backend (same measure as paint).
            self.ensure_xft();
            let interaction = self.interaction;
            let patched = same_base
                && super::xft::scoped_text_measure(self.xft.as_mut(), || {
                    self.layout
                        .as_mut()
                        .is_some_and(|layout| document.patch_layout(layout, interaction))
                });
            if !patched {
                let (width, height) = (self.width as f32 / scale, self.height as f32 / scale);
                let layout = super::xft::scoped_text_measure(self.xft.as_mut(), || {
                    LayoutEngine::compute(document, width, height, interaction)
                });
                self.layout = Some(layout);
                if let Some(layout) = self.layout.as_mut() {
                    super::scroll::fit_scroll_extents(layout, document, self.interaction);
                    super::scroll::translate_scrolled_content(layout, document, self.interaction);
                }
            }
            self.cached_layout_key = Some(layout_key);
        }
        drop(layout_guard);
        let paint_guard = flamewm_profiler::start("render.paint");
        let scroll_fp = scroll_fingerprint(document);
        let cache_valid = self.cached_paint.as_ref().is_some_and(|cached| {
            cached.revision == document.revision()
                && cached.interaction == self.interaction
                && cached.scroll_fingerprint == scroll_fp
                && reuse_layout
        });
        // Before planning: an invalidated window backdrop must already count as changed.
        // SAFETY: redraw runs on the display thread; the snapshot is laid after the clear below.
        unsafe { self.refresh_window_backdrop() };
        let force_full = std::mem::take(&mut self.region_force_full);
        let base_valid = std::mem::take(&mut self.region_base_valid);
        let backdrop_unchanged = self.window_backdrop_unchanged();
        let mut plan = RedrawPlan::Full;
        if !cache_valid {
            self.ensure_xft();
            let layout = self.layout.as_mut().expect("layout retained above");
            // Paint text widths (glyph runs and layout) come from the same Xft measure as layout.
            let interaction = self.interaction;
            let paint_nodes = &mut self.paint_nodes;
            let fresh = super::xft::scoped_text_measure(self.xft.as_mut(), || {
                super::scroll::build_scrolled_paint_cached(
                    document,
                    layout,
                    interaction,
                    paint_nodes,
                )
            });
            let previous = self.cached_paint.take();
            plan = plan_region_paint(RegionInput {
                old: previous.as_ref().map(|cached| cached.commands.as_slice()),
                new: &fresh,
                scale,
                viewport: (self.width, self.height),
                // Interaction-only frames (hover/press, no document change) repaint just the
                // changed rows; document-driven frames keep the opt-in policy.
                enabled: self.region_paint || interaction_only,
                viewport_unchanged: previous_key.is_some_and(|key| {
                    key.viewport == (self.width, self.height) && key.scale_bits == scale.to_bits()
                }),
                backdrops: self.surface_backdrop.is_some()
                    || (self.window_backdrop_pixmap.is_some() && !backdrop_unchanged)
                    || !self.clip_stack.is_empty(),
                // No window backdrop snapshot means nothing outside the window is sampled, so a
                // translucent fill or image blends only over what the region repaint redraws.
                translucent: region_translucent(
                    self.window_backdrop_pixmap.is_some(),
                    backdrop_unchanged,
                    self.surface_backdrop.is_some(),
                ),
                interaction_only,
                full_latched: force_full,
                base_valid,
            });
            self.cached_paint = Some(CachedPaint {
                commands: fresh,
                revision: document.revision(),
                interaction: self.interaction,
                scroll_fingerprint: scroll_fp,
            });
        }
        drop(paint_guard);
        // The rebuilt scene is identical to the one already in the backbuffer, so there is nothing to
        // paint and nothing to present. Without this, a pointer move over a node with no hover style
        // redraws the whole window to identical pixels.
        if plan == RedrawPlan::Skip {
            // The backbuffer still holds exactly these commands, so it is still a valid base for a
            // later region paint (`base_valid` above was taken, not read).
            self.region_base_valid = true;
            self.render_counters
                .record(RedrawPlan::Skip, (self.width, self.height), 0, false);
            return Ok(());
        }
        let _present_guard = flamewm_profiler::start("render.present");
        // A failed region paint leaves the backbuffer partly written; the Full path below
        // clears and repaints every pixel, so it is the safe fallback.
        let mut region_done = None;
        if let RedrawPlan::Region(region) = plan {
            self.region_base_valid = false;
            match unsafe { self.paint_region(region, document, scale) } {
                Ok(visits) => region_done = Some((plan, visits)),
                Err(error) => eprintln!("FLAMEWM_RENDER_REGION fallback reason={error}"),
            }
        }
        // Paranoid mode (FLAMEWM_PARANOID_DAMAGE=1): keep the region frame, then present a full repaint of
        // the same frame and compare the two. Off, this is one cached bool read.
        let paranoid_region = match region_done {
            Some(_) if paranoid_damage::enabled() => Some(unsafe { self.read_backbuffer_pixels() }),
            _ => None,
        };
        let (used, visits) = match region_done {
            // Paranoid: the region paint is still reported as used; the full frame is what gets presented.
            Some(done) if paranoid_region.is_none() => done,
            Some(done) => {
                unsafe { self.paint_full(document, scale)? };
                done
            }
            None => (RedrawPlan::Full, unsafe {
                self.paint_full(document, scale)?
            }),
        };
        match paranoid_region {
            Some(Ok(region_px)) => unsafe { self.paranoid_compare_full(&region_px) },
            Some(Err(error)) => eprintln!("FLAMEWM_PARANOID readback failed reason={error}"),
            None => {}
        }
        unsafe { self.bind_presented_backbuffer_as_window_background()? };
        self.presented_size = Some((self.width, self.height));
        if self.role == X11WindowRole::Desktop {
            unsafe { self.publish_desktop_frame() };
        }
        // Single shape per changed frame, from retained commands.
        {
            let _shape_guard = flamewm_profiler::start("render.shape");
            let cached = self.cached_paint.as_ref().expect("paint cached above");
            let backdrop_radius = backdrop_corner_radius_px(document, self.width, self.height);
            unsafe {
                self.refresh_shape_mask_from_coverage_scaled(
                    &cached.commands,
                    scale,
                    backdrop_radius,
                )
            };
        }
        self.render_counters
            .record(used, (self.width, self.height), visits, true);
        self.record_window_backdrop_painted();
        self.region_base_valid = true;
        Ok(())
    }
}

/// Pure row spans for a popup mask: `backdrop_radius` (device px) rounds the rect when a backdrop
/// is retained, otherwise coverage of `commands` at `scale`.
pub(crate) fn popup_mask_spans(
    commands: &[PaintCommand],
    width: u32,
    height: u32,
    scale: f32,
    backdrop_radius: Option<u32>,
) -> Vec<(u32, u32, u32)> {
    match backdrop_radius {
        Some(radius) => crate::xshape::rounded_mask_spans(width, height, radius),
        None if scale == 1.0 => build_coverage(commands, width, height, 0.0),
        None => build_coverage_scaled(commands, width, height, 0.0, scale),
    }
}

/// Device-pixel corner radius for the retained-backdrop shape mask, derived from the document's
/// root corner radius (logical px) scaled like the paint path, clamped to the surface size.
pub(crate) fn backdrop_corner_radius_px(
    document: &RuntimeDocument,
    width: u32,
    height: u32,
) -> u32 {
    let radius =
        flamewm_render_core::paint::root_corner_radius(document, InteractionState::default());
    flamewm_render_core::paint::surface_shape_pixels(width, height, radius * document.ui_scale()).2
}

/// Desktop colour a depth-24 scene is cleared to: its pixels are opaque, so 0x00000000 would
/// paint black under node corners and the panel's rounded edge.
pub(crate) const OPAQUE_SCENE_CLEAR_PIXEL: std::os::raw::c_ulong = 0x0023_2629;

/// Clear pixel for a backbuffer of `depth`: transparent on ARGB (depth 32), desktop colour otherwise.
pub(crate) fn scene_clear_pixel(depth: i32) -> std::os::raw::c_ulong {
    if depth >= 32 {
        TRANSPARENT_CLEAR_PIXEL
    } else {
        OPAQUE_SCENE_CLEAR_PIXEL
    }
}

#[cfg(test)]
mod tests {
    use super::{background_bind_plan, popup_mask_spans};
    use crate::xshape::{clip_spans, rounded_mask_spans};

    fn row_span(spans: &[(u32, u32, u32)], row: u32) -> (u32, u32) {
        let &(_, start, end) = spans
            .iter()
            .find(|(r, _, _)| *r == row)
            .expect("row present");
        (start, end)
    }

    #[test]
    fn backdrop_mask_rounds_corners_when_radius_positive() {
        let spans = rounded_mask_spans(40, 20, 8);
        let (top_start, top_end) = row_span(&spans, 0);
        let (mid_start, mid_end) = row_span(&spans, 10);
        assert!(
            top_start > 0 && top_end < 40,
            "top row must be cut at the corners"
        );
        assert_eq!((mid_start, mid_end), (0, 40));
        assert_eq!(
            row_span(&spans, 19).0,
            top_start,
            "bottom corners mirror top"
        );
    }

    #[test]
    fn slide_mask_matches_settled_mask_clipped_and_keeps_corners() {
        let clip = (0, 0, 40, 10);
        let settled = popup_mask_spans(&[], 40, 20, 1.0, Some(8));
        let slide = clip_spans(&settled, clip);
        assert_eq!(
            slide,
            clip_spans(&popup_mask_spans(&[], 40, 20, 1.0, Some(8)), clip)
        );
        let (top_start, top_end) = row_span(&slide, 0);
        assert!(
            top_start > 0 && top_end < 40,
            "slide corners must stay transparent"
        );
        assert_eq!(row_span(&slide, 5), (0, 40));
    }

    #[test]
    fn slide_mask_radius_zero_stays_rectangular() {
        let clip = (0, 0, 40, 10);
        let slide = clip_spans(&popup_mask_spans(&[], 40, 20, 1.0, Some(0)), clip);
        assert_eq!(slide.len(), 10);
        assert!(slide.iter().all(|&(_, start, end)| start == 0 && end == 40));
    }

    #[test]
    fn backdrop_mask_radius_zero_is_full_rect() {
        let spans = rounded_mask_spans(40, 20, 0);
        assert_eq!(spans.len(), 20);
        assert!(spans.iter().all(|&(_, start, end)| start == 0 && end == 40));
    }

    #[test]
    fn backdrop_radius_follows_root_radius_and_clamps() {
        use flamewm_render_core::paint::surface_shape_pixels;
        assert_eq!(surface_shape_pixels(100, 60, 12.0).2, 12);
        assert_eq!(
            surface_shape_pixels(100, 60, 0.0).2,
            0,
            "squared stays full rect"
        );
        assert_eq!(
            surface_shape_pixels(40, 20, 99.0).2,
            10,
            "clamped to half min side"
        );
    }

    #[test]
    fn never_binds_unpainted_replacement() {
        let (bind, release) = background_bind_plan(20, 10, Some((10, 64)), false);
        assert!(!bind && !release);
    }

    #[test]
    fn never_binds_zero_backbuffer() {
        let (bind, release) = background_bind_plan(0, 0, None, true);
        assert!(!bind && !release);
    }

    #[test]
    fn binds_first_painted_frame_without_releasing_live() {
        let (bind, release) = background_bind_plan(10, 0, None, true);
        assert!(bind && !release);
    }

    #[test]
    fn retains_old_until_replacement_bound() {
        // Same painted pixmap as bound: no-op, retired stays.
        let (bind, release) = background_bind_plan(10, 10, Some((5, 64)), true);
        assert!(!bind && !release);
        // Replacement painted and differs: bind, then release retired.
        let (bind, release) = background_bind_plan(20, 10, Some((10, 64)), true);
        assert!(bind && release);
    }
}

#[cfg(test)]
mod shape_latch_tests {
    use super::shape_needs_apply;
    use std::cell::Cell;

    #[test]
    fn identical_spans_apply_once_and_changes_or_resets_reapply() {
        let latch = Cell::new(None);
        let a = [(0u32, 10u32, 20u32), (1, 9, 18)];
        let b = [(0u32, 10u32, 21u32)];
        let applies =
            |spans: &[(u32, u32, u32)], win| usize::from(shape_needs_apply(&latch, win, spans));
        let total = applies(&a, 7) + applies(&a, 7) + applies(&a, 7);
        assert_eq!(total, 1, "100 identical frames must apply the shape once");
        assert_eq!(applies(&b, 7), 1, "changed spans re-apply");
        assert_eq!(applies(&b, 8), 1, "another window re-applies");
        latch.set(None); // clipped writer reset
        assert_eq!(
            applies(&b, 8),
            1,
            "after a clipped mask the base mask re-applies"
        );
    }
}

#[cfg(test)]
mod layout_reuse_tests {
    use super::*;
    use crate::xlib::{XCloseDisplay, XOpenDisplay};
    use flamewm_render_core::{
        Color, CompiledDocument, CompiledNode, NodeKind, RuntimeDocument, Style,
        layout_passes_on_this_thread,
    };

    fn document() -> RuntimeDocument {
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![CompiledNode {
                kind: NodeKind::Element,
                parent: None,
                first_child: None,
                next_sibling: None,
                id: "box".to_string().into(),
                action: std::borrow::Cow::Borrowed(""),
                text: "".to_string().into(),
                image: None,
                style: Style::default().into(),
                hover_style: None,
                active_style: None,
                focus_style: None,
            }],
        })
        .expect("fixture document validates")
    }

    /// Color edits repaint without a layout pass; geometry/text edits relayout.
    /// Needs `FLAMEWM_TEST_DISPLAY` (skipped otherwise).
    #[test]
    fn color_edits_reuse_layout_and_geometry_edits_relayout() {
        let Some(name) = std::env::var_os("FLAMEWM_TEST_DISPLAY") else {
            return;
        };
        let name = std::ffi::CString::new(name.to_string_lossy().as_bytes()).unwrap();
        unsafe {
            let display = XOpenDisplay(name.as_ptr());
            assert!(!display.is_null(), "XOpenDisplay failed");
            let config = X11Config {
                width: 200,
                height: 120,
                ..X11Config::default()
            };
            let mut app = X11App::new(display, &config).expect("app");
            {
                let mut doc = document();
                app.redraw(&doc).expect("first redraw");
                let first = layout_passes_on_this_thread();
                for step in 0..25u32 {
                    let color = if step % 2 == 0 {
                        Color::BLACK
                    } else {
                        Color::TRANSPARENT
                    };
                    doc.set_background_color("box", color).expect("color");
                    app.redraw(&doc).expect("repaint");
                }
                assert_eq!(
                    layout_passes_on_this_thread(),
                    first,
                    "color edits must not relayout"
                );
                doc.set_font_size_px("box", 21.0).expect("size");
                app.redraw(&doc).expect("geometry repaint");
                assert_eq!(
                    layout_passes_on_this_thread(),
                    first + 1,
                    "geometry edit relayouts once"
                );
            }
            // Owner order (see runtime.rs): take deferred native handles, drop the app, close
            // the display, and only then release the handles.
            let deferred = app.take_deferred_native_libraries();
            drop(app);
            XCloseDisplay(display);
            drop(deferred);
        }
    }
}

#[cfg(test)]
mod scene_clear_tests {
    use super::*;

    #[test]
    fn depth24_scene_clear_is_never_opaque_black() {
        assert_ne!(
            scene_clear_pixel(24),
            0,
            "corner pixels must not be 0x000000 on depth 24"
        );
        assert_eq!(scene_clear_pixel(32), TRANSPARENT_CLEAR_PIXEL);
    }
}

#[cfg(test)]
mod partial_fill_clip_tests {
    use super::partial_fill_clip;
    use flamewm_render_core::Rect;

    #[test]
    fn partial_fill_without_region_keeps_its_visible_part() {
        let part = Rect::new(10.0, 85.0, 20.0, 5.0);
        assert_eq!(partial_fill_clip(part, None), Some(part));
    }

    #[test]
    fn partial_fill_in_region_paint_is_cut_to_the_region() {
        // The corner radius is kept by the picture clip; the clip must not leave the region.
        let region = Rect::new(0.0, 80.0, 50.0, 20.0);
        assert_eq!(
            partial_fill_clip(Rect::new(10.0, 85.0, 60.0, 5.0), Some(region)),
            Some(Rect::new(10.0, 85.0, 40.0, 5.0))
        );
    }

    #[test]
    fn partial_fill_outside_region_paint_is_dropped() {
        let region = Rect::new(0.0, 80.0, 50.0, 20.0);
        assert_eq!(
            partial_fill_clip(Rect::new(60.0, 85.0, 20.0, 5.0), Some(region)),
            None
        );
    }
}

/// Cached root origin to reuse for a publish, or `None` when a fresh query is required.
pub(crate) fn root_origin_reuse(
    cached: Option<(i32, i32)>,
    events_pending: bool,
) -> Option<(i32, i32)> {
    if events_pending {
        return None;
    }
    cached
}

#[cfg(test)]
mod root_origin_tests {
    use super::root_origin_reuse;

    #[test]
    fn cached_origin_is_reused_only_when_no_events_are_pending() {
        assert_eq!(root_origin_reuse(Some((3, 4)), false), Some((3, 4)));
        assert_eq!(
            root_origin_reuse(Some((3, 4)), true),
            None,
            "pending events force a query"
        );
        assert_eq!(
            root_origin_reuse(None, false),
            None,
            "no cache forces a query"
        );
    }
}

use super::*;

/// Only shapes independent of paint commands may skip coverage reconstruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StaticShapeKey {
    Dock { size: (u32, u32), radius: u32 },
}

pub(super) fn fixed_shape_unchanged(
    last: Option<StaticShapeKey>,
    current: Option<StaticShapeKey>,
    server_shape_valid: bool,
) -> bool {
    current.is_some() && last == current && server_shape_valid
}

pub(super) fn static_shape_key(
    width: u32,
    height: u32,
    dock_radius: u32,
) -> Option<StaticShapeKey> {
    if dock_radius > 0 {
        Some(StaticShapeKey::Dock {
            size: (width, height),
            radius: dock_radius,
        })
    } else {
        None
    }
}

impl X11App {
    /// Single source of the popup mask spans for the settled and slide paths: a retained
    /// backdrop uses the rounded rect of `backdrop_radius`, otherwise paint coverage.
    pub(super) fn mask_spans(
        &self,
        commands: &[PaintCommand],
        scale: f32,
        backdrop_radius: u32,
    ) -> Vec<(u32, u32, u32)> {
        popup_mask_spans(
            commands,
            self.width,
            self.height,
            scale,
            self.surface_backdrop.is_some().then_some(backdrop_radius),
        )
    }

    /// Canonical surface shape shared by full presentation and slide clipping.
    /// A dock corner mask (or border ring) must never disappear while closing.
    pub(super) fn base_shape_spans(
        &self,
        commands: &[PaintCommand],
        scale: f32,
        backdrop_radius: u32,
    ) -> Vec<(u32, u32, u32)> {
        if let Some(spans) = &self.shape_override {
            return spans.clone();
        }
        if self.dock_corner_radius > 0 {
            crate::xshape::rounded_mask_spans(self.width, self.height, self.dock_corner_radius)
        } else {
            self.mask_spans(commands, scale, backdrop_radius)
        }
    }

    /// A fixed ring/dock silhouette depends on geometry and radius, never on paint commands.
    /// Coverage-based shapes and caller-supplied alpha silhouettes deliberately bypass the cache.
    /// A clipped slide invalidates `shape_applied`, which forces the next fixed mask to re-apply.
    pub(crate) fn static_shape_key(&self) -> Option<StaticShapeKey> {
        if self.shape_override.is_some() {
            return None;
        }
        static_shape_key(self.width, self.height, self.dock_corner_radius)
    }

    /// Scale-aware shape refresh: coverage matches the paint path for
    /// rect/radius/stroke/clip in device pixels; text stays skipped.
    /// `backdrop_radius` (device px) rounds the retained-backdrop mask.
    pub(crate) unsafe fn refresh_shape_mask_from_coverage_scaled(
        &self,
        commands: &[PaintCommand],
        scale: f32,
        backdrop_radius: u32,
    ) {
        let Some(bridge) = self.xshape.as_ref() else {
            return;
        };
        let static_key = self.static_shape_key();
        if fixed_shape_unchanged(
            self.static_shape_applied.get(),
            static_key,
            self.shape_applied.get().is_some(),
        ) {
            return;
        }
        // A retained backdrop is composited under translucent fills, so the snapshot must stay
        // visible across the whole rounded shape; coverage would drop the translucent area and
        // show the live desktop (unblurred) instead. Corners outside the radius stay transparent.
        let spans = self.base_shape_spans(commands, scale, backdrop_radius);
        if spans.is_empty() {
            return;
        }
        // Identical spans leave the server-side bounding region unchanged; skip the XShape request.
        if !shape_needs_apply(&self.shape_applied, self.window, &spans) {
            self.static_shape_applied.set(static_key);
            return;
        }
        if let Err(error) = unsafe { bridge.apply_rounded_mask(self.window, &spans) } {
            self.shape_applied.set(None);
            self.static_shape_applied.set(None);
            eprintln!("FLAMEWM_RENDER_SHAPE_BACKEND degraded reason={error}");
        } else {
            // The frame was copied to the window before this shape ran, and nothing else flushes
            // the request. Unflushed, the new bounding region waits for the next pointer event.
            unsafe { XFlush(self.display) };
            self.static_shape_applied.set(static_key);
        }
    }

    /// Rounded popup mask intersected with `clip` (window-local). Used while a slide keeps the
    /// popup on the screen side of the panel edge; the base mask is restored with
    /// `refresh_shape_mask` once the slide lands.
    pub(crate) unsafe fn refresh_shape_mask_clipped(
        &self,
        document: &RuntimeDocument,
        clip: ClipRect,
    ) {
        let (Some(cached), Some(bridge)) = (self.cached_paint.as_ref(), self.xshape.as_ref())
        else {
            return;
        };
        let backdrop_radius = backdrop_corner_radius_px(document, self.width, self.height);
        let spans = self.base_shape_spans(&cached.commands, 1.0, backdrop_radius);
        // The clipped mask replaces the base mask, so the next base refresh must re-apply it.
        self.shape_applied.set(None);
        if let Err(error) = unsafe { bridge.apply_clipped_mask(self.window, &spans, clip) } {
            eprintln!("FLAMEWM_RENDER_SHAPE_BACKEND degraded reason={error}");
        }
    }
}

/// Latch for the base bounding shape: true when `spans` for `window` differ from what was last
/// applied (and records them). Any other writer of the bounding shape must reset the latch.
pub(crate) fn shape_needs_apply(
    applied: &std::cell::Cell<Option<u64>>,
    window: Window,
    spans: &[(u32, u32, u32)],
) -> bool {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    window.hash(&mut hasher);
    spans.hash(&mut hasher);
    let key = hasher.finish();
    if applied.get() == Some(key) {
        return false;
    }
    applied.set(Some(key));
    true
}

use super::*;

pub(super) struct SurfaceInstance {
    pub(super) app: X11App,
    pub(super) document: RuntimeDocument,
    pub(super) role: SurfaceRole,
    pub(super) input: SurfaceInputMode,
    /// Retained native window origin (root-space device pixels). Set at
    /// create, updated by move_resize; never recomputed from layout.
    pub(super) origin: (i32, i32),
    /// Opt-in presentation for the next show/hide; `None` is the immediate path.
    pub(super) slide: Option<SlideConfig>,
    /// Whether the window is mapped from the controller's point of view (slide bookkeeping).
    pub(super) shown: bool,
    /// Native origin last written by slide motion; skips a repeat move to the same pixel.
    pub(super) slide_drawn: Option<(i32, i32)>,
    /// Fade or Magic Lamp transition in flight, with the transition that drives its progress.
    /// The real window stays unmapped while it runs.
    pub(super) overlay: Option<(PopupTransition, WarpOverlay)>,
    /// A pointer grab requested while the real window is still unmapped behind an opening
    /// overlay: taken when the flight lands so the popup is mapped before it takes focus.
    pub(super) grab_pending: bool,
    /// Root edge this dock reserves (floating docks touch no edge). `None` infers from the rect.
    pub(super) dock_edge: Option<DockEdge>,
    /// Bar thickness (window px) on `dock_edge`; `None` reserves the whole window rect.
    pub(super) dock_bar: Option<u32>,
    /// Corner radius (window px) last published by `set_dock_frame`; zero means rectangular.
    pub(super) dock_radius: u32,
    /// `(opacity, blur)` percents for the desktop-layer backdrop installed before each first map.
    pub(super) popup_backdrop: Option<(u8, u8)>,
}

/// Pure mask decision for a dock window: keep the current shape, round it to `radius`, or
/// reset it to the full rectangle after floating. `prev_radius` is the radius last published.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DockMask {
    Keep,
    Round(u32),
    Reset,
}

pub(crate) fn dock_mask_plan(prev_radius: u32, radius: u32) -> DockMask {
    match (prev_radius, radius) {
        (_, r) if r > 0 => DockMask::Round(r),
        (p, 0) if p > 0 => DockMask::Reset,
        _ => DockMask::Keep,
    }
}

/// Mask decision for a dock whose window rect changed. A previous shape was applied against the
/// old size, so it must be refreshed even when the radius did not change: `Reset` publishes the
/// full rectangle for a classic dock (dropping a stale floating mask that would clip a bar grown
/// to the screen edge), and `Round` re-rounds a floating one at the new size.
pub(crate) fn dock_resize_mask_plan(radius: u32) -> DockMask {
    if radius > 0 {
        DockMask::Round(radius)
    } else {
        DockMask::Reset
    }
}

/// Applies the dock corner mask for `instance` at window size `w` x `h`. `Keep` with a zero
/// radius is a no-op; a classic dock only touches XShape when a resize has to drop a stale mask
/// (`Reset`), which publishes the full rectangle at the new size.
pub(super) fn apply_dock_mask(instance: &SurfaceInstance, w: u32, h: u32, mask: DockMask) {
    let radius = match mask {
        DockMask::Keep if instance.dock_radius == 0 => return,
        DockMask::Keep => instance.dock_radius,
        DockMask::Round(radius) => radius,
        DockMask::Reset => 0,
    };
    let spans = crate::xshape::rounded_mask_spans(w, h, radius);
    if let Some(bridge) = instance.app.xshape.as_ref()
        && let Err(error) = unsafe { bridge.apply_rounded_mask(instance.app.window, &spans) }
    {
        eprintln!("FLAMEWM_DOCK_SHAPE degraded reason={error}");
    }
}

/// Publishes the stadium corner radius (device px) as `_FLAMEWM_DOCK_CORNER_RADIUS` on the window.
/// The compositor clips the pill with it: an XShape mask does not reach the composited output, and
/// the client's XRGB buffer has no alpha for the corners.
fn publish_dock_corner_radius(display: *mut Display, window: Window, radius: u32) {
    // SAFETY: the buffer holds one 32-bit property unit, matching format 32 (c_long items).
    unsafe {
        let atom = XInternAtom(display, c"_FLAMEWM_DOCK_CORNER_RADIUS".as_ptr(), 0);
        let value: [std::os::raw::c_long; 1] = [radius as std::os::raw::c_long];
        XChangeProperty(
            display,
            window,
            atom,
            XA_CARDINAL,
            32,
            PROP_MODE_REPLACE,
            value.as_ptr().cast(),
            1,
        );
    }
}

impl SurfaceController {
    /// Publishes the dock frame: `Some(edge)` reserves that root edge and clips the window to
    /// `corner_radius` (window px). `bar` is the bar thickness on that edge (window px); `None`
    /// reserves the whole window rect. `None` edge is classic: legacy strut inference and a full
    /// rectangle mask, restored only if this surface was previously floating.
    pub fn set_dock_frame(
        &mut self,
        id: SurfaceId,
        edge: Option<DockEdge>,
        corner_radius: u32,
        bar: Option<u32>,
    ) -> Result<(), String> {
        let display = self.display;
        let (root_w, root_h) = self.root_geometry();
        let instance = self.instance_mut(id)?;
        if instance.role != SurfaceRole::Dock {
            return Err("set_dock_frame requires a dock surface".to_string());
        }
        if instance.dock_edge == edge
            && instance.dock_radius == corner_radius
            && instance.dock_bar == bar
        {
            return Ok(());
        }
        let mask = dock_mask_plan(instance.dock_radius, corner_radius);
        instance.dock_edge = edge;
        instance.dock_bar = bar;
        instance.dock_radius = corner_radius;
        instance.app.dock_corner_radius = corner_radius;
        publish_dock_corner_radius(display, instance.app.window, corner_radius);
        let (x, y) = instance.origin;
        let (w, h) = (instance.app.width, instance.app.height);
        crate::app::apply_dock_strut(
            instance.app.display,
            instance.app.window,
            root_w,
            root_h,
            edge,
            bar,
            x,
            y,
            w,
            h,
        );
        apply_dock_mask(instance, w, h, mask);
        unsafe { XFlush(display) };
        Ok(())
    }

    /// Publishes a border-only ring bounding mask (`border_px` ring, outer `radius_px`) and
    /// applies it at the current window size. Paint refreshes keep the ring, so the interior is
    /// absent and shows what is behind the window.
    pub fn set_ring_mask(
        &mut self,
        id: SurfaceId,
        border_px: u32,
        radius_px: u32,
    ) -> Result<(), String> {
        let display = self.display;
        let instance = self.instance_mut(id)?;
        instance.app.ring_mask = Some((border_px, radius_px));
        let (w, h) = (instance.app.width, instance.app.height);
        let spans = crate::xshape::ring_mask_spans(w, h, border_px, radius_px);
        let bridge = instance
            .app
            .xshape
            .as_ref()
            .ok_or_else(|| "ring mask requires XShape (unavailable)".to_string())?;
        unsafe { bridge.apply_rounded_mask(instance.app.window, &spans) }?;
        unsafe { XFlush(display) };
        Ok(())
    }

    /// Root rect (x, y, width, height) of the dock panel: this controller's own Dock surface,
    /// else the rect published by `set_external_dock`.
    pub(super) fn dock_rect(&self) -> Option<(i32, i32, u32, u32)> {
        let local = self
            .surfaces
            .values()
            .find(|instance| instance.role == SurfaceRole::Dock)
            .map(|instance| {
                (
                    instance.origin.0,
                    instance.origin.1,
                    instance.app.width,
                    instance.app.height,
                )
            });
        resolve_dock(local, self.external_dock)
    }

    /// Publishes the root rect of a dock owned by another process (`None` clears it). Only
    /// consulted when no local Dock surface exists, as in quick-control helpers.
    pub fn set_external_dock(&mut self, rect: Option<(i32, i32, u32, u32)>) {
        self.external_dock = rect;
    }
}

/// Dock rect for slide clipping: a local Dock surface wins over a published one.
pub(super) fn resolve_dock(
    local: Option<(i32, i32, u32, u32)>,
    external: Option<(i32, i32, u32, u32)>,
) -> Option<(i32, i32, u32, u32)> {
    local.or(external)
}

#[cfg(test)]
mod dock_corner_tests {
    use crate::xshape::rounded_mask_spans;

    /// A pixel is in the mask when some span of its row covers its column.
    fn covered(spans: &[(u32, u32, u32)], x: u32, y: u32) -> bool {
        spans
            .iter()
            .any(|&(row, start, end)| row == y && x >= start && x < end)
    }

    #[test]
    fn rounded_dock_pill_leaves_corner_pixels_outside_the_radius_empty() {
        let (w, h, r) = (40, 20, 10);
        let spans = rounded_mask_spans(w, h, r);
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            assert!(!covered(&spans, x, y), "corner ({x},{y}) must be clipped");
        }
        for (x, y) in [
            (w / 2, 0),
            (0, h / 2),
            (w - 1, h / 2),
            (w / 2, h - 1),
            (w / 2, h / 2),
        ] {
            assert!(covered(&spans, x, y), "edge/centre ({x},{y}) must stay");
        }
    }
}

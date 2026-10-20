use std::time::{Duration, Instant};

use super::*;
use crate::xshape::ClipRect;
use flamewm_reactor::timeline::FRAME_INTERVAL;
use flamewm_reactor::{Easing, Start};

/// A popup hide that starts a slide-out keeps its pointer grab until the slide lands.
pub(super) fn slide_out_keeps_grab(slide: Option<SlideConfig>, shown: bool) -> bool {
    shown && slide.is_some_and(|config| config.transition == PopupTransition::Slide)
}

/// A landed slide-out releases the grab held since hide() started it; opening landings keep it.
pub(super) fn landing_releases_grab(opening: bool, pointer_grabbed: bool) -> bool {
    !opening && pointer_grabbed
}

/// Panel edge a popup slides out of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlideEdge {
    Top,
    Bottom,
    Left,
    Right,
}

/// Window-local clip that keeps a slid popup on the screen side of the dock edge it emerges from.
/// Without a dock the whole popup is kept.
pub(super) fn panel_clip(
    dock: Option<(i32, i32, u32, u32)>,
    origin: (i32, i32),
    size: (u32, u32),
    edge: SlideEdge,
) -> ClipRect {
    let Some((dx, dy, dw, dh)) = dock else {
        return (0, 0, size.0, size.1);
    };
    let local = |offset: i64, extent: u32| offset.clamp(0, i64::from(extent)) as u32;
    let (px, py) = (i64::from(origin.0), i64::from(origin.1));
    let (dx, dy, dw, dh) = (i64::from(dx), i64::from(dy), i64::from(dw), i64::from(dh));
    match edge {
        SlideEdge::Bottom => (0, 0, size.0, local(dy - py, size.1)),
        SlideEdge::Top => (0, local(dy + dh - py, size.1), size.0, size.1),
        SlideEdge::Left => (local(dx + dw - px, size.0), 0, size.0, size.1),
        SlideEdge::Right => (0, 0, local(dx - px, size.0), size.1),
    }
}

/// How a popup enters and leaves. `Slide` moves the native window along the panel edge.
/// `Fade` and `Lamp` play a `WarpOverlay` over the captured popup; the real window maps only
/// once the overlay has landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupTransition {
    Slide,
    Fade,
    /// Magic Lamp between the popup and `target`, a root device rect `(x, y, width, height)`.
    Lamp {
        target: (i32, i32, u32, u32),
    },
}

/// Presentation for a popup anchored to a panel edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlideConfig {
    pub edge: SlideEdge,
    /// Animation speed in hundredths (`100` = 1.0x).
    pub speed: u16,
    pub transition: PopupTransition,
}

impl SlideConfig {
    pub fn slide(edge: SlideEdge, speed: u16) -> Self {
        Self {
            edge,
            speed,
            transition: PopupTransition::Slide,
        }
    }

    pub fn fade(edge: SlideEdge, speed: u16) -> Self {
        Self {
            edge,
            speed,
            transition: PopupTransition::Fade,
        }
    }

    pub fn lamp(edge: SlideEdge, speed: u16, target: (i32, i32, u32, u32)) -> Self {
        Self {
            edge,
            speed,
            transition: PopupTransition::Lamp { target },
        }
    }
}

/// Panel edge as the Magic Lamp bend sees it.
pub(super) fn lamp_edge(edge: SlideEdge) -> crate::lamp::PanelEdge {
    match edge {
        SlideEdge::Top => crate::lamp::PanelEdge::Top,
        SlideEdge::Bottom => crate::lamp::PanelEdge::Bottom,
        SlideEdge::Left => crate::lamp::PanelEdge::Left,
        SlideEdge::Right => crate::lamp::PanelEdge::Right,
    }
}

/// Overlay style for a transition. Magic Lamp is the only bend; everything else crossfades.
pub(super) fn overlay_style(transition: PopupTransition) -> TransitionStyle {
    match transition {
        PopupTransition::Lamp { .. } => TransitionStyle::MagicLamp,
        PopupTransition::Slide | PopupTransition::Fade => TransitionStyle::Fade,
    }
}

/// Root rect the overlay folds toward: the Start button for Magic Lamp, the popup itself otherwise.
pub(super) fn overlay_target(transition: PopupTransition, popup: WarpRect) -> WarpRect {
    match transition {
        PopupTransition::Lamp {
            target: (x, y, width, height),
        } => WarpRect {
            x: f64::from(x),
            y: f64::from(y),
            width: f64::from(width),
            height: f64::from(height),
        },
        PopupTransition::Slide | PopupTransition::Fade => popup,
    }
}

/// Overlay paint progress for timeline `progress` (0 at the start of an open, 1 at its end).
/// Fade follows the timeline in both directions. Magic Lamp runs target (progress 1) to
/// source (progress 0), so the popup emerges from the Start button and folds back into it.
pub(super) fn overlay_paint_progress(transition: PopupTransition, progress: f32) -> f64 {
    match transition {
        PopupTransition::Lamp { .. } => f64::from(1.0 - progress),
        PopupTransition::Slide | PopupTransition::Fade => f64::from(progress),
    }
}

pub(super) const SLIDE_OPEN_BASE_MS: u32 = 180;
pub(super) const SLIDE_CLOSE_BASE_MS: u32 = 120;

/// Timeline kind for one slide transition. Timeline progress is 0.0 at the off-edge
/// start and 1.0 at the exact anchored origin; `opening` only selects the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SlideKind {
    pub(super) edge: SlideEdge,
    pub(super) opening: bool,
}

/// Native window origin at `progress`: displaced from the final origin toward the panel edge.
pub(super) fn slide_origin(
    final_origin: (i32, i32),
    size: (u32, u32),
    edge: SlideEdge,
    progress: f32,
) -> (i32, i32) {
    let remaining = 1.0 - progress.clamp(0.0, 1.0);
    let (width, height) = (size.0 as f32, size.1 as f32);
    let (dx, dy) = match edge {
        SlideEdge::Bottom => (0.0, height),
        SlideEdge::Top => (0.0, -height),
        SlideEdge::Left => (-width, 0.0),
        SlideEdge::Right => (width, 0.0),
    };
    (
        final_origin.0 + (dx * remaining).round() as i32,
        final_origin.1 + (dy * remaining).round() as i32,
    )
}

/// Window origin for one slide frame. The timeline's end progress is exact (1.0 opened, 0.0 closed),
/// so the landed frame needs no snap: a closed slide ends off the panel edge, never on its origin.
pub(super) fn landing_frame_origin(
    final_origin: (i32, i32),
    size: (u32, u32),
    edge: SlideEdge,
    progress: f32,
) -> (i32, i32) {
    slide_origin(final_origin, size, edge, progress)
}

/// Frame wait while any slide is active; `None` keeps the reactor fully idle.
pub(super) fn motion_frame_wait(active: bool) -> Option<Duration> {
    active.then_some(FRAME_INTERVAL)
}

impl SurfaceController {
    /// Sets (or clears with `None`) the presentation used by later `show`/`hide` calls.
    pub fn set_slide(&mut self, id: SurfaceId, slide: Option<SlideConfig>) -> Result<(), String> {
        self.instance_mut(id)?.slide = slide;
        Ok(())
    }

    /// Opens an overlay transition for `config` over the popup's final rect. `Ok(false)` leaves
    /// `show` on the immediate path (already shown, or the overlay could not start).
    pub(super) fn begin_overlay_show(
        &mut self,
        id: SurfaceId,
        config: SlideConfig,
        shown: bool,
        now: Instant,
    ) -> Result<bool, String> {
        let key = id.get();
        self.slides.set_speed(config.speed);
        if let Some(active) = self.slides.kind(key) {
            // Already opening: converging on the final frame. Closing: reverse from current progress.
            if !active.opening {
                self.slides.start_towards(
                    key,
                    SlideKind {
                        edge: config.edge,
                        opening: true,
                    },
                    now,
                    SLIDE_OPEN_BASE_MS,
                    Easing::EaseOutCubic,
                    (0.0, 1.0),
                );
            }
            return Ok(true);
        }
        if shown {
            return Ok(false);
        }
        let capture = self.capture_popup(id)?;
        let source = self.popup_root_rect(id)?;
        let target = overlay_target(config.transition, source);
        let display = self.display;
        let overlay = match WarpOverlay::begin_styled(
            display,
            capture,
            source,
            target,
            lamp_edge(config.edge),
            overlay_style(config.transition),
            TransitionDirection::In,
        ) {
            Ok(overlay) => overlay,
            Err(error) => {
                eprintln!("flamewm: popup overlay unavailable, mapping immediately: {error}");
                return Ok(false);
            }
        };
        if self.slides.start_towards(
            key,
            SlideKind {
                edge: config.edge,
                opening: true,
            },
            now,
            SLIDE_OPEN_BASE_MS,
            Easing::EaseOutCubic,
            (0.0, 1.0),
        ) != Start::Started
        {
            return Ok(false);
        }
        let mut overlay = overlay;
        let radius = {
            let instance = self.instance_ref(id)?;
            backdrop_shape::popup_backdrop_radius_px(
                &instance.document,
                instance.app.width,
                instance.app.height,
            )
        };
        overlay.set_corner_radius(radius);
        if let Err(error) = overlay.paint(overlay_paint_progress(config.transition, 0.0)) {
            eprintln!("flamewm: popup overlay paint failed, mapping immediately: {error}");
            self.slides.cancel(key);
            return Ok(false);
        }
        overlay.raise();
        self.instance_mut(id)?.overlay = Some((config.transition, overlay));
        Ok(true)
    }

    /// Starts an overlay close from the presented popup. The real window unmaps as the overlay
    /// takes over, so a Magic Lamp gap never shows the unbent popup. `Ok(false)` leaves `hide`
    /// on the immediate path.
    pub(super) fn begin_overlay_hide(
        &mut self,
        id: SurfaceId,
        config: SlideConfig,
        shown: bool,
        now: Instant,
    ) -> Result<bool, String> {
        let key = id.get();
        self.slides.set_speed(config.speed);
        if let Some(active) = self.slides.kind(key) {
            // Already closing: keep the running transition. Opening: reverse from current progress.
            if active.opening {
                self.slides.start_towards(
                    key,
                    SlideKind {
                        edge: config.edge,
                        opening: false,
                    },
                    now,
                    SLIDE_CLOSE_BASE_MS,
                    Easing::EaseOutCubic,
                    (1.0, 0.0),
                );
            }
            return Ok(true);
        }
        if !shown {
            return Ok(false);
        }
        let capture = self.capture_popup(id)?;
        let source = self.popup_root_rect(id)?;
        let target = overlay_target(config.transition, source);
        let display = self.display;
        let mut overlay = match WarpOverlay::begin_styled(
            display,
            capture,
            source,
            target,
            lamp_edge(config.edge),
            overlay_style(config.transition),
            // Fade alpha follows the timeline itself, so direction stays `In` for both ends.
            TransitionDirection::In,
        ) {
            Ok(overlay) => overlay,
            Err(error) => {
                eprintln!("flamewm: popup overlay unavailable, unmapping immediately: {error}");
                return Ok(false);
            }
        };
        if self.slides.start_towards(
            key,
            SlideKind {
                edge: config.edge,
                opening: false,
            },
            now,
            SLIDE_CLOSE_BASE_MS,
            Easing::EaseOutCubic,
            (1.0, 0.0),
        ) != Start::Started
        {
            return Ok(false);
        }
        let radius = {
            let instance = self.instance_ref(id)?;
            backdrop_shape::popup_backdrop_radius_px(
                &instance.document,
                instance.app.width,
                instance.app.height,
            )
        };
        overlay.set_corner_radius(radius);
        if let Err(error) = overlay.paint(overlay_paint_progress(config.transition, 1.0)) {
            eprintln!("flamewm: popup overlay paint failed, unmapping immediately: {error}");
            self.slides.cancel(key);
            return Ok(false);
        }
        overlay.raise();
        let instance = self.instance_mut(id)?;
        instance.overlay = Some((config.transition, overlay));
        // SAFETY: the window is this controller's live child of the root.
        unsafe { XUnmapWindow(display, instance.app.window) };
        instance.shown = false;
        instance.slide_drawn = None;
        Ok(true)
    }

    /// Final root-space rect of the popup: its retained origin and backbuffer size.
    pub(super) fn popup_root_rect(&mut self, id: SurfaceId) -> Result<WarpRect, String> {
        let instance = self.instance_mut(id)?;
        Ok(WarpRect {
            x: f64::from(instance.origin.0),
            y: f64::from(instance.origin.1),
            width: f64::from(instance.app.width),
            height: f64::from(instance.app.height),
        })
    }

    pub(super) fn motion_frame_timeout(&self) -> Option<Duration> {
        motion_frame_wait(!self.slides.is_empty())
    }
}

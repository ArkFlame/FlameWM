//! Serialized snap-preview overlay (J11, J09 border-only).
//!
//! The preview is a compiled `snap-preview.html` document hosted on a
//! `SurfaceRole::Overlay` + `SurfaceInputMode::PassThrough` surface, hidden
//! at init. It never duplicates snap math: candidates come from
//! `flamewm_window_core::snap_target` and geometry from
//! `flamewm_window_core::snap_geometry`, both resolved by the caller in
//! `wm.rs`. This module only owns surface lifecycle and the skip-redraw
//! guard (no polling, no timers).
//!
//! Hot path is border/outline only: `update` positions the pass-through
//! surface at the candidate rect and styles the existing selection-style
//! outline/border material (`border` + `background_clear` on `FILL_NODE`).
//! It performs no synchronous root image capture here
//! (`backdrop::capture_root_rect_auto` opens its own display and runs
//! `XGetImage` plus a pixel loop under `render.overlay.capture`); the
//! backdrop image node stays hidden during the hot path and is installed only
//! after a matching worker result reaches the WM thread.
//!
//! Backdrop readback is an asynchronous worker boundary: motion uses `try_send`
//! into a bounded latest-state mailbox (the `sync_channel`-style contract is
//! latest/coalesce, not an unbounded queue), while `std::thread::spawn` owns
//! `capture_root_rect_auto`. Every request carries a `generation`, candidate,
//! geometry, and opacity; stale results are fenced/cancelled by that state.
//!
//! Opacity: the compiled CSS bakes the 20% default fill
//! (`--snap-fill:#ef404833`). The runtime updates the process-local projection
//! when the authoritative settings snapshot changes; the existing WM caller
//! continues to pass the default when it has no direct settings reference.

mod worker;

use std::sync::atomic::{AtomicU8, Ordering};

use flamewm_api::Rect;
use flamewm_render_x11::backdrop;
use flamewm_ui_x11::{
    RuntimeImage, SurfaceConfig, SurfaceHandle, SurfaceInputMode, SurfaceRole, SurfaceRuntime,
    UiColor, UiDocumentAccess, UiTemplate, decode_document,
};

use flamewm_window_core::SnapTarget;

const COMPILED_UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-snap-preview.rwr"));

/// Default fill opacity used by the WM caller before a settings projection arrives.
pub const DEFAULT_PREVIEW_OPACITY_PERCENT: u8 = 20;

static LIVE_PREVIEW_OPACITY_PERCENT: AtomicU8 = AtomicU8::new(DEFAULT_PREVIEW_OPACITY_PERCENT);

/// Publish the validated settings projection used by the WM-owned preview.
pub(crate) fn set_live_opacity(opacity_percent: u8) {
    LIVE_PREVIEW_OPACITY_PERCENT.store(opacity_percent.min(60), Ordering::Release);
}

fn live_opacity() -> u8 {
    LIVE_PREVIEW_OPACITY_PERCENT.load(Ordering::Acquire)
}

fn effective_opacity(requested: u8) -> u8 {
    if requested == DEFAULT_PREVIEW_OPACITY_PERCENT {
        live_opacity()
    } else {
        requested
    }
}

/// Accent red backing the preview border/fill (`#ef4048`): shared with
/// the desktop-selection material owner, one material two consumers.
pub const ACCENT_RGB: (u8, u8, u8) = backdrop::BACKDROP_ACCENT_RGB;

/// Node in the compiled document that carries the preview fill.
const FILL_NODE: &str = "snap-preview";

/// Image node from the compiled document: always kept hidden in the
/// J09 border-only flow so no opaque full rect is ever shown.
const BACKDROP_NODE: &str = "snap-preview-backdrop";

/// Strong accent border over the cleared fill (shared accent family).
fn border_color() -> UiColor {
    UiColor {
        r: ACCENT_RGB.0,
        g: ACCENT_RGB.1,
        b: ACCENT_RGB.2,
        a: backdrop::BACKDROP_BORDER_ALPHA,
    }
}

/// Translucent accent fill for an opacity percent. Pure and headless-testable.
/// Delegates to the shared desktop-selection material owner.
#[cfg(test)]
#[must_use]
pub fn fill_color(opacity_percent: u8) -> UiColor {
    let (r, g, b, a) = backdrop::fill_color(opacity_percent);
    UiColor { r, g, b, a }
}

fn checked_size(value: i32) -> Option<u32> {
    u32::try_from(value).ok().filter(|size| *size > 0)
}

/// Serialized preview surface: hidden until a drag candidate arrives.
pub struct SnapPreviewSurface {
    runtime: Option<SurfaceRuntime>,
    surface: Option<SurfaceHandle>,
    worker: worker::SnapPreviewWorker,
    generation: u64,
    visible: bool,
    candidate: Option<SnapTarget>,
    geometry: Option<Rect>,
    opacity: u8,
}

impl SnapPreviewSurface {
    #[must_use]
    pub fn new() -> Self {
        Self {
            runtime: None,
            surface: None,
            worker: worker::SnapPreviewWorker::new(),
            generation: 1,
            visible: false,
            candidate: None,
            geometry: None,
            opacity: live_opacity(),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn visible(&self) -> bool {
        self.visible
    }

    #[cfg(test)]
    #[must_use]
    pub fn candidate(&self) -> Option<SnapTarget> {
        self.candidate
    }

    #[cfg(test)]
    #[must_use]
    pub fn geometry(&self) -> Option<Rect> {
        self.geometry
    }

    /// True when candidate, geometry, and palette are unchanged and the
    /// surface is already shown, so the caller can skip the redraw.
    #[must_use]
    pub fn is_current(&self, candidate: SnapTarget, geometry: Rect, opacity: u8) -> bool {
        self.visible
            && self.candidate == Some(candidate)
            && self.geometry == Some(geometry)
            && self.opacity == effective_opacity(opacity)
    }

    /// Show or move the preview for a live drag candidate. `None` hides.
    /// Total without a display: records intent, leaves `visible` false.
    pub fn update(&mut self, candidate: Option<SnapTarget>, geometry: Option<Rect>, opacity: u8) {
        let opacity = effective_opacity(opacity);
        let (Some(candidate), Some(geometry)) = (candidate, geometry) else {
            self.hide();
            return;
        };
        if self.is_current(candidate, geometry, opacity) {
            worker::record_request_coalesced();
            if let Some(request) = self.current_request() {
                worker::emit_result_trace("coalesce", request, || {
                    "source=surface.current_state".to_owned()
                });
            }
            return;
        }
        let Some((width, height)) = checked_size(geometry.width).zip(checked_size(geometry.height))
        else {
            worker::record_failure(worker::FailureStage::InvalidGeometry);
            self.hide_with_reason(worker::HideReason::InvalidGeometry);
            return;
        };
        let same_intent = self.candidate == Some(candidate)
            && self.geometry == Some(geometry)
            && self.opacity == opacity;
        if !same_intent {
            // Invalidate the old surface/result before publishing the next
            // request. The worker may still be finishing the prior XGetImage;
            // its generation envelope keeps that result stale for J26.
            self.hide_with_reason(worker::HideReason::Replace);
            let generation = self.generation;
            let _ = self.worker.try_send(worker::BackdropRequest {
                generation,
                candidate,
                geometry,
                opacity,
                capture: (geometry.x, geometry.y, width, height),
            });
            self.candidate = Some(candidate);
            self.geometry = Some(geometry);
            self.opacity = opacity;
        } else {
            worker::record_request_coalesced();
            if let Some(request) = self.current_request() {
                worker::emit_result_trace("coalesce", request, || {
                    "source=surface.pending_state".to_owned()
                });
            }
        }
        if !self.ensure_surface() {
            self.hide_with_reason(worker::HideReason::SurfaceFailure);
            self.visible = false;
            return;
        }
        let (Some(runtime), Some(surface)) = (self.runtime.as_mut(), self.surface) else {
            worker::record_failure(worker::FailureStage::SurfaceUnavailable);
            self.hide_with_reason(worker::HideReason::SurfaceFailure);
            self.visible = false;
            return;
        };
        if runtime
            .move_resize(surface, geometry.x, geometry.y, width, height)
            .is_err()
        {
            worker::record_failure(worker::FailureStage::MoveResize);
            self.hide_with_reason(worker::HideReason::MoveResizeFailure);
            return;
        }
        // J09 border-only flow, gated by is_current above: no synchronous
        // root image capture in the hot path (no XOpenDisplay/XGetImage
        // pixel loop under render.overlay.capture). Style the existing
        // selection-style outline/border material on FILL_NODE
        // (`border` + `background_clear`: border/outline geometry only, no
        // opaque full rect) and keep the backdrop image node hidden.
        let styled = runtime.with_document(surface, |document| {
            document
                .border(FILL_NODE, border_color())
                .map_err(|error: String| error)?;
            document
                .background_clear(FILL_NODE)
                .map_err(|error: String| error)?;
            document
                .visible(BACKDROP_NODE, false)
                .map_err(|error: String| error)?;
            Ok(())
        });
        if styled.is_err() {
            worker::record_failure(worker::FailureStage::Style);
            self.hide_with_reason(worker::HideReason::StyleFailure);
            return;
        }
        if runtime.redraw(surface).is_err() {
            worker::record_failure(worker::FailureStage::Redraw);
            self.hide_with_reason(worker::HideReason::RedrawFailure);
            return;
        }
        if runtime.show(surface).is_err() {
            worker::record_failure(worker::FailureStage::Show);
            self.hide_with_reason(worker::HideReason::ShowFailure);
            return;
        }
        let _ = runtime.raise(surface);
        self.visible = true;
    }

    /// Raw worker wake source. The WM registers this once; the callback only
    /// marks readiness, while `drain_worker` consumes it at a loop boundary.
    #[must_use]
    pub(crate) fn wake_fd(&self) -> std::os::fd::RawFd {
        self.worker.wake_fd()
    }

    /// Consume worker readiness and apply only the result for the current
    /// preview intent. All surface and document mutations stay on the WM
    /// thread; stale and failed captures leave the preview hidden.
    pub(crate) fn drain_worker(&mut self) {
        {
            let _guard = worker::start_wake_drain();
            self.worker.drain_wake();
        }
        let Some(result) = ({
            let _guard = worker::start_result_take();
            self.worker.take_result()
        }) else {
            return;
        };
        let Some(request) = self.current_request() else {
            worker::emit_result_trace("stale_drop", result.request, || {
                "reason=no-current-request".to_owned()
            });
            worker::record_result_stale_stage();
            worker::record_result_stale_drop();
            return;
        };
        let matches = {
            let match_started = std::time::Instant::now();
            let _guard = worker::start_result_match();
            let matches = result.matches(request);
            worker::emit_result_trace("match", result.request, || {
                format!(
                    "outcome={} expected={} duration_us={}",
                    if matches { "match" } else { "stale" },
                    worker::request_identity(request),
                    match_started.elapsed().as_micros(),
                )
            });
            matches
        };
        worker::record_result_match(matches);
        if !matches {
            worker::emit_result_trace("stale_drop", result.request, || {
                format!(
                    "reason=exact-fence expected={}",
                    worker::request_identity(request)
                )
            });
            worker::record_result_stale_stage();
            worker::record_result_stale_drop();
            return;
        }
        let apply_started = std::time::Instant::now();
        let Some(pixels) = result.pixels else {
            worker::record_failure(worker::FailureStage::ApplyError);
            emit_snap_failure(&result);
            worker::emit_result_trace("failure", result.request, || {
                format!(
                    "operation=apply duration_us={} error={}",
                    apply_started.elapsed().as_micros(),
                    worker::compact_error(
                        result
                            .error
                            .as_deref()
                            .unwrap_or("backdrop result contained no pixels")
                    )
                )
            });
            self.hide_with_reason(worker::HideReason::ApplyError);
            return;
        };
        let (Some(runtime), Some(surface)) = (self.runtime.as_mut(), self.surface) else {
            worker::record_failure(worker::FailureStage::ApplyError);
            worker::emit_result_trace("failure", result.request, || {
                format!(
                    "operation=apply duration_us={} error=surface-unavailable",
                    apply_started.elapsed().as_micros()
                )
            });
            self.hide_with_reason(worker::HideReason::ApplyError);
            return;
        };
        let updated = runtime.with_document(surface, |document| {
            document.image_rgba8(
                BACKDROP_NODE,
                RuntimeImage {
                    source: "snap-preview-backdrop".to_owned(),
                    width: result.width,
                    height: result.height,
                    pixels,
                },
            )?;
            document.visible(BACKDROP_NODE, true)?;
            Ok(())
        });
        if let Err(error) = updated {
            worker::record_failure(worker::FailureStage::ApplyError);
            worker::emit_result_trace("failure", result.request, || {
                format!(
                    "operation=document duration_us={} error={}",
                    apply_started.elapsed().as_micros(),
                    worker::compact_error(&format!("{error:?}"))
                )
            });
            self.hide_with_reason(worker::HideReason::ApplyError);
            return;
        }
        if let Err(error) = runtime.redraw(surface) {
            worker::record_failure(worker::FailureStage::ApplyError);
            worker::record_stage(worker::FailureStage::Redraw);
            worker::emit_result_trace("failure", result.request, || {
                format!(
                    "operation=redraw duration_us={} error={}",
                    apply_started.elapsed().as_micros(),
                    worker::compact_error(&format!("{error:?}"))
                )
            });
            self.hide_with_reason(worker::HideReason::RedrawFailure);
            return;
        }
        if let Err(error) = runtime.show(surface) {
            worker::record_failure(worker::FailureStage::ApplyError);
            worker::record_stage(worker::FailureStage::Show);
            worker::emit_result_trace("failure", result.request, || {
                format!(
                    "operation=show duration_us={} error={}",
                    apply_started.elapsed().as_micros(),
                    worker::compact_error(&format!("{error:?}"))
                )
            });
            self.hide_with_reason(worker::HideReason::ShowFailure);
            return;
        }
        let _ = runtime.raise(surface);
        self.visible = true;
        worker::record_result_apply();
        worker::emit_result_trace("apply", result.request, || {
            format!(
                "outcome=ok duration_us={} pixels={}",
                apply_started.elapsed().as_micros(),
                result.width as usize * result.height as usize * 4
            )
        });
    }

    /// Hide on release, cancel, unmanage, or shutdown. Never fails.
    pub fn hide(&mut self) {
        self.hide_with_reason(worker::HideReason::Request);
    }

    fn hide_with_reason(&mut self, reason: worker::HideReason) {
        let generation_before = self.generation;
        let candidate = self.candidate;
        let geometry = self.geometry;
        let opacity = self.opacity;
        worker::record_hide_reason(reason);
        if let (Some(runtime), Some(surface)) = (self.runtime.as_mut(), self.surface) {
            let _ = runtime.hide(surface);
        }
        self.generation = self.generation.wrapping_add(1).max(1);
        worker::emit_hide_trace(
            generation_before,
            self.generation,
            candidate,
            geometry,
            opacity,
            reason,
        );
        self.visible = false;
        self.candidate = None;
        self.geometry = None;
    }

    /// Lazily create the overlay surface. `false` without a display.
    fn ensure_surface(&mut self) -> bool {
        if self.surface.is_some() {
            return true;
        }
        let mut runtime = match SurfaceRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => {
                worker::record_failure(worker::FailureStage::SurfaceEnsure);
                return false;
            }
        };
        let document = match decode_document(COMPILED_UI) {
            Ok(document) => document,
            Err(_) => {
                worker::record_failure(worker::FailureStage::SurfaceEnsure);
                return false;
            }
        };
        let config = SurfaceConfig {
            width: 1,
            height: 1,
            title: "FlameWM snap preview".to_owned(),
            role: SurfaceRole::Overlay,
            input: SurfaceInputMode::PassThrough,
            initially_visible: false,
            x: 0,
            y: 0,
        };
        match runtime.create_surface(UiTemplate::new(document), config) {
            Ok(handle) => {
                self.runtime = Some(runtime);
                self.surface = Some(handle);
                true
            }
            Err(_) => {
                worker::record_failure(worker::FailureStage::SurfaceCreate);
                false
            }
        }
    }

    fn current_request(&self) -> Option<worker::BackdropRequest> {
        let candidate = self.candidate?;
        let geometry = self.geometry?;
        let width = checked_size(geometry.width)?;
        let height = checked_size(geometry.height)?;
        Some(worker::BackdropRequest {
            generation: self.generation,
            candidate,
            geometry,
            opacity: self.opacity,
            capture: (geometry.x, geometry.y, width, height),
        })
    }
}

fn emit_snap_failure(result: &worker::BackdropResult) {
    let request = result.request;
    let error = result
        .error
        .as_deref()
        .unwrap_or("backdrop result contained no pixels")
        .to_owned();
    flamewm_debug::emit(
        flamewm_debug::DebugEventId("wm.snap.failure"),
        std::time::Duration::from_millis(250),
        || {
            format!(
                "stage=result generation={} candidate={:?} geometry=({},{} {}x{}) error={}",
                request.generation,
                request.candidate,
                request.geometry.x,
                request.geometry.y,
                request.geometry.width,
                request.geometry.height,
                worker::compact_error(&error),
            )
        },
    );
}

impl Default for SnapPreviewSurface {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> Rect {
        Rect::new(0, 0, 960, 1080)
    }

    #[test]
    fn hidden_at_init() {
        let preview = SnapPreviewSurface::new();
        assert!(!preview.visible());
        assert_eq!(preview.candidate(), None);
        assert_eq!(preview.geometry(), None);
    }

    #[test]
    fn none_candidate_hides() {
        let mut preview = SnapPreviewSurface::new();
        preview.update(None, None, DEFAULT_PREVIEW_OPACITY_PERCENT);
        assert!(!preview.visible());
        assert_eq!(preview.candidate(), None);
    }

    #[test]
    fn unchanged_candidate_geometry_palette_skips_redraw() {
        let preview = SnapPreviewSurface {
            runtime: None,
            surface: None,
            worker: worker::SnapPreviewWorker::new(),
            generation: 1,
            visible: true,
            candidate: Some(SnapTarget::LeftHalf),
            geometry: Some(geometry()),
            opacity: DEFAULT_PREVIEW_OPACITY_PERCENT,
        };
        assert!(preview.is_current(
            SnapTarget::LeftHalf,
            geometry(),
            DEFAULT_PREVIEW_OPACITY_PERCENT
        ));
        assert!(!preview.is_current(
            SnapTarget::RightHalf,
            geometry(),
            DEFAULT_PREVIEW_OPACITY_PERCENT
        ));
        assert!(!preview.is_current(
            SnapTarget::LeftHalf,
            Rect::new(960, 0, 960, 1080),
            DEFAULT_PREVIEW_OPACITY_PERCENT
        ));
        assert!(!preview.is_current(SnapTarget::LeftHalf, geometry(), 40));
    }

    #[test]
    fn hide_clears_candidate_and_visibility() {
        let mut preview = SnapPreviewSurface {
            runtime: None,
            surface: None,
            worker: worker::SnapPreviewWorker::new(),
            generation: 1,
            visible: true,
            candidate: Some(SnapTarget::LeftHalf),
            geometry: Some(geometry()),
            opacity: DEFAULT_PREVIEW_OPACITY_PERCENT,
        };
        preview.hide();
        assert!(!preview.visible());
        assert_eq!(preview.candidate(), None);
        assert_eq!(preview.geometry(), None);
    }

    #[test]
    fn default_opacity_is_twenty_percent() {
        assert_eq!(DEFAULT_PREVIEW_OPACITY_PERCENT, 20);
        assert_eq!(
            fill_color(20),
            UiColor {
                r: 0xef,
                g: 0x40,
                b: 0x48,
                a: 51,
            }
        );
    }

    #[test]
    fn changed_rect_records_new_geometry_without_display() {
        if SurfaceRuntime::new().is_ok() {
            return;
        }
        let mut preview = SnapPreviewSurface::new();
        preview.update(
            Some(SnapTarget::LeftHalf),
            Some(geometry()),
            DEFAULT_PREVIEW_OPACITY_PERCENT,
        );
        assert_eq!(preview.geometry(), Some(geometry()));
        // Changed rect updates the recorded preview intent; no backdrop
        // capture runs here (headless: stays hidden, no display needed).
        let moved = Rect::new(960, 0, 960, 1080);
        preview.update(
            Some(SnapTarget::LeftHalf),
            Some(moved),
            DEFAULT_PREVIEW_OPACITY_PERCENT,
        );
        assert!(!preview.visible());
        assert_eq!(preview.candidate(), Some(SnapTarget::LeftHalf));
        assert_eq!(preview.geometry(), Some(moved));
    }

    #[test]
    fn update_without_display_stays_hidden_but_records_intent() {
        if SurfaceRuntime::new().is_ok() {
            return;
        }
        let mut preview = SnapPreviewSurface::new();
        preview.update(
            Some(SnapTarget::LeftHalf),
            Some(geometry()),
            DEFAULT_PREVIEW_OPACITY_PERCENT,
        );
        assert!(!preview.visible());
        assert_eq!(preview.candidate(), Some(SnapTarget::LeftHalf));
    }

    fn production_source() -> &'static str {
        include_str!("snap_preview.rs")
            .split_once("#[cfg(test)]\nmod tests")
            .map_or_else(
                || panic!("snap preview test boundary missing"),
                |(source, _)| source,
            )
    }

    fn require_production_markers(test: &str, markers: &[&str]) {
        let source = production_source();
        let missing = markers
            .iter()
            .copied()
            .filter(|marker| !source.contains(marker))
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "{test} is RED: missing production contract markers {missing:?}; the worker/compositor repair must add the behavior rather than weakening this test"
        );
    }

    fn require_source_markers(test: &str, source: &str, markers: &[&str]) {
        let missing = markers
            .iter()
            .copied()
            .filter(|marker| !source.contains(marker))
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "{test} is RED: missing implementation-owner markers {missing:?}; the worker/compositor repair must add the behavior rather than weakening this test"
        );
    }

    // CONTRACT-REGRESSION T09: captured pixels must pass through the shared
    // selection compositor so backdrop pattern and accent-red alpha survive.
    // FAILURE MUTATION: keep the border-only FILL_NODE path and omit the
    // composite_selection_material call.
    #[test]
    fn t09_capture_result_uses_selection_composite() {
        let mut pixels = Vec::with_capacity(6 * 6 * 4);
        for y in 0..6_u8 {
            for x in 0..6_u8 {
                pixels.extend_from_slice(&[x.saturating_mul(10), y.saturating_mul(10), 40, 255]);
            }
        }
        let capture = backdrop::BackdropCapture {
            width: 6,
            height: 6,
            pixels,
        };
        let (fill, border) = backdrop::selection_material(DEFAULT_PREVIEW_OPACITY_PERCENT);
        let output = backdrop::composite_selection_material(&capture, Some(fill), border);
        let first_inner = &output[(2 * 6 + 2) * 4..][..3];
        let second_inner = &output[(3 * 6 + 3) * 4..][..3];
        assert_ne!(first_inner, second_inner, "backdrop pattern was discarded");
        assert!(output[0] > 200, "accent-red border alpha was discarded");
        require_source_markers(
            "T09 capture result uses selection composite",
            include_str!("snap_preview/worker.rs"),
            &["composite_selection_material(&capture"],
        );
        require_source_markers(
            "T09 capture result uses selection composite",
            include_str!("../../flamewm-render-x11/src/backdrop.rs"),
            &["pub struct BackdropCapture"],
        );
    }

    // CONTRACT-REGRESSION T10: motion submission must never wait on capture.
    // FAILURE MUTATION: call a blocking sender from the WM event loop.
    #[test]
    fn t10_capture_request_submission_is_nonblocking() {
        require_production_markers(
            "T10 capture request submission is nonblocking",
            &["try_send("],
        );
    }

    // CONTRACT-REGRESSION T11: pending work is bounded and newest state wins.
    // FAILURE MUTATION: use an unbounded queue or retain every motion request.
    #[test]
    fn t11_capture_requests_are_bounded_latest_wins() {
        require_production_markers(
            "T11 capture requests are bounded latest-wins",
            &["sync_channel", "latest", "coalesce"],
        );
    }

    // CONTRACT-REGRESSION T12: a result from an old request generation cannot
    // mutate the current preview after hide, cancel, or replacement.
    // FAILURE MUTATION: apply every worker result without a generation fence.
    #[test]
    fn t12_stale_capture_results_are_dropped_or_cancelled() {
        require_production_markers(
            "T12 stale capture results are dropped or cancelled",
            &["generation", "stale", "cancel"],
        );
    }

    // CONTRACT-REGRESSION T13: 200 same-state motions must collapse before
    // capture work is requested, while a state change remains observable.
    // FAILURE MUTATION: enqueue one capture for every identical motion.
    #[test]
    fn t13_same_state_motion_burst_coalesces() {
        require_production_markers(
            "T13 same-state motion burst coalesces",
            &["coalesce", "is_current"],
        );
    }

    // CONTRACT-REGRESSION T14: root capture belongs to a worker, not the WM
    // event-loop turn, so a slow XGetImage cannot freeze pointer processing.
    // FAILURE MUTATION: perform capture_root_rect_auto synchronously in update.
    #[test]
    fn t14_capture_runs_off_the_wm_event_loop() {
        require_production_markers(
            "T14 capture runs off the WM event loop",
            &["std::thread::spawn", "capture_root_rect_auto"],
        );
    }
}

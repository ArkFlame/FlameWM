use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::*;
use crate::native::cursor::DeferredNativeLibraryHandles;
use crate::xlib::*;
use flamewm_reactor::timeline::FRAME_INTERVAL;
use flamewm_reactor::{Easing, Reactor, Start, Timeline};
use flamewm_render_core::{Rect, SurfaceDamage};

// Input-focus FFI is not in the pinned xlib.rs surface. It is declared here
// beside its only callers, the popup grab lifecycle that also owns XGrabPointer.
unsafe extern "C" {
    pub(crate) fn XGetInputFocus(
        display: *mut Display,
        focus_return: *mut Window,
        revert_to_return: *mut std::os::raw::c_int,
    ) -> std::os::raw::c_int;
    pub(crate) fn XSetInputFocus(
        display: *mut Display,
        focus: Window,
        revert_to: std::os::raw::c_int,
        time: Time,
    ) -> std::os::raw::c_int;
}

const REVERT_TO_POINTER_ROOT: std::os::raw::c_int = 1;

/// Popup focus handoff, called where the popup takes its pointer grab. Focus
/// moves to the popup so Escape reaches it from any pointer position, and the
/// displaced focus is kept on the app. Passive WM key grabs (Super) keep firing
/// because the popup sits under the root; no active keyboard grab is taken.
/// X errors (e.g. BadMatch) are non-fatal: the process error handler ignores
/// protocol errors.
pub(crate) unsafe fn popup_take_focus(display: *mut Display, app: &mut X11App) {
    if x_io_broken() {
        return;
    }
    // SAFETY: display is the live controller connection; out-pointers are locals.
    unsafe {
        if app.saved_input_focus.is_none() {
            let mut focus: Window = 0;
            let mut revert: std::os::raw::c_int = 0;
            XGetInputFocus(display, &mut focus, &mut revert);
            app.saved_input_focus = Some((focus, revert));
        }
        XSetInputFocus(display, app.window, REVERT_TO_POINTER_ROOT, CURRENT_TIME);
    }
}

/// Restore the focus displaced by `popup_take_focus`, only while the popup
/// still holds focus, so WM focus changes made during the popup are kept. A
/// stale saved window fails with BadWindow (ignored); the X server then leaves
/// focus on PointerRoot once the popup is unmapped.
pub(crate) unsafe fn popup_restore_focus(display: *mut Display, app: &mut X11App) {
    let Some((previous, revert)) = app.saved_input_focus.take() else {
        return;
    };
    if x_io_broken() {
        return;
    }
    // SAFETY: display is the live controller connection; out-pointers are locals.
    unsafe {
        let mut current: Window = 0;
        let mut current_revert: std::os::raw::c_int = 0;
        XGetInputFocus(display, &mut current, &mut current_revert);
        if current == app.window {
            XSetInputFocus(display, previous, revert, CURRENT_TIME);
        }
    }
}

/// Stable role label for geometry traces (one id/role per placement).
pub fn surface_role_label(role: SurfaceRole) -> &'static str {
    match role {
        SurfaceRole::Normal => "normal",
        SurfaceRole::Desktop => "desktop",
        SurfaceRole::Dock => "dock",
        SurfaceRole::PopupMenu => "popup-menu",
        SurfaceRole::DropdownMenu => "dropdown-menu",
        SurfaceRole::Overlay => "overlay",
    }
}

/// Reserved generic action for a ButtonRelease that lands outside the
/// grabbed surface while a pointer grab is active. Feature modules share
/// this single transient-lifecycle signal; it is never Start-specific.
pub const OUTSIDE_RELEASE_ACTION: &str = "surface.outside.release";

/// Pure bounds check: window-relative press/release coords outside the
/// grabbed surface rect. With an active pointer grab X reports releases
/// to the grab window even when the pointer is outside, so coords outside
/// `[0, width) x [0, height)` mean "outside the grabbed surface".
pub fn button_release_is_outside(x: i32, y: i32, width: u32, height: u32) -> bool {
    x < 0 || y < 0 || x >= width.max(1) as i32 || y >= height.max(1) as i32
}

/// Build the generic outside-release controller event for one surface.
pub fn outside_release_event(
    surface: SurfaceId,
    button: u32,
    x: f32,
    y: f32,
    time_ms: u64,
) -> SurfaceControllerEvent {
    SurfaceControllerEvent {
        surface,
        event: ActionEvent {
            action: OUTSIDE_RELEASE_ACTION.to_string(),
            phase: ActionPhase::Release,
            button,
            x,
            y,
            inside: false,
            time_ms,
            text: None,
        },
    }
}

pub fn run_with_surface_controller_events<F>(
    mut document: RuntimeDocument,
    config: X11Config,
    mut on_event: F,
) -> Result<(), String>
where
    F: FnMut(&SurfaceControllerEvent, &mut RuntimeDocument) -> Result<(), String>,
{
    unsafe {
        let display = XOpenDisplay(ptr::null());
        if display.is_null() {
            return Err("XOpenDisplay failed; DISPLAY is unset or unreachable".to_string());
        }
        let mut app = match X11App::new(display, &config) {
            Ok(app) => app,
            Err(error) => {
                XCloseDisplay(display);
                return Err(error);
            }
        };
        let surface = SurfaceId(1);
        let result = app.event_loop(&mut document, &mut |event, document| {
            on_event(
                &SurfaceControllerEvent {
                    surface,
                    event: event.clone(),
                },
                document,
            )
        });
        let deferred_native = unsafe { app.take_deferred_native_libraries() };
        drop(app);
        XCloseDisplay(display);
        drop(deferred_native);
        result
    }
}

struct SurfaceInstance {
    app: X11App,
    document: RuntimeDocument,
    role: SurfaceRole,
    input: SurfaceInputMode,
    /// Retained native window origin (root-space device pixels). Set at
    /// create, updated by move_resize; never recomputed from layout.
    origin: (i32, i32),
    /// Opt-in slide presentation for the next show/hide; `None` is the immediate path.
    slide: Option<SlideConfig>,
    /// Whether the window is mapped from the controller's point of view (slide bookkeeping).
    shown: bool,
    /// Native origin last written by slide motion; skips a repeat move to the same pixel.
    slide_drawn: Option<(i32, i32)>,
}

/// Panel edge a popup slides out of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlideEdge {
    Top,
    Bottom,
    Left,
    Right,
}

/// Slide presentation for a popup anchored to a panel edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlideConfig {
    pub edge: SlideEdge,
    /// Animation speed in hundredths (`100` = 1.0x).
    pub speed: u16,
}

const SLIDE_OPEN_BASE_MS: u32 = 180;
const SLIDE_CLOSE_BASE_MS: u32 = 120;

/// Timeline kind for one slide transition. Timeline progress is 0.0 at the off-edge
/// start and 1.0 at the exact anchored origin; `opening` only selects the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SlideKind {
    edge: SlideEdge,
    opening: bool,
}

/// Native window origin at `progress`: displaced from the final origin toward the panel edge.
fn slide_origin(final_origin: (i32, i32), size: (u32, u32), edge: SlideEdge, progress: f32) -> (i32, i32) {
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

/// Frame wait while any slide is active; `None` keeps the reactor fully idle.
fn motion_frame_wait(active: bool) -> Option<Duration> {
    active.then_some(FRAME_INTERVAL)
}

/// Headless/test fallback extent (device pixels at scale 1.0). Used only
/// when no retained layout exists; mapped-surface queries never hit it.
pub const HEADLESS_FALLBACK_EXTENT: (f32, f32) = (1350.0, 641.0);

/// Single-present stage order for first show: project -> measure ->
/// anchor -> prepare -> paint -> shape -> commit -> map+raise ->
/// present -> grab -> flush. Each stage runs at most once per cycle;
/// the cycle owns show/raise/present/grab with no feature redraw.
pub const PRESENT_STAGE_ORDER: [&str; 10] = [
    "project",
    "measure",
    "anchor",
    "prepare",
    "paint",
    "shape",
    "commit",
    "map_raise",
    "present",
    "grab_flush",
];

/// Correlated surface-geometry trace transaction. One id traces one
/// placement: anchor rect -> intrinsic size -> fitted root rect ->
/// SurfaceRuntime request -> SurfaceController request -> native
/// XMoveResizeWindow request -> observed Configure/Map root rect ->
/// retained origin. Hot path is in-memory only (no X roundtrip); the
/// observed stage is filled solely by `note_observed_rect` from an
/// already-delivered Configure/Map event or a debug-only reply probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeometryTraceTxn(u64);

impl GeometryTraceTxn {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// One stage of a geometry trace. Stored in-memory; formatted only when
/// `FLAMEWM_GEOM_TRACE=1` and emitted through `flamewm-debug` when
/// `FLAMEWM_DEBUG=1`. Never roundtrips X on the hot path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeometryTraceStage {
    pub txn: u64,
    pub stage: &'static str,
    pub surface: u64,
    pub role: &'static str,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// In-memory ring of geometry trace stages, shared by the trace contract.
/// Single owner: `flamewm-render-x11`. UI/shell layers only record through
/// the typed `GeometryTrace` handle and read via `snapshot_for_txn`.
#[derive(Debug, Default)]
pub struct GeometryTraceLog {
    stages: Vec<GeometryTraceStage>,
}

impl GeometryTraceLog {
    pub fn record(&mut self, stage: GeometryTraceStage) {
        self.stages.push(stage);
        if self.stages.len() > 512 {
            let excess = self.stages.len() - 512;
            self.stages.drain(..excess);
        }
    }

    pub fn snapshot_for_txn(&self, txn: u64) -> Vec<GeometryTraceStage> {
        self.stages
            .iter()
            .copied()
            .filter(|stage| stage.txn == txn)
            .collect()
    }

    /// Debug/roundtrip-only observed-rect probe for one native window.
    /// Called off the hot path only (debug tooling); production geometry
    /// commits never issue a synchronous X roundtrip.
    pub fn note_observed(
        &mut self,
        txn: u64,
        surface: u64,
        role: &'static str,
        rect: (i32, i32, i32, i32),
    ) {
        self.record(GeometryTraceStage {
            txn,
            stage: "observed",
            surface,
            role,
            x: rect.0,
            y: rect.1,
            width: rect.2,
            height: rect.3,
        });
    }
}

fn geometry_trace_log() -> &'static std::sync::Mutex<GeometryTraceLog> {
    static LOG: std::sync::OnceLock<std::sync::Mutex<GeometryTraceLog>> =
        std::sync::OnceLock::new();
    LOG.get_or_init(|| std::sync::Mutex::new(GeometryTraceLog::default()))
}

fn next_geometry_txn() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let txn = NEXT.fetch_add(1, Ordering::Relaxed);
    if txn == 0 {
        NEXT.fetch_add(1, Ordering::Relaxed)
    } else {
        txn
    }
}

/// Typed trace handle threaded through shell -> SurfaceRuntime ->
/// SurfaceController -> native. Source of truth for one correlated
/// placement; carries the txn id and the surface role label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeometryTrace {
    pub txn: u64,
    pub surface: u64,
    pub role: &'static str,
}

impl GeometryTrace {
    pub fn begin(role: &'static str) -> Self {
        Self {
            txn: next_geometry_txn(),
            surface: 0,
            role,
        }
    }

    pub fn for_surface(self, surface: u64) -> Self {
        Self { surface, ..self }
    }

    pub fn txn(&self) -> GeometryTraceTxn {
        GeometryTraceTxn(self.txn)
    }

    fn emit(&self, stage: &'static str, rect: (i32, i32, i32, i32)) {
        if let Ok(mut log) = geometry_trace_log().lock() {
            log.record(GeometryTraceStage {
                txn: self.txn,
                stage,
                surface: self.surface,
                role: self.role,
                x: rect.0,
                y: rect.1,
                width: rect.2,
                height: rect.3,
            });
        }
        if std::env::var_os("FLAMEWM_GEOM_TRACE").is_some_and(|v| v == "1") {
            eprintln!(
                "geom-trace txn={} stage={} surface={} role={} rect={},{},{},{}",
                self.txn, stage, self.surface, self.role, rect.0, rect.1, rect.2, rect.3,
            );
        }
    }

    pub fn anchor(&self, rect: (i32, i32, i32, i32)) {
        self.emit("anchor", rect);
    }

    pub fn intrinsic(&self, size: (i32, i32)) {
        self.emit("intrinsic", (0, 0, size.0, size.1));
    }

    pub fn fitted(&self, rect: (i32, i32, i32, i32)) {
        self.emit("fitted", rect);
    }

    pub fn ui_request(&self, rect: (i32, i32, i32, i32)) {
        self.emit("ui-request", rect);
    }

    pub fn native_request(&self, rect: (i32, i32, i32, i32)) {
        self.emit("native-request", rect);
    }

    pub fn retained(&self, rect: (i32, i32, i32, i32)) {
        self.emit("retained", rect);
    }

    /// Debug-only observed stage (Configure/Map reply or async event).
    /// Never called on the production hot path.
    pub fn observed(&self, rect: (i32, i32, i32, i32)) {
        if let Ok(mut log) = geometry_trace_log().lock() {
            log.note_observed(self.txn, self.surface, self.role, rect);
        }
    }
}

/// Snapshot of all recorded stages for one transaction id. Used by tests
/// and debug tooling to prove one correlated id traces the full chain.
pub fn geometry_trace_snapshot(txn: u64) -> Vec<GeometryTraceStage> {
    geometry_trace_log()
        .lock()
        .map(|log| log.snapshot_for_txn(txn))
        .unwrap_or_default()
}

/// DEBUG-ONLY gate: true only when `FLAMEWM_DEBUG=1`. Normal mode must
/// issue zero extra X syncs; callers check this before any probe.
pub fn debug_enabled() -> bool {
    std::env::var_os("FLAMEWM_DEBUG").is_some_and(|v| v == "1")
}

/// DEBUG-ONLY pure stack-order predicate (no X): bottom-to-top roles must
/// satisfy desktop < normal < dock < popup (popup covers menu/dropdown/
/// overlay). Test/canary use only.
pub fn stack_order_ok(bottom_to_top: &[SurfaceRole]) -> bool {
    fn rank(role: SurfaceRole) -> u8 {
        match role {
            SurfaceRole::Desktop => 0,
            SurfaceRole::Normal => 1,
            SurfaceRole::Dock => 2,
            SurfaceRole::PopupMenu | SurfaceRole::DropdownMenu | SurfaceRole::Overlay => 3,
        }
    }
    let mut last: Option<u8> = None;
    for role in bottom_to_top {
        let rank = rank(*role);
        // Non-decreasing rank keeps equal-class siblings total without
        // imposing an order inside one class.
        if let Some(prev) = last {
            if rank < prev {
                return false;
            }
        }
        last = Some(rank);
    }
    true
}

/// Pure single-cycle recorder for the present operation. X calls stay in
/// the controller methods; this enforces one ordered pass per surface.
#[derive(Debug, Default)]
pub struct PresentCycle {
    done: [bool; 10],
}

impl PresentCycle {
    pub fn new() -> Self {
        Self { done: [false; 10] }
    }

    /// Advance exactly one stage in order. Rejects skips, repeats, and
    /// unknown stage names so a frame can never double-present.
    pub fn advance(&mut self, stage: &str) -> Result<(), String> {
        let index = PRESENT_STAGE_ORDER
            .iter()
            .position(|known| *known == stage)
            .ok_or_else(|| format!("unknown present stage {stage}"))?;
        if self.done[index] {
            return Err(format!("present stage {stage} already ran this cycle"));
        }
        if (0..index).any(|earlier| !self.done[earlier]) {
            return Err(format!("present stage {stage} ran out of order"));
        }
        self.done[index] = true;
        Ok(())
    }

    pub fn run_full_cycle(&mut self) -> Result<(), String> {
        for stage in PRESENT_STAGE_ORDER {
            self.advance(stage)?;
        }
        Ok(())
    }

    pub fn is_complete(&self) -> bool {
        self.done.iter().all(|done| *done)
    }
}

pub struct SurfaceController {
    display: *mut Display,
    screen: i32,
    next_id: u64,
    surfaces: HashMap<SurfaceId, SurfaceInstance>,
    windows: HashMap<Window, SurfaceId>,
    /// Dynamic Xft/XRender/XShape/Xcursor handles outlive all apps and
    /// XCloseDisplay. Backend X resources are drained before this owner closes
    /// the display; the handles are dropped only afterwards.
    deferred_native_libraries: Vec<DeferredNativeLibraryHandles>,
    /// Interned `_NET_WM_ICON_GEOMETRY` atom; 0 until first use.
    icon_geometry_atom: Atom,
    /// Active slide transitions keyed by surface id. Empty when idle (no frame wakeups).
    slides: Timeline<SlideKind>,
}

impl SurfaceController {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let display = XOpenDisplay(ptr::null());
            if display.is_null() {
                return Err("XOpenDisplay failed; DISPLAY is unset or unreachable".to_string());
            }
            install_x_io_error_handler();
            install_x_protocol_error_handler();
            Ok(Self {
                screen: XDefaultScreen(display),
                display,
                next_id: 1,
                surfaces: HashMap::new(),
                windows: HashMap::new(),
                deferred_native_libraries: Vec::new(),
                icon_geometry_atom: 0,
                slides: Timeline::new(),
            })
        }
    }

    pub fn connection_fd(&self) -> RawFd {
        unsafe { XConnectionNumber(self.display) as RawFd }
    }

    pub fn root_geometry(&self) -> (u32, u32) {
        unsafe {
            (
                XDisplayWidth(self.display, self.screen).max(0) as u32,
                XDisplayHeight(self.display, self.screen).max(0) as u32,
            )
        }
    }

    pub fn create_surface(
        &mut self,
        document: RuntimeDocument,
        config: SurfaceConfig,
    ) -> Result<SurfaceId, String> {
        let id = SurfaceId(self.next_id);
        self.next_id = self.next_id.checked_add(1).ok_or("surface id exhausted")?;
        let (role, input) = (config.role, config.input);
        let x11_config = X11Config::from(config.clone());
        let app =
            unsafe { X11App::new_surface(self.display, &x11_config, config.x, config.y, false)? };
        let window = app.window;
        let mut instance = SurfaceInstance {
            app,
            document,
            role,
            input,
            origin: (config.x, config.y),
            slide: None,
            shown: config.initially_visible,
            slide_drawn: None,
        };
        // Overlay/passthrough surfaces are excluded from WM client
        // management: override-redirect + empty input region + no grab/focus.
        if overlay_excluded_from_wm(role) {
            unsafe {
                let mut attributes = XSetWindowAttributes {
                    background_pixmap: 0,
                    background_pixel: TRANSPARENT_CLEAR_PIXEL,
                    border_pixmap: 0,
                    border_pixel: 0,
                    bit_gravity: 0,
                    win_gravity: 0,
                    backing_store: 0,
                    backing_planes: 0,
                    backing_pixel: 0,
                    save_under: 0,
                    event_mask: 0,
                    do_not_propagate_mask: 0,
                    override_redirect: 1,
                    colormap: 0,
                    cursor: 0,
                };
                XChangeWindowAttributes(
                    self.display,
                    window,
                    CW_OVERRIDE_REDIRECT,
                    &mut attributes,
                );
            }
        }
        if input == SurfaceInputMode::PassThrough {
            if let Some(bridge) = instance.app.xshape.as_ref() {
                // Empty input region: pointer falls through the surface.
                if let Err(error) = unsafe { bridge.apply_input_mask(window, &[]) } {
                    mark_x_error_seen();
                    return Err(error);
                }
            } else {
                return Err(
                    "passthrough surface requires XShape input region (unavailable)".to_string(),
                );
            }
        }
        // Hidden create: mark Full dirty, no layout/paint/present/map/shape.
        // First show renders retained offscreen, then maps (popup map+raise).
        if !config.initially_visible {
            instance.app.mark_full();
            instance.document.mark_dirty(SurfaceDamage::Full);
            self.windows.insert(window, id);
            self.surfaces.insert(id, instance);
            return Ok(id);
        }
        // Visible create: paint retained offscreen first (redraw refreshes
        // shape once), then map (popup map+raise) + present/flush.
        unsafe {
            instance.app.redraw(&instance.document).map_err(|error| {
                mark_x_error_seen();
                error
            })?;
        }
        unsafe {
            if surface_role_needs_popup_raise(role) {
                XMapWindow(self.display, window);
                XRaiseWindow(self.display, window);
            } else {
                XMapWindow(self.display, window);
            }
            instance.app.present_scene().map_err(|error| {
                mark_x_error_seen();
                error
            })?;
            XFlush(self.display);
        }
        self.windows.insert(window, id);
        self.surfaces.insert(id, instance);
        Ok(id)
    }

    pub fn destroy_surface(&mut self, id: SurfaceId) -> Result<(), String> {
        if let Some(mut instance) = self.surfaces.remove(&id) {
            self.slides.cancel(id.get());
            self.windows.remove(&instance.app.window);
            if instance.app.pointer_grabbed {
                unsafe {
                    XUngrabPointer(self.display, CURRENT_TIME);
                    popup_restore_focus(self.display, &mut instance.app);
                }
                instance.app.pointer_grabbed = false;
            }
            if x_io_broken() || x_error_seen() || !display_fd_alive(self.display) {
                // No backend may call into a broken display. Preserve the
                // complete app until process teardown rather than unloading a
                // library whose close-display hook may still be referenced.
                std::mem::forget(instance);
            } else {
                // SAFETY: this controller still owns the live display.
                self.deferred_native_libraries
                    .push(unsafe { instance.app.take_deferred_native_libraries() });
            }
            Ok(())
        } else {
            Err(format!("unknown surface {}", id.0))
        }
    }

    /// Close one surface in the same turn: ungrab-once (only when grabbed),
    /// then unmap + Expose-present (retained re-blit) + flush. No repaint.
    pub fn close_surface(&mut self, id: SurfaceId) -> Result<(), String> {
        let display = self.display;
        let instance = self.instance_mut(id)?;
        if instance.app.pointer_grabbed {
            unsafe {
                XUngrabPointer(display, CURRENT_TIME);
                popup_restore_focus(display, &mut instance.app);
            }
            instance.app.pointer_grabbed = false;
        }
        unsafe {
            XUnmapWindow(display, instance.app.window);
            instance.app.present_scene().map_err(|error| {
                mark_x_error_seen();
                error
            })?;
            XFlush(display);
        }
        Ok(())
    }

    /// Sets (or clears with `None`) the slide presentation used by later `show`/`hide` calls.
    pub fn set_slide(&mut self, id: SurfaceId, slide: Option<SlideConfig>) -> Result<(), String> {
        self.instance_mut(id)?.slide = slide;
        Ok(())
    }

    pub fn show(&mut self, id: SurfaceId) -> Result<(), String> {
        // First-show single present: project->measure->anchor->prepare->
        // paint->shape->commit->map+raise->present->grab->flush in one
        // ordered cycle, no feature redraw. Second show sees clean damage
        // and maps/presents without repainting.
        let role = self.surface_role(id)?;
        let display = self.display;
        let key = id.get();
        let now = Instant::now();
        let (slide, shown, size, origin, window) = {
            let instance = self.instance_mut(id)?;
            (
                instance.slide,
                instance.shown,
                (instance.app.width, instance.app.height),
                instance.origin,
                instance.app.window,
            )
        };
        if let Some(config) = slide {
            self.slides.set_speed(config.speed);
            let opening = SlideKind { edge: config.edge, opening: true };
            if shown {
                match self.slides.kind(key) {
                    // Double open while opening: already converging on the final frame.
                    Some(active) if active.opening => return Ok(()),
                    // Reverse from the current progress; the window is still mapped.
                    Some(_) => {
                        self.slides.start_towards(key, opening, now, SLIDE_OPEN_BASE_MS, Easing::EaseOutCubic, (0.0, 1.0));
                        return Ok(());
                    }
                    None => {}
                }
            } else if self
                .slides
                .start_towards(key, opening, now, SLIDE_OPEN_BASE_MS, Easing::EaseOutCubic, (0.0, 1.0))
                == Start::Started
            {
                // Place off-edge before mapping so the first visible frame is the start frame.
                let (x, y) = slide_origin(origin, size, config.edge, 0.0);
                unsafe { XMoveResizeWindow(display, window, x, y, size.0, size.1) };
                self.instance_mut(id)?.slide_drawn = Some((x, y));
            }
            // A refused start (timeline full or disabled) maps at the final origin with no slide.
        }
        let instance = self.instance_mut(id)?;
        let mut cycle = PresentCycle::new();
        cycle.advance("project")?;
        cycle.advance("measure")?;
        cycle.advance("anchor")?;
        cycle.advance("prepare")?;
        let painted = unsafe {
            instance
                .app
                .redraw_if_dirty(&mut instance.document)
                .map_err(|error| {
                    mark_x_error_seen();
                    error
                })?
        };
        cycle.advance("paint")?;
        if painted {
            // SAFETY: display/window live; shape follows retained repaint.
            unsafe { instance.app.refresh_shape_mask(&instance.document) };
        }
        cycle.advance("shape")?;
        cycle.advance("commit")?;
        unsafe {
            if surface_role_needs_popup_raise(role) {
                XMapWindow(display, instance.app.window);
                XRaiseWindow(display, instance.app.window);
            } else {
                XMapWindow(display, instance.app.window);
            }
        }
        cycle.advance("map_raise")?;
        unsafe {
            instance.app.present_scene().map_err(|error| {
                mark_x_error_seen();
                error
            })?;
            XFlush(display);
        }
        cycle.advance("present")?;
        cycle.advance("grab_flush")?;
        debug_assert!(cycle.is_complete());
        let instance = self.instance_mut(id)?;
        instance.shown = true;
        Ok(())
    }

    pub fn hide(&mut self, id: SurfaceId) -> Result<(), String> {
        let display = self.display;
        let now = Instant::now();
        let key = id.get();
        let (slide, shown) = {
            let instance = self.instance_mut(id)?;
            if instance.app.pointer_grabbed {
                unsafe {
                    XUngrabPointer(display, CURRENT_TIME);
                    popup_restore_focus(display, &mut instance.app);
                }
                instance.app.pointer_grabbed = false;
            }
            (instance.slide, instance.shown)
        };
        if let (Some(config), true) = (slide, shown) {
            // Already closing: keep the running transition.
            if self.slides.kind(key).is_some_and(|active| !active.opening) {
                unsafe { XFlush(display) };
                return Ok(());
            }
            self.slides.set_speed(config.speed);
            let closing = SlideKind { edge: config.edge, opening: false };
            // A new close starts fully shown; a reversed open continues from its progress.
            if self.slides.start_towards(key, closing, now, SLIDE_CLOSE_BASE_MS, Easing::EaseOutCubic, (1.0, 0.0))
                == Start::Started
            {
                unsafe { XFlush(display) };
                return Ok(());
            }
        }
        // No slide, or the timeline refused the transition: unmap immediately.
        self.slides.cancel(key);
        let instance = self.instance_mut(id)?;
        unsafe {
            XUnmapWindow(display, instance.app.window);
            XFlush(display);
        }
        instance.shown = false;
        instance.slide_drawn = None;
        Ok(())
    }

    /// Moves every active slide to `now`: one native move per changed pixel, the exact anchored
    /// origin on the last frame, and unmap when a slide-out completes. Content is never
    /// repainted here.
    fn advance_motion(&mut self, now: Instant) {
        let display = self.display;
        for (id, instance) in self.surfaces.iter_mut() {
            let key = id.get();
            let (Some(active), Some(progress)) = (self.slides.kind(key), self.slides.progress(key, now)) else {
                continue;
            };
            let finished = self.slides.finished(key, now) == Some(true);
            let window = instance.app.window;
            let size = (instance.app.width, instance.app.height);
            let (x, y) = if finished {
                instance.origin
            } else {
                slide_origin(instance.origin, size, active.edge, progress)
            };
            if instance.slide_drawn != Some((x, y)) {
                unsafe { XMoveResizeWindow(display, window, x, y, size.0, size.1) };
                instance.slide_drawn = Some((x, y));
            }
            if !finished {
                continue;
            }
            self.slides.finish(key);
            if !active.opening {
                unsafe { XUnmapWindow(display, window) };
                instance.shown = false;
                instance.slide_drawn = None;
            }
        }
        unsafe { XFlush(display) };
    }

    fn motion_frame_timeout(&self) -> Option<Duration> {
        motion_frame_wait(!self.slides.is_empty())
    }
    pub fn raise(&self, id: SurfaceId) -> Result<(), String> {
        self.with_window(id, |display, window| unsafe {
            XRaiseWindow(display, window)
        })
    }

    pub fn move_resize(
        &mut self,
        id: SurfaceId,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        self.move_resize_traced(id, x, y, width, height, None)
    }

    /// Traced variant: records the native-request + retained stages under
    /// one txn (role resolved from the surface). Emits no X roundtrip.
    pub fn move_resize_traced(
        &mut self,
        id: SurfaceId,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        trace: Option<GeometryTrace>,
    ) -> Result<(), String> {
        // Native move is single and authoritative: size changes go through
        // the transactional geometry commit (retained size -> backbuffer ->
        // window resize -> repaint); pure moves keep the retained size with
        // one XMoveResizeWindow.
        let size_changed = {
            let instance = self
                .surfaces
                .get(&id)
                .ok_or_else(|| format!("unknown surface {}", id.0))?;
            width.max(1) != instance.app.width || height.max(1) != instance.app.height
        };
        let role_label = self
            .surfaces
            .get(&id)
            .map(|instance| surface_role_label(instance.role))
            .unwrap_or("unknown");
        let span_trace = trace
            .map(|t| t.for_surface(id.get()))
            .unwrap_or_else(|| GeometryTrace {
                txn: next_geometry_txn(),
                surface: id.get(),
                role: role_label,
            });
        span_trace.native_request((x, y, width.max(1) as i32, height.max(1) as i32));
        if size_changed {
            let display = self.display;
            let instance = self.instance_mut(id)?;
            unsafe {
                instance
                    .app
                    .commit_geometry(&instance.document, width, height)
                    .map_err(|error| {
                        mark_x_error_seen();
                        error
                    })?
            };
            unsafe {
                XMoveResizeWindow(
                    display,
                    instance.app.window,
                    x,
                    y,
                    instance.app.width,
                    instance.app.height,
                )
            };
            instance.origin = (x, y);
            instance.slide_drawn = None;
            span_trace.retained((x, y, instance.app.width as i32, instance.app.height as i32));
            if instance.role == SurfaceRole::Dock {
                let root_w = unsafe { XDisplayWidth(display, instance.app.screen).max(1) as u32 };
                let root_h = unsafe { XDisplayHeight(display, instance.app.screen).max(1) as u32 };
                let (ox, oy) = instance.origin;
                let (w, h) = (instance.app.width, instance.app.height);
                crate::app::apply_dock_strut(
                    instance.app.display,
                    instance.app.window,
                    root_w,
                    root_h,
                    ox,
                    oy,
                    w,
                    h,
                );
            }
            unsafe { XFlush(display) };
            return Ok(());
        }
        let display = self.display;
        let instance = self.instance_mut(id)?;
        let (w, h) = (instance.app.width, instance.app.height);
        unsafe { XMoveResizeWindow(display, instance.app.window, x, y, w, h) };
        instance.origin = (x, y);
        instance.slide_drawn = None;
        if instance.role == SurfaceRole::Dock {
            let root_w = unsafe { XDisplayWidth(display, instance.app.screen).max(1) as u32 };
            let root_h = unsafe { XDisplayHeight(display, instance.app.screen).max(1) as u32 };
            crate::app::apply_dock_strut(
                instance.app.display,
                instance.app.window,
                root_w,
                root_h,
                x,
                y,
                w,
                h,
            );
        }
        span_trace.retained((x, y, w as i32, h as i32));
        unsafe { XFlush(display) };
        Ok(())
    }

    /// Native owner for an X11App window's requested move+size. Keeps the
    /// established order: one XMoveResizeWindow to the requested rect, then
    /// the transactional commit for the requested size.
    pub(crate) unsafe fn move_app_window_geometry(
        app: &mut crate::app::X11App,
        document: &RuntimeDocument,
        request: crate::WindowGeometry,
    ) -> Result<(), String> {
        unsafe {
            XMoveResizeWindow(
                app.display,
                app.window,
                request.x,
                request.y,
                request.width,
                request.height,
            )
        };
        unsafe { app.commit_geometry(document, request.width, request.height) }
    }

    /// Publishes window-relative pager cell rects on a surface's X window
    /// (`FLAMEWM_PAGER_CELLS`). Callers pass rects derived from the retained
    /// layout of that same surface.
    pub fn set_pager_cells(&mut self, id: SurfaceId, cells: &[[u64; 4]]) -> Result<(), String> {
        let display = self.display;
        let instance = self.instance_mut(id)?;
        crate::app::apply_pager_cells(instance.app.display, instance.app.window, cells);
        unsafe { XFlush(display) };
        Ok(())
    }

    /// Sets `_NET_WM_ICON_GEOMETRY` on a client window (not a surface this
    /// controller owns) to a root-space rect. Errors from a vanished client
    /// are ignored by the protocol handler; the caller never aborts.
    pub fn set_window_icon_geometry(&mut self, window: u64, rect: [u32; 4]) -> Result<(), String> {
        let property = self.icon_geometry_atom();
        crate::app::apply_window_icon_geometry(
            self.display,
            window as Window,
            property,
            Some(rect.map(u64::from)),
        );
        unsafe { XFlush(self.display) };
        Ok(())
    }

    /// Deletes `_NET_WM_ICON_GEOMETRY` from a client window.
    pub fn clear_window_icon_geometry(&mut self, window: u64) -> Result<(), String> {
        let property = self.icon_geometry_atom();
        crate::app::apply_window_icon_geometry(self.display, window as Window, property, None);
        unsafe { XFlush(self.display) };
        Ok(())
    }

    fn icon_geometry_atom(&mut self) -> Atom {
        if self.icon_geometry_atom == 0 {
            let name = std::ffi::CString::new("_NET_WM_ICON_GEOMETRY")
                .expect("static atom contains no NUL");
            self.icon_geometry_atom = unsafe { XInternAtom(self.display, name.as_ptr(), 0) };
        }
        self.icon_geometry_atom
    }

    /// Debug/roundtrip-only observed-rect probe for one surface. Off the
    /// hot path: reads retained state (no X sync). Real async
    /// ConfigureNotify/MapNotify reconciliation stays in `app/events.rs`.
    pub fn debug_observed_rect(
        &self,
        id: SurfaceId,
        trace: GeometryTrace,
    ) -> Result<(i32, i32, i32, i32), String> {
        let rect = self.surface_device_rect(id)?;
        let observed = (
            rect.x as i32,
            rect.y as i32,
            rect.width as i32,
            rect.height as i32,
        );
        trace.for_surface(id.get()).observed(observed);
        Ok(observed)
    }

    /// DEBUG-ONLY actual root-rect probe after present (X native owner).
    /// Normal mode: refuses up front, zero extra X sync. Debug/test mode:
    /// exactly one retained root-rect read (origin + size, no X roundtrip,
    /// no layout recompute), then records the `observed` trace stage.
    /// Call once per popup show.
    pub fn debug_probe_root_rect(
        &self,
        id: SurfaceId,
        trace: GeometryTrace,
    ) -> Result<(i32, i32, i32, i32), String> {
        if !debug_enabled() && !cfg!(test) {
            return Err("debug-only probe refused outside FLAMEWM_DEBUG=1".to_string());
        }
        self.debug_observed_rect(id, trace)
    }

    /// DEBUG/TEST-ONLY stacking canary over retained role order.
    /// Checks bottom-to-top creation order satisfies desktop < normal <
    /// dock < popup via [`stack_order_ok`]. Zero X sync in all modes;
    /// a live QueryTree roundtrip stays in the nested-X harness, never on
    /// this path. Returns `Ok(true)` when the order holds, `Ok(false)`
    /// on inversion. Refuses (zero work) outside debug/test. Never
    /// reorders here; the caller re-uses the existing map/raise owner to
    /// repair and reports.
    pub fn debug_stack_canary(&self) -> Result<bool, String> {
        if !debug_enabled() && !cfg!(test) {
            return Err("debug-only canary refused outside FLAMEWM_DEBUG=1".to_string());
        }
        let mut ids: Vec<SurfaceId> = self.surfaces.keys().copied().collect();
        ids.sort_by_key(|id| id.get());
        let roles: Vec<SurfaceRole> = ids
            .iter()
            .filter_map(|id| self.surfaces.get(id).map(|instance| instance.role))
            .collect();
        Ok(stack_order_ok(&roles))
    }

    /// Retained native origin update (root-space device pixels). Called
    /// after a successful move so geometry queries track the window.
    pub fn set_origin(&mut self, id: SurfaceId, x: i32, y: i32) -> Result<(), String> {
        self.instance_mut(id)?.origin = (x, y);
        Ok(())
    }

    pub fn grab_pointer(&mut self, id: SurfaceId) -> Result<(), String> {
        let instance = self
            .surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        if !instance.input.allows_grab() {
            return Err(format!(
                "surface {} is pass-through; pointer grab refused",
                id.0
            ));
        }
        let display = self.display;
        let instance = self.instance_mut(id)?;
        let result = unsafe {
            XGrabPointer(
                display,
                instance.app.window,
                1,
                (BUTTON_PRESS_MASK | BUTTON_RELEASE_MASK | POINTER_MOTION_MASK) as u32,
                GRAB_MODE_ASYNC,
                GRAB_MODE_ASYNC,
                0,
                0,
                CURRENT_TIME,
            )
        };
        if result != 0 {
            // Nonzero grab status (AlreadyGrabbed/Frozen/InvalidTime/
            // NotViewable) is a protocol reply on a live connection, not a
            // dead server: do NOT set the x-error latch (Drop would skip
            // teardown). Caller treats it as a transient popup refusal.
            return Err(format!("XGrabPointer failed for surface {}", id.0));
        }
        instance.app.pointer_grabbed = true;
        unsafe {
            popup_take_focus(display, &mut instance.app);
            XFlush(display);
        }
        Ok(())
    }

    pub fn ungrab_pointer(&mut self, id: SurfaceId) -> Result<(), String> {
        let display = self.display;
        let instance = self.instance_mut(id)?;
        unsafe {
            XUngrabPointer(display, CURRENT_TIME);
            popup_restore_focus(display, &mut instance.app);
        }
        instance.app.pointer_grabbed = false;
        unsafe {
            XFlush(display);
        }
        Ok(())
    }

    pub fn redraw(&mut self, id: SurfaceId) -> Result<(), String> {
        let _guard = flamewm_profiler::start("render.present");
        let instance = self.instance_mut(id)?;
        unsafe {
            instance.app.redraw(&instance.document).map_err(|error| {
                mark_x_error_seen();
                error
            })
        }
    }

    pub fn redraw_dirty(&mut self) -> Result<(), String> {
        let _guard = flamewm_profiler::start("render.present");
        let ids: Vec<SurfaceId> = self.surfaces.keys().copied().collect();
        for id in ids {
            let instance = self.instance_mut(id)?;
            // SAFETY: instance owns a live display + app resources.
            let painted = unsafe {
                instance
                    .app
                    .redraw_if_dirty(&mut instance.document)
                    .map_err(|error| {
                        mark_x_error_seen();
                        error
                    })?
            };
            if painted {
                let instance = self.instance_mut(id)?;
                // SAFETY: display/window live; mask follows repaint.
                unsafe { instance.app.refresh_shape_mask(&instance.document) };
            }
        }
        Ok(())
    }

    /// Mark damage on one surface without painting (merged max coverage).
    pub fn mark_surface_dirty(
        &mut self,
        id: SurfaceId,
        damage: SurfaceDamage,
    ) -> Result<(), String> {
        self.instance_mut(id)?.app.mark_dirty(damage);
        self.instance_mut(id)?.document.mark_dirty(damage);
        Ok(())
    }

    /// Peek the merged damage latch for one surface (no consume).
    pub fn peek_surface_damage(&self, id: SurfaceId) -> Result<SurfaceDamage, String> {
        let instance = self
            .surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        Ok(instance
            .app
            .pending_damage
            .merge(instance.document.peek_damage()))
    }

    pub fn pump_events<F>(&mut self, mut on_action: F) -> Result<usize, String>
    where
        F: FnMut(&SurfaceControllerEvent, &mut RuntimeDocument) -> Result<(), String>,
    {
        let mut handled = 0;
        loop {
            let queued = unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) };
            if queued <= 0 {
                break;
            }
            let mut event = MaybeUninit::<XEvent>::uninit();
            unsafe {
                XNextEvent(self.display, event.as_mut_ptr());
            }
            let event = unsafe { event.assume_init() };
            let Some(id) = self.windows.get(&unsafe { event.xany.window }).copied() else {
                continue;
            };
            let event_type = unsafe { event.type_ };
            // Pointer input is ignored while the popup is moving: the menu is not hit-testable
            // until its final frame. Keyboard input still reaches the final layout.
            let moving = self.slides.kind(id.get()).is_some();
            if moving && matches!(event_type, BUTTON_PRESS | BUTTON_RELEASE | MOTION_NOTIFY) {
                continue;
            }
            let outside_release = if event_type == BUTTON_RELEASE {
                let button = unsafe { event.xbutton };
                self.surfaces
                    .get(&id)
                    .map(|instance| {
                        instance.app.pointer_grabbed
                            && button_release_is_outside(
                                button.x,
                                button.y,
                                instance.app.width,
                                instance.app.height,
                            )
                    })
                    .unwrap_or(false)
            } else {
                false
            };
            unsafe {
                XPutBackEvent(self.display, &event as *const XEvent as *mut XEvent);
            }
            let release_info = if outside_release {
                let button = unsafe { event.xbutton };
                Some((button.button, button.x, button.y, button.time as u64))
            } else {
                None
            };
            let closed = {
                let instance = self.instance_mut(id)?;
                let mut callback = |event: &ActionEvent, document: &mut RuntimeDocument| {
                    on_action(
                        &SurfaceControllerEvent {
                            surface: id,
                            event: event.clone(),
                        },
                        document,
                    )
                };
                unsafe {
                    instance
                        .app
                        .event_loop_once(&mut instance.document, &mut callback)
                        .map_err(|error| {
                            mark_x_error_seen();
                            error
                        })?;
                }
                // Drain the document-owned damage latch into the app latch so
                // `redraw_dirty` can coalesce; event_loop already merged its
                // own marks via redraw_if_dirty.
                let pending = instance.document.take_damage();
                instance.app.mark_dirty(pending);
                instance.app.close_requested
            };
            if let Some((button, x, y, time_ms)) = release_info {
                let instance = self.instance_mut(id)?;
                let scale = instance.document.ui_scale();
                let outside =
                    outside_release_event(id, button, x as f32 / scale, y as f32 / scale, time_ms);
                on_action(&outside, &mut instance.document)?;
            }
            handled += 1;
            if closed {
                self.destroy_surface(id)?;
            }
        }
        Ok(handled)
    }

    pub fn document_mut(&mut self, id: SurfaceId) -> Result<&mut RuntimeDocument, String> {
        Ok(&mut self.instance_mut(id)?.document)
    }

    fn retained(&self, id: SurfaceId) -> Result<(&X11App, &RuntimeDocument), String> {
        let instance = self
            .surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        Ok((&instance.app, &instance.document))
    }

    /// Retained-live surface extent in root space: native origin + the
    /// retained window size. Reads retained state only; no LayoutEngine
    /// recompute, no headless fallback.
    pub fn surface_device_rect(&self, id: SurfaceId) -> Result<Rect, String> {
        let (app, _) = self.retained(id)?;
        let origin = self
            .surfaces
            .get(&id)
            .map(|instance| instance.origin)
            .unwrap_or((0, 0));
        Ok(Rect {
            x: origin.0 as f32,
            y: origin.1 as f32,
            width: app.width as f32,
            height: app.height as f32,
        })
    }

    /// Retained-live node rect in root space: native origin + the retained
    /// layout box scaled by ui_scale. None-shaped as Err when no retained
    /// layout exists (mapped-surface queries never use 1350x641 here).
    pub fn node_device_rect(&self, id: SurfaceId, index: u32) -> Result<Rect, String> {
        let (app, document) = self.retained(id)?;
        let origin = self
            .surfaces
            .get(&id)
            .map(|instance| instance.origin)
            .unwrap_or((0, 0));
        let local = app
            .node_global_rect(document, index)
            .ok_or_else(|| format!("surface {} has no retained layout", id.0))?;
        Ok(Rect {
            x: local.x + origin.0 as f32,
            y: local.y + origin.1 as f32,
            width: local.width,
            height: local.height,
        })
    }

    /// Node rect by string id from retained layout (root-space device rect).
    pub fn node_device_rect_by_id(&self, id: SurfaceId, node: &str) -> Result<Rect, String> {
        let (app, document) = self.retained(id)?;
        let index = document
            .node_by_id(node)
            .ok_or_else(|| format!("no node with id '{node}'"))?;
        let origin = self
            .surfaces
            .get(&id)
            .map(|instance| instance.origin)
            .unwrap_or((0, 0));
        let local = app
            .node_global_rect(document, index)
            .ok_or_else(|| format!("surface {} has no retained layout", id.0))?;
        Ok(Rect {
            x: local.x + origin.0 as f32,
            y: local.y + origin.1 as f32,
            width: local.width,
            height: local.height,
        })
    }

    /// Retained intrinsic content size of the document root in device
    /// pixels. Reads the retained layout; no LayoutEngine recompute.
    pub fn document_intrinsic_device_size(&self, id: SurfaceId) -> Result<(f32, f32), String> {
        let (app, document) = self.retained(id)?;
        app.document_intrinsic_size(document)
            .ok_or_else(|| format!("surface {} has no retained layout", id.0))
    }

    pub fn controller_event(&self, id: SurfaceId, event: ActionEvent) -> SurfaceControllerEvent {
        SurfaceControllerEvent { surface: id, event }
    }

    pub fn surface_role(&self, id: SurfaceId) -> Result<SurfaceRole, String> {
        Ok(self
            .surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?
            .role)
    }

    pub fn surface_input_mode(&self, id: SurfaceId) -> Result<SurfaceInputMode, String> {
        Ok(self
            .surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?
            .input)
    }

    /// Overlay management policy: overlays stay raisable but are never
    /// WM-managed, never focusable, and never in the workarea/taskbar.
    pub fn is_wm_managed(&self, id: SurfaceId) -> Result<bool, String> {
        Ok(!overlay_excluded_from_wm(self.surface_role(id)?))
    }

    pub fn is_pointer_grabbed(&self, id: SurfaceId) -> Result<bool, String> {
        Ok(self
            .surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?
            .app
            .pointer_grabbed)
    }

    pub fn run_with_reactor<F, R>(
        &mut self,
        reactor: &mut Reactor,
        mut on_action: F,
        mut on_tick: R,
    ) -> Result<(), String>
    where
        F: FnMut(&SurfaceControllerEvent, &mut RuntimeDocument) -> Result<(), String>,
        R: FnMut(&mut Self) -> Result<(), String>,
    {
        let token = reactor
            .register_fd(
                ReactorPebble(self.connection_fd()),
                calloop::Interest::READ,
                move |_, _| {
                    // X readiness only wakes dispatch; pump happens below.
                },
            )
            .map_err(|error| error.to_string())?;
        loop {
            reactor
                .dispatch(self.motion_frame_timeout())
                .map_err(|error| error.to_string())?;
            let drained = self.pump_events(&mut on_action)?;
            self.advance_motion(Instant::now());
            if self.surfaces.is_empty() {
                break;
            }
            // Timer sources registered by the caller also wake dispatch, so
            // tick callbacks always run; redraws follow drained X events.
            on_tick(self)?;
            if drained > 0 {
                self.redraw_dirty()?;
            }
        }
        reactor.remove(token).map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Dispatch one reactor cycle and return typed events after document callbacks finish.
    ///
    /// The UI bridge uses this boundary to apply surface operations without exposing the
    /// borrowed runtime document or holding the controller borrow across those operations.
    pub fn run_with_reactor_step(
        &mut self,
        reactor: &mut Reactor,
    ) -> Result<(Vec<SurfaceControllerEvent>, usize), String> {
        let token = reactor
            .register_fd(
                ReactorPebble(self.connection_fd()),
                calloop::Interest::READ,
                move |_, _| {
                    // X readiness only wakes dispatch; pump happens below.
                },
            )
            .map_err(|error| error.to_string())?;
        let mut events = Vec::new();
        let result: Result<usize, String> = (|| {
            reactor
                .dispatch(self.motion_frame_timeout())
                .map_err(|error| error.to_string())?;
            let drained = self.pump_events(|event, _document| {
                events.push(event.clone());
                Ok(())
            })?;
            self.advance_motion(Instant::now());
            Ok(drained)
        })();
        reactor.remove(token).map_err(|error| error.to_string())?;
        Ok((events, result?))
    }

    pub fn has_surfaces(&self) -> bool {
        !self.surfaces.is_empty()
    }

    fn instance_mut(&mut self, id: SurfaceId) -> Result<&mut SurfaceInstance, String> {
        self.surfaces
            .get_mut(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))
    }

    fn with_window<F>(&self, id: SurfaceId, operation: F) -> Result<(), String>
    where
        F: FnOnce(*mut Display, Window) -> i32,
    {
        let instance = self
            .surfaces
            .get(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        if operation(self.display, instance.app.window) == 0 {
            mark_x_error_seen();
            return Err(format!("X11 operation failed for surface {}", id.0));
        }
        unsafe {
            XFlush(self.display);
        }
        Ok(())
    }
}

/// Borrowed X connection fd wrapper for reactor registration.
/// The display outlives the registration; calloop never owns the fd.
struct ReactorPebble(RawFd);

impl AsFd for ReactorPebble {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        unsafe { std::os::fd::BorrowedFd::borrow_raw(self.0) }
    }
}

impl AsRawFd for ReactorPebble {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_release_bounds_are_window_relative() {
        assert!(!button_release_is_outside(10, 10, 800, 600));
        assert!(!button_release_is_outside(0, 0, 800, 600));
        assert!(!button_release_is_outside(799, 599, 800, 600));
        assert!(button_release_is_outside(-1, 10, 800, 600));
        assert!(button_release_is_outside(10, -1, 800, 600));
        assert!(button_release_is_outside(800, 10, 800, 600));
        assert!(button_release_is_outside(10, 600, 800, 600));
    }

    #[test]
    fn outside_event_uses_reserved_generic_action() {
        let event = outside_release_event(SurfaceId(3), 1, 850.0, 10.0, 42);
        assert_eq!(event.surface, SurfaceId(3));
        assert_eq!(event.event.action, OUTSIDE_RELEASE_ACTION);
        assert_eq!(event.event.phase, ActionPhase::Release);
        assert_eq!(event.event.button, 1);
        assert!(!event.event.inside);
        assert_eq!(event.event.time_ms, 42);
        assert!(event.event.text.is_none());
    }

    #[test]
    fn surface_id_round_trips() {
        let id = SurfaceId(7);
        assert_eq!(id.get(), 7);
    }

    #[test]
    fn overlay_maps_to_notification_type_and_is_unmanaged() {
        assert_eq!(
            overlay_window_type_name(SurfaceRole::Overlay),
            "_NET_WM_WINDOW_TYPE_NOTIFICATION"
        );
        assert!(overlay_excluded_from_wm(SurfaceRole::Overlay));
        assert!(!overlay_excluded_from_wm(SurfaceRole::Normal));
        assert!(!overlay_excluded_from_wm(SurfaceRole::Dock));
    }

    #[test]
    fn passthrough_refuses_grab_and_focus() {
        assert!(!SurfaceInputMode::PassThrough.allows_grab());
        assert!(!SurfaceInputMode::PassThrough.allows_focus());
        assert!(SurfaceInputMode::Interactive.allows_grab());
        assert!(SurfaceInputMode::Interactive.allows_focus());
    }

    #[test]
    fn config_maps_overlay_role() {
        let config = SurfaceConfig {
            role: SurfaceRole::Overlay,
            ..SurfaceConfig::default()
        };
        let x11: X11Config = config.into();
        assert_eq!(x11.role, X11WindowRole::Overlay);
    }

    #[test]
    fn popup_policy_maps_raise_for_transients_only() {
        assert!(!surface_role_needs_popup_raise(SurfaceRole::Normal));
        assert!(!surface_role_needs_popup_raise(SurfaceRole::Desktop));
        assert!(!surface_role_needs_popup_raise(SurfaceRole::Dock));
        assert!(surface_role_needs_popup_raise(SurfaceRole::PopupMenu));
        assert!(surface_role_needs_popup_raise(SurfaceRole::DropdownMenu));
        assert!(surface_role_needs_popup_raise(SurfaceRole::Overlay));
        assert!(!x11_role_needs_popup_raise(X11WindowRole::Normal));
        assert!(x11_role_needs_popup_raise(X11WindowRole::PopupMenu));
        assert!(x11_role_needs_popup_raise(X11WindowRole::DropdownMenu));
        assert!(x11_role_needs_popup_raise(X11WindowRole::Overlay));
    }

    #[test]
    fn damage_latch_merges_without_paint() {
        use flamewm_render_core::Rect;
        assert!(SurfaceDamage::None.is_empty());
        assert!(!SurfaceDamage::Full.is_empty());
        assert!(!SurfaceDamage::Region(Rect::default()).is_empty());
        assert_eq!(
            SurfaceDamage::None.merge(SurfaceDamage::Full),
            SurfaceDamage::Full
        );
        assert_eq!(
            SurfaceDamage::Full.merge(SurfaceDamage::None),
            SurfaceDamage::Full
        );
        assert_eq!(
            SurfaceDamage::None.merge(SurfaceDamage::None),
            SurfaceDamage::None
        );
    }

    #[test]
    fn stack_order_predicate_desktop_normal_dock_popup() {
        assert!(stack_order_ok(&[
            SurfaceRole::Desktop,
            SurfaceRole::Normal,
            SurfaceRole::Dock,
            SurfaceRole::PopupMenu,
        ]));
        assert!(stack_order_ok(&[
            SurfaceRole::Desktop,
            SurfaceRole::Normal,
            SurfaceRole::Normal,
            SurfaceRole::Dock,
            SurfaceRole::DropdownMenu,
            SurfaceRole::Overlay,
        ]));
        // Empty/single are trivially ordered.
        assert!(stack_order_ok(&[]));
        assert!(stack_order_ok(&[SurfaceRole::Dock]));
        // Inversions fail: popup below dock/normal, dock below normal,
        // normal below desktop.
        assert!(!stack_order_ok(&[
            SurfaceRole::PopupMenu,
            SurfaceRole::Dock,
        ]));
        assert!(!stack_order_ok(&[SurfaceRole::Dock, SurfaceRole::Normal,]));
        assert!(!stack_order_ok(&[
            SurfaceRole::Normal,
            SurfaceRole::Desktop,
        ]));
        assert!(!stack_order_ok(&[
            SurfaceRole::Desktop,
            SurfaceRole::Dock,
            SurfaceRole::Normal,
        ]));
    }

    #[test]
    fn debug_helpers_are_gated() {
        // Gating is pure: without FLAMEWM_DEBUG=1 the canary/probe report
        // refusal intent (non-debug callers never reach X). Exact env
        // behavior is asserted without touching process env.
        assert!(!debug_enabled() || std::env::var_os("FLAMEWM_DEBUG").is_some_and(|v| v == "1"));
    }
    #[test]
    fn one_txn_traces_full_geometry_chain() {
        // C04: one correlated id traces anchor/intrinsic/fitted/ui-request/
        // native-request/observed/retained with the same txn + role.
        let trace = GeometryTrace::begin("start-menu").for_surface(7);
        trace.anchor((8, 400, 48, 48));
        trace.intrinsic((320, 480));
        trace.fitted((8, 400, 320, 480));
        trace.ui_request((8, 400, 320, 480));
        trace.native_request((8, 400, 320, 480));
        trace.observed((8, 400, 320, 480));
        trace.retained((8, 400, 320, 480));
        let stages = geometry_trace_snapshot(trace.txn);
        let names: Vec<&str> = stages.iter().map(|s| s.stage).collect();
        assert_eq!(
            names,
            [
                "anchor",
                "intrinsic",
                "fitted",
                "ui-request",
                "native-request",
                "observed",
                "retained"
            ]
        );
        assert!(stages.iter().all(|s| s.txn == trace.txn));
        assert!(stages.iter().all(|s| s.role == "start-menu"));
        assert!(stages.iter().all(|s| s.surface == 7));
        // Distinct txns never leak into each other.
        let other = GeometryTrace::begin("start-menu").for_surface(7);
        assert_ne!(other.txn, trace.txn);
        other.anchor((0, 0, 10, 10));
        assert_eq!(geometry_trace_snapshot(other.txn).len(), 1);
        assert_eq!(geometry_trace_snapshot(trace.txn).len(), 7);
    }
}

#[cfg(test)]
mod present_cycle_tests {
    use super::*;

    #[test]
    fn full_cycle_runs_each_stage_once_in_order() {
        let mut cycle = PresentCycle::new();
        cycle.run_full_cycle().expect("full cycle runs");
        assert!(cycle.is_complete());
        assert!(cycle.advance("present").is_err());
        assert!(cycle.advance("bogus").is_err());
    }

    #[test]
    fn out_of_order_stage_is_rejected() {
        let mut cycle = PresentCycle::new();
        assert!(cycle.advance("paint").is_err());
        for stage in ["project", "measure", "anchor"] {
            cycle.advance(stage).expect("in-order stage runs");
        }
        assert!(cycle.advance("paint").is_err());
        cycle.advance("prepare").expect("prepare runs");
        cycle.advance("paint").expect("paint runs after prepare");
        assert!(!cycle.is_complete());
    }

    #[test]
    fn retained_geometry_helpers_are_headless_free() {
        // Pure helpers: scale 1.0/1.5 math + origin offset, no X needed.
        fn scaled_box_rect(
            layout: &flamewm_render_core::LayoutResult,
            scale: f32,
            index: u32,
        ) -> Option<Rect> {
            layout.boxes.get(index as usize).map(|b| Rect {
                x: b.rect.x * scale,
                y: b.rect.y * scale,
                width: b.rect.width * scale,
                height: b.rect.height * scale,
            })
        }
        fn root_space_rect(origin: (i32, i32), local: Rect) -> Rect {
            Rect {
                x: local.x + origin.0 as f32,
                y: local.y + origin.1 as f32,
                width: local.width,
                height: local.height,
            }
        }
        use flamewm_render_core::{LayoutBox, LayoutResult};
        let rect = Rect {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        };
        let layout = LayoutResult {
            boxes: vec![LayoutBox { rect }],
            contents: vec![rect],
            revision: 1,
            z_order: vec![0],
        };
        let at_scale1 = scaled_box_rect(&layout, 1.0, 0).expect("box retained");
        assert_eq!((at_scale1.width, at_scale1.height), (100.0, 50.0));
        let at_scale15 = scaled_box_rect(&layout, 1.5, 0).expect("box retained");
        assert_eq!((at_scale15.x, at_scale15.width), (15.0, 150.0));
        let rooted = root_space_rect((40, 60), at_scale15);
        assert_eq!((rooted.x, rooted.y), (55.0, 90.0));
        // Intrinsic device size scales the retained content extent.
        let content = layout.contents[0];
        assert_eq!((content.width * 1.5, content.height * 1.5), (150.0, 75.0));
        // Headless fallback extent is the documented constant, never a
        // mapped-surface query result.
        assert_eq!(HEADLESS_FALLBACK_EXTENT, (1350.0, 641.0));
    }
}

impl Drop for SurfaceController {
    fn drop(&mut self) {
        // Null guard mirrors ExternalDecorationRenderer::drop. On a broken
        // X connection (IO-error latch set) skip per-surface X teardown and
        // XCloseDisplay entirely: the fd may stay open so fd probes see
        // "alive", yet any X call touches dead state (SIGSEGV). Forgetting
        // leaks client-side state the dead server can no longer free; the
        // normal path is unchanged (surfaces -> windows -> close).
        if self.display.is_null()
            || x_io_broken()
            || x_error_seen()
            || !display_fd_alive(self.display)
        {
            let surfaces = std::mem::take(&mut self.surfaces);
            std::mem::forget(surfaces);
            self.windows.clear();
            let deferred = std::mem::take(&mut self.deferred_native_libraries);
            std::mem::forget(deferred);
            return;
        }
        let mut deferred_native = std::mem::take(&mut self.deferred_native_libraries);
        for instance in self.surfaces.values_mut() {
            // SAFETY: the display owner has verified a live connection, and
            // this owner retains every dynamic handle beyond XCloseDisplay.
            deferred_native.push(unsafe { instance.app.take_deferred_native_libraries() });
        }
        self.surfaces.clear();
        self.windows.clear();
        unsafe {
            XCloseDisplay(self.display);
        }
        drop(deferred_native);
    }
}

/// True while the X connection fd still exists. After an X IO error the
/// fd is gone; probing via /proc needs std only (no new deps).
fn display_fd_alive(display: *mut Display) -> bool {
    let fd = unsafe { XConnectionNumber(display) };
    if fd < 0 {
        return false;
    }
    std::path::Path::new(&format!("/proc/self/fd/{fd}")).exists()
}

#[cfg(test)]
mod slide_motion_tests {
    use super::*;

    const FINAL: (i32, i32) = (100, 500);
    const SIZE: (u32, u32) = (300, 400);

    #[test]
    fn slide_origin_starts_off_the_panel_edge_per_edge() {
        assert_eq!(slide_origin(FINAL, SIZE, SlideEdge::Bottom, 0.0), (100, 900));
        assert_eq!(slide_origin(FINAL, SIZE, SlideEdge::Top, 0.0), (100, 100));
        assert_eq!(slide_origin(FINAL, SIZE, SlideEdge::Left, 0.0), (-200, 500));
        assert_eq!(slide_origin(FINAL, SIZE, SlideEdge::Right, 0.0), (400, 500));
    }

    #[test]
    fn final_frame_is_exact_anchored_origin_for_every_edge() {
        for edge in [SlideEdge::Top, SlideEdge::Bottom, SlideEdge::Left, SlideEdge::Right] {
            assert_eq!(slide_origin(FINAL, SIZE, edge, 1.0), FINAL, "{edge:?}");
            assert_eq!(slide_origin(FINAL, SIZE, edge, 1.5), FINAL, "{edge:?} clamped");
        }
    }

    const OPEN: SlideKind = SlideKind { edge: SlideEdge::Bottom, opening: true };
    const CLOSE: SlideKind = SlideKind { edge: SlideEdge::Bottom, opening: false };

    #[test]
    fn progress_endpoints_and_ease_out_midpoint() {
        let t0 = Instant::now();
        let mut slides = Timeline::new();
        slides.start_towards(1, OPEN, t0, SLIDE_OPEN_BASE_MS, Easing::EaseOutCubic, (0.0, 1.0));
        assert_eq!(slides.progress(1, t0), Some(0.0));
        assert_eq!(slides.progress(1, t0 + Duration::from_millis(180)), Some(1.0));
        assert_eq!(slides.progress(1, t0 + Duration::from_secs(10)), Some(1.0));
        assert_eq!(slides.finished(1, t0 + Duration::from_millis(179)), Some(false));
        assert_eq!(slides.finished(1, t0 + Duration::from_millis(180)), Some(true));
        // Ease-out cubic: halfway in time is already past half the travel.
        assert!(slides.progress(1, t0 + Duration::from_millis(90)).unwrap() > 0.5);
    }

    #[test]
    fn reverse_mid_flight_continues_from_current_progress() {
        let t0 = Instant::now();
        let mut slides = Timeline::new();
        slides.start_towards(1, OPEN, t0, SLIDE_OPEN_BASE_MS, Easing::EaseOutCubic, (0.0, 1.0));
        let now = t0 + Duration::from_millis(60);
        let before = slides.progress(1, now).unwrap();
        slides.start_towards(1, CLOSE, now, SLIDE_CLOSE_BASE_MS, Easing::EaseOutCubic, (1.0, 0.0));
        assert!((slides.progress(1, now).unwrap() - before).abs() < 1e-6, "no jump on reverse");
        assert_eq!(slides.progress(1, now + Duration::from_millis(120)), Some(0.0));
        // Reversing back to open from the midpoint is also jump-free.
        let mid = now + Duration::from_millis(40);
        let mid_value = slides.progress(1, mid).unwrap();
        slides.start_towards(1, OPEN, mid, SLIDE_OPEN_BASE_MS, Easing::EaseOutCubic, (0.0, 1.0));
        assert!((slides.progress(1, mid).unwrap() - mid_value).abs() < 1e-6);
        assert_eq!(slides.progress(1, mid + Duration::from_millis(180)), Some(1.0));
    }

    #[test]
    fn close_from_fully_shown_starts_at_one_and_ends_at_zero() {
        let t0 = Instant::now();
        let mut slides = Timeline::new();
        slides.start_towards(1, CLOSE, t0, SLIDE_CLOSE_BASE_MS, Easing::EaseOutCubic, (1.0, 0.0));
        assert_eq!(slides.progress(1, t0), Some(1.0));
        assert_eq!(slides.progress(1, t0 + Duration::from_millis(120)), Some(0.0));
    }

    #[test]
    fn disabled_or_idle_state_arms_no_frame_wait() {
        assert_eq!(motion_frame_wait(false), None);
        assert_eq!(motion_frame_wait(true), Some(FRAME_INTERVAL));
    }

    #[test]
    fn disabled_timeline_refuses_slide_so_surface_maps_immediately() {
        let mut slides = Timeline::<SlideKind>::new();
        slides.set_enabled(false);
        let now = Instant::now();
        assert_eq!(
            slides.start_towards(1, OPEN, now, SLIDE_OPEN_BASE_MS, Easing::EaseOutCubic, (0.0, 1.0)),
            Start::Immediate
        );
        assert!(slides.is_empty());
        assert_eq!(motion_frame_wait(!slides.is_empty()), None);
    }

    #[test]
    fn durations_scale_with_speed() {
        let mut slides = Timeline::<SlideKind>::new();
        slides.set_speed(100);
        assert_eq!(slides.duration_for(SLIDE_OPEN_BASE_MS), Duration::from_millis(180));
        slides.set_speed(200);
        assert_eq!(slides.duration_for(SLIDE_OPEN_BASE_MS), Duration::from_millis(90));
        slides.set_speed(25);
        assert_eq!(slides.duration_for(SLIDE_OPEN_BASE_MS), Duration::from_millis(720));
    }
}

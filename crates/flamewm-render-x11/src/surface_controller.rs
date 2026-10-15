use std::collections::HashSet;
use std::ffi::CStr;
use std::os::fd::{AsFd, AsRawFd, RawFd};
use std::os::raw::c_char;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::*;
use crate::native::cursor::DeferredNativeLibraryHandles;
use crate::xinput::*;
use crate::xlib::*;
use crate::xshape::ClipRect;
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
    /// Reads WM_NAME of `window`; the caller frees the string with XFree.
    pub(crate) fn XFetchName(
        display: *mut Display,
        window: Window,
        window_name_return: *mut *mut c_char,
    ) -> std::os::raw::c_int;
}

/// Selection key of the quick-popup press observer. Never a real surface id.
const PRESS_OBSERVER: SurfaceId = SurfaceId(u64::MAX);

/// One root press seen while the press observer is armed. `child_window_title`
/// is the WM_NAME of the top-level window under the pointer, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPressObservation {
    pub x_root: i32,
    pub y_root: i32,
    pub button: u32,
    pub time_ms: u64,
    pub child_window_title: Option<String>,
}

const REVERT_TO_POINTER_ROOT: std::os::raw::c_int = 1;

/// XInput2 2.0 raw selection for popup outside-dismissal. RawButtonPress is
/// delivered to this client without any pointer grab, so every click reaches the
/// window beneath it. Selected only while a popup holds its pointer grab.
/// The root selection is shared by every armed popup: it is written when the
/// first surface arms and cleared only when the last armed surface disarms.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RawPressSelection {
    opcode: Option<std::os::raw::c_int>,
    armed: HashSet<SurfaceId>,
}

impl RawPressSelection {
    pub(crate) fn new(opcode: Option<std::os::raw::c_int>) -> Self {
        Self { opcode, armed: HashSet::new() }
    }

    /// XInput2 major opcode of the raw events this selection accepts.
    pub(crate) fn opcode(&self) -> Option<std::os::raw::c_int> {
        self.opcode
    }

    /// Arms `surface`. Returns true only when the set goes empty -> non-empty,
    /// i.e. the root selection must be written. Idempotent per surface.
    pub(crate) fn arm(&mut self, surface: SurfaceId) -> bool {
        if self.opcode.is_none() {
            return false;
        }
        let was_empty = self.armed.is_empty();
        self.armed.insert(surface) && was_empty
    }

    /// Disarms `surface`. Returns true only when the set goes non-empty -> empty,
    /// i.e. the root selection must be cleared. Idempotent per surface.
    pub(crate) fn disarm(&mut self, surface: SurfaceId) -> bool {
        self.armed.remove(&surface) && self.armed.is_empty()
    }
}

/// Event mask bits for XI_RawButtonPress only.
fn raw_press_mask() -> [u8; 2] {
    let mut mask = [0u8; 2];
    mask[(XI_RAW_BUTTON_PRESS >> 3) as usize] |= 1 << (XI_RAW_BUTTON_PRESS & 7);
    mask
}

/// Select (`armed`) or clear this client's RawButtonPress selection on the root.
/// An empty mask (mask_len 0) removes only this client's selection.
unsafe fn write_raw_press_selection(display: *mut Display, root: Window, armed: bool) {
    let mut mask = raw_press_mask();
    let mut event_mask = XIEventMask {
        deviceid: XI_ALL_MASTER_DEVICES,
        mask_len: if armed { mask.len() as std::os::raw::c_int } else { 0 },
        mask: mask.as_mut_ptr(),
    };
    // SAFETY: live connection; the mask outlives the request.
    unsafe {
        XISelectEvents(display, root, &mut event_mask, 1);
        XFlush(display);
    }
}

/// WM_NAME of `window`, or `None` when unset or unreadable.
unsafe fn window_title(display: *mut Display, window: Window) -> Option<String> {
    let mut name: *mut c_char = std::ptr::null_mut();
    // SAFETY: live connection; `name` is freed below when XFetchName returns one.
    unsafe {
        if XFetchName(display, window, &mut name) == 0 || name.is_null() {
            return None;
        }
        let title = CStr::from_ptr(name).to_string_lossy().into_owned();
        XFree(name.cast());
        Some(title)
    }
}

/// XInput2 opcode when the server speaks XI 2.0. `None` disables raw dismissal;
/// FocusOut and Escape still dismiss the popup.
unsafe fn xi_raw_press_opcode(display: *mut Display) -> Option<std::os::raw::c_int> {
    // SAFETY: live connection; out-pointers are locals.
    unsafe {
        let mut opcode: std::os::raw::c_int = 0;
        let mut first_event: std::os::raw::c_int = 0;
        let mut first_error: std::os::raw::c_int = 0;
        if XQueryExtension(
            display,
            b"XInputExtension\0".as_ptr().cast(),
            &mut opcode,
            &mut first_event,
            &mut first_error,
        ) == 0
        {
            return None;
        }
        let mut major: std::os::raw::c_int = 2;
        let mut minor: std::os::raw::c_int = 0;
        let status = XIQueryVersion(display, &mut major, &mut minor);
        (status == 0 && (major, minor) >= (2, 0)).then_some(opcode)
    }
}

pub(crate) unsafe fn grab_popup_presses(
    display: *mut Display,
    root: Window,
    selection: &mut RawPressSelection,
    surface: SurfaceId,
) {
    if selection.arm(surface) {
        // SAFETY: display/root are the live controller connection and root window.
        unsafe { write_raw_press_selection(display, root, true) };
    }
}

pub(crate) unsafe fn release_popup_presses(
    display: *mut Display,
    root: Window,
    selection: &mut RawPressSelection,
    surface: SurfaceId,
) {
    if selection.disarm(surface) {
        // SAFETY: same live connection; the selection was armed by grab_popup_presses.
        unsafe { write_raw_press_selection(display, root, false) };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrabbedPressOutcome {
    Inside,
    Outside,
}

/// Classify a root-space press against the popup rect (device px).
pub fn classify_grabbed_press(
    x_root: i32,
    y_root: i32,
    origin: (i32, i32),
    width: u32,
    height: u32,
) -> GrabbedPressOutcome {
    if button_release_is_outside(x_root - origin.0, y_root - origin.1, width, height) {
        GrabbedPressOutcome::Outside
    } else {
        GrabbedPressOutcome::Inside
    }
}

/// Pure mapping of one root RawButtonPress to the popup's outside-dismiss event.
/// `popup` is (origin, size) in device px; `None` for any other event type or a
/// press inside the popup rect.
pub(crate) fn raw_press_outside_event(
    surface: SurfaceId,
    event_type: std::os::raw::c_int,
    button: u32,
    press: (i32, i32),
    popup: ((i32, i32), (u32, u32)),
    scale: f32,
    time_ms: u64,
) -> Option<SurfaceControllerEvent> {
    let (origin, (width, height)) = popup;
    if event_type != XI_RAW_BUTTON_PRESS
        || classify_grabbed_press(press.0, press.1, origin, width, height) != GrabbedPressOutcome::Outside
    {
        return None;
    }
    Some(outside_release_event(
        surface,
        button,
        (press.0 - origin.0) as f32 / scale,
        (press.1 - origin.1) as f32 / scale,
        time_ms,
    ))
}

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

/// Pure bounds check: window-relative coords outside `[0, width) x [0, height)`
/// mean "outside the surface". Popups take no active pointer grab; outside
/// presses arrive as raw root presses (see `RawPressSelection`).
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

/// Device-px corner radius of a popup, the same value the retained shape mask uses
/// (`app::render::backdrop_corner_radius_px`, duplicated here because that module is private).
fn popup_backdrop_radius_px(document: &RuntimeDocument, width: u32, height: u32) -> u32 {
    let radius =
        flamewm_render_core::paint::root_corner_radius(document, flamewm_render_core::InteractionState::default());
    flamewm_render_core::paint::surface_shape_pixels(width, height, radius * document.ui_scale()).2
}

/// Anti-aliased coverage (0..=1) of pixel `(px, py)` inside a `width` x `height` rounded rect.
/// Pixel centres are sampled, so the arc has a one-pixel ramp rather than a hard edge.
fn rounded_coverage(px: u32, py: u32, width: u32, height: u32, radius: u32) -> f32 {
    if radius == 0 {
        return 1.0;
    }
    let (w, h) = (width as f32, height as f32);
    let r = (radius as f32).min(w / 2.0).min(h / 2.0);
    let lx = px as f32 + 0.5;
    let ly = py as f32 + 0.5;
    let cx = lx.clamp(r, w - r);
    let cy = ly.clamp(r, h - r);
    let dist = ((lx - cx).powi(2) + (ly - cy).powi(2)).sqrt();
    (r + 0.5 - dist).clamp(0.0, 1.0)
}

/// A popup hide that starts a slide-out keeps its pointer grab until the slide lands.
fn slide_out_keeps_grab(slide: Option<SlideConfig>, shown: bool) -> bool {
    shown && slide.is_some_and(|config| config.transition == PopupTransition::Slide)
}

/// Outside presses during a slide-out are ignored: the popup is already closing and must not
/// dismiss again while its grab is still held.
fn slide_out_swallows_press(active: Option<SlideKind>) -> bool {
    active.is_some_and(|kind| !kind.opening)
}

/// Masks an opaque ARGB snapshot (0xAARRGGBB, row-major) to the rounded shape, writing
/// premultiplied words so corners outside the arc are transparent in every channel.
fn round_backdrop_words(words: &mut [u32], width: u32, height: u32, radius: u32) {
    if width == 0 {
        return;
    }
    for (index, word) in words.iter_mut().enumerate() {
        let index = index as u32;
        let coverage = rounded_coverage(index % width, index / width, width, height, radius);
        if coverage >= 1.0 {
            continue;
        }
        let alpha = (coverage * 255.0).round() as u32;
        let premul = |shift: u32| ((*word >> shift) & 0xff) * alpha / 255;
        *word = (alpha << 24) | (premul(16) << 16) | (premul(8) << 8) | premul(0);
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
    /// Opt-in presentation for the next show/hide; `None` is the immediate path.
    slide: Option<SlideConfig>,
    /// Whether the window is mapped from the controller's point of view (slide bookkeeping).
    shown: bool,
    /// Native origin last written by slide motion; skips a repeat move to the same pixel.
    slide_drawn: Option<(i32, i32)>,
    /// Fade or Magic Lamp transition in flight, with the transition that drives its progress.
    /// The real window stays unmapped while it runs.
    overlay: Option<(PopupTransition, WarpOverlay)>,
    /// A pointer grab requested while the real window is still unmapped behind an opening
    /// overlay: taken when the flight lands so the popup is mapped before it takes focus.
    grab_pending: bool,
    /// Root edge this dock reserves (floating docks touch no edge). `None` infers from the rect.
    dock_edge: Option<DockEdge>,
    /// Corner radius (window px) last published by `set_dock_frame`; zero means rectangular.
    dock_radius: u32,
    /// `(opacity, blur)` percents for a pseudo-transparency backdrop captured before each first map.
    popup_backdrop: Option<(u8, u8)>,
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

/// Applies the dock corner mask for `instance` at window size `w` x `h`. `Keep` with a zero
/// radius is a no-op, so classic docks never touch the XShape state.
fn apply_dock_mask(instance: &SurfaceInstance, w: u32, h: u32, mask: DockMask) {
    let radius = match mask {
        DockMask::Keep if instance.dock_radius == 0 => return,
        DockMask::Keep => instance.dock_radius,
        DockMask::Round(radius) => radius,
        DockMask::Reset => 0,
    };
    let spans = crate::xshape::rounded_mask_spans(w, h, radius);
    if let Some(bridge) = instance.app.xshape.as_ref() {
        if let Err(error) = unsafe { bridge.apply_rounded_mask(instance.app.window, &spans) } {
            eprintln!("FLAMEWM_DOCK_SHAPE degraded reason={error}");
        }
    }
}

#[cfg(test)]
mod dock_frame_plan_tests {
    use super::*;

    #[test]
    fn classic_to_classic_keeps_shape() {
        assert_eq!(dock_mask_plan(0, 0), DockMask::Keep);
    }

    #[test]
    fn floating_to_classic_resets_rectangle() {
        assert_eq!(dock_mask_plan(12, 0), DockMask::Reset);
    }

    #[test]
    fn floating_radius_rounds_and_resize_keeps_it() {
        assert_eq!(dock_mask_plan(0, 12), DockMask::Round(12));
        assert_eq!(dock_mask_plan(12, 12), DockMask::Round(12));
        assert_eq!(dock_mask_plan(12, 6), DockMask::Round(6));
    }
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
fn panel_clip(dock: Option<(i32, i32, u32, u32)>, origin: (i32, i32), size: (u32, u32), edge: SlideEdge) -> ClipRect {
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
    Lamp { target: (i32, i32, u32, u32) },
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
        Self { edge, speed, transition: PopupTransition::Slide }
    }

    pub fn fade(edge: SlideEdge, speed: u16) -> Self {
        Self { edge, speed, transition: PopupTransition::Fade }
    }

    pub fn lamp(edge: SlideEdge, speed: u16, target: (i32, i32, u32, u32)) -> Self {
        Self { edge, speed, transition: PopupTransition::Lamp { target } }
    }
}

/// Panel edge as the Magic Lamp bend sees it.
fn lamp_edge(edge: SlideEdge) -> crate::lamp::PanelEdge {
    match edge {
        SlideEdge::Top => crate::lamp::PanelEdge::Top,
        SlideEdge::Bottom => crate::lamp::PanelEdge::Bottom,
        SlideEdge::Left => crate::lamp::PanelEdge::Left,
        SlideEdge::Right => crate::lamp::PanelEdge::Right,
    }
}

/// Overlay style for a transition. Magic Lamp is the only bend; everything else crossfades.
fn overlay_style(transition: PopupTransition) -> TransitionStyle {
    match transition {
        PopupTransition::Lamp { .. } => TransitionStyle::MagicLamp,
        PopupTransition::Slide | PopupTransition::Fade => TransitionStyle::Fade,
    }
}

/// Root rect the overlay folds toward: the Start button for Magic Lamp, the popup itself otherwise.
fn overlay_target(transition: PopupTransition, popup: WarpRect) -> WarpRect {
    match transition {
        PopupTransition::Lamp { target: (x, y, width, height) } => WarpRect {
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
fn overlay_paint_progress(transition: PopupTransition, progress: f32) -> f64 {
    match transition {
        PopupTransition::Lamp { .. } => f64::from(1.0 - progress),
        PopupTransition::Slide | PopupTransition::Fade => f64::from(progress),
    }
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

/// Core X event types and SubstructureNotifyMask, defined here because `xlib.rs` is pinned.
const ROOT_CHILD_DESTROY_NOTIFY: std::os::raw::c_int = 17;
const ROOT_CHILD_UNMAP_NOTIFY: std::os::raw::c_int = 18;
const ROOT_CHILD_MAP_NOTIFY: std::os::raw::c_int = 19;
const ROOT_CHILD_CONFIGURE_NOTIFY: std::os::raw::c_int = 22;
const ROOT_SUBSTRUCTURE_NOTIFY_MASK: std::ffi::c_long = 1 << 19;

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
    /// A root child was mapped, unmapped, configured or destroyed since the last `take_root_changed`.
    root_changed: bool,
    /// XInput2 raw-press selection that dismisses popups on outside presses.
    press_selection: RawPressSelection,
    /// Set while the quick-popup press observer holds its root selection.
    press_observer_armed: bool,
    /// Root presses observed while armed, drained by the owner.
    raw_press_observations: Vec<RawPressObservation>,
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
            let press_opcode = xi_raw_press_opcode(display);
            if press_opcode.is_none() {
                eprintln!("flamewm: XInput2 2.0 unavailable; popups dismiss on focus loss and Escape only");
            }
            Ok(Self {
                screen: XDefaultScreen(display),
                display,
                next_id: 1,
                surfaces: HashMap::new(),
                windows: HashMap::new(),
                deferred_native_libraries: Vec::new(),
                icon_geometry_atom: 0,
                slides: Timeline::new(),
                root_changed: false,
                press_selection: RawPressSelection::new(press_opcode),
                press_observer_armed: false,
                raw_press_observations: Vec::new(),
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

    /// Publishes the dock frame: `Some(edge)` reserves that root edge and clips the window to
    /// `corner_radius` (window px). `None` is classic: legacy strut inference and a full
    /// rectangle mask, restored only if this surface was previously floating.
    pub fn set_dock_frame(
        &mut self,
        id: SurfaceId,
        edge: Option<DockEdge>,
        corner_radius: u32,
    ) -> Result<(), String> {
        let display = self.display;
        let (root_w, root_h) = self.root_geometry();
        let instance = self.instance_mut(id)?;
        if instance.role != SurfaceRole::Dock {
            return Err("set_dock_frame requires a dock surface".to_string());
        }
        if instance.dock_edge == edge && instance.dock_radius == corner_radius {
            return Ok(());
        }
        let mask = dock_mask_plan(instance.dock_radius, corner_radius);
        instance.dock_edge = edge;
        instance.dock_radius = corner_radius;
        instance.app.dock_corner_radius = corner_radius;
        let (x, y) = instance.origin;
        let (w, h) = (instance.app.width, instance.app.height);
        crate::app::apply_dock_strut(
            instance.app.display,
            instance.app.window,
            root_w,
            root_h,
            edge,
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
            overlay: None,
            grab_pending: false,
            dock_edge: None,
            dock_radius: 0,
            popup_backdrop: None,
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

    fn root_window(&self) -> Window {
        // SAFETY: the controller owns the live display; root is a plain query.
        unsafe { XRootWindow(self.display, self.screen) }
    }

    /// Dismiss the grabbed popup when a root RawButtonPress lands outside it. The
    /// press is only observed: it is never grabbed, replayed or consumed.
    fn dismiss_on_raw_press(
        &mut self,
        event: &XEvent,
        on_action: &mut impl FnMut(&SurfaceControllerEvent, &mut RuntimeDocument) -> Result<(), String>,
    ) -> Result<(), String> {
        let display = self.display;
        // SAFETY: `event` is a GenericEvent per the caller's `GENERIC_EVENT` check.
        let mut cookie = unsafe { generic_cookie(event) };
        // SAFETY: the cookie is a copy of this XNextEvent generic event; Xlib fills `data`.
        if unsafe { XGetEventData(display, &mut cookie) } == 0 {
            return Ok(());
        }
        // SAFETY: XGetEventData succeeded, so `data` is an XIRawEvent until XFreeEventData.
        let raw = unsafe { *(cookie.data as *const XIRawEvent) };
        unsafe { XFreeEventData(display, &mut cookie) };
        if raw.evtype != XI_RAW_BUTTON_PRESS {
            return Ok(());
        }
        let root = self.root_window();
        let mut root_return: Window = 0;
        let mut child_return: Window = 0;
        let (mut x_root, mut y_root, mut win_x, mut win_y) = (0, 0, 0, 0);
        let mut mask: std::os::raw::c_uint = 0;
        // SAFETY: live connection; out-pointers are locals.
        let on_screen = unsafe {
            XQueryPointer(
                display,
                root,
                &mut root_return,
                &mut child_return,
                &mut x_root,
                &mut y_root,
                &mut win_x,
                &mut win_y,
                &mut mask,
            )
        };
        if on_screen == 0 {
            return Ok(());
        }
        if self.press_observer_armed {
            let title = (child_return != 0)
                .then(|| unsafe { window_title(display, child_return) })
                .flatten();
            self.raw_press_observations.push(RawPressObservation {
                x_root,
                y_root,
                button: raw.detail as u32,
                time_ms: raw.time as u64,
                child_window_title: title,
            });
        }
        let owner = self
            .surfaces
            .iter()
            .find(|(id, instance)| {
                instance.app.pointer_grabbed && !slide_out_swallows_press(self.slides.kind(id.get()))
            })
            .map(|(id, instance)| {
                (*id, instance.origin, (instance.app.width, instance.app.height), instance.document.ui_scale())
            });
        let Some((id, origin, size, scale)) = owner else {
            return Ok(());
        };
        let Some(outside) = raw_press_outside_event(
            id,
            raw.evtype,
            raw.detail as u32,
            (x_root, y_root),
            (origin, size),
            scale,
            raw.time as u64,
        ) else {
            return Ok(());
        };
        let instance = self.instance_mut(id)?;
        on_action(&outside, &mut instance.document)
    }

    /// Observe root presses (no grab, no replay) while quick popups are open.
    /// Idempotent; the observer shares the root selection with popup grabs.
    pub fn arm_raw_press_observer(&mut self) {
        if self.press_observer_armed {
            return;
        }
        self.press_observer_armed = true;
        // SAFETY: live controller connection and root window.
        unsafe {
            grab_popup_presses(self.display, self.root_window(), &mut self.press_selection, PRESS_OBSERVER)
        };
    }

    /// Stop observing and drop anything still queued. Idempotent.
    pub fn disarm_raw_press_observer(&mut self) {
        if !self.press_observer_armed {
            return;
        }
        self.press_observer_armed = false;
        self.raw_press_observations.clear();
        // SAFETY: same live connection; armed by arm_raw_press_observer.
        unsafe {
            release_popup_presses(self.display, self.root_window(), &mut self.press_selection, PRESS_OBSERVER)
        };
    }

    /// Root presses observed since the last call, in arrival order.
    pub fn take_raw_press_observations(&mut self) -> Vec<RawPressObservation> {
        std::mem::take(&mut self.raw_press_observations)
    }

    pub fn destroy_surface(&mut self, id: SurfaceId) -> Result<(), String> {
        if let Some(mut instance) = self.surfaces.remove(&id) {
            self.slides.cancel(id.get());
            self.windows.remove(&instance.app.window);
            if instance.app.pointer_grabbed {
                unsafe {
                    release_popup_presses(self.display, self.root_window(), &mut self.press_selection, id);
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
        let root = self.root_window();
        let instance = self
            .surfaces
            .get_mut(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        if instance.app.pointer_grabbed {
            unsafe {
                release_popup_presses(display, root, &mut self.press_selection, id);
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
        // The popup is unmapped here without a slide-out: drop slide bookkeeping so the next show
        // starts a fresh slide instead of taking the "already shown" path with no animation.
        instance.shown = false;
        instance.slide_drawn = None;
        // SAFETY: display/window live; a closed popup must not keep a slide clip for its next show.
        unsafe { instance.app.refresh_shape_mask(&instance.document) };
        // Immediate close drops any Fade/Lamp overlay without playing it.
        instance.overlay = None;
        self.slides.cancel(id.get());
        Ok(())
    }

    /// Sets (or clears with `None`) the presentation used by later `show`/`hide` calls.
    pub fn set_slide(&mut self, id: SurfaceId, slide: Option<SlideConfig>) -> Result<(), String> {
        self.instance_mut(id)?.slide = slide;
        Ok(())
    }

    /// Root rect (x, y, width, height) of the dock panel, if one is mapped.
    fn dock_rect(&self) -> Option<(i32, i32, u32, u32)> {
        self.surfaces
            .values()
            .find(|instance| instance.role == SurfaceRole::Dock)
            .map(|instance| (instance.origin.0, instance.origin.1, instance.app.width, instance.app.height))
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
        if !shown {
            // Before any paint or map: the root under the unmapped popup is still clean.
            if let Err(error) = self.install_popup_backdrop(id) {
                eprintln!("flamewm-render-x11: popup backdrop skipped (nonfatal): {error}");
            }
        }
        if let Some(config) = slide.filter(|config| config.transition != PopupTransition::Slide) {
            if self.begin_overlay_show(id, config, shown, now)? {
                return Ok(());
            }
        }
        let mut emerging: Option<SlideEdge> = None;
        if let Some(config) = slide.filter(|config| config.transition == PopupTransition::Slide) {
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
                emerging = Some(config.edge);
            }
            // A refused start (timeline full or disabled) maps at the final origin with no slide.
        }
        let dock = self.dock_rect();
        let instance = self.instance_mut(id)?;
        let mut cycle = PresentCycle::new();
        cycle.advance("project")?;
        cycle.advance("measure")?;
        cycle.advance("anchor")?;
        cycle.advance("prepare")?;
        unsafe {
            instance
                .app
                .redraw_if_dirty(&mut instance.document)
                .map_err(|error| {
                    mark_x_error_seen();
                    error
                })?;
        }
        cycle.advance("paint")?;
        // SAFETY: display/window live; shape follows retained repaint. A slide clips to the
        // panel side before the window maps, so its first visible frame is already clipped.
        match emerging {
            Some(edge) => unsafe {
                let start = slide_origin(origin, size, edge, 0.0);
                instance
                    .app
                    .refresh_shape_mask_clipped(&instance.document, panel_clip(dock, start, size, edge));
            },
            None => unsafe { instance.app.refresh_shape_mask(&instance.document) },
        }
        cycle.advance("shape")?;
        cycle.advance("commit")?;
        unsafe {
            XMapWindow(display, window);
            // Slides stack above application windows; the panel clip keeps the emerging part hidden.
            if emerging.is_some() || surface_role_needs_popup_raise(role) {
                XRaiseWindow(display, window);
            }
        }
        cycle.advance("map_raise")?;
        let instance = self.instance_mut(id)?;
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

    /// Releases this popup's pointer grab and restores focus. Idempotent.
    fn release_grab(&mut self, id: SurfaceId) -> Result<(), String> {
        let display = self.display;
        let root = self.root_window();
        let instance = self
            .surfaces
            .get_mut(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        if instance.app.pointer_grabbed {
            unsafe {
                release_popup_presses(display, root, &mut self.press_selection, id);
                popup_restore_focus(display, &mut instance.app);
            }
            instance.app.pointer_grabbed = false;
        }
        Ok(())
    }

    pub fn hide(&mut self, id: SurfaceId) -> Result<(), String> {
        let display = self.display;
        let now = Instant::now();
        let key = id.get();
        let (slide, shown) = {
            let instance = self
                .surfaces
                .get_mut(&id)
                .ok_or_else(|| format!("unknown surface {}", id.0))?;
            (instance.slide, instance.shown)
        };
        // A slide-out keeps the grab so an outside press during the close is still captured;
        // advance_motion releases it when the slide lands.
        if !slide_out_keeps_grab(slide, shown) {
            self.release_grab(id)?;
        }
        if let Some(config) = slide.filter(|config| config.transition != PopupTransition::Slide) {
            if self.begin_overlay_hide(id, config, shown, now)? {
                unsafe { XFlush(display) };
                return Ok(());
            }
        }
        let slide = slide.filter(|config| config.transition == PopupTransition::Slide);
        if let (Some(config), true) = (slide, shown) {
            // Already closing: keep the running transition.
            if self.slides.kind(key).is_some_and(|active| !active.opening) {
                unsafe { XFlush(display) };
                return Ok(());
            }
            self.slides.set_speed(config.speed);
            let closing = SlideKind { edge: config.edge, opening: false };
            let window = self.instance_mut(id)?.app.window;
            // Slide-out starts above application windows; the panel clip hides the sinking part.
            unsafe { XRaiseWindow(display, window) };
            // A new close starts fully shown; a reversed open continues from its progress.
            if self.slides.start_towards(key, closing, now, SLIDE_CLOSE_BASE_MS, Easing::EaseOutCubic, (1.0, 0.0))
                == Start::Started
            {
                unsafe { XFlush(display) };
                return Ok(());
            }
        }
        // No slide, or the timeline refused the transition: unmap immediately.
        self.release_grab(id)?;
        self.slides.cancel(key);
        let instance = self.instance_mut(id)?;
        unsafe {
            XUnmapWindow(display, instance.app.window);
            XFlush(display);
        }
        instance.shown = false;
        instance.slide_drawn = None;
        // The snapshot is recaptured on the next show, so a closed popup keeps no stale pixels.
        instance.app.clear_surface_backdrop();
        Ok(())
    }

    /// Paints pending popup content into its backbuffer and captures it without mapping the window.
    fn capture_popup(&mut self, id: SurfaceId) -> Result<CapturedImage, String> {
        let display = self.display;
        let instance = self.instance_mut(id)?;
        let (width, height) = (instance.app.width, instance.app.height);
        // SAFETY: the app owns its backbuffer and document; both are live for this call.
        unsafe {
            let painted = instance
                .app
                .redraw_if_dirty(&mut instance.document)
                .map_err(|error| {
                    mark_x_error_seen();
                    error
                })?;
            if painted {
                instance.app.refresh_shape_mask(&instance.document);
            }
        }
        let backdrop = crate::backdrop::capture_drawable_rect(
            display,
            instance.app.backbuffer as u64,
            (0, 0, width, height),
        )?;
        crate::xcomposite::rgba_to_argb_owned(backdrop.width, backdrop.height, backdrop.pixels)
    }

    /// Opens an overlay transition for `config` over the popup's final rect. `Ok(false)` leaves
    /// `show` on the immediate path (already shown, or the overlay could not start).
    fn begin_overlay_show(
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
                    SlideKind { edge: config.edge, opening: true },
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
            SlideKind { edge: config.edge, opening: true },
            now,
            SLIDE_OPEN_BASE_MS,
            Easing::EaseOutCubic,
            (0.0, 1.0),
        ) != Start::Started
        {
            return Ok(false);
        }
        let mut overlay = overlay;
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
    fn begin_overlay_hide(
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
                    SlideKind { edge: config.edge, opening: false },
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
            SlideKind { edge: config.edge, opening: false },
            now,
            SLIDE_CLOSE_BASE_MS,
            Easing::EaseOutCubic,
            (1.0, 0.0),
        ) != Start::Started
        {
            return Ok(false);
        }
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
    fn popup_root_rect(&mut self, id: SurfaceId) -> Result<WarpRect, String> {
        let instance = self.instance_mut(id)?;
        Ok(WarpRect {
            x: f64::from(instance.origin.0),
            y: f64::from(instance.origin.1),
            width: f64::from(instance.app.width),
            height: f64::from(instance.app.height),
        })
    }

    /// Moves every active slide to `now`: one native move per changed pixel, the exact anchored
    /// origin on the last frame, and unmap when a slide-out completes. Content is never
    /// repainted here.
    fn advance_motion(&mut self, now: Instant) {
        let display = self.display;
        let root = self.root_window();
        let dock = self.dock_rect();
        for (id, instance) in self.surfaces.iter_mut() {
            let key = id.get();
            let (Some(active), Some(progress)) = (self.slides.kind(key), self.slides.progress(key, now)) else {
                continue;
            };
            let finished = self.slides.finished(key, now) == Some(true);
            if let Some((transition, overlay)) = instance.overlay.as_mut() {
                let landed = finished
                    || overlay.paint(overlay_paint_progress(*transition, progress)).is_err();
                if !landed {
                    continue;
                }
                self.slides.finish(key);
                if active.opening {
                    // Map and present the real popup, then drop the overlay: same turn, no gap.
                    // SAFETY: the window is this controller's live child of the root.
                    unsafe {
                        XMapWindow(display, instance.app.window);
                        XRaiseWindow(display, instance.app.window);
                    }
                    let _ = unsafe { instance.app.present_scene() };
                    instance.shown = true;
                    if std::mem::take(&mut instance.grab_pending) {
                        // SAFETY: the window was just mapped; display/root are live.
                        unsafe { grab_popup_presses(display, root, &mut self.press_selection, *id) };
                        instance.app.pointer_grabbed = true;
                        unsafe { popup_take_focus(display, &mut instance.app) };
                    }
                } else {
                    instance.grab_pending = false;
                }
                instance.overlay = None;
                instance.slide_drawn = None;
                continue;
            }
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
                // Keep the popup on the screen side of the panel edge while it moves.
                let clip = panel_clip(dock, (x, y), size, active.edge);
                unsafe { instance.app.refresh_shape_mask_clipped(&instance.document, clip) };
            }
            if !finished {
                continue;
            }
            // Slide landed: the base rounded mask is exact again.
            unsafe { instance.app.refresh_shape_mask(&instance.document) };
            self.slides.finish(key);
            if active.opening {
                // Back on top: the popup sits against the panel, so the raise is invisible.
                unsafe { XRaiseWindow(display, window) };
            } else {
                // Slide-out landed: release the grab held since hide() started the close.
                if instance.app.pointer_grabbed {
                    unsafe {
                        release_popup_presses(display, root, &mut self.press_selection, *id);
                        popup_restore_focus(display, &mut instance.app);
                    }
                    instance.app.pointer_grabbed = false;
                }
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
                    instance.dock_edge,
                    ox,
                    oy,
                    w,
                    h,
                );
                apply_dock_mask(instance, w, h, dock_mask_plan(0, instance.dock_radius));
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
                instance.dock_edge,
                x,
                y,
                w,
                h,
            );
            apply_dock_mask(instance, w, h, dock_mask_plan(0, instance.dock_radius));
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
        let root = self.root_window();
        let instance = self
            .surfaces
            .get_mut(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        if instance.overlay.is_some() && !instance.shown {
            // The real window maps when the opening flight lands; grab then.
            instance.grab_pending = true;
            return Ok(());
        }
        // SAFETY: live connection; raw press selection only (see grab_popup_presses).
        unsafe { grab_popup_presses(display, root, &mut self.press_selection, id) };
        instance.app.pointer_grabbed = true;
        unsafe {
            popup_take_focus(display, &mut instance.app);
            XFlush(display);
        }
        Ok(())
    }

    pub fn ungrab_pointer(&mut self, id: SurfaceId) -> Result<(), String> {
        let display = self.display;
        let root = self.root_window();
        let instance = self
            .surfaces
            .get_mut(&id)
            .ok_or_else(|| format!("unknown surface {}", id.0))?;
        unsafe {
            release_popup_presses(display, root, &mut self.press_selection, id);
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

    /// Sets (or clears with `None`) the `(opacity, blur)` percents of the backdrop captured for
    /// the popup's next first show. Opaque popups (`opacity >= 100`) never capture.
    pub fn set_popup_backdrop(&mut self, id: SurfaceId, percents: Option<(u8, u8)>) -> Result<(), String> {
        self.instance_mut(id)?.popup_backdrop = percents;
        Ok(())
    }

    /// Captures the root under an unmapped popup, blurs it, and installs it beneath the translucent
    /// fills. A blank or uniform capture (root not painted yet) installs nothing: solid colour.
    fn install_popup_backdrop(&mut self, id: SurfaceId) -> Result<(), String> {
        let Some((opacity, blur)) = self.instance_mut(id)?.popup_backdrop else {
            return Ok(());
        };
        let snapshot = if opacity < 100 {
            self.capture_root_under(id)
                .ok()
                .filter(|capture| crate::backdrop::capture_is_ready(&capture.pixels))
        } else {
            None
        };
        let instance = self.instance_mut(id)?;
        let Some(capture) = snapshot else {
            instance.app.clear_surface_backdrop();
            return Ok(());
        };
        let (width, height) = (capture.width, capture.height);
        let mut words =
            crate::backdrop::blurred_argb_words(width, height, capture.pixels, opacity, blur);
        let radius = popup_backdrop_radius_px(&instance.document, width, height);
        round_backdrop_words(&mut words, width, height, radius);
        // SAFETY: the popup's display and backbuffer are live and the window is still unmapped.
        let installed = unsafe { instance.app.set_surface_backdrop(&words, width, height) };
        if installed.is_err() {
            instance.app.clear_surface_backdrop();
        }
        installed?;
        self.redraw(id)
    }

    /// Root pixels under the surface's current device rect, as straight RGBA8. Feeds the
    /// pseudo-transparency snapshot; the caller owns blur and refresh policy.
    pub fn capture_root_under(&self, id: SurfaceId) -> Result<crate::backdrop::BackdropCapture, String> {
        let rect = self.surface_device_rect(id)?;
        let root = unsafe { XRootWindow(self.display, self.screen) };
        crate::backdrop::capture_drawable_rect(
            self.display,
            root as u64,
            (rect.x as i32, rect.y as i32, rect.width as u32, rect.height as u32),
        )
    }

    /// Subscribes to root child map/unmap/configure/destroy (SubstructureNotify is shared, so the
    /// window manager's redirect is unaffected). Idempotent.
    pub fn watch_root_changes(&self) {
        // SAFETY: the display is live; the root window is the screen's root.
        unsafe {
            XSelectInput(self.display, XRootWindow(self.display, self.screen), ROOT_SUBSTRUCTURE_NOTIFY_MASK);
        }
    }

    /// Whether root children changed since the last call, then clears the flag.
    pub fn take_root_changed(&mut self) -> bool {
        std::mem::take(&mut self.root_changed)
    }

    /// Root pixels under a mapped surface, read with the surface unmapped so its own translucent
    /// fills never enter the snapshot. A surface that was mapped is mapped again before returning.
    pub fn capture_root_under_unmapped(&mut self, id: SurfaceId) -> Result<crate::backdrop::BackdropCapture, String> {
        let display = self.display;
        let instance = self.instance_mut(id)?;
        let (window, was_shown) = (instance.app.window, instance.shown);
        if was_shown {
            // SAFETY: display and window are live; the pending unmap is flushed before the read.
            unsafe {
                XUnmapWindow(display, window);
                XSync(display, 0);
            }
        }
        let captured = self.capture_root_under(id);
        if was_shown {
            // SAFETY: display and window are live; the window repaints on the Expose that follows.
            unsafe {
                XMapWindow(display, window);
                XFlush(display);
            }
        }
        captured
    }

    /// Installs a static ARGB snapshot (row-major 0xAARRGGBB) under the surface's translucent
    /// fills, then repaints. `None` removes it.
    pub fn set_surface_backdrop(
        &mut self,
        id: SurfaceId,
        backdrop: Option<(&[u32], u32, u32)>,
    ) -> Result<(), String> {
        {
            let instance = self.instance_mut(id)?;
            match backdrop {
                Some((argb, width, height)) => unsafe {
                    instance.app.set_surface_backdrop(argb, width, height)?
                },
                None => instance.app.clear_surface_backdrop(),
            }
        }
        self.redraw(id)
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
            let root = unsafe { XRootWindow(self.display, self.screen) };
            if unsafe { event.type_ } == GENERIC_EVENT
                && Some(unsafe { generic_cookie(&event) }.extension) == self.press_selection.opcode()
            {
                // Raw press from the popup selection: observe only, never grabbed or replayed.
                self.dismiss_on_raw_press(&event, &mut on_action)?;
                handled += 1;
                continue;
            }
            if unsafe { event.xany.window } == root {
                // Other root children coming and going change what lies under a pseudo-transparency
                // bar. Our own surfaces are excluded: a capture unmaps and remaps them itself.
                let kind = unsafe { event.type_ };
                let child = unsafe { event.xmap.window };
                self.root_changed |= matches!(kind, ROOT_CHILD_MAP_NOTIFY | ROOT_CHILD_UNMAP_NOTIFY | ROOT_CHILD_CONFIGURE_NOTIFY | ROOT_CHILD_DESTROY_NOTIFY)
                    && !self.windows.contains_key(&child);
                continue;
            }
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
            // Outside presses arrive as raw root presses (dismiss_on_raw_press).
            // A release that ends outside the popup (e.g. a slider drag) never dismisses.
            unsafe {
                XPutBackEvent(self.display, &event as *const XEvent as *mut XEvent);
            }
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
    fn grabbed_press_classifies_inside_and_outside_in_root_space() {
        let origin = (100, 50);
        // 200x100 popup: [100,300) x [50,150)
        assert_eq!(classify_grabbed_press(100, 50, origin, 200, 100), GrabbedPressOutcome::Inside);
        assert_eq!(classify_grabbed_press(299, 149, origin, 200, 100), GrabbedPressOutcome::Inside);
        assert_eq!(classify_grabbed_press(300, 80, origin, 200, 100), GrabbedPressOutcome::Outside);
        assert_eq!(classify_grabbed_press(99, 80, origin, 200, 100), GrabbedPressOutcome::Outside);
        assert_eq!(classify_grabbed_press(150, 150, origin, 200, 100), GrabbedPressOutcome::Outside);
    }

    #[test]
    fn popup_press_policy_never_grabs_the_pointer_or_buttons() {
        // Production code must not grab or replay presses; dismissal observes raw presses only.
        let source = include_str!("surface_controller.rs");
        let production = source.split("#[cfg(test)]").next().unwrap_or(source);
        assert!(!production.contains("XGrabPointer("), "active pointer grab reintroduced");
        assert!(!production.contains("XGrabButton("), "passive button grab reintroduced");
        assert!(!production.contains("XAllowEvents("), "press replay reintroduced");
        assert!(production.contains("XISelectEvents("), "raw press selection missing");
    }

    #[test]
    fn raw_press_mask_selects_only_raw_button_press() {
        // XI_RawButtonPress is event 15: byte 15 >> 3 = 1, bit 15 & 7 = 7.
        assert_eq!(raw_press_mask(), [0, 0x80]);
        assert_eq!(XI_RAW_BUTTON_PRESS, 15);
    }

    #[test]
    fn raw_press_selection_arms_and_disarms_idempotently() {
        let a = SurfaceId(1);
        let mut selection = RawPressSelection::new(Some(2));
        assert!(!selection.disarm(a), "disarm while idle must not write");
        assert!(selection.arm(a), "first arm writes the selection");
        assert!(!selection.arm(a), "second arm of the same surface must not write again");
        assert!(selection.disarm(a), "disarm of the last armed surface writes");
        assert!(!selection.disarm(a), "second disarm must not write again");
        assert!(selection.arm(a), "re-arm after disarm writes again");
    }

    #[test]
    fn raw_press_selection_is_shared_across_two_armed_surfaces() {
        let (a, b) = (SurfaceId(1), SurfaceId(2));
        let mut selection = RawPressSelection::new(Some(2));
        assert!(selection.arm(a), "first surface writes the selection");
        assert!(!selection.arm(b), "second surface joins without rewriting");
        assert!(!selection.disarm(a), "closing one popup must keep the selection for the other");
        assert!(!selection.arm(b), "b stays armed and idempotent");
        assert!(selection.disarm(b), "last surface out clears the selection");
        assert!(!selection.disarm(a), "a already disarmed");
        assert!(selection.arm(a), "re-arm after full clear writes again");
    }

    #[test]
    fn press_observer_shares_the_root_selection_with_popup_grabs() {
        let mut selection = RawPressSelection::new(Some(2));
        let popup = SurfaceId(7);
        assert!(selection.arm(PRESS_OBSERVER), "observer writes the root selection");
        assert!(!selection.arm(popup), "popup rides the observer's selection");
        assert!(selection.arm(PRESS_OBSERVER) == false, "re-arm is idempotent");
        assert!(!selection.disarm(PRESS_OBSERVER), "popup keeps the selection");
        assert!(selection.disarm(popup), "last disarm clears the root selection");
        assert!(!selection.disarm(PRESS_OBSERVER), "disarm is idempotent");
    }

    #[test]
    fn raw_press_selection_without_xinput2_never_writes() {
        let mut selection = RawPressSelection::new(None);
        assert_eq!(selection.opcode(), None);
        assert!(!selection.arm(SurfaceId(1)));
        assert!(!selection.disarm(SurfaceId(1)));
    }

    #[test]
    fn raw_press_outside_maps_only_outside_presses_to_dismiss_event() {
        let popup = ((100, 50), (200, 100));
        let outside = raw_press_outside_event(SurfaceId(3), XI_RAW_BUTTON_PRESS, 1, (300, 80), popup, 1.0, 42)
            .expect("press right of the popup dismisses");
        assert_eq!(outside.surface, SurfaceId(3));
        assert_eq!(outside.event.action, OUTSIDE_RELEASE_ACTION);
        assert_eq!(outside.event.phase, ActionPhase::Release);
        assert_eq!((outside.event.x, outside.event.y), (200.0, 30.0));
        assert_eq!(outside.event.button, 1);
        assert_eq!(outside.event.time_ms, 42);
        assert!(!outside.event.inside);

        let scaled = raw_press_outside_event(SurfaceId(3), XI_RAW_BUTTON_PRESS, 3, (300, 80), popup, 2.0, 7)
            .expect("scaled outside press dismisses");
        assert_eq!((scaled.event.x, scaled.event.y), (100.0, 15.0));
        assert_eq!(scaled.event.button, 3);

        assert!(raw_press_outside_event(SurfaceId(3), XI_RAW_BUTTON_PRESS, 1, (150, 80), popup, 1.0, 0).is_none());
        assert!(raw_press_outside_event(SurfaceId(3), XI_RAW_BUTTON_PRESS, 1, (299, 149), popup, 1.0, 0).is_none());
        assert!(raw_press_outside_event(SurfaceId(3), 16, 1, (300, 80), popup, 1.0, 0).is_none(), "release is not a press");
    }

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

    #[test]
    fn popup_backdrop_corners_are_transparent_and_centre_untouched() {
        let mut words = vec![0xff80_8080_u32; 12 * 12];
        round_backdrop_words(&mut words, 12, 12, 6);
        assert_eq!(words[0] >> 24, 0, "top-left corner outside the arc");
        assert_eq!(words[11] >> 24, 0, "top-right corner outside the arc");
        assert_eq!(words[11 * 12] >> 24, 0, "bottom-left corner outside the arc");
        assert_eq!(words[11 * 12 + 11] >> 24, 0, "bottom-right corner outside the arc");
        assert_eq!(words[6 * 12 + 6], 0xff80_8080, "centre keeps the snapshot");
        assert!(
            words.iter().any(|w| (1..0xff).contains(&(w >> 24))),
            "arc has anti-aliased partial alpha"
        );
        for word in &words {
            let alpha = word >> 24;
            for shift in [0, 8, 16] {
                assert!(((word >> shift) & 0xff) <= alpha, "premultiplied: {word:#010x}");
            }
        }
    }

    #[test]
    fn slide_out_keeps_grab_only_for_shown_slide_popups() {
        let slide = Some(SlideConfig::slide(SlideEdge::Bottom, 200));
        let fade = Some(SlideConfig::fade(SlideEdge::Bottom, 200));
        assert!(slide_out_keeps_grab(slide, true), "shown slide keeps grab during slide-out");
        assert!(!slide_out_keeps_grab(slide, false), "unmapped popup releases at once");
        assert!(!slide_out_keeps_grab(fade, true), "fade releases at once");
        assert!(!slide_out_keeps_grab(None, true), "no transition releases at once");
    }

    #[test]
    fn outside_press_is_swallowed_only_while_sliding_out() {
        let closing = Some(SlideKind { edge: SlideEdge::Bottom, opening: false });
        let opening = Some(SlideKind { edge: SlideEdge::Bottom, opening: true });
        assert!(slide_out_swallows_press(closing), "second outside press during slide-out is ignored");
        assert!(!slide_out_swallows_press(opening), "a reversed open still dismisses");
        assert!(!slide_out_swallows_press(None), "settled popup dismisses normally");
    }

    #[test]
    fn popup_backdrop_square_corners_stay_opaque() {
        let mut words = vec![0xff10_2030_u32; 12 * 12];
        round_backdrop_words(&mut words, 12, 12, 0);
        assert!(words.iter().all(|w| *w == 0xff10_2030));
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
    fn panel_clip_keeps_only_the_screen_side_of_the_dock_edge() {
        // Bottom dock at y 1040..1080: rows above the panel top survive the clip.
        let bottom = Some((0, 1040, 1920, 40));
        assert_eq!(panel_clip(bottom, (100, 900), (300, 400), SlideEdge::Bottom), (0, 0, 300, 140));
        // Still fully below the panel: nothing may show.
        assert_eq!(panel_clip(bottom, (100, 1100), (300, 400), SlideEdge::Bottom), (0, 0, 300, 0));
        // Top dock at y 0..40: rows from the panel bottom down survive.
        let top = Some((0, 0, 1920, 40));
        assert_eq!(panel_clip(top, (100, -200), (300, 400), SlideEdge::Top), (0, 240, 300, 400));
        // Left and right docks clip columns on the same rule.
        let left = Some((0, 0, 60, 1080));
        assert_eq!(panel_clip(left, (-100, 0), (300, 400), SlideEdge::Left), (160, 0, 300, 400));
        let right = Some((1860, 0, 60, 1080));
        assert_eq!(panel_clip(right, (1700, 0), (300, 400), SlideEdge::Right), (0, 0, 160, 400));
        // No dock: nothing is clipped.
        assert_eq!(panel_clip(None, (1, 2), (300, 400), SlideEdge::Bottom), (0, 0, 300, 400));
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

    #[test]
    fn constructors_pick_each_transition() {
        assert_eq!(SlideConfig::slide(SlideEdge::Top, 100).transition, PopupTransition::Slide);
        assert_eq!(SlideConfig::fade(SlideEdge::Top, 100).transition, PopupTransition::Fade);
        let lamp = SlideConfig::lamp(SlideEdge::Bottom, 100, (10, 900, 48, 40));
        assert_eq!(lamp.transition, PopupTransition::Lamp { target: (10, 900, 48, 40) });
        assert_eq!(lamp.edge, SlideEdge::Bottom);
    }

    #[test]
    fn fade_paint_follows_timeline_in_both_directions() {
        // Closing starts from the fully shown timeline value, so alpha starts at 1 and ends at 0.
        assert_eq!(overlay_paint_progress(PopupTransition::Fade, 0.0), 0.0);
        assert_eq!(overlay_paint_progress(PopupTransition::Fade, 1.0), 1.0);
        assert_eq!(overlay_style(PopupTransition::Fade), TransitionStyle::Fade);
    }

    #[test]
    fn lamp_paint_runs_target_to_source_on_open_and_back_on_close() {
        let lamp = PopupTransition::Lamp { target: (0, 0, 1, 1) };
        // Open: starts folded into the button (warp progress 1) and unfolds to the window (0).
        assert_eq!(overlay_paint_progress(lamp, 0.0), 1.0);
        assert_eq!(overlay_paint_progress(lamp, 1.0), 0.0);
        assert_eq!(overlay_style(lamp), TransitionStyle::MagicLamp);
    }

    #[test]
    fn lamp_target_is_the_start_button_rect_and_fade_keeps_popup_rect() {
        let popup = WarpRect { x: 100.0, y: 500.0, width: 300.0, height: 400.0 };
        assert_eq!(
            overlay_target(PopupTransition::Lamp { target: (12, 1050, 48, 36) }, popup),
            WarpRect { x: 12.0, y: 1050.0, width: 48.0, height: 36.0 }
        );
        assert_eq!(overlay_target(PopupTransition::Fade, popup), popup);
    }

    #[test]
    fn panel_edge_maps_one_to_one_for_the_lamp_bend() {
        assert_eq!(lamp_edge(SlideEdge::Top), crate::lamp::PanelEdge::Top);
        assert_eq!(lamp_edge(SlideEdge::Bottom), crate::lamp::PanelEdge::Bottom);
        assert_eq!(lamp_edge(SlideEdge::Left), crate::lamp::PanelEdge::Left);
        assert_eq!(lamp_edge(SlideEdge::Right), crate::lamp::PanelEdge::Right);
    }

    #[test]
    fn reversed_fade_keeps_its_timeline_value() {
        // A fade-out reversed mid-flight continues from the same timeline value, so the overlay
        // paint argument does not jump.
        let t0 = Instant::now();
        let mut slides = Timeline::new();
        slides.start_towards(1, CLOSE, t0, SLIDE_CLOSE_BASE_MS, Easing::EaseOutCubic, (1.0, 0.0));
        let now = t0 + Duration::from_millis(40);
        let before = overlay_paint_progress(PopupTransition::Fade, slides.progress(1, now).unwrap());
        slides.start_towards(1, OPEN, now, SLIDE_OPEN_BASE_MS, Easing::EaseOutCubic, (0.0, 1.0));
        let after = overlay_paint_progress(PopupTransition::Fade, slides.progress(1, now).unwrap());
        assert!((before - after).abs() < 1e-6, "no jump on reverse");
    }
}

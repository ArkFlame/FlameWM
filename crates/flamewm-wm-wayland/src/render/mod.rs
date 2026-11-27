//! Frame composition: layer surfaces, windows with server-side titlebars, popups, frame callbacks.

use std::collections::HashMap;
use std::error::Error;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::colour_filter::{ColourFilter, effective_matrix};
use flamewm_api::settings::ZOOM_LEVEL_MIN;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::{Element, Id, Kind};
use smithay::backend::renderer::gles::{GlesTexProgram, GlesTexture};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Bind, ExportMem, Offscreen};
use smithay::backend::winit::WinitGraphicsBackend;
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::desktop::{PopupManager, layer_map_for_output};
use smithay::render_elements;
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Size, Transform};
use smithay::wayland::shell::wlr_layer::Layer;

mod backdrop;
mod dock_backdrop;
pub(crate) use dock_backdrop::backdrop_captures;
mod switcher_thumbs;
mod titlebar;
pub use switcher_thumbs::serve_switcher_thumbs;
pub(crate) use titlebar::{premultiplied_argb_bytes, put};

pub(crate) use backdrop::*;
pub(crate) use dock_backdrop::*;
pub use titlebar::*;

use crate::chrome_buttons::Feedback;
use crate::clip::RoundedSurfaceElement;
use crate::model::{Button, Hit, Managed, Mode, Rect, TITLEBAR_HEIGHT, WindowId};
use crate::session_lock::Screen;
use crate::state::State;
use flamewm_image_core::{Rect as ImageRect, RgbaImage, frosted_backdrop};
use flamewm_render_core::outline;
use flamewm_skin::recipes::decoration::DecorationStyle;
use flamewm_skin::recipes::window_chrome::{self as chrome, WindowControlRole};

render_elements! {
    pub Elem<=GlowRenderer>;
    Solid=SolidColorRenderElement,
    Surface=WaylandSurfaceRenderElement<GlowRenderer>,
    Rounded=RoundedSurfaceElement,
    Memory=MemoryRenderBufferRenderElement<GlowRenderer>,
}

pub(crate) const CLEAR: [f32; 4] = [0.07, 0.08, 0.11, 1.0];
/// Opaque colour of the compositor's stand-in lock cover. An output without a client surface
/// clears to this tone, so no window behind the lock can show through.
const LOCK_COVER: [f32; 4] = CLEAR;

/// One opaque full-area element. Used for outputs the lock client did not give a surface for.
fn solid_lock_cover(rect: Rectangle<i32, Physical>) -> Elem {
    Elem::Solid(SolidColorRenderElement::new(
        Id::new(),
        rect,
        CommitCounter::default(),
        LOCK_COVER,
        Kind::Unspecified,
    ))
}

/// Physical rectangle an opaque cover fills on one output. Falls back to the compositor's output
/// size until the output advertises a mode. Pure so the every-output rule is unit-tested.
fn lock_placeholder_rect(
    mode_size: Option<Size<i32, Physical>>,
    fallback: Size<u32, Logical>,
    scale: f64,
) -> Rectangle<i32, Physical> {
    let size: Size<i32, Physical> = mode_size.unwrap_or_else(|| {
        Size::from((
            (f64::from(fallback.w) * scale).round() as i32,
            (f64::from(fallback.h) * scale).round() as i32,
        ))
    });
    Rectangle::from_size(size)
}

/// The cover an output shows when no live lock surface is bound to it.
fn lock_cover_placeholder(state: &State, output: &smithay::output::Output) -> Elem {
    let scale = output.current_scale().fractional_scale();
    solid_lock_cover(lock_placeholder_rect(
        output.current_mode().map(|mode| mode.size),
        state.lock_size(),
        scale,
    ))
}

// Shared Xephyr skin metrics and palette, never per-backend coloured-square placeholders.
/// Titlebar fill from `surface::titlebar_fill`, packed as `alpha << 24 | rgb`. The default is the
/// flat opaque solid (`reduceTransparency` defaults on), so nothing paints translucent before settings arrive.
static TITLEBAR_FILL: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0xff00_0000 | flamewm_ui_core::glass::STANDARD_RGB);
/// Desktop blur percent from `surface::surface_blur_percent`; 0 when flat.
static DESKTOP_BLUR_PERCENT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// `reduceTransparency` as last committed. When set, no desktop blur is captured and no surface
/// takes a backdrop: every material is opaque, so the blur has nothing to show.
static GLASS_FLAT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
/// Blur radius at 100% surface blur. Same value as the X11 `MAX_BLUR_RADIUS_PX` (render-x11).
const MAX_BLUR_RADIUS_PX: f32 = 24.0;

/// `appearance=light` from the committed settings snapshot, written by `sync_chrome_settings`.
/// Read by the titlebar paint path so the material and its ink follow one appearance.
static TITLEBAR_LIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Records the snapshot's appearance. Returns whether it changed, so the caller drops cached bars.
pub(crate) fn set_titlebar_light(light: bool) -> bool {
    TITLEBAR_LIGHT.swap(light, std::sync::atomic::Ordering::Relaxed) != light
}

/// `titlebarButtonStyle` is square, from the committed settings snapshot (`sync_chrome_settings`).
/// Unset or any other value is the circle default, as on X11.
static TITLEBAR_SQUARE_BUTTONS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Records the button style. Returns whether it changed, so the caller drops cached bars.
pub(crate) fn set_titlebar_button_shape(
    shape: flamewm_skin::recipes::decoration::ButtonShape,
) -> bool {
    let square = shape == flamewm_skin::recipes::decoration::ButtonShape::Square;
    TITLEBAR_SQUARE_BUTTONS.swap(square, std::sync::atomic::Ordering::Relaxed) != square
}

/// Button shape the titlebar paint path draws, as last committed.
pub(crate) fn titlebar_button_shape() -> flamewm_skin::recipes::decoration::ButtonShape {
    if TITLEBAR_SQUARE_BUTTONS.load(std::sync::atomic::Ordering::Relaxed) {
        flamewm_skin::recipes::decoration::ButtonShape::Square
    } else {
        flamewm_skin::recipes::decoration::ButtonShape::Circle
    }
}

/// Titlebar material for one appearance: the shipped Dark glass, or the Light panel token.
pub(crate) const fn titlebar_material(light: bool) -> u32 {
    if light {
        flamewm_skin::palette::LIGHT_PANEL.0
    } else {
        flamewm_skin::recipes::decoration::DEFAULT_SURFACE_RGB.0
    }
}

/// Title and control ink for one appearance's titlebar material, picked by contrast like the X11
/// frame (`ChromeTokens::ink`).
pub(crate) fn ink_for_material(light: bool) -> flamewm_skin::palette::Palette {
    flamewm_skin::palette::ink_palette_on(flamewm_skin::palette::Rgb(titlebar_material(light)))
}

/// Material the bar paints right now.
pub(crate) fn current_titlebar_material() -> u32 {
    titlebar_fill().0
}

/// Records the shared glass fill and flat flag. Returns whether the titlebar fill changed.
pub(crate) fn set_glass_fill(rgb: u32, alpha: u8, blur_percent: u8, flat: bool) -> bool {
    let packed = (u32::from(alpha) << 24) | (rgb & 0x00ff_ffff);
    let changed = TITLEBAR_FILL.swap(packed, std::sync::atomic::Ordering::Relaxed) != packed;
    DESKTOP_BLUR_PERCENT.store(blur_percent, std::sync::atomic::Ordering::Relaxed);
    GLASS_FLAT.store(flat, std::sync::atomic::Ordering::Relaxed);
    changed
}

/// Titlebar `(rgb, alpha)` as committed by `set_glass_fill`.
pub(crate) fn titlebar_fill() -> (u32, u8) {
    let packed = TITLEBAR_FILL.load(std::sync::atomic::Ordering::Relaxed);
    (packed & 0x00ff_ffff, (packed >> 24) as u8)
}

/// Whether `reduceTransparency` (the flat default) is in effect.
pub(crate) fn glass_flat() -> bool {
    GLASS_FLAT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Title and control ink for the titlebar material this backend paints now.
pub(crate) fn titlebar_ink() -> flamewm_skin::palette::Palette {
    ink_for_material(TITLEBAR_LIGHT.load(std::sync::atomic::Ordering::Relaxed))
}

const BORDER: u32 = flamewm_skin::recipes::window_chrome::WINDOW_CHROME.border.0;

/// Keep one premultiplied ARGB bitmap per managed window. No raster work or allocations on
/// steady-state frames: rebuilding happens only on resize/focus/hover/title changes.
/// Physical pixel of a logical coordinate at `scale`; identity at 1.0.
pub(crate) fn physical_point(x: i32, y: i32, scale: f64) -> Point<i32, Physical> {
    (
        (f64::from(x) * scale).round() as i32,
        (f64::from(y) * scale).round() as i32,
    )
        .into()
}

#[cfg(test)]
mod physical_point_guard {
    use super::physical_point;

    #[test]
    fn outline_offsets_round_like_surfaces_at_fractional_scale() {
        // 10 logical * 1.25 = 12.5 must round to 13 (surface rule), not stay fractional.
        let p = physical_point(0, 10, 1.25);
        assert_eq!((p.x, p.y), (0, 13));
        assert_eq!(physical_point(-30, -50, 1.5).x, -45);
    }
}

#[cfg(test)]
mod lock_cover_placeholder_tests {
    use super::*;

    #[test]
    fn a_placeholder_fills_the_reported_output_mode() {
        let mode = Size::<i32, Physical>::from((2560, 1440));
        let rect = lock_placeholder_rect(Some(mode), Size::from((1280u32, 720u32)), 1.25);
        assert_eq!(rect.size, mode);
        assert_eq!(rect.loc, Point::from((0, 0)));
    }

    #[test]
    fn a_placeholder_falls_back_to_the_scaled_output_size() {
        let rect = lock_placeholder_rect(None, Size::from((1280u32, 720u32)), 1.5);
        assert_eq!(rect.size, Size::<i32, Physical>::from((1920, 1080)));
    }

    #[test]
    fn a_placeholder_is_an_opaque_solid_cover() {
        let rect = Rectangle::from_size(Size::<i32, Physical>::from((640, 480)));
        assert!(matches!(solid_lock_cover(rect), Elem::Solid(_)));
        assert_eq!(LOCK_COVER[3], 1.0);
    }
}

/// A logical corner radius in physical pixels at `scale`; identity at 1.0.
fn physical_radius(radius: u8, scale: f64) -> u8 {
    (f64::from(radius) * scale).round().min(f64::from(u8::MAX)) as u8
}

/// Physical rectangle of a decorated client's body: the logical client rect rounded like surfaces.
fn client_backdrop_rect(rect: Rect, scale: f64) -> Rectangle<i32, Physical> {
    let origin = physical_point(rect.x, rect.y, scale);
    let end = physical_point(rect.x + rect.w, rect.y + rect.h, scale);
    Rectangle::new(origin, (end.x - origin.x, end.y - origin.y).into())
}

/// Solid frame body in the window material, painted under a client that may not fill its rect.
fn client_backdrop(rect: Rect, scale: f64) -> SolidColorRenderElement {
    let rgb = current_titlebar_material() & 0x00ff_ffff;
    let channel = |shift: u32| f32::from(((rgb >> shift) & 0xff) as u8) / 255.0;
    SolidColorRenderElement::new(
        Id::new(),
        client_backdrop_rect(rect, scale),
        CommitCounter::default(),
        [channel(16), channel(8), channel(0), 1.0],
        Kind::Unspecified,
    )
}
fn surface_elements(
    renderer: &mut GlowRenderer,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    x: i32,
    y: i32,
    scale: f64,
) -> Vec<Elem> {
    let location = physical_point(x, y, scale);
    render_elements_from_surface_tree::<_, WaylandSurfaceRenderElement<GlowRenderer>>(
        renderer,
        surface,
        location,
        scale,
        1.0,
        Kind::Unspecified,
    )
    .into_iter()
    .map(Elem::Surface)
    .collect()
}

/// Front-to-back element list: Smithay draws the first element on top. Input consumes the same
/// `scene::front_to_back` traversal, so every surface that can visually cover another is tested
/// before the covered owner receives pointer input.
/// Elements for the live lock surface covering `output`, placed at the output's origin. An output
/// without a live surface still gets an opaque placeholder, so every output is covered and none
/// falls back to the desktop while the lock is held.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub fn output_lock_elements(
    state: &State,
    output: &smithay::output::Output,
    renderer: &mut GlowRenderer,
) -> Vec<Elem> {
    match state.lock_surface_on(output) {
        Some(surface) => surface_elements(
            renderer,
            surface.wl_surface(),
            0,
            0,
            output.current_scale().fractional_scale(),
        ),
        None => vec![lock_cover_placeholder(state, output)],
    }
}

/// Appends the frame's scene elements to `out`, so a frame builder keeps one element buffer instead of
/// allocating a second Vec and copying every element into it on each painted frame.
pub fn build_elements_into(state: &State, renderer: &mut GlowRenderer, out: &mut Vec<Elem>) {
    let scale = state.output_scale();
    let mut decorations = state.decorations.borrow_mut();
    decorations.retain(|window| state.model.get(window).is_some());
    // Topmost: the snap preview is drawn over every window while a move grab targets a snap.
    out.extend(state.snap_preview.elements(renderer, scale));
    // Append straight into `out`: a per-frame intermediate `Vec` here cost one allocation plus a
    // full element copy on every painted frame in both backends.
    scene_elements_into(
        state,
        renderer,
        &mut decorations,
        crate::scene::front_to_back(state),
        false,
        out,
    );
}

/// Elements for `items` in front-to-back order. `capture` builds the scene under a Dock backdrop:
/// it produces no backdrop elements, so a translucent surface never captures another one.
/// Owned result for callers that cannot append into an existing buffer.
fn scene_elements(
    state: &State,
    renderer: &mut GlowRenderer,
    decorations: &mut Decorations,
    items: impl IntoIterator<Item = crate::scene::SceneItem>,
    capture: bool,
) -> Vec<Elem> {
    let mut out: Vec<Elem> = Vec::new();
    scene_elements_into(state, renderer, decorations, items, capture, &mut out);
    out
}

/// Whether `outer` fully contains `inner`, so every pixel of `inner` has an opaque `outer` pixel
/// in front of it.
fn covers(outer: Rectangle<i32, Physical>, inner: Rectangle<i32, Physical>) -> bool {
    inner.size.w > 0
        && inner.size.h > 0
        && outer.loc.x <= inner.loc.x
        && outer.loc.y <= inner.loc.y
        && outer.loc.x.saturating_add(outer.size.w) >= inner.loc.x.saturating_add(inner.size.w)
        && outer.loc.y.saturating_add(outer.size.h) >= inner.loc.y.saturating_add(inner.size.h)
}

/// Whether a managed window's frame can contribute a pixel to `visible`.
///
/// A window dragged fully past an output edge, or placed outside this output's rect, is clipped
/// away entirely. `scene_elements_into` skips its elements for exactly that reason; the frame
/// callback is skipped with the same predicate, because a callback is what unblocks the client's
/// next commit and would otherwise keep an invisible window rasterising at the refresh rate.
fn frame_reaches_output(frame: Rect, scale: f64, visible: Rectangle<i32, Physical>) -> bool {
    client_backdrop_rect(frame, scale).overlaps(visible)
}

/// Collects the opaque areas the just-built elements of one owner paint, so a lower window that
/// is fully covered can skip building its own surface, decoration and backdrop elements.
///
/// Memory elements (titlebar and outline strips) are never treated as occluders: they carry
/// rounded, antialiased and translucent chrome, so a buffer that claims its whole rect would
/// wrongly hide a window behind a glass bar. Solid bodies and client surfaces report their true
/// opaque regions, which is exactly what the renderer may rely on.
fn collect_occluders(elements: &[Elem], scale: f64, occluders: &mut Vec<Rectangle<i32, Physical>>) {
    /// Bounded so a frame with many windows cannot grow the list without limit; a scene past the
    /// cap simply keeps building the lowest windows, which is only a missed optimisation.
    const MAX_OCCLUDERS: usize = 64;
    for element in elements {
        if occluders.len() >= MAX_OCCLUDERS {
            return;
        }
        let regions = match element {
            Elem::Solid(solid) => solid.opaque_regions(scale.into()),
            Elem::Surface(surface) => surface.opaque_regions(scale.into()),
            Elem::Rounded(rounded) => rounded.opaque_regions(scale.into()),
            Elem::Memory(_) | Elem::_GenericCatcher(_) => continue,
        };
        // `Element::opaque_regions` is element-local, as the renderer expects: shift each region
        // by this element's placed geometry so the stored rects are in output space.
        let origin = element.geometry(scale.into()).loc;
        for mut region in regions {
            if region.size.w > 0 && region.size.h > 0 {
                region.loc += origin;
                occluders.push(region);
            }
        }
    }
}

#[cfg(test)]
mod occluder_tests {
    use super::covers;
    use smithay::utils::{Physical, Rectangle};

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Physical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn a_larger_rect_covers_a_smaller_one() {
        assert!(covers(rect(0, 0, 100, 80), rect(10, 10, 40, 40)));
    }

    #[test]
    fn an_equal_rect_covers_itself() {
        assert!(covers(rect(5, 7, 30, 20), rect(5, 7, 30, 20)));
    }

    #[test]
    fn a_partial_overlap_is_not_coverage() {
        // One pixel of the inner rect sticks out on the right and bottom.
        assert!(!covers(rect(0, 0, 101, 60), rect(10, 10, 92, 40)));
        assert!(!covers(rect(0, 0, 100, 80), rect(10, 10, 40, 80)));
    }

    #[test]
    fn a_disjoint_rect_is_not_covered() {
        assert!(!covers(rect(0, 0, 50, 50), rect(60, 60, 10, 10)));
    }

    #[test]
    fn an_empty_rect_is_never_culled() {
        assert!(!covers(rect(0, 0, 100, 100), rect(10, 10, 0, 0)));
    }
}

#[cfg(test)]
mod frame_reaches_output_tests {
    use super::{Rect, frame_reaches_output};
    use smithay::utils::{Physical, Rectangle, Size};

    fn visible() -> Rectangle<i32, Physical> {
        Rectangle::from_size(Size::from((1280, 800)))
    }

    #[test]
    fn a_frame_inside_the_output_reaches_it() {
        assert!(frame_reaches_output(
            Rect::new(100, 100, 400, 300),
            1.0,
            visible()
        ));
    }

    #[test]
    fn a_frame_dragged_past_the_right_edge_is_culled() {
        assert!(!frame_reaches_output(
            Rect::new(1280, 100, 400, 300),
            1.0,
            visible()
        ));
    }

    #[test]
    fn a_frame_straddling_the_edge_still_reaches_it() {
        assert!(frame_reaches_output(
            Rect::new(1200, 100, 400, 300),
            1.0,
            visible()
        ));
    }

    #[test]
    fn the_scale_is_applied_before_the_check() {
        // 1024 logical * 1.25 = 1280 physical: the first column past the output, so it is culled
        // even though the same frame would fit at scale 1.0.
        let frame = Rect::new(1024, 100, 256, 300);
        assert!(!frame_reaches_output(frame, 1.25, visible()));
        assert!(frame_reaches_output(frame, 1.0, visible()));
    }
}

/// [`scene_elements`] appending into `out`, so a frame builder reuses its element buffer.
fn scene_elements_into(
    state: &State,
    renderer: &mut GlowRenderer,
    decorations: &mut Decorations,
    items: impl IntoIterator<Item = crate::scene::SceneItem>,
    capture: bool,
    out: &mut Vec<Elem>,
) {
    use crate::scene::SceneItem;

    let scale = state.output_scale();
    let hovered_button = state.model.hit(state.pointer.x, state.pointer.y);
    // Physical area this output shows. A managed window whose whole frame misses it (another
    // output of the layout, or dragged past the edge) cannot contribute a pixel, so its element
    // list is skipped rather than built and clipped away by the renderer.
    let visible = client_backdrop_rect(state.output_rect(), scale);
    // Opaque areas already drawn in front of the current item. The traversal is front to back, so
    // a window whose frame fits inside one of these is fully hidden and needs no element work.
    let mut occluders: Vec<Rectangle<i32, Physical>> = Vec::new();
    for item in items {
        match item {
            SceneItem::Layer { surface, .. } => {
                out.extend(layer_elements(state, renderer, &surface));
            }
            SceneItem::Unmanaged(index) => {
                let Some(entry) = state.unmanaged.get(index) else {
                    continue;
                };
                if let Some(surface) = entry.surface.wl_surface() {
                    let geometry = entry.surface.geometry();
                    let elements =
                        surface_elements(renderer, &surface, geometry.loc.x, geometry.loc.y, scale);
                    // A Dock's published pill radius clips its corners in the shader. The X11 buffer
                    // may be XRGB (software XWayland), whose corner pixels carry no alpha and would
                    // otherwise composite as opaque black.
                    let radius = u8::try_from(entry.corner_radius).unwrap_or(u8::MAX);
                    let program = (radius > 0)
                        .then(|| decorations.clip_program(renderer))
                        .flatten();
                    out.extend(
                        elements
                            .into_iter()
                            .map(|element| match (element, &program) {
                                (Elem::Surface(inner), Some(program))
                                    if RoundedSurfaceElement::clippable(&inner) =>
                                {
                                    Elem::Rounded(RoundedSurfaceElement::new(
                                        inner,
                                        program,
                                        [radius; 4],
                                    ))
                                }
                                (element, _) => element,
                            }),
                    );
                    // Translucent X11 menus and tooltips blur the desktop beneath them. A Dock (strut)
                    // blends over the scene beneath it instead, captured by `dock_backdrop`.
                    if !entry.below && !capture && !glass_flat() {
                        let rect = Rect::new(
                            geometry.loc.x,
                            geometry.loc.y,
                            geometry.size.w,
                            geometry.size.h,
                        );
                        let backdrop = if entry.strut.is_some() {
                            decorations.dock_backdrop(
                                renderer,
                                (rect.x, rect.y, rect.w, rect.h),
                                scale,
                            )
                        } else {
                            decorations.surface_backdrop(renderer, rect, scale)
                        };
                        out.extend(backdrop);
                    }
                }
            }
            SceneItem::Popup { surface, origin } => {
                out.extend(surface_elements(
                    renderer, &surface, origin.x, origin.y, scale,
                ));
            }
            SceneItem::Managed(id) => {
                // Borrow, never clone: this runs once per visible window on every painted frame, and
                // `Managed` carries a dozen rects and modes. Nothing below mutates the window model.
                let Some(managed) = state.model.get(id) else {
                    continue;
                };
                let Some(surface) = state.window_wl_surface(id) else {
                    continue;
                };
                let frame = managed.frame();
                let frame_rect = client_backdrop_rect(frame, scale);
                if !frame_reaches_output(frame, scale, visible)
                    || occluders.iter().any(|outer| covers(*outer, frame_rect))
                {
                    continue;
                }
                let first = out.len();
                // Front-to-back list: the outline comes first in this window's group, so the ring
                // sits above the client and the titlebar and above every window behind it.
                out.extend(decorations.outline_elements(
                    renderer,
                    id,
                    frame,
                    outline_radius(managed),
                    scale,
                ));
                let window_backdrop = !capture
                    && window_needs_backdrop(
                        true,
                        false,
                        DockMaterial {
                            opacity: managed.material.0,
                            blur: managed.material.1,
                        },
                        glass_flat(),
                    );
                let radii = client_corner_radii(managed, managed.titlebars)
                    .map(|radius| physical_radius(radius, scale));
                let program = if radii.iter().any(|radius| *radius > 0) {
                    decorations.clip_program(renderer)
                } else {
                    None
                };
                // Only the toplevel's own surface carries the window silhouette: a subsurface is
                // clipped by its parent's rounded edge, not by arcs around its own smaller rect.
                let root = Id::from_wayland_resource(&surface);
                out.extend(
                    surface_elements(renderer, &surface, managed.rect.x, managed.rect.y, scale)
                        .into_iter()
                        .map(|element| match (element, &program) {
                            (Elem::Surface(inner), Some(program))
                                if inner.id() == &root
                                    && RoundedSurfaceElement::clippable(&inner) =>
                            {
                                Elem::Rounded(RoundedSurfaceElement::new(inner, program, radii))
                            }
                            (element, _) => element,
                        }),
                );
                if window_backdrop {
                    out.extend(decorations.window_backdrop(
                        renderer,
                        id,
                        (
                            managed.rect.x,
                            managed.rect.y,
                            managed.rect.w,
                            managed.rect.h,
                        ),
                        scale,
                    ));
                }
                if managed.decorated() {
                    // A cell-grid client may commit a buffer shorter than its configured size. Paint
                    // the frame body behind the client over its full extent, so no desktop shows
                    // through the gap (KWin and xfwm4 paint a decorated window's body the same way).
                    // Pushed after the surface, so it sits below the client and the titlebar.
                    if !window_backdrop {
                        out.push(Elem::Solid(client_backdrop(managed.rect, scale)));
                    }
                    let focused = state.focused == Some(id);
                    let titlebar = Rect::new(frame.x, frame.y, frame.w, TITLEBAR_HEIGHT);
                    let hovered = match hovered_button {
                        Some(Hit::Button(hit, button)) if hit == id => Some(button),
                        _ => None,
                    };
                    // Pressed only while the pointer is still on the control that was pressed.
                    let pressed = match state.pressed_hit {
                        Some(hit)
                            if state.model.hit(state.pointer.x, state.pointer.y) == Some(hit) =>
                        {
                            hovered
                        }
                        _ => None,
                    };
                    let title = state.window_title(id);
                    if let Some(element) = decorations.element(
                        renderer,
                        id,
                        titlebar,
                        (
                            focused,
                            managed.mode == Mode::Maximized,
                            managed.mode.rounded_frame(),
                        ),
                        Feedback {
                            hover: hovered,
                            pressed,
                        },
                        title,
                        scale,
                    ) {
                        out.push(element);
                    }
                }
                // Everything this window paints is now in `out`; its opaque areas hide whatever
                // lower windows are fully behind it.
                collect_occluders(&out[first..], scale, &mut occluders);
            }
        }
    }
    if state.blue_light_intensity > 0 {
        // The amber veil covers the whole output and sits last, so it warms every layer the
        // compositor draws, including docks and XWayland clients. This is the Wayland stand-in for
        // the X11 gamma ramp: Smithay exposes no DRM gamma, and an overlay is the only honest
        // cross-backend option (see `flamewm_api::blue_light::overlay_rgba`).
        let size = state.physical_output_size();
        out.push(Elem::Solid(SolidColorRenderElement::new(
            smithay::backend::renderer::element::Id::new(),
            Rectangle::new((0, 0).into(), (size.w, size.h).into()),
            smithay::backend::renderer::utils::CommitCounter::default(),
            flamewm_api::blue_light::overlay_rgba(state.blue_light_intensity),
            Kind::Unspecified,
        )));
    }
}

fn layer_elements(
    state: &State,
    renderer: &mut GlowRenderer,
    layer: &smithay::desktop::LayerSurface,
) -> Vec<Elem> {
    let Some(geometry) = layer_map_for_output(&state.output).layer_geometry(layer) else {
        return Vec::new();
    };
    surface_elements(
        renderer,
        layer.layer_surface().wl_surface(),
        geometry.loc.x,
        geometry.loc.y,
        state.output_scale(),
    )
}

pub fn render_frame(state: &mut State) {
    let _span = crate::profile::render();
    // The host handed out a frame opportunity whether or not this turn paints, so the pacing
    // interval starts here. Stamping only after the scene gate left `winit_frame_at` stale on a
    // skipped frame, and the next burst of requests then saw `elapsed >= WINIT_FRAME_INTERVAL` and
    // bypassed the gate on every one of them.
    state.note_winit_frame();
    // A host-drawn pointer means the host window draws the cursor itself, so a pointer-motion
    // request only sets the redraw flag (`request_cursor_paint`), never the scene flag. Painting
    // then rebuilt the whole element list, re-checked both backdrop captures and submitted a
    // byte-identical frame. Consume the scene flag and skip that paint: every path that changes
    // pixels calls `request_paint`/`request_redraw` and sets it again. A dump arms its own scene
    // flag through `request_redraw`, and the lock screen never takes this path.
    if !state.take_scene_dirty()
        && state.dump.is_none()
        && !state.session_locked
        && State::host_draws_pointer_status(&state.cursor_status)
    {
        return;
    }
    let Some(mut backend) = state.backend.take() else {
        return;
    };
    state.flush_pending_configure();
    if let Err(error) = render_into(state, &mut backend) {
        eprintln!("flamewm-wayland: frame failed: {error}");
    }
    state.backend = Some(backend);
    // Work finished during the paint (a switcher thumbnail, a screencopy readback) calls
    // `request_redraw` while `backend` is taken, so `request_winit_frame` found no window to ask
    // and its frame request was skipped. The scene flag is set again, so ask for that frame now
    // instead of leaving the new pixels waiting for an unrelated host event.
    if state.take_scene_dirty() {
        state.request_paint();
    }
}

fn render_into(
    state: &mut State,
    backend: &mut WinitGraphicsBackend<GlowRenderer>,
) -> Result<(), Box<dyn Error>> {
    let size = backend.window_size();
    refresh_desktop_backdrop(state, backend.renderer(), size)?;
    if let Err(error) = refresh_dock_backdrops(state, backend.renderer()) {
        eprintln!("flamewm-wayland: dock backdrop failed: {error}");
    }
    // While locked, the lock client owns the screen: painting the desktop behind it would put
    // other clients' content back on the output the lock exists to cover. A dead locker leaves an
    // opaque cover (`screen_for`), it never reveals the desktop.
    crate::session_lock::prune_dead_locks(state);
    let elements = match crate::session_lock::screen_for(state.session_locked, state.lock_surface())
    {
        Screen::Lock(surface) => surface_elements(
            backend.renderer(),
            surface.wl_surface(),
            0,
            0,
            state.output_scale(),
        ),
        Screen::Blank => vec![solid_lock_cover(Rectangle::from_size(size))],
        Screen::Desktop => {
            // The host window draws a named pointer itself (see `State::sync_host_cursor`).
            let mut elements = if State::host_draws_pointer_status(&state.cursor_status) {
                Vec::new()
            } else {
                state.cursors.elements(
                    backend.renderer(),
                    &state.cursor_status,
                    state.pointer,
                    state.output_scale(),
                )
            };
            build_elements_into(state, backend.renderer(), &mut elements);
            elements
        }
    };
    serve_screencopy(state, backend.renderer(), &elements, size);
    serve_switcher_thumbs(state, backend.renderer());
    // Partial repaint: when the driver reports a valid swapchain age, the damage tracker only
    // touches the rectangles that changed (the pointer is the common case), instead of clearing
    // and redrawing the full output on every motion event. Some nested software-EGL drivers return
    // no age; zero remains the safe full-repaint fallback.
    // Software renderers are latched at startup (see `log_gl_renderer`): the nested winit EGL
    // surface answers EGL_BAD_SURFACE to BUFFER_AGE, which smithay logs as an ERROR per query.
    // A refusal from any other driver is latched on its first answer and repainted in full.
    // The first frame is never queried: before its first submit the nested EGL surface is not
    // yet bound and presented, so the query answers EGL_BAD_SURFACE and latches a full repaint.
    let age = if buffer_age_probe_allowed(
        FIRST_FRAME_SUBMITTED.load(std::sync::atomic::Ordering::Relaxed),
        BUFFER_AGE_REFUSED.load(std::sync::atomic::Ordering::Relaxed),
    ) {
        backend.buffer_age().unwrap_or_else(|| {
            BUFFER_AGE_REFUSED.store(true, std::sync::atomic::Ordering::Relaxed);
            0
        })
    } else {
        0
    };
    // A retained texture + full blit was measured slower than direct window damage under llvmpipe
    // (marquee 142% vs 89% CPU, one run each), so the window path stays the only one.
    let filter = state.colour_filter;
    let zoom = state.zoom_level;
    // The composite pass serves both the colour filter and the magnifier; the default (no filter,
    // zoom 1) draws the window tracker path unchanged.
    let composite = filter != ColourFilter::None || zoom > ZOOM_LEVEL_MIN;
    if composite && state.colour_program.is_none() {
        state.colour_program = Some(match crate::colour_pass::compile(backend.renderer()) {
            Ok(program) => Ok(program),
            Err(error) => {
                eprintln!("flamewm-wayland: colour filter shader failed: {error}");
                Err(())
            }
        });
    }
    let composite_program = match &state.colour_program {
        Some(Ok(program)) if composite => Some(program.clone()),
        _ => None,
    };
    let matrix = effective_matrix(filter);
    let scale = state.output_scale();
    // The pointer is followed in the frame of the output under it (layout space to output space).
    let primary = [state.output_rect()];
    let outputs = crate::output_layout::pointer_layout(&state.pointer_layout, &primary);
    let (px, py) =
        crate::output_layout::relative_to_output(outputs, state.pointer.x, state.pointer.y);
    let view = crate::zoom::viewport(
        state.zoom_follow,
        (f64::from(size.w), f64::from(size.h)),
        zoom,
        (px * scale, py * scale),
        state.zoom_origin,
    );
    state.zoom_origin = Some((view.x, view.y));
    let source = crate::colour_pass::source_rect(view, f64::from(size.h));
    if let Some(path) = state.dump.take() {
        // Readback goes through an offscreen texture: copying from the window surface target
        // left the GL context lost for the next swap (EGL_BAD_ALLOC), so the window path stays
        // readback-free. A filtered dump reads back the filtered pixels, so the runtime proof
        // compares what the filter actually produces.
        match &composite_program {
            Some(program) => {
                let pixels = crate::colour_pass::composite_pixels(
                    backend.renderer(),
                    program,
                    &matrix,
                    source,
                    &state.output,
                    &elements,
                    size,
                )?;
                write_ppm(&path, size, &pixels, readback_is_flipped(&state.output))?;
            }
            None => dump_offscreen(backend.renderer(), &state.output, &elements, size, &path)?,
        }
        state.dumped = true;
        if state.exit_after_dump {
            state.loop_signal.stop();
        }
    }
    let damage = if let Some(program) = &composite_program {
        // The composite pass repaints the full output with a fresh tracker, so the window tracker's
        // partial-repaint state is left intact for when the composite is off again.
        let mut tracker = OutputDamageTracker::from_output(&state.output);
        crate::colour_pass::draw_composite(
            backend,
            program,
            &matrix,
            source,
            &mut tracker,
            &elements,
            size,
        )?;
        None
    } else {
        let (renderer, mut framebuffer) = backend.bind()?;
        let result =
            state
                .damage
                .render_output(renderer, &mut framebuffer, age, &elements, CLEAR)?;
        result.damage.cloned()
    };
    let feedback = crate::presentation_scale::collect_feedback(state, &state.output);
    backend.submit(damage.as_deref())?;
    FIRST_FRAME_SUBMITTED.store(true, std::sync::atomic::Ordering::Relaxed);
    crate::presentation_scale::presented(feedback, &state.output);
    send_frame_callbacks(state);
    Ok(())
}

/// Renders `elements` into an offscreen texture and reads it back as RGBA rows (`Abgr8888`
/// bytes, memory order R, G, B, A). The one readback path shared by PPM dumps and screencopy.
pub fn offscreen_pixels(
    renderer: &mut GlowRenderer,
    output: &smithay::output::Output,
    elements: &[Elem],
    size: Size<i32, Physical>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let buffer_size: Size<i32, Buffer> = Size::from((size.w, size.h));
    let mut texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
    let mut tracker = OutputDamageTracker::from_output(output);
    {
        let mut target = renderer.bind(&mut texture)?;
        tracker.render_output(renderer, &mut target, 0, elements, CLEAR)?;
    }
    let target = renderer.bind(&mut texture)?;
    let mapping =
        renderer.copy_framebuffer(&target, Rectangle::from_size(buffer_size), Fourcc::Abgr8888)?;
    Ok(renderer.map_texture(&mapping)?.to_vec())
}

pub fn dump_offscreen(
    renderer: &mut GlowRenderer,
    output: &smithay::output::Output,
    elements: &[Elem],
    size: Size<i32, Physical>,
    path: &Path,
) -> Result<(), Box<dyn Error>> {
    let pixels = offscreen_pixels(renderer, output, elements, size)?;
    write_ppm(path, size, &pixels, readback_is_flipped(output))
}

/// Whether a readback of `output` must reverse its rows (flipped transforms).
#[must_use]
pub fn readback_is_flipped(output: &smithay::output::Output) -> bool {
    matches!(
        output.current_transform(),
        Transform::Flipped | Transform::Flipped180
    )
}

/// Answers queued screencopy requests from one readback of the frame about to be shown.
/// Does nothing (no GPU work) when no client asked for a copy.
pub fn serve_screencopy(
    state: &mut State,
    renderer: &mut GlowRenderer,
    elements: &[Elem],
    size: Size<i32, Physical>,
) {
    if state.screencopy_pending.is_empty() {
        return;
    }
    let pending = std::mem::take(&mut state.screencopy_pending);
    match offscreen_pixels(renderer, &state.output, elements, size) {
        Ok(rgba) => crate::screencopy::serve(
            pending,
            size.w.max(0) as u32,
            size.h.max(0) as u32,
            &rgba,
            readback_is_flipped(&state.output),
        ),
        Err(error) => {
            eprintln!("flamewm-wayland: screencopy readback failed: {error}");
            crate::screencopy::fail(pending);
        }
    }
}

fn send_frame_callbacks(state: &State) {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u32);
    let throttle = Some(Duration::from_millis(16));
    let time = Duration::from_millis(u64::from(time));
    let output = &state.output;
    let scale = state.output_scale();
    // Same visible area the frame builder culls against, so a window whose elements are skipped
    // also gets no callback.
    let visible = client_backdrop_rect(state.output_rect(), scale);
    for (id, top) in &state.toplevels {
        // A hidden window (other workspace, minimised) gets no frame callback: the callback is what
        // unblocks the client's next commit, so it would keep rasterising off screen at the refresh
        // rate. The first frame after the window is shown resumes its pipeline.
        if state.model.is_hidden(*id) {
            continue;
        }
        // Off-output window: fully clipped, so its element list is skipped and any callback would
        // only drive rasterisation nobody can see.
        if state
            .model
            .get(*id)
            .is_some_and(|managed| !frame_reaches_output(managed.frame(), scale, visible))
        {
            continue;
        }
        let surface = top.wl_surface();
        send_frames_surface_tree(surface, output, time, throttle, |_, _| Some(output.clone()));
        for (popup, _) in PopupManager::popups_for_surface(surface) {
            send_frames_surface_tree(popup.wl_surface(), output, time, throttle, |_, _| {
                Some(output.clone())
            });
        }
    }
    // XWayland presents only after its previous frame callback: without these, an X11 client
    // keeps the first (usually blank) buffer on screen. Hidden managed windows are skipped for the
    // same reason as hidden Wayland toplevels above, and an off-output frame is skipped too;
    // unmanaged ones (docks, menus) are always drawn.
    let x11_surfaces = state
        .x11
        .iter()
        .filter(|(id, _)| {
            !state.model.is_hidden(*id)
                && state
                    .model
                    .get(*id)
                    .is_none_or(|managed| frame_reaches_output(managed.frame(), scale, visible))
        })
        .map(|(_, window)| window)
        .chain(state.unmanaged.iter().map(|entry| &entry.surface));
    for surface in x11_surfaces.filter_map(smithay::xwayland::X11Surface::wl_surface) {
        send_frames_surface_tree(&surface, output, time, throttle, |_, _| {
            Some(output.clone())
        });
    }
    // Scoped so the per-output layer map lock is released before the cursor callback, never held
    // across unrelated work (smithay deadlocks on a second guard for the same output).
    {
        let map = layer_map_for_output(output);
        for layer in map.layers() {
            send_frames_surface_tree(
                layer.layer_surface().wl_surface(),
                output,
                time,
                throttle,
                |_, _| Some(output.clone()),
            );
        }
    }
    send_cursor_frame(state, time);
}

/// Frame callback for the client's cursor surface, so an animated cursor keeps committing.
pub fn send_cursor_frame(state: &State, time: Duration) {
    if let smithay::input::pointer::CursorImageStatus::Surface(surface) = &state.cursor_status {
        let output = &state.output;
        send_frames_surface_tree(surface, output, time, None, |_, _| Some(output.clone()));
    }
}

/// Writes a framebuffer readback as binary PPM (P6), converting BGRA rows to RGB.
///
/// `flip_rows` reverses the row order. A GL readback starts at the bottom row, so it is upright
/// for a `Transform::Normal` output and upside down once the surface flip is in the transform.
fn write_ppm(
    path: &Path,
    size: Size<i32, Physical>,
    pixels: &[u8],
    flip_rows: bool,
) -> Result<(), Box<dyn Error>> {
    let row = size.w as usize * 4;
    let mut out = format!("P6\n{} {}\n255\n", size.w, size.h).into_bytes();
    let mut lines: Vec<&[u8]> = pixels.chunks_exact(row).collect();
    if flip_rows {
        lines.reverse();
    }
    for line in lines {
        for px in line.as_chunks::<4>().0 {
            out.extend_from_slice(&px[..3]);
        }
    }
    std::fs::write(path, out)?;
    eprintln!(
        "flamewm-wayland: wrote frame {}x{} to {}",
        size.w,
        size.h,
        path.display()
    );
    Ok(())
}

pub(crate) fn packaged_root() -> std::path::PathBuf {
    flamewm_api::paths::packaged_root(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .as_path(),
    )
}

/// Serializes tests that mutate or compare frames against the process-wide glass statics.
#[cfg(test)]
pub(crate) static GLASS_STATICS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod decoration_tests;
#[cfg(test)]
mod flat_default_tests;
mod gl_probe;
#[cfg(test)]
mod outline_tests;
#[cfg(test)]
mod renderer_class_tests;
#[cfg(test)]
mod scale_tests;
#[cfg(test)]
mod surface_backdrop_tests;
pub use gl_probe::*;

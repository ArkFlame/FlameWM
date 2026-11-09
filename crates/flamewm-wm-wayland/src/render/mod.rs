//! Frame composition: layer surfaces, windows with server-side titlebars, popups, frame callbacks.

use std::collections::HashMap;
use std::error::Error;
use std::path::Path;
use std::time::{Duration, Instant};

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
use smithay::utils::{Buffer, Physical, Point, Rectangle, Size, Transform};
use smithay::wayland::shell::wlr_layer::Layer;

mod backdrop;
mod dock_backdrop;
mod titlebar;
pub(crate) use titlebar::{premultiplied_argb_bytes, put};

pub(crate) use backdrop::*;
pub(crate) use dock_backdrop::*;
pub use titlebar::*;

use crate::chrome_buttons::Feedback;
use crate::clip::RoundedSurfaceElement;
use crate::model::{Button, Hit, Managed, Mode, Rect, TITLEBAR_HEIGHT, WindowId};
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

const CLEAR: [f32; 4] = [0.07, 0.08, 0.11, 1.0];
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

/// A logical corner radius in physical pixels at `scale`; identity at 1.0.
fn physical_radius(radius: u8, scale: f64) -> u8 {
    (f64::from(radius) * scale).round().min(f64::from(u8::MAX)) as u8
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
pub fn build_elements(state: &State, renderer: &mut GlowRenderer) -> Vec<Elem> {
    let scale = state.output_scale();
    let mut out: Vec<Elem> = Vec::new();
    let mut decorations = state.decorations.borrow_mut();
    decorations.retain(|window| state.model.get(window).is_some());
    // Topmost: the snap preview is drawn over every window while a move grab targets a snap.
    out.extend(state.snap_preview.elements(renderer, scale));
    out.extend(scene_elements(
        state,
        renderer,
        &mut decorations,
        crate::scene::front_to_back(state),
        false,
    ));
    out
}

/// Elements for `items` in front-to-back order. `capture` builds the scene under a Dock backdrop:
/// it produces no backdrop elements, so a translucent surface never captures another one.
fn scene_elements(
    state: &State,
    renderer: &mut GlowRenderer,
    decorations: &mut Decorations,
    items: impl IntoIterator<Item = crate::scene::SceneItem>,
    capture: bool,
) -> Vec<Elem> {
    use crate::scene::SceneItem;

    let scale = state.output_scale();
    let mut out: Vec<Elem> = Vec::new();
    let hovered_button = state.model.hit(state.pointer.x, state.pointer.y);
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
                let Some(managed) = state.model.get(id).cloned() else {
                    continue;
                };
                let Some(surface) = state.window_wl_surface(id) else {
                    continue;
                };
                // Front-to-back list: the outline comes first in this window's group, so the ring
                // sits above the client and the titlebar and above every window behind it.
                out.extend(decorations.outline_elements(
                    renderer,
                    id,
                    managed.frame(),
                    outline_radius(&managed),
                    scale,
                ));
                let radii = client_corner_radii(&managed, managed.titlebars)
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
                if managed.decorated() {
                    let frame = managed.frame();
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
    out
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
    let Some(mut backend) = state.backend.take() else {
        return;
    };
    state.note_winit_frame();
    state.flush_pending_configure();
    if let Err(error) = render_into(state, &mut backend) {
        eprintln!("flamewm-wayland: frame failed: {error}");
    }
    state.backend = Some(backend);
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
    // other clients' content back on the output the lock exists to cover.
    // A locker that crashed mid-lock must not keep the screen covered forever.
    crate::session_lock::prune_dead_locks(state);
    let elements = if let Some(surface) = state.lock_surface() {
        surface_elements(
            backend.renderer(),
            surface.wl_surface(),
            0,
            0,
            state.output_scale(),
        )
    } else {
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
        elements.extend(build_elements(state, backend.renderer()));
        elements
    };
    if let Some(path) = state.dump.take() {
        // Readback goes through an offscreen texture: copying from the window surface target
        // left the GL context lost for the next swap (EGL_BAD_ALLOC), so the window path stays
        // readback-free.
        dump_offscreen(backend.renderer(), &state.output, &elements, size, &path)?;
        state.dumped = true;
        if state.exit_after_dump {
            state.loop_signal.stop();
        }
    }
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
    let damage = {
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

pub fn dump_offscreen(
    renderer: &mut GlowRenderer,
    output: &smithay::output::Output,
    elements: &[Elem],
    size: Size<i32, Physical>,
    path: &Path,
) -> Result<(), Box<dyn Error>> {
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
    let pixels = renderer.map_texture(&mapping)?;
    let flip_rows = matches!(
        output.current_transform(),
        Transform::Flipped | Transform::Flipped180
    );
    write_ppm(path, size, pixels, flip_rows)
}

fn send_frame_callbacks(state: &State) {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u32)
        .unwrap_or(0);
    let throttle = Some(Duration::from_millis(16));
    let time = Duration::from_millis(u64::from(time));
    let output = &state.output;
    for (_, top) in &state.toplevels {
        let surface = top.wl_surface();
        send_frames_surface_tree(surface, output, time, throttle, |_, _| Some(output.clone()));
        for (popup, _) in PopupManager::popups_for_surface(surface) {
            send_frames_surface_tree(popup.wl_surface(), output, time, throttle, |_, _| {
                Some(output.clone())
            });
        }
    }
    // XWayland presents only after its previous frame callback: without these, an X11 client
    // keeps the first (usually blank) buffer on screen.
    let x11_surfaces = state
        .x11
        .iter()
        .map(|(_, window)| window)
        .chain(state.unmanaged.iter().map(|entry| &entry.surface));
    for surface in x11_surfaces.filter_map(smithay::xwayland::X11Surface::wl_surface) {
        send_frames_surface_tree(&surface, output, time, throttle, |_, _| {
            Some(output.clone())
        });
    }
    let layers: Vec<_> = layer_map_for_output(output).layers().cloned().collect();
    for layer in layers {
        send_frames_surface_tree(
            layer.layer_surface().wl_surface(),
            output,
            time,
            throttle,
            |_, _| Some(output.clone()),
        );
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
        for px in line.chunks_exact(4) {
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

#[cfg(test)]
mod decoration_tests {
    use super::*;

    #[test]
    fn titlebar_ink_is_the_contrast_pick_for_the_painted_material() {
        // Dark keeps the shipped Dark glass and Dark ink, chosen by the shared owner.
        assert_eq!(
            ink_for_material(false),
            flamewm_skin::palette::ink_palette_on(flamewm_skin::palette::Rgb(
                flamewm_skin::recipes::decoration::DEFAULT_SURFACE_RGB.0
            ))
        );
        assert_eq!(
            ink_for_material(false),
            flamewm_skin::palette::Palette::RWR_0_0_9
        );
    }

    #[test]
    fn light_titlebar_paints_light_material_with_light_ink() {
        assert_eq!(
            titlebar_material(true),
            flamewm_skin::palette::LIGHT_PANEL.0
        );
        assert_eq!(
            ink_for_material(true),
            flamewm_skin::palette::Palette::LIGHT
        );
        assert_ne!(ink_for_material(true), ink_for_material(false));
    }

    #[test]
    fn chrome_uses_skin_height_and_rounded_alpha() {
        assert_eq!(TITLEBAR_HEIGHT, 31);
        let pixels = chrome_pixels(200, true, Mode::Normal, None, "FlameWM Terminal");
        assert_eq!(pixels.len(), 200 * 31 * 4);
        assert_eq!(pixels[3], 0, "outer corner must be transparent");
        assert!(pixels[(15 * 200 + 10) * 4 + 3] > 0);
    }

    #[test]
    fn titlebar_corner_radius_is_system_radius_clamped_and_square_in_layout_states() {
        assert_eq!(
            titlebar_corner_radius(true, 200, 31),
            12,
            "system Rounded radius for a floating window"
        );
        assert_eq!(
            titlebar_corner_radius(false, 200, 31),
            0,
            "every layout state is square"
        );
        assert_eq!(
            titlebar_corner_radius(true, 10, 31),
            5,
            "clamped to half width"
        );
        assert_eq!(titlebar_corner_radius(true, 200, 0), 0, "zero height");
    }

    #[test]
    fn titlebar_top_corners_are_antialiased_at_system_radius() {
        let pixels = chrome_pixels(200, true, Mode::Normal, None, "Terminal");
        let alpha = |x: usize, y: usize| pixels[(y * 200 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0, "outer corner pixel is outside the arc");
        let partial = (0..12).any(|y| {
            (0..12).any(|x| {
                let a = alpha(x, y);
                a > 0 && a < titlebar_fill().1
            })
        });
        assert!(partial, "the 12px arc has partially covered pixels");
        assert_eq!(
            alpha(24, 1),
            titlebar_fill().1,
            "straight top edge past the radius is full"
        );
    }

    /// The client silhouette is the X11 shape's radius policy: a decorated window only owns the
    /// straight band above its client, and only a floating window is rounded at all.
    #[test]
    fn client_corner_radii_follow_the_x11_shape_policy() {
        let deco =
            |mode: Mode| client_corner_radii_for(mode, true, 300, 200, DecorationStyle::Rounded);
        assert_eq!(
            deco(Mode::Normal),
            [0, 0, 12, 12],
            "band is straight on top"
        );
        assert_eq!(
            client_corner_radii_for(Mode::Normal, false, 300, 200, DecorationStyle::Rounded),
            [12; 4],
            "without the band the client carries both arcs"
        );
        assert_eq!(
            deco(Mode::Snapped(flamewm_window_core::SnapTarget::LeftHalf)),
            [0; 4],
            "a layout state is square like maximized and fullscreen"
        );
        for mode in [Mode::Maximized, Mode::Fullscreen] {
            assert_eq!(deco(mode), [0; 4], "{mode:?} stays square");
            assert_eq!(
                client_corner_radii_for(mode, false, 300, 200, DecorationStyle::Rounded),
                [0; 4],
                "{mode:?} is square without a band too"
            );
        }
    }

    /// A window narrower than the arc diameter must hand the shader clamped radii: half the short
    /// side when the client carries four arcs, the full short side when only the bottom pair does.
    #[test]
    fn client_corner_radii_clamp_to_the_short_side() {
        assert_eq!(
            client_corner_radii_for(Mode::Normal, false, 40, 400, DecorationStyle::Rounded),
            [12; 4],
            "a 40px side still fits the skin radius"
        );
        assert_eq!(
            client_corner_radii_for(Mode::Normal, false, 16, 400, DecorationStyle::Rounded),
            [8; 4],
            "four arcs need half the short side each"
        );
        assert_eq!(
            client_corner_radii_for(Mode::Normal, true, 16, 400, DecorationStyle::Rounded),
            [0, 0, 12, 12],
            "a banded client fits the bottom pair in the full short side"
        );
        assert_eq!(
            client_corner_radii_for(Mode::Normal, false, 10, 10, DecorationStyle::Rounded),
            [5; 4]
        );
    }

    /// The radius comes from the skin, so a squared decoration style is square everywhere and no
    /// hard-coded 24 can survive a style change.
    #[test]
    fn client_corner_radii_come_from_the_skin_style() {
        assert_eq!(
            client_corner_radii_for(Mode::Normal, false, 300, 200, DecorationStyle::Squared),
            [0; 4]
        );
        assert_eq!(
            client_corner_radii_for(Mode::Normal, true, 300, 200, DecorationStyle::Squared),
            [0; 4]
        );
        assert_eq!(
            client_corner_radii_for(Mode::Normal, false, 300, 200, DecorationStyle::Rounded),
            [u8::try_from(flamewm_skin::recipes::decoration::ROUNDED_RADIUS).unwrap(); 4]
        );
        // The same skin radius the titlebar arc uses, at this compositor's physical bar height.
        assert_eq!(
            client_corner_radii_for(Mode::Normal, true, 300, 200, DecorationStyle::Rounded)[2],
            u8::try_from(titlebar_corner_radius(true, 300, TITLEBAR_HEIGHT)).unwrap()
        );
    }

    /// The outline ring follows the same placement policy: a layout state's ring is a plain
    /// rectangular border (radius 0), a floating window's ring follows the skin arc.
    #[test]
    fn outline_radius_is_square_for_every_layout_state() {
        use crate::model::WindowModel;

        let work = Rect::new(0, 0, 300, 200);
        let mut model = WindowModel::new(1);
        model.add(1, Rect::new(10, 40, 300, 200), 0);
        let ring = |model: &WindowModel| outline_radius(model.get(1).expect("window"));
        assert_eq!(ring(&model), 12, "floating keeps the skin arc");
        model.snap_to(
            1,
            flamewm_window_core::SnapTarget::LeftHalf,
            work,
            Rect::new(10, 40, 300, 200),
        );
        assert_eq!(ring(&model), 0, "snapped draws a rectangular ring");
        model.maximize_to(1, work, Rect::new(10, 40, 300, 200));
        assert_eq!(ring(&model), 0, "maximized draws a rectangular ring");
        model.set_fullscreen(1, work, true);
        assert_eq!(ring(&model), 0, "fullscreen draws a rectangular ring");
    }

    /// The wrapper reads the band and the mode from the window itself, so the caller cannot
    /// disagree with the frame it is clipping.
    #[test]
    fn client_corner_radii_read_the_windows_band_and_mode() {
        use crate::model::WindowModel;

        let work = Rect::new(0, 0, 300, 200);
        let mut model = WindowModel::new(1);
        model.add(1, Rect::new(10, 40, 300, 200), 0);
        let radii = |model: &WindowModel| {
            let window = model.get(1).expect("window");
            client_corner_radii(window, window.titlebars)
        };
        assert_eq!(radii(&model), [0, 0, 12, 12], "decorated: bottom corners");
        model.configure(false, 0);
        assert_eq!(radii(&model), [12; 4], "no band: all four corners");
        model.configure(true, 0);
        model.maximize_to(1, work, Rect::new(10, 40, 300, 200));
        assert_eq!(radii(&model), [0; 4], "maximized is square");
        model.set_fullscreen(1, work, true);
        assert_eq!(radii(&model), [0; 4], "fullscreen is square");
    }

    #[test]
    fn maximized_titlebar_corner_is_not_cut() {
        let pixels = chrome_pixels(200, true, Mode::Maximized, None, "Terminal");
        assert!(pixels[3] > 0, "square corner stays opaque");
    }

    #[test]
    fn close_hover_is_local_to_control_and_keeps_alpha_corners() {
        let idle = chrome_pixels(400, true, Mode::Normal, None, "Terminal");
        let hover = chrome_pixels(400, true, Mode::Normal, Some(Button::Close), "Terminal");
        let index = |x: usize, y: usize| (y * 400 + x) * 4;
        assert_ne!(
            &idle[index(385, 15)..index(385, 15) + 4],
            &hover[index(385, 15)..index(385, 15) + 4],
        );
        assert_eq!(
            idle[index(10, 15)..index(10, 15) + 4],
            hover[index(10, 15)..index(10, 15) + 4]
        );
        assert_eq!(hover[3], 0, "antialiased corners must not become opaque");
    }

    #[test]
    fn desktop_capture_is_reused_until_size_or_commits_change() {
        let commit = |n: usize| CommitCounter::from(n);
        let mut cache = Decorations::default();
        assert!(!cache.desktop_current((4, 3), &[]), "empty cache must miss");
        cache.set_desktop((4, 3), vec![commit(1), commit(7)], vec![0; 4]);
        assert!(cache.desktop_current((4, 3), &[commit(1), commit(7)]));
        // Wallpaper repaint: same surfaces and size, one surface committed again.
        assert!(!cache.desktop_current((4, 3), &[commit(1), commit(8)]));
        // Surface added or removed on the desktop layer.
        assert!(!cache.desktop_current((4, 3), &[commit(1)]));
        // Output resized.
        assert!(!cache.desktop_current((5, 3), &[commit(1), commit(7)]));
    }

    #[test]
    fn idle_desktop_drops_the_output_sized_capture() {
        let commit = |n: usize| CommitCounter::from(n);
        let mut cache = Decorations::default();
        cache.set_desktop((4, 3), vec![commit(1)], vec![0; 4]);
        cache.drop_desktop();
        assert!(
            cache.desktop.is_none(),
            "idle capture must not stay resident"
        );
        assert!(
            !cache.desktop_current((4, 3), &[commit(1)]),
            "next use recaptures"
        );
    }

    #[test]
    fn desktop_recapture_is_throttled_for_one_interval_then_allowed() {
        let mut cache = Decorations::default();
        let now = Instant::now();
        assert_eq!(cache.recapture_wait((4, 3), now), None, "no capture yet");
        cache.set_desktop((4, 3), Vec::new(), vec![0; 4]);
        let captured = cache.desktop.as_ref().expect("capture").captured_at;
        let early = captured + Duration::from_millis(40);
        assert_eq!(
            cache.recapture_wait((4, 3), early),
            Some(DESKTOP_RECAPTURE_INTERVAL - Duration::from_millis(40))
        );
        assert_eq!(
            cache.recapture_wait((4, 3), captured + DESKTOP_RECAPTURE_INTERVAL),
            None
        );
        // A resized output is never throttled: the old capture cannot be shown at the new size.
        assert_eq!(cache.recapture_wait((5, 3), early), None);
    }

    #[test]
    fn maximize_glyph_tracks_real_window_state() {
        let normal = chrome_pixels(400, true, Mode::Normal, None, "Terminal");
        let maximized = chrome_pixels(400, true, Mode::Maximized, None, "Terminal");
        assert_ne!(normal, maximized);
        assert_eq!(normal.len(), maximized.len());
    }

    #[test]
    fn narrow_frame_never_writes_outside_buffer() {
        for width in [1, 12, 38, 77, 114] {
            assert_eq!(
                chrome_pixels(
                    width,
                    true,
                    Mode::Normal,
                    Some(Button::Close),
                    "Long window title"
                )
                .len(),
                width as usize * TITLEBAR_HEIGHT as usize * 4,
            );
        }
    }

    #[test]
    fn proportional_title_renders_unicode_inside_the_title_region_only() {
        let _guard = GLASS_STATICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let width = 320;
        let blank = chrome_pixels(width, true, Mode::Normal, None, "");
        let titled = chrome_pixels(
            width,
            true,
            Mode::Normal,
            None,
            "Terminal \u{2014} \u{dc}n\u{ef}code",
        );
        let strip = (width - chrome::controls_width()) as usize;
        let mut changed = 0;
        for (index, (a, b)) in blank
            .chunks_exact(4)
            .zip(titled.chunks_exact(4))
            .enumerate()
        {
            if a != b {
                changed += 1;
                assert!(
                    index % width as usize <= strip,
                    "title pixel inside controls strip"
                );
            }
        }
        assert!(
            changed > 40,
            "real glyphs should cover many pixels, got {changed}"
        );
        if let Ok(dir) = std::env::var("FLAMEWM_DUMP_CHROME") {
            let mut ppm = format!("P6\n{width} {TITLEBAR_HEIGHT}\n255\n").into_bytes();
            for px in titled.chunks_exact(4) {
                let a = u32::from(px[3]);
                let bg = 0x40u32;
                for c in &px[..3] {
                    ppm.push(((u32::from(*c) * a + bg * (255 - a)) / 255) as u8);
                }
            }
            std::fs::write(format!("{dir}/titlebar.ppm"), ppm).unwrap();
        }
    }

    #[test]
    fn title_text_stays_out_of_button_pixels_on_narrow_windows() {
        for width in [1, 12, 38, 114, 150, 220] {
            let available = chrome::title_available_width(
                0,
                width - chrome::controls_width(),
                chrome::TITLE_PAD,
            );
            let text =
                chrome::elide_title("A very long application window title", available, |_| 6);
            let text_width = text.chars().count() as i32 * 6;
            let x = chrome::center_title_x(
                width,
                text_width,
                0,
                width - chrome::controls_width(),
                chrome::TITLE_PAD,
            );
            assert!(text_width <= available);
            if !text.is_empty() {
                assert!(x + text_width <= width - chrome::controls_width() - chrome::TITLE_PAD);
            }
        }
    }

    #[test]
    fn chrome_stable_and_buttons_are_glyphs_not_coloured_squares() {
        let _guard = GLASS_STATICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let first = chrome_pixels(400, true, Mode::Normal, None, "FlameWM Terminal");
        assert_eq!(
            first,
            chrome_pixels(400, true, Mode::Normal, None, "FlameWM Terminal")
        );
        assert_ne!(
            first,
            chrome_pixels(400, false, Mode::Normal, None, "FlameWM Terminal")
        );
        assert_ne!(
            first,
            chrome_pixels(
                400,
                true,
                Mode::Normal,
                Some(Button::Close),
                "FlameWM Terminal"
            )
        );
        // Transparent body alpha is the same under every control at rest.
        let left = (15 * 400 + 200) * 4 + 3;
        let near_control = (15 * 400 + 365) * 4 + 3;
        assert!(first[left] > 0 && first[near_control] > 0);
    }
}

#[cfg(test)]
mod outline_tests {
    use super::*;
    use crate::model::WindowModel;
    use flamewm_api::settings::{SettingValue, SettingsSnapshot};

    fn decorated() -> Decorations {
        let mut decorations = Decorations::default();
        assert!(decorations.set_outline(0x00ff_ffff, 127, 1));
        decorations
    }

    /// The settings come from the shared surface owner the X11 frame already reads, so the
    /// Wayland ring cannot drift from it: the committed defaults are neutral gray (#999999) at half opacity.
    #[test]
    fn outline_defaults_to_gray_at_half_opacity() {
        let stroke = flamewm_ui_core::surface::window_outline_stroke(&SettingsSnapshot::new(1));
        assert_eq!((stroke.rgb, stroke.alpha), (0x0099_9999, 127));
        let mut decorations = Decorations::default();
        assert!(
            decorations
                .ring_strips(1, Rect::new(0, 0, 300, 200), 24)
                .is_none(),
            "nothing is drawn before the first settings sync"
        );
        assert!(decorations.set_outline(stroke.rgb, stroke.alpha, stroke.width));
        let ring = decorations
            .ring_strips(1, Rect::new(0, 0, 300, 200), 24)
            .expect("ring");
        assert_eq!(
            (ring.paint.rgb, ring.paint.alpha, ring.paint.width),
            (0x0099_9999, 127, 1)
        );
        assert!(
            !decorations.set_outline(stroke.rgb, stroke.alpha, stroke.width),
            "an unchanged snapshot must not request a redraw"
        );
    }

    #[test]
    fn outlines_off_or_zero_opacity_draw_nothing() {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("outlines".into(), SettingValue::Boolean(false));
        let stroke = flamewm_ui_core::surface::window_outline_stroke(&snapshot);
        assert_eq!(stroke.alpha, 0, "outlines off means no ring");
        let mut decorations = Decorations::default();
        assert!(decorations.set_outline(stroke.rgb, stroke.alpha, stroke.width));
        assert!(
            decorations
                .ring_strips(1, Rect::new(0, 0, 300, 200), 24)
                .is_none()
        );
        // A zero `windowOutlineOpacity` is the same: no strips, so no elements.
        decorations.set_outline(0x00ff_ffff, 0, 1);
        assert!(
            decorations
                .ring_strips(1, Rect::new(0, 0, 300, 200), 24)
                .is_none()
        );
    }

    /// The ring is cached per window: an unchanged frame reuses the strips, while a move, a
    /// resize, a new radius and new outline settings each rebuild them.
    #[test]
    fn outline_cache_rebuilds_only_when_an_input_changes() {
        let mut decorations = decorated();
        let frame = Rect::new(10, 20, 300, 200);
        let ring = decorations.ring_strips(1, frame, 24).expect("ring");
        assert_eq!(
            (
                ring.origin,
                ring.width,
                ring.height,
                ring.radius,
                ring.builds
            ),
            ((10, 20), 300, 200, 24, 1)
        );
        let strips = ring.strips.len();
        let ring = decorations.ring_strips(1, frame, 24).expect("ring");
        assert_eq!(ring.builds, 1, "unchanged inputs reuse the cache");
        assert_eq!(ring.strips.len(), strips);

        let moved = Rect::new(11, 20, 300, 200);
        let ring = decorations.ring_strips(1, moved, 24).expect("ring");
        assert_eq!((ring.origin, ring.builds), ((11, 20), 2), "a move rebuilds");

        let resized = Rect::new(11, 20, 301, 200);
        let ring = decorations.ring_strips(1, resized, 24).expect("ring");
        assert_eq!((ring.width, ring.builds), (301, 3), "a resize rebuilds");

        let ring = decorations.ring_strips(1, resized, 12).expect("ring");
        assert_eq!((ring.radius, ring.builds), (12, 4), "a new radius rebuilds");

        assert!(decorations.set_outline(0x0011_2233, 127, 2));
        let ring = decorations.ring_strips(1, resized, 12).expect("ring");
        assert_eq!(
            (ring.paint.rgb, ring.paint.width, ring.builds),
            (0x0011_2233, 2, 1),
            "new settings drop the cached strips"
        );
    }

    /// Strip origins plus buffer sizes are the frame rect: every corner and edge is uploaded and
    /// the interior is not, so a caller can place the strips from the frame origin alone.
    #[test]
    fn ring_strips_tile_the_frame_rect() {
        let mut decorations = decorated();
        let frame = Rect::new(40, 60, 320, 240);
        let (w, h) = (frame.w as u32, frame.h as u32);
        let ring = decorations.ring_strips(1, frame, 24).expect("ring");
        let mut covered = vec![false; (w * h) as usize];
        for strip in &ring.strips {
            let (x, y, sw, sh) = (strip.offset.0, strip.offset.1, strip.size.0, strip.size.1);
            assert!(sw > 0 && sh > 0);
            assert!(
                x >= 0 && y >= 0 && x as u32 + sw as u32 <= w && y as u32 + sh as u32 <= h,
                "strip at {x},{y} {sw}x{sh} leaves the frame"
            );
            for row in 0..sh as u32 {
                for column in 0..sw as u32 {
                    covered[((y as u32 + row) * w + x as u32 + column) as usize] = true;
                }
            }
        }
        let covered = |x: u32, y: u32| covered[(y * w + x) as usize];
        assert!(
            covered(0, 0) && covered(w - 1, 0) && covered(0, h - 1) && covered(w - 1, h - 1),
            "all four corners are uploaded"
        );
        assert!(
            covered(w / 2, 0)
                && covered(w / 2, h - 1)
                && covered(0, h / 2)
                && covered(w - 1, h / 2),
            "both edges are uploaded"
        );
        assert!(
            !covered(w / 2, h / 2),
            "the interior stays out of the upload"
        );
    }

    /// The ring radius is the client clip radius, so the arc and the clipped corner are
    /// concentric; maximized, fullscreen and squared windows get a square ring.
    #[test]
    fn ring_radius_matches_the_client_clip_radius() {
        let work = Rect::new(0, 0, 300, 200);
        let mut model = WindowModel::new(1);
        model.add(1, Rect::new(10, 40, 300, 200), 0);
        let ring = |model: &WindowModel| outline_radius(model.get(1).expect("window"));
        let clip = |model: &WindowModel| {
            let window = model.get(1).expect("window");
            client_corner_radii(window, window.titlebars)
        };
        assert_eq!(ring(&model), clip(&model)[2], "bottom arc is concentric");
        model.configure(false, 0);
        assert_eq!(ring(&model), clip(&model)[0], "no band: same radius");
        model.configure(true, 0);
        model.maximize_to(1, work, Rect::new(10, 40, 300, 200));
        assert_eq!(ring(&model), 0, "maximized is square");
        model.set_fullscreen(1, work, true);
        assert_eq!(ring(&model), 0, "fullscreen is square");
    }
}

/// True for Mesa's CPU rasterizers: GLES works but nothing is hardware accelerated.
pub fn is_software_renderer(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["llvmpipe", "softpipe", "swrast", "software rasterizer"]
        .iter()
        .any(|soft| lower.contains(soft))
}

/// Set once a driver refuses `EGL_BUFFER_AGE_EXT`: every later frame repaints in full.
static BUFFER_AGE_REFUSED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Set after the first successful submit, when the window surface is bound and presented.
static FIRST_FRAME_SUBMITTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Whether the buffer-age query may run: only once a frame was submitted and no driver refused it.
pub(crate) const fn buffer_age_probe_allowed(first_frame_submitted: bool, refused: bool) -> bool {
    first_frame_submitted && !refused
}

/// Whether the buffer-age probe is skipped from the start: a software renderer never answers it.
pub fn buffer_age_unsupported(renderer_name: &str) -> bool {
    is_software_renderer(renderer_name)
}

/// Whether the buffer-age probe is skipped from the start on this backend. The nested winit EGL
/// surface answers `EGL_BAD_SURFACE` to `BUFFER_AGE` on real Intel GL too (reproduced on
/// `Mesa Intel(R) UHD Graphics (TGL GT1)`, one ERROR per first redraw), so nested sessions always
/// repaint in full: age 0 is the always-correct fallback.
pub fn buffer_age_unsupported_on(backend: &str, renderer_name: &str) -> bool {
    backend == "winit" || buffer_age_unsupported(renderer_name)
}

/// Logs the real GL driver once, so "gles" is never mistaken for hardware acceleration.
#[allow(unsafe_code)] // glGetString is a read-only query on the current context.
pub fn log_gl_renderer(renderer: &mut GlowRenderer, backend: &str) {
    use smithay::reexports::glow::HasContext;
    let name = renderer
        .with_context(|gl| unsafe { gl.get_parameter_string(smithay::reexports::glow::RENDERER) })
        .unwrap_or_else(|_| "unknown".to_owned());
    let class = if is_software_renderer(&name) {
        "software"
    } else {
        "hardware"
    };
    if buffer_age_unsupported_on(backend, &name) {
        BUFFER_AGE_REFUSED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    eprintln!("FLAMEWM_GL_RENDERER backend={backend} renderer=\"{name}\" class={class}");
}

#[cfg(test)]
mod renderer_class_tests {
    use super::{buffer_age_probe_allowed, buffer_age_unsupported, is_software_renderer};

    #[test]
    fn buffer_age_is_not_queried_before_first_submit() {
        assert!(!buffer_age_probe_allowed(false, false));
        assert!(buffer_age_probe_allowed(true, false));
        assert!(!buffer_age_probe_allowed(true, true));
    }

    #[test]
    fn mesa_cpu_rasterizers_are_software() {
        assert!(is_software_renderer("llvmpipe (LLVM 19.1.0, 256 bits)"));
        assert!(is_software_renderer("Software Rasterizer"));
        assert!(!is_software_renderer(
            "Mesa Intel(R) UHD Graphics (TGL GT1)"
        ));
    }

    #[test]
    fn software_renderer_skips_buffer_age_probe() {
        // The nested llvmpipe surface answers EGL_BAD_SURFACE to BUFFER_AGE (one ERROR per start).
        assert!(buffer_age_unsupported("llvmpipe (LLVM 21.1.8, 256 bits)"));
        assert!(!buffer_age_unsupported(
            "Mesa Intel(R) UHD Graphics (TGL GT1)"
        ));
    }

    #[test]
    fn nested_winit_skips_buffer_age_probe_on_hardware_too() {
        // Real Intel GL on the nested winit surface: EGL_BAD_SURFACE on every age query.
        assert!(super::buffer_age_unsupported_on(
            "winit",
            "Mesa Intel(R) UHD Graphics (TGL GT1)"
        ));
        assert!(!super::buffer_age_unsupported_on(
            "drm",
            "Mesa Intel(R) UHD Graphics (TGL GT1)"
        ));
        assert!(super::buffer_age_unsupported_on(
            "drm",
            "llvmpipe (LLVM 21.1.8, 256 bits)"
        ));
    }
}

pub(crate) fn packaged_root() -> std::path::PathBuf {
    flamewm_api::paths::packaged_root(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .as_path(),
    )
}

#[cfg(test)]
mod surface_backdrop_tests {
    use super::*;

    #[test]
    fn opaque_argb_bytes_swap_rgb_to_little_endian_bgra() {
        assert_eq!(
            opaque_argb_bytes(&[0x11, 0x22, 0x33, 0x00]),
            vec![0x33, 0x22, 0x11, 0xff]
        );
    }

    /// The outline strip upload is premultiplied, so its antialiased arc blends without a fringe:
    /// a straight-alpha white at half coverage would otherwise read as full white.
    #[test]
    fn premultiplied_argb_bytes_scale_channels_by_alpha() {
        assert_eq!(
            premultiplied_argb_bytes(&[255, 255, 255, 128]),
            vec![128, 128, 128, 128]
        );
        assert_eq!(
            premultiplied_argb_bytes(&[0x11, 0x22, 0x33, 0]),
            vec![0, 0, 0, 0],
            "a zero-coverage pixel stays fully transparent"
        );
        assert_eq!(
            premultiplied_argb_bytes(&[0x11, 0x22, 0x33, 255]),
            vec![0x33, 0x22, 0x11, 255]
        );
    }

    #[test]
    fn crop_clamps_to_the_output_and_samples_the_reduced_capture() {
        // Output 8x8 stored at 2x2: stored pixel (x, y) has R = 10 * (2y + x) + 1.
        let pixels: Vec<u8> = (0..4u8).flat_map(|i| [10 * i + 1, 0, 0, 255]).collect();
        let mut decorations = Decorations::default();
        decorations.set_desktop((8, 8), Vec::new(), pixels);
        // The top-left output pixel maps to the top-left stored pixel.
        let corner = decorations.crop((0, 0), 1, 1, 1.0).expect("desktop set");
        assert_eq!(corner[0], 1);
        // Past the bottom-right edge every sample clamps to the bottom-right stored pixel.
        let clamped = decorations
            .crop((100, 100), 2, 2, 1.0)
            .expect("desktop set");
        let reds: Vec<u8> = clamped.chunks_exact(4).map(|px| px[0]).collect();
        assert_eq!(reds, vec![31, 31, 31, 31]);
    }

    #[test]
    fn desktop_capture_is_stored_at_reduced_size() {
        assert_eq!(stored_size((1280, 800)), (320, 200));
        assert_eq!(
            stored_size((1281, 801)),
            (321, 201),
            "partial blocks round up"
        );
        assert_eq!(stored_size((1, 1)), (1, 1));
    }

    #[test]
    fn strip_downscale_box_averages_blocks_across_strips() {
        // 4x8 output in two 4-row strips; the top block is 0, the bottom block is 200 (every channel).
        let width = 4;
        let top = vec![0u8; width * 4 * 4];
        let bottom = vec![200u8; width * 4 * 4];
        let mut stored = vec![0u8; 2 * 4];
        downscale_strip(&top, (width, 0, 4), &mut stored, (1, 2));
        downscale_strip(&bottom, (width, 4, 4), &mut stored, (1, 2));
        assert_eq!(&stored[0..4], &[0, 0, 0, 0]);
        assert_eq!(&stored[4..8], &[200, 200, 200, 200]);
    }
}

/// Serializes tests that mutate or compare frames against the process-wide glass statics.
#[cfg(test)]
pub(crate) static GLASS_STATICS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod flat_default_tests {
    use super::*;

    /// Process-wide statics: this test restores the flat default it changes.
    #[test]
    fn flat_glass_commits_opaque_fill_and_no_desktop_blur() {
        let _guard = GLASS_STATICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set_glass_fill(0x0017_2135, 255, 0, true);
        assert!(glass_flat());
        assert_eq!(titlebar_fill(), (0x0017_2135, 255));
        assert_eq!(
            DESKTOP_BLUR_PERCENT.load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        set_glass_fill(0x0033_3333, 165, 80, false);
        assert!(!glass_flat());
        assert_eq!(titlebar_fill(), (0x0033_3333, 165));
        set_glass_fill(0x0017_2135, 255, 0, true);
    }
}

#[cfg(test)]
mod scale_tests {
    use super::{physical_point, physical_radius};

    #[test]
    fn default_scale_is_identity_for_points_and_radii() {
        assert_eq!(physical_point(-7, 1366, 1.0), (-7, 1366).into());
        for radius in [0_u8, 1, 12, 255] {
            assert_eq!(physical_radius(radius, 1.0), radius);
        }
    }

    #[test]
    fn fractional_scale_rounds_points_and_radii_to_physical_pixels() {
        assert_eq!(physical_point(1000, 500, 1.25), (1250, 625).into());
        assert_eq!(physical_point(3, 3, 1.25), (4, 4).into());
        assert_eq!(physical_radius(12, 1.25), 15);
        assert_eq!(physical_radius(250, 2.0), 255);
    }
}

//! Frame composition: layer surfaces, windows with server-side titlebars, popups, frame callbacks.

use std::collections::HashMap;
use std::error::Error;
use std::path::Path;
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::solid::{SolidColorBuffer, SolidColorRenderElement};
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::{Bind, ExportMem, Offscreen};
use smithay::backend::winit::WinitGraphicsBackend;
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::desktop::{PopupManager, layer_map_for_output};
use smithay::render_elements;
use smithay::utils::{Buffer, Physical, Point, Rectangle, Size, Transform};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::model::{Button, Rect, TITLEBAR_HEIGHT, WindowId, button_rect};
use crate::state::State;

render_elements! {
    pub Elem<=GlowRenderer>;
    Solid=SolidColorRenderElement,
    Surface=WaylandSurfaceRenderElement<GlowRenderer>,
    Memory=MemoryRenderBufferRenderElement<GlowRenderer>,
}

const CLEAR: [f32; 4] = [0.07, 0.08, 0.11, 1.0];
const TITLE_FOCUSED: [f32; 4] = [0.23, 0.29, 0.40, 1.0];
const TITLE_IDLE: [f32; 4] = [0.16, 0.18, 0.22, 1.0];
const BUTTON_CLOSE: [f32; 4] = [0.75, 0.31, 0.30, 1.0];
const BUTTON_MAXIMIZE: [f32; 4] = [0.31, 0.55, 0.31, 1.0];
const BUTTON_MINIMIZE: [f32; 4] = [0.72, 0.63, 0.25, 1.0];

/// Titlebar and button fills, kept per window and part. A stable buffer keeps its element id across
/// frames and bumps its commit only when its size or colour changes, so the damage trackers redraw
/// a titlebar when it changes instead of on every frame (a fresh id each frame read as a new
/// element, which repainted every titlebar on each pointer move).
#[derive(Default)]
pub struct Decorations {
    parts: HashMap<(WindowId, Part), SolidColorBuffer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Part {
    Titlebar,
    Button(Button),
}

impl Decorations {
    fn element(&mut self, window: WindowId, part: Part, rect: Rect, color: [f32; 4]) -> Elem {
        let buffer = self.parts.entry((window, part)).or_default();
        buffer.update((rect.w, rect.h), color);
        Elem::Solid(SolidColorRenderElement::from_buffer(
            buffer,
            (rect.x, rect.y),
            1.0,
            1.0,
            Kind::Unspecified,
        ))
    }

    /// Drops the fills of windows that no longer exist.
    fn retain(&mut self, live: impl Fn(WindowId) -> bool) {
        self.parts.retain(|(window, _), _| live(*window));
    }
}

fn surface_elements(
    renderer: &mut GlowRenderer,
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    x: i32,
    y: i32,
) -> Vec<Elem> {
    let location: Point<i32, Physical> = Point::from((x, y));
    render_elements_from_surface_tree::<_, WaylandSurfaceRenderElement<GlowRenderer>>(
        renderer,
        surface,
        location,
        1.0,
        1.0,
        Kind::Unspecified,
    )
    .into_iter()
    .map(Elem::Surface)
    .collect()
}

/// Front-to-back element list: smithay draws the first element on top.
/// Order: overlay and top layers, then windows topmost first (each window: popups, surface,
/// decorations), then bottom and background layers.
pub fn build_elements(state: &State, renderer: &mut GlowRenderer) -> Vec<Elem> {
    let map = layer_map_for_output(&state.output);
    let layers: Vec<_> = map.layers().cloned().collect();
    drop(map);
    let mut out: Vec<Elem> = Vec::new();
    let mut back: Vec<Elem> = Vec::new();
    let mut decorations = state.decorations.borrow_mut();
    decorations.retain(|window| state.model.get(window).is_some());
    for group in [Layer::Overlay, Layer::Top] {
        for layer in layers.iter().rev().filter(|l| l.layer() == group) {
            out.extend(layer_elements(state, renderer, layer));
        }
    }

    // Override-redirect menus and docks sit above managed windows.
    for entry in state.unmanaged.iter().rev().filter(|entry| !entry.below) {
        if let Some(surface) = entry.surface.wl_surface() {
            let geometry = entry.surface.geometry();
            out.extend(surface_elements(
                renderer,
                &surface,
                geometry.loc.x,
                geometry.loc.y,
            ));
        }
    }

    for id in state.model.visible_top_first() {
        let Some(managed) = state.model.get(id).cloned() else {
            continue;
        };
        let Some(surface) = state.window_wl_surface(id) else {
            continue;
        };

        for (popup, offset) in PopupManager::popups_for_surface(&surface) {
            out.extend(surface_elements(
                renderer,
                popup.wl_surface(),
                managed.rect.x + offset.x,
                managed.rect.y + offset.y,
            ));
        }
        out.extend(surface_elements(
            renderer,
            &surface,
            managed.rect.x,
            managed.rect.y,
        ));

        if managed.decorated() {
            let frame = managed.frame();
            let focused = state.focused == Some(id);
            let titlebar = Rect::new(frame.x, frame.y, frame.w, TITLEBAR_HEIGHT);
            // Front to back: the buttons sit on the titlebar, so they come first.
            for (button, color) in [
                (Button::Close, BUTTON_CLOSE),
                (Button::Maximize, BUTTON_MAXIMIZE),
                (Button::Minimize, BUTTON_MINIMIZE),
            ] {
                out.push(decorations.element(
                    id,
                    Part::Button(button),
                    button_rect(frame, button),
                    color,
                ));
            }
            out.push(decorations.element(
                id,
                Part::Titlebar,
                titlebar,
                if focused { TITLE_FOCUSED } else { TITLE_IDLE },
            ));
        }
    }

    for layer in layers
        .iter()
        .rev()
        .filter(|l| matches!(l.layer(), Layer::Bottom | Layer::Background))
    {
        back.extend(layer_elements(state, renderer, layer));
    }
    out.extend(back);
    // X11 desktop background sits beneath the wallpaper layers' siblings: last in the list.
    for entry in state.unmanaged.iter().rev().filter(|entry| entry.below) {
        if let Some(surface) = entry.surface.wl_surface() {
            let geometry = entry.surface.geometry();
            out.extend(surface_elements(
                renderer,
                &surface,
                geometry.loc.x,
                geometry.loc.y,
            ));
        }
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
    )
}

/// Draws one frame to the nested window, honours a pending screenshot, and sends frame callbacks.
pub fn render_frame(state: &mut State) {
    let Some(mut backend) = state.backend.take() else {
        return;
    };
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
    let mut elements =
        state
            .cursors
            .elements(backend.renderer(), &state.cursor_status, state.pointer);
    elements.extend(build_elements(state, backend.renderer()));
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
    let age = backend.buffer_age().unwrap_or(0);
    let damage = {
        let (renderer, mut framebuffer) = backend.bind()?;
        let result =
            state
                .damage
                .render_output(renderer, &mut framebuffer, age, &elements, CLEAR)?;
        result.damage.cloned()
    };
    backend.submit(damage.as_deref())?;
    send_frame_callbacks(state);
    Ok(())
}

/// Renders the element list into a private texture and writes it as PPM. Uses its own damage
/// tracker so the window tracker's buffer-age state is untouched.
///
/// Public because the tty backend has no window to read back from and reuses this for
/// `FLAMEWM_WAYLAND_DUMP`; its element list is the same [`Elem`] type.
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
    use smithay::backend::renderer::element::Element;

    fn solid(element: Elem) -> SolidColorRenderElement {
        match element {
            Elem::Solid(solid) => solid,
            _ => panic!("decorations are solid fills"),
        }
    }

    #[test]
    fn a_titlebar_keeps_its_id_and_commits_only_on_change() {
        let mut decorations = Decorations::default();
        let rect = Rect::new(10, 20, 300, TITLEBAR_HEIGHT);
        let first = solid(decorations.element(7, Part::Titlebar, rect, TITLE_IDLE));
        let again = solid(decorations.element(7, Part::Titlebar, rect, TITLE_IDLE));
        assert_eq!(first.id(), again.id(), "same window part, same element");
        assert_eq!(
            first.current_commit(),
            again.current_commit(),
            "unchanged: no damage"
        );
        let focused = solid(decorations.element(7, Part::Titlebar, rect, TITLE_FOCUSED));
        assert_eq!(focused.id(), first.id());
        assert_ne!(
            focused.current_commit(),
            first.current_commit(),
            "a colour change is damage"
        );
        let button = solid(decorations.element(7, Part::Button(Button::Close), rect, BUTTON_CLOSE));
        assert_ne!(button.id(), first.id(), "each part has its own element");
        decorations.retain(|window| window != 7);
        assert!(
            decorations.parts.is_empty(),
            "a closed window's fills are dropped"
        );
    }
}

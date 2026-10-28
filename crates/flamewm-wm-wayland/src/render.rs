//! Frame composition: layer surfaces, windows with server-side titlebars, popups, frame callbacks.

use std::collections::HashMap;
use std::error::Error;
use std::path::Path;
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::memory::{
    MemoryRenderBuffer, MemoryRenderBufferRenderElement,
};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
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

use crate::model::{Button, Hit, Mode, Rect, TITLEBAR_HEIGHT, WindowId};
use crate::state::State;
use flamewm_skin::recipes::window_chrome::{self as chrome, WindowControlRole};

render_elements! {
    pub Elem<=GlowRenderer>;
    Solid=SolidColorRenderElement,
    Surface=WaylandSurfaceRenderElement<GlowRenderer>,
    Memory=MemoryRenderBufferRenderElement<GlowRenderer>,
}

const CLEAR: [f32; 4] = [0.07, 0.08, 0.11, 1.0];
// Shared Xephyr skin metrics and palette, never per-backend coloured-square placeholders.
const SURFACE: u32 = flamewm_skin::recipes::decoration::DEFAULT_SURFACE_RGB.0;
const SURFACE_ALPHA: u8 = flamewm_skin::recipes::decoration::DEFAULT_SURFACE_ALPHA;
const GLYPH: u32 = flamewm_skin::palette::TEXT.0;
const BORDER: u32 = flamewm_skin::recipes::window_chrome::WINDOW_CHROME.border.0;
const CLOSE_HOVER: u32 = flamewm_skin::recipes::window_chrome::WINDOW_CHROME
    .close_hover
    .0;

/// Keep one premultiplied ARGB bitmap per managed window. No raster work or allocations on
/// steady-state frames: rebuilding happens only on resize/focus/hover/title changes.
#[derive(Default)]
pub struct Decorations {
    parts: HashMap<WindowId, ChromeBuffer>,
}

struct ChromeBuffer {
    width: i32,
    active: bool,
    maximized: bool,
    hover: Option<Button>,
    title: String,
    buffer: MemoryRenderBuffer,
}

impl Decorations {
    fn element(
        &mut self,
        renderer: &mut GlowRenderer,
        window: WindowId,
        rect: Rect,
        active: bool,
        maximized: bool,
        hover: Option<Button>,
        title: &str,
    ) -> Option<Elem> {
        // GL texture dimensions must match the allocated CPU surface, even for narrow frames.
        if rect.w <= 0 || rect.w > 32_767 {
            self.parts.remove(&window);
            return None;
        }
        let stale = self.parts.get(&window).is_none_or(|part| {
            part.width != rect.w
                || part.active != active
                || part.maximized != maximized
                || part.hover != hover
                || part.title != title
        });
        if stale {
            let pixels = chrome_pixels(rect.w, active, maximized, hover, title);
            let buffer = MemoryRenderBuffer::from_slice(
                &pixels,
                Fourcc::Argb8888,
                (rect.w, TITLEBAR_HEIGHT),
                1,
                Transform::Normal,
                None,
            );
            self.parts.insert(
                window,
                ChromeBuffer {
                    width: rect.w,
                    active,
                    maximized,
                    hover,
                    title: title.to_owned(),
                    buffer,
                },
            );
        }
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            Point::from((f64::from(rect.x), f64::from(rect.y))),
            &self.parts.get(&window)?.buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .ok()
        .map(Elem::Memory)
    }

    fn retain(&mut self, live: impl Fn(WindowId) -> bool) {
        self.parts.retain(|window, _| live(*window));
    }
}

/// Straight alpha composition into little-endian, premultiplied BGRA (DRM Argb8888).
fn blend(px: &mut [u8], rgb: u32, alpha: u8) {
    let a = u32::from(alpha);
    let inv = 255 - a;
    let r = (rgb >> 16) & 255;
    let g = (rgb >> 8) & 255;
    let b = rgb & 255;
    for (index, color) in [b, g, r].into_iter().enumerate() {
        px[index] = ((color * a + u32::from(px[index]) * inv + 127) / 255) as u8;
    }
    px[3] = (a + (u32::from(px[3]) * inv + 127) / 255) as u8;
}

fn put(pixels: &mut [u8], width: usize, height: usize, x: i32, y: i32, rgb: u32, a: u8) {
    if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
        return;
    }
    let at = (y as usize * width + x as usize) * 4;
    blend(&mut pixels[at..at + 4], rgb, a);
}

/// Minimal 5x7 fallback glyphs used only for native titlebars. The X11 backend continues to
/// use IBM Plex/Xft; the compositor does not call X11 or a subprocess during frame painting.
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 18, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 30, 1, 1, 17, 14],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        '-' => [0, 0, 0, 31, 0, 0, 0],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        '_' => [0, 0, 0, 0, 0, 0, 31],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        ' ' => [0; 7],
        _ => [0, 0, 4, 10, 4, 0, 0],
    }
}

fn chrome_pixels(
    width: i32,
    active: bool,
    maximized: bool,
    hover: Option<Button>,
    title: &str,
) -> Vec<u8> {
    let w = width.max(1).min(32767) as usize;
    let h = TITLEBAR_HEIGHT as usize;
    let mut out = vec![0u8; w * h * 4];
    // Rounded top corners use alpha, not an opaque bounding rectangle. The actual pixels
    // underneath remain visible via Smithay's compositor.
    let radius = flamewm_skin::recipes::window_chrome::WINDOW_CHROME
        .metrics
        .radius as i32;
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let corner = if y < radius && x < radius {
                Some((radius - 1 - x, radius - 1 - y))
            } else if y < radius && x >= w as i32 - radius {
                Some((x - (w as i32 - radius), radius - 1 - y))
            } else {
                None
            };
            let coverage = corner.map_or(1.0, |(dx, dy)| {
                let distance = ((dx as f32 + 0.5).powi(2) + (dy as f32 + 0.5).powi(2)).sqrt();
                (radius as f32 + 0.5 - distance).clamp(0.0, 1.0)
            });
            let alpha = (f32::from(SURFACE_ALPHA) * coverage).round() as u8;
            put(&mut out, w, h, x, y, SURFACE, alpha);
            if y == 0 || x == 0 || x == w as i32 - 1 {
                put(
                    &mut out,
                    w,
                    h,
                    x,
                    y,
                    BORDER,
                    (63.0 * coverage).round() as u8,
                );
            }
        }
    }
    let slot = chrome::controls_width();
    let glyph_edge = u32::from(chrome::WINDOW_CHROME.metrics.icon).clamp(4, 128);
    for geometry in
        chrome::control_button_geometries(chrome::SceneRect::new(0, 0, width, TITLEBAR_HEIGHT))
    {
        let role = match geometry.role {
            WindowControlRole::Maximize if maximized => WindowControlRole::Restore,
            role => role,
        };
        let button = match role {
            WindowControlRole::Close => Button::Close,
            WindowControlRole::Minimize => Button::Minimize,
            WindowControlRole::Maximize | WindowControlRole::Restore => Button::Maximize,
        };
        let center = geometry.bounds.x + geometry.bounds.width / 2;
        let cy = geometry.bounds.height / 2;
        if hover == Some(button) && button == Button::Close {
            // Hover is a disk like the X11 default; do not paint an opaque square over the
            // rounded semi-transparent titlebar. The glyph stays legible on red.
            let radius = glyph_edge as f32 / 2.0;
            for y in
                (cy - glyph_edge as i32 / 2).max(0)..(cy + glyph_edge as i32 / 2 + 1).min(h as i32)
            {
                for x in (center - glyph_edge as i32 / 2).max(0)
                    ..(center + glyph_edge as i32 / 2 + 1).min(w as i32)
                {
                    let dx = x as f32 + 0.5 - center as f32;
                    let dy = y as f32 + 0.5 - cy as f32;
                    let coverage = (radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
                    if coverage > 0.0 {
                        put(
                            &mut out,
                            w,
                            h,
                            x,
                            y,
                            CLOSE_HOVER,
                            (coverage * 255.0).round() as u8,
                        );
                    }
                }
            }
        }
        if let Some(glyph) = chrome::control_glyph_rgba(role, glyph_edge, GLYPH) {
            let start_x = center - glyph_edge as i32 / 2;
            let start_y = cy - glyph_edge as i32 / 2;
            for (index, rgba) in glyph.chunks_exact(4).enumerate() {
                let alpha = rgba[3];
                if alpha > 0 {
                    let x = start_x + (index % glyph_edge as usize) as i32;
                    let y = start_y + (index / glyph_edge as usize) as i32;
                    put(&mut out, w, h, x, y, GLYPH, alpha);
                }
            }
        }
    }
    // Stable, centered title, ellipsized before reaching the controls. Rasterised only when
    // text/size changes. The source atlas is deliberately tiny for safe nested startup.
    let right_occupied = width.saturating_sub(slot);
    let available = chrome::title_available_width(0, right_occupied, chrome::TITLE_PAD);
    let alpha = if active { 230 } else { 166 };
    let px = (TITLEBAR_HEIGHT as f32 * 0.42).max(8.0);
    let drawn = crate::font::with_glyphs(px, |glyph_of| {
        let visible = chrome::elide_title(title, available, |c| glyph_of(c).advance);
        let text_width: i32 = visible.chars().map(|c| glyph_of(c).advance).sum();
        let start_x =
            chrome::center_title_x(width, text_width, 0, right_occupied, chrome::TITLE_PAD);
        // Baseline places the x-height optically centred in the bar.
        let baseline = TITLEBAR_HEIGHT / 2 + (px * 0.36).round() as i32;
        let mut pen = start_x;
        for c in visible.chars() {
            let glyph = glyph_of(c);
            for (index, coverage) in glyph.coverage.iter().enumerate() {
                if *coverage == 0 || glyph.width == 0 {
                    continue;
                }
                let gx = pen + glyph.left + (index % glyph.width) as i32;
                let gy = baseline - glyph.top + (index / glyph.width) as i32;
                // Never paint into the controls strip, whatever the glyph overhang.
                if gx < right_occupied {
                    let a = (u32::from(*coverage) * u32::from(alpha) / 255) as u8;
                    put(&mut out, w, h, gx, gy, GLYPH, a);
                }
            }
            pen += glyph.advance;
        }
    });
    if drawn.is_none() {
        let visible_title = chrome::elide_title(title, available, |_| 6);
        let chars: Vec<char> = visible_title.chars().collect();
        let text_width = chars.len() as i32 * 6;
        let start_x =
            chrome::center_title_x(width, text_width, 0, right_occupied, chrome::TITLE_PAD);
        let baseline = (TITLEBAR_HEIGHT - 7) / 2;
        for (i, c) in chars.into_iter().enumerate() {
            for (y, row) in glyph(c).into_iter().enumerate() {
                for x in 0..5 {
                    if row & (1 << (4 - x)) != 0 {
                        put(
                            &mut out,
                            w,
                            h,
                            start_x + i as i32 * 6 + x,
                            baseline + y as i32,
                            GLYPH,
                            alpha,
                        );
                    }
                }
            }
        }
    }
    out
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

    let hovered_button = state.model.hit(state.pointer.x, state.pointer.y);
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
            let hovered = match hovered_button {
                Some(Hit::Button(hit, button)) if hit == id => Some(button),
                _ => None,
            };
            let title = state.window_title(id);
            if let Some(element) = decorations.element(
                renderer,
                id,
                titlebar,
                focused,
                managed.mode == Mode::Maximized,
                hovered,
                title,
            ) {
                out.push(element);
            }
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
    // winit's buffer_age() calls eglQuerySurface(EGL_BUFFER_AGE_EXT). On nested
    // software EGL (XWayland glamor unavailable) some drivers repeatedly emit
    // EGL_BAD_SURFACE for this query. Age zero is always correct: a complete
    // repaint avoids depending on preserved swapchain contents.
    let age = 0;
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

    #[test]
    fn chrome_uses_skin_height_and_rounded_alpha() {
        assert_eq!(TITLEBAR_HEIGHT, 31);
        let pixels = chrome_pixels(200, true, false, None, "FlameWM Terminal");
        assert_eq!(pixels.len(), 200 * 31 * 4);
        assert_eq!(pixels[3], 0, "outer corner must be transparent");
        assert!(pixels[(15 * 200 + 10) * 4 + 3] > 0);
    }

    #[test]
    fn close_hover_is_local_to_control_and_keeps_alpha_corners() {
        let idle = chrome_pixels(400, true, false, None, "Terminal");
        let hover = chrome_pixels(400, true, false, Some(Button::Close), "Terminal");
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
    fn maximize_glyph_tracks_real_window_state() {
        let normal = chrome_pixels(400, true, false, None, "Terminal");
        let maximized = chrome_pixels(400, true, true, None, "Terminal");
        assert_ne!(normal, maximized);
        assert_eq!(normal.len(), maximized.len());
    }

    #[test]
    fn narrow_frame_never_writes_outside_buffer() {
        for width in [1, 12, 38, 77, 114] {
            assert_eq!(
                chrome_pixels(width, true, false, Some(Button::Close), "Long window title").len(),
                width as usize * TITLEBAR_HEIGHT as usize * 4,
            );
        }
    }

    #[test]
    fn proportional_title_renders_unicode_inside_the_title_region_only() {
        let width = 320;
        let blank = chrome_pixels(width, true, false, None, "");
        let titled = chrome_pixels(width, true, false, None, "Terminal \u{2014} \u{dc}n\u{ef}code");
        let strip = (width - chrome::controls_width()) as usize;
        let mut changed = 0;
        for (index, (a, b)) in blank.chunks_exact(4).zip(titled.chunks_exact(4)).enumerate() {
            if a != b {
                changed += 1;
                assert!(index % width as usize <= strip, "title pixel inside controls strip");
            }
        }
        assert!(changed > 40, "real glyphs should cover many pixels, got {changed}");
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
        let first = chrome_pixels(400, true, false, None, "FlameWM Terminal");
        assert_eq!(
            first,
            chrome_pixels(400, true, false, None, "FlameWM Terminal")
        );
        assert_ne!(
            first,
            chrome_pixels(400, false, false, None, "FlameWM Terminal")
        );
        assert_ne!(
            first,
            chrome_pixels(400, true, false, Some(Button::Close), "FlameWM Terminal")
        );
        // Transparent body alpha is the same under every control at rest.
        let left = (15 * 400 + 200) * 4 + 3;
        let near_control = (15 * 400 + 365) * 4 + 3;
        assert!(first[left] > 0 && first[near_control] > 0);
    }
}

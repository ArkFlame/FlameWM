//! Window chrome: decoration cache, titlebar and outline rasterisation, corner geometry.

use super::*;
use crate::chrome_buttons::{self, Feedback};

#[derive(Default)]
pub struct Decorations {
    pub(super) parts: HashMap<WindowId, ChromeBuffer>,
    /// Blurred desktop layer (straight RGBA, output-sized) that translucent titlebars blend over.
    pub(super) desktop: Option<DesktopBackdrop>,
    /// True while a one-shot wake timer is armed to recapture the desktop after the throttle window.
    pub(super) desktop_wake_armed: bool,
    /// Output-sized GL texture the desktop capture renders into, kept between captures and replaced
    /// only when the output size changes. Resident while the decorations exist, about 4 bytes per output pixel.
    pub(super) scratch: Option<((usize, usize), GlesTexture)>,
    /// Blurred desktop under override-redirect `XWayland` surfaces, keyed by their rect. Cleared
    /// with the desktop capture it was cropped from.
    pub(super) surfaces: HashMap<(i32, i32, i32, i32), MemoryRenderBuffer>,
    /// Rounded-client clip program, compiled on first use. `Some(Err(()))` records the already
    /// logged failure so a broken shader is not recompiled for every frame or window.
    pub(super) clip: Option<Result<GlesTexProgram, ()>>,
    /// Resolved `windowOutlineColor` / `windowOutlineOpacity` / `windowOutlineWidth` / `outlines`,
    /// pushed from the shared settings owner. `None` until the first settings sync draws nothing.
    pub(super) outline: Option<OutlinePaint>,
    /// Ring strips per managed window from the shared rasteriser, rebuilt only on input change.
    pub(super) rings: HashMap<WindowId, OutlineRing>,
}

/// Resolved window outline colour, opacity and band width.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct OutlinePaint {
    pub(super) rgb: u32,
    pub(super) alpha: u8,
    pub(super) width: u32,
}

/// One uploaded ring strip: offset from the frame origin, size in pixels, and its buffer.
pub(super) struct OutlineStrip {
    pub(super) offset: (i32, i32),
    pub(super) size: (i32, i32),
    pub(super) buffer: MemoryRenderBuffer,
}

/// One window's cached outline: the inputs it was built from and the strips to draw.
pub(super) struct OutlineRing {
    pub(super) origin: (i32, i32),
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) radius: u32,
    pub(super) paint: OutlinePaint,
    /// How many times the strips below were rasterised (a cache hit keeps the count).
    pub(super) builds: u32,
    pub(super) strips: Vec<OutlineStrip>,
}

impl OutlineRing {
    /// Whether the cached strips still describe this window at these inputs.
    pub(super) fn matches(
        &self,
        origin: (i32, i32),
        size: (u32, u32),
        radius: u32,
        paint: OutlinePaint,
    ) -> bool {
        self.origin == origin
            && (self.width, self.height) == size
            && self.radius == radius
            && self.paint == paint
    }
}

/// Opaque ARGB8888 bytes (little-endian B, G, R, A) from straight RGBA pixels.
pub(super) fn opaque_argb_bytes(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4)
        .flat_map(|px| [px[2], px[1], px[0], 255])
        .collect()
}

/// Premultiplied ARGB8888 bytes (little-endian B, G, R, A) from straight RGBA pixels. The memory
/// element's texture is premultiplied, so a straight-alpha outline would show a bright fringe on
/// its antialiased arc.
pub(crate) fn premultiplied_argb_bytes(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4)
        .flat_map(|px| {
            let alpha = u32::from(px[3]);
            let scale = |channel: u8| ((u32::from(channel) * alpha + 127) / 255) as u8;
            [scale(px[2]), scale(px[1]), scale(px[0]), px[3]]
        })
        .collect()
}

pub(super) struct ChromeBuffer {
    pub(super) origin: (i32, i32),
    pub(super) width: i32,
    pub(super) active: bool,
    pub(super) maximized: bool,
    /// Corner-radius eligibility of the window's placement mode (see `Mode::rounded_frame`).
    pub(super) rounded: bool,
    /// Pointer feedback the cached pixels were drawn with.
    pub(super) feedback: Feedback,
    pub(super) title: String,
    /// Blurred desktop under the bar, kept so one control can be repainted without a new crop.
    pub(super) backdrop: Option<Vec<u8>>,
    pub(super) buffer: MemoryRenderBuffer,
}

impl ChromeBuffer {
    /// Repaints the controls whose hover or pressed state differs from the cached bar. Only those
    /// button rects are damaged; the rest of the bitmap and its damage bag are untouched.
    pub(super) fn repaint_feedback(&mut self, feedback: Feedback) {
        if self.feedback == feedback {
            return;
        }
        let (w, h) = (
            self.width.max(1).min(32767) as usize,
            TITLEBAR_HEIGHT as usize,
        );
        let slots = control_slots(self.width, self.maximized);
        let changed = [
            self.feedback.hover,
            self.feedback.pressed,
            feedback.hover,
            feedback.pressed,
        ];
        let rounded = self.rounded;
        let ChromeBuffer {
            buffer, backdrop, ..
        } = self;
        let mut ctx = buffer.render();
        for (index, target) in changed.iter().enumerate() {
            let Some(button) = *target else { continue };
            if changed[..index].contains(target) {
                continue;
            }
            let Some(mut slot) = slots.iter().find(|slot| slot.button == button).copied() else {
                continue;
            };
            // Narrow bars place controls past their right edge; damage and clear only the columns
            // the buffer owns, or the GL upload exceeds the texture.
            let Some((x0, x1)) = owned_span(slot.x0, slot.x1, w as i32) else {
                continue;
            };
            slot.x0 = x0;
            slot.x1 = x1;
            let rect = Rectangle::<i32, Buffer>::new((x0, 0).into(), ((x1 - x0), TITLEBAR_HEIGHT).into());
            let _ = ctx.draw(|mem| {
                repaint_slot(mem, w, h, rounded, backdrop.as_deref(), &slot, feedback);
                Ok::<_, std::convert::Infallible>(vec![rect])
            });
        }
        drop(ctx);
        self.feedback = feedback;
    }
}

/// Columns `x0..x1` clipped to a `w`-wide bar, or `None` when the bar owns none of them.
fn owned_span(x0: i32, x1: i32, w: i32) -> Option<(i32, i32)> {
    let (x0, x1) = (x0.clamp(0, w), x1.clamp(0, w));
    (x1 > x0).then_some((x0, x1))
}

/// One titlebar control: its button, glyph role and horizontal span in the bar.
#[derive(Clone, Copy)]
pub(super) struct ControlSlot {
    pub(super) button: Button,
    role: WindowControlRole,
    x0: i32,
    x1: i32,
    center: i32,
    cy: i32,
}

pub(super) fn control_slots(width: i32, maximized: bool) -> Vec<ControlSlot> {
    chrome::control_button_geometries(chrome::SceneRect::new(0, 0, width, TITLEBAR_HEIGHT))
        .into_iter()
        .map(|geometry| {
            let role = match geometry.role {
                WindowControlRole::Maximize if maximized => WindowControlRole::Restore,
                role => role,
            };
            let button = match role {
                WindowControlRole::Close => Button::Close,
                WindowControlRole::Minimize => Button::Minimize,
                WindowControlRole::Maximize | WindowControlRole::Restore => Button::Maximize,
            };
            ControlSlot {
                button,
                role,
                x0: geometry.bounds.x,
                x1: geometry.bounds.x + geometry.bounds.width,
                center: geometry.bounds.x + geometry.bounds.width / 2,
                cy: geometry.bounds.height / 2,
            }
        })
        .collect()
}

/// Opaque-over-backdrop titlebar background for columns `x0..x1` of a `w` x `h` bar.
#[allow(clippy::too_many_arguments)]
fn paint_background(
    out: &mut [u8],
    w: usize,
    h: usize,
    rounded: bool,
    backdrop: Option<&[u8]>,
    x0: i32,
    x1: i32,
) {
    // Rounded top corners use alpha, not an opaque bounding rectangle. The actual pixels
    // underneath remain visible via Smithay's compositor.
    let radius = titlebar_corner_radius(rounded, w as i32, h as i32);
    // Flat glass takes no blurred desktop under the fill: the opaque solid covers it anyway.
    let backdrop = if glass_flat() { None } else { backdrop };
    let (_, fill_alpha) = titlebar_fill();
    for y in 0..h as i32 {
        for x in x0.max(0)..x1.min(w as i32) {
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
            if let Some(back) = backdrop {
                let at = (y as usize * w + x as usize) * 4;
                let rgb = u32::from_be_bytes([0, back[at], back[at + 1], back[at + 2]]);
                put(out, w, h, x, y, rgb, (255.0 * coverage).round() as u8);
            }
            let alpha = (f32::from(fill_alpha) * coverage).round() as u8;
            put(out, w, h, x, y, current_titlebar_material(), alpha);
            if y == 0 || x == 0 || x == w as i32 - 1 {
                put(out, w, h, x, y, BORDER, (63.0 * coverage).round() as u8);
            }
        }
    }
}

/// Rebuilds one control's columns: clears them, lays the background, then draws the control.
/// Only these columns change; the rest of the bar is left as it was.
#[allow(clippy::too_many_arguments)]
fn repaint_slot(
    out: &mut [u8],
    w: usize,
    h: usize,
    rounded: bool,
    backdrop: Option<&[u8]>,
    slot: &ControlSlot,
    feedback: Feedback,
) {
    for y in 0..h {
        let row = y * w * 4;
        out[row + slot.x0 as usize * 4..row + slot.x1 as usize * 4].fill(0);
    }
    paint_background(out, w, h, rounded, backdrop, slot.x0, slot.x1);
    let glyph_edge = u32::from(chrome::WINDOW_CHROME.metrics.icon).clamp(4, 128);
    chrome_buttons::paint_control(
        out,
        w,
        h,
        slot.center,
        slot.cy,
        glyph_edge,
        slot.role,
        slot.button,
        feedback,
        crate::render::titlebar_button_shape(),
        (slot.x0, slot.x1),
    );
}

impl Decorations {
    /// Whether the cached desktop capture still matches the output and the desktop layer's commits.
    pub(super) fn desktop_current(&self, size: (usize, usize), commits: &[CommitCounter]) -> bool {
        self.desktop.as_ref().is_some_and(|d| {
            d.width == size.0 && d.height == size.1 && d.commits.as_slice() == commits
        })
    }

    /// Replaces the desktop capture. Titlebars and surface backdrops baked from the old one are rebuilt.
    pub(super) fn set_desktop(
        &mut self,
        size: (usize, usize),
        commits: Vec<CommitCounter>,
        pixels: Vec<u8>,
    ) {
        let (sw, sh) = stored_size(size);
        debug_assert_eq!(
            pixels.len(),
            sw * sh * 4,
            "capture must be stored at the reduced size"
        );
        self.parts.clear();
        self.surfaces.clear();
        self.desktop = Some(DesktopBackdrop {
            width: size.0,
            height: size.1,
            commits,
            pixels,
            captured_at: Instant::now(),
        });
    }

    /// Time left before the cached capture may be replaced, or `None` when a recapture may run
    /// now (no capture, output resized, or the capture is at least [`DESKTOP_RECAPTURE_INTERVAL`] old).
    pub(super) fn recapture_wait(&self, size: (usize, usize), now: Instant) -> Option<Duration> {
        let desktop = self.desktop.as_ref()?;
        if (desktop.width, desktop.height) != size {
            return None;
        }
        DESKTOP_RECAPTURE_INTERVAL
            .checked_sub(now.duration_since(desktop.captured_at))
            .filter(|wait| !wait.is_zero())
    }

    /// Blurred desktop under a `w` x `h` titlebar at `origin`, clamped to the output edges.
    /// `origin` and `w`/`h` are logical; each logical pixel samples the physical capture at `scale`.
    pub(super) fn crop(
        &self,
        origin: (i32, i32),
        w: usize,
        h: usize,
        scale: f64,
    ) -> Option<Vec<u8>> {
        let desktop = self.desktop.as_ref()?;
        let stored = stored_size((desktop.width, desktop.height));
        let stored_scale = DESKTOP_SCALE as f32;
        let scale = scale as f32;
        let mut out = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            // Output pixel centre mapped into stored space, clamped to the output edges.
            let oy = ((origin.1 + y as i32) as f32 * scale)
                .clamp(0.0, (desktop.height as i32 - 1) as f32);
            let fy = ((oy + 0.5) / stored_scale - 0.5).clamp(0.0, (stored.1 - 1) as f32);
            for x in 0..w {
                let ox = ((origin.0 + x as i32) as f32 * scale)
                    .clamp(0.0, (desktop.width as i32 - 1) as f32);
                let fx = ((ox + 0.5) / stored_scale - 0.5).clamp(0.0, (stored.0 - 1) as f32);
                out.extend_from_slice(&sample_bilinear(&desktop.pixels, stored, fx, fy));
            }
        }
        Some(out)
    }

    /// Frees the output-sized capture and everything baked from it, for an idle desktop.
    pub(super) fn drop_desktop(&mut self) {
        self.parts.clear();
        self.surfaces.clear();
        self.desktop = None;
    }

    pub(super) fn element(
        &mut self,
        renderer: &mut GlowRenderer,
        window: WindowId,
        rect: Rect,
        (active, maximized, rounded): (bool, bool, bool),
        feedback: Feedback,
        title: &str,
        scale: f64,
    ) -> Option<Elem> {
        // GL texture dimensions must match the allocated CPU surface, even for narrow frames.
        if rect.w <= 0 || rect.w > 32_767 {
            self.parts.remove(&window);
            return None;
        }
        let origin = (rect.x, rect.y);
        let same_geometry = self.parts.get(&window).is_some_and(|part| {
            part.origin == origin
                && part.width == rect.w
                && part.active == active
                && part.maximized == maximized
                && part.rounded == rounded
                && part.title == title
        });
        if same_geometry {
            // Only pointer feedback moved: repaint the affected controls in place.
            if let Some(part) = self.parts.get_mut(&window) {
                part.repaint_feedback(feedback);
            }
        } else {
            // The blurred desktop is cropped at the titlebar origin, so a moved titlebar is rebuilt.
            let backdrop = self.crop(origin, rect.w as usize, TITLEBAR_HEIGHT as usize, scale);
            let pixels = chrome_pixels_over(
                rect.w,
                active,
                maximized,
                rounded,
                feedback,
                title,
                backdrop.as_deref(),
            );
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
                    origin,
                    width: rect.w,
                    active,
                    maximized,
                    rounded,
                    feedback,
                    title: title.to_owned(),
                    backdrop,
                    buffer,
                },
            );
        }
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            Point::from((f64::from(rect.x) * scale, f64::from(rect.y) * scale)),
            &self.parts.get(&window)?.buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .ok()
        .map(Elem::Memory)
    }

    /// Blurred desktop under an override-redirect surface at `rect`. Placed beneath the surface,
    /// so only its translucent pixels show it; opaque pixels cover it.
    pub(super) fn surface_backdrop(
        &mut self,
        renderer: &mut GlowRenderer,
        rect: Rect,
        scale: f64,
    ) -> Option<Elem> {
        if rect.w <= 0 || rect.h <= 0 || rect.w > 32_767 || rect.h > 32_767 {
            return None;
        }
        let key = (rect.x, rect.y, rect.w, rect.h);
        if !self.surfaces.contains_key(&key) {
            // Bounded: a menu moved around leaves stale rects behind, so start over when full.
            if self.surfaces.len() >= 32 {
                self.surfaces.clear();
            }
            let pixels = self.crop((rect.x, rect.y), rect.w as usize, rect.h as usize, scale)?;
            let buffer = MemoryRenderBuffer::from_slice(
                &opaque_argb_bytes(&pixels),
                Fourcc::Argb8888,
                (rect.w, rect.h),
                1,
                Transform::Normal,
                None,
            );
            self.surfaces.insert(key, buffer);
        }
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            Point::from((f64::from(rect.x) * scale, f64::from(rect.y) * scale)),
            self.surfaces.get(&key)?,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .ok()
        .map(Elem::Memory)
    }

    pub(super) fn retain(&mut self, live: impl Fn(WindowId) -> bool) {
        self.parts.retain(|window, _| live(*window));
        self.rings.retain(|window, _| live(*window));
    }

    /// Drops every cached bar and outline ring. Used when the titlebar material or its ink changes,
    /// since both are baked into the cached pixels.
    pub fn invalidate_bars(&mut self) {
        self.parts.clear();
        self.rings.clear();
    }

    /// Records the resolved outline settings. Returns whether the value changed, so the caller
    /// only requests a redraw for a real settings change.
    pub fn set_outline(&mut self, rgb: u32, alpha: u8, width: u32) -> bool {
        let paint = OutlinePaint { rgb, alpha, width };
        if self.outline == Some(paint) {
            return false;
        }
        self.outline = Some(paint);
        // Every cached strip carries the colour and alpha, so all of them are stale now.
        self.rings.clear();
        true
    }

    /// Ring strips for `window`, rebuilt only when the frame, the radius or the outline settings
    /// changed. `None` when outlines are off (`outlines=false` or zero opacity).
    pub(super) fn ring_strips(
        &mut self,
        window: WindowId,
        frame: Rect,
        radius: u8,
    ) -> Option<&OutlineRing> {
        let paint = self.outline?;
        // GL texture bound, the same one the titlebar buffer is limited to.
        if paint.alpha == 0
            || paint.width == 0
            || frame.w <= 0
            || frame.h <= 0
            || frame.w > 32_767
            || frame.h > 32_767
        {
            return None;
        }
        let origin = (frame.x, frame.y);
        let size = (frame.w as u32, frame.h as u32);
        let radius = u32::from(radius);
        let stale = self
            .rings
            .get(&window)
            .is_none_or(|ring| !ring.matches(origin, size, radius, paint));
        if stale {
            let builds = self.rings.get(&window).map_or(0, |ring| ring.builds) + 1;
            let strips = outline::outline_strips(
                size.0,
                size.1,
                radius,
                paint.width,
                paint.rgb,
                paint.alpha,
            )
            .into_iter()
            .map(|(x, y, w, h, rgba)| OutlineStrip {
                offset: (x as i32, y as i32),
                size: (w as i32, h as i32),
                buffer: MemoryRenderBuffer::from_slice(
                    &premultiplied_argb_bytes(&rgba),
                    Fourcc::Argb8888,
                    (w as i32, h as i32),
                    1,
                    Transform::Normal,
                    None,
                ),
            })
            .collect();
            self.rings.insert(
                window,
                OutlineRing {
                    origin,
                    width: size.0,
                    height: size.1,
                    radius,
                    paint,
                    builds,
                    strips,
                },
            );
        }
        self.rings.get(&window)
    }

    /// The 1px window outline as overlay elements positioned at `frame`: the shared rasteriser's
    /// perimeter strips, rounded to the same physical radius as the client clip. Empty when the
    /// outline is off, so a disabled `outlines` setting costs no elements at all.
    pub(super) fn outline_elements(
        &mut self,
        renderer: &mut GlowRenderer,
        window: WindowId,
        frame: Rect,
        radius: u8,
        scale: f64,
    ) -> Vec<Elem> {
        let Some(ring) = self.ring_strips(window, frame, radius) else {
            return Vec::new();
        };
        ring.strips
            .iter()
            .filter_map(|strip| {
                MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    Point::from((
                        f64::from(frame.x.saturating_add(strip.offset.0)) * scale,
                        f64::from(frame.y.saturating_add(strip.offset.1)) * scale,
                    )),
                    &strip.buffer,
                    None,
                    None,
                    Some(Size::from(strip.size)),
                    Kind::Unspecified,
                )
                .ok()
                .map(Elem::Memory)
            })
            .collect()
    }

    /// The rounded-client clip program, compiled on first use.
    ///
    /// Fails open: a shader that does not compile logs once and every window keeps its square
    /// silhouette, which is exactly the pre-clip behaviour, instead of failing the frame.
    pub(super) fn clip_program(&mut self, renderer: &mut GlowRenderer) -> Option<GlesTexProgram> {
        let cached = self.clip.get_or_insert_with(|| {
            crate::clip::compile(renderer).map_err(|error| {
                eprintln!("flamewm-wayland: rounded client clip disabled: {error}");
            })
        });
        cached.clone().ok()
    }
}

/// Straight alpha composition into little-endian, premultiplied BGRA (DRM Argb8888).
pub(super) fn blend(px: &mut [u8], rgb: u32, alpha: u8) {
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

pub(crate) fn put(pixels: &mut [u8], width: usize, height: usize, x: i32, y: i32, rgb: u32, a: u8) {
    if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
        return;
    }
    let at = (y as usize * width + x as usize) * 4;
    blend(&mut pixels[at..at + 4], rgb, a);
}

/// Minimal 5x7 fallback glyphs used only for native titlebars. The X11 backend continues to
/// use IBM Plex/Xft; the compositor does not call X11 or a subprocess during frame painting.
pub(super) fn glyph(c: char) -> [u8; 7] {
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

/// Top-corner radius of a Wayland titlebar. The skin decoration owner supplies the system Rounded
/// radius for a window the placement policy rounds (see `Mode::rounded_frame`), scaled from the
/// logical titlebar to the physical titlebar height. Layout states stay square. Clamped to half
/// the width.
pub(super) fn titlebar_corner_radius(rounded: bool, width: i32, height: i32) -> i32 {
    scaled_corner_radius(rounded, DecorationStyle::Rounded, height).min(width.max(0) / 2)
}

/// Skin radius in physical pixels: the logical radius scaled from the logical titlebar metric to
/// a physical titlebar `physical_height` pixels tall, the same rounding as the X11 frame shape.
/// Zero for a squared decoration style and for windows outside the rounded-eligible set.
pub(super) fn scaled_corner_radius(
    floating: bool,
    style: DecorationStyle,
    physical_height: i32,
) -> i32 {
    let logical = u32::from(flamewm_skin::recipes::decoration::corner_radius(
        floating, style,
    ));
    let logical_h = u32::from(
        flamewm_skin::recipes::window_chrome::WINDOW_CHROME
            .metrics
            .titlebar,
    );
    let scaled = (logical * physical_height.max(0) as u32 + logical_h / 2) / logical_h.max(1);
    i32::try_from(scaled).unwrap_or(i32::MAX)
}

/// Per-corner radius of a managed window's client silhouette, in physical pixels, ordered
/// top left, top right, bottom right, bottom left — the clip shader's `corner_radius` order.
///
/// This is the X11 client shape expressed as a radius instead of an `XShape` mask: the titlebar
/// band above the client is straight, so a decorated window only rounds its bottom corners, while
/// a window without the band rounds all four. Maximized and fullscreen clients stay square
/// (`maximized_and_fullscreen_radius_zero`).
pub(super) fn client_corner_radii(managed: &Managed, titlebars: bool) -> [u8; 4] {
    client_corner_radii_for(
        managed.mode,
        titlebars,
        managed.rect.w,
        managed.rect.h,
        DecorationStyle::Rounded,
    )
}

/// [`client_corner_radii`] with the window dimensions and skin style passed in, so the policy is
/// testable without a `Managed` and so a squared decoration style is provably square.
pub(super) fn client_corner_radii_for(
    mode: Mode,
    titlebars: bool,
    width: i32,
    height: i32,
    style: DecorationStyle,
) -> [u8; 4] {
    // Only a floating window is rounded; every layout state is square, exactly as on X11.
    let floating = mode.rounded_frame();
    let band = titlebars && !matches!(mode, Mode::Fullscreen);
    let radius = scaled_corner_radius(floating, style, TITLEBAR_HEIGHT).max(0);
    // Each arc needs its own diameter of room: half the short side keeps four corners distinct,
    // while a banded client has only the bottom pair to fit.
    let room = if band {
        width.min(height)
    } else {
        width.min(height) / 2
    };
    let radius = u8::try_from(radius.min(room).min(i32::from(u8::MAX))).unwrap_or(u8::MAX);
    if band {
        [0, 0, radius, radius]
    } else {
        [radius; 4]
    }
}

/// Physical radius of the window outline ring: the same skin radius the client clip arcs use at
/// this compositor's physical titlebar height, so the ring arc and the clipped client corner are
/// concentric. Square for every layout state and for a squared decoration style; a radius of 0
/// makes the ring raster a plain rectangular border on all four edges.
pub(super) fn outline_radius(managed: &Managed) -> u8 {
    let radius = scaled_corner_radius(
        managed.mode.rounded_frame(),
        DecorationStyle::Rounded,
        TITLEBAR_HEIGHT,
    )
    .max(0);
    u8::try_from(radius.min(i32::from(u8::MAX))).unwrap_or(u8::MAX)
}

#[cfg(test)]
pub(super) fn chrome_pixels(
    width: i32,
    active: bool,
    mode: Mode,
    hover: Option<Button>,
    title: &str,
) -> Vec<u8> {
    chrome_pixels_over(
        width,
        active,
        mode == Mode::Maximized,
        mode.rounded_frame(),
        Feedback {
            hover,
            pressed: None,
        },
        title,
        None,
    )
}

/// Titlebar pixels. With `backdrop` (straight RGBA, one opaque pixel per titlebar pixel, the
/// blurred desktop beneath) the translucent fill composes over it, so the bar is opaque inside
/// its rounded shape and matches the X11 baked titlebar. `maximized` picks the control glyph,
/// `rounded` the corner arc: both come from the window's placement mode, never from each other.
pub(super) fn chrome_pixels_over(
    width: i32,
    active: bool,
    maximized: bool,
    rounded: bool,
    feedback: Feedback,
    title: &str,
    backdrop: Option<&[u8]>,
) -> Vec<u8> {
    let w = width.max(1).min(32767) as usize;
    let h = TITLEBAR_HEIGHT as usize;
    let mut out = vec![0u8; w * h * 4];
    paint_background(&mut out, w, h, rounded, backdrop, 0, w as i32);
    let glyph_edge = u32::from(chrome::WINDOW_CHROME.metrics.icon).clamp(4, 128);
    for slot in control_slots(width, maximized) {
        chrome_buttons::paint_control(
            &mut out,
            w,
            h,
            slot.center,
            slot.cy,
            glyph_edge,
            slot.role,
            slot.button,
            feedback,
            crate::render::titlebar_button_shape(),
            (slot.x0, slot.x1),
        );
    }
    // Stable, centered title, ellipsized before reaching the controls. Rasterised only when
    // text/size changes. The source atlas is deliberately tiny for safe nested startup.
    let slot = chrome::controls_width();
    let right_occupied = width.saturating_sub(slot);
    let available = chrome::title_available_width(0, right_occupied, chrome::TITLE_PAD);
    let alpha = if active { 230 } else { 166 };
    let ink_rgb = titlebar_ink().text.0;
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
                    put(&mut out, w, h, gx, gy, ink_rgb, a);
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
                            ink_rgb,
                            alpha,
                        );
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod hover_repaint_tests {
    use super::*;

    const W: i32 = 400;
    const H: usize = TITLEBAR_HEIGHT as usize;

    fn bar(feedback: Feedback) -> Vec<u8> {
        chrome_pixels_over(W, true, false, false, feedback, "Terminal", None)
    }

    #[test]
    fn hover_repaint_touches_only_the_button_rect() {
        let _guard = crate::render::GLASS_STATICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let rest = bar(Feedback::default());
        let slot = control_slots(W, false)
            .into_iter()
            .find(|slot| slot.button == Button::Minimize)
            .expect("minimize slot");
        let hover = Feedback {
            hover: Some(Button::Minimize),
            pressed: None,
        };

        let mut partial = rest.clone();
        repaint_slot(&mut partial, W as usize, H, false, None, &slot, hover);

        // Same result as a full rebuild with the hover state...
        assert_eq!(partial, bar(hover));
        // ...and every pixel outside the button's columns is byte-identical to the rest bar.
        for y in 0..H {
            for x in 0..W as usize {
                if (slot.x0..slot.x1).contains(&(x as i32)) {
                    continue;
                }
                let at = (y * W as usize + x) * 4;
                assert_eq!(
                    partial[at..at + 4],
                    rest[at..at + 4],
                    "pixel ({x},{y}) changed"
                );
            }
        }
    }

    #[test]
    fn moving_hover_between_buttons_repaints_both_back_to_match_full_rebuild() {
        let _guard = crate::render::GLASS_STATICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut pixels = bar(Feedback::default());
        let slots = control_slots(W, false);
        let step = |pixels: &mut Vec<u8>, from: Feedback, to: Feedback| {
            for slot in &slots {
                if [from.hover, from.pressed, to.hover, to.pressed].contains(&Some(slot.button)) {
                    repaint_slot(pixels, W as usize, H, false, None, slot, to);
                }
            }
        };
        let min = Feedback {
            hover: Some(Button::Minimize),
            pressed: None,
        };
        let close = Feedback {
            hover: Some(Button::Close),
            pressed: Some(Button::Close),
        };
        step(&mut pixels, Feedback::default(), min);
        step(&mut pixels, min, close);
        assert_eq!(pixels, bar(close));
    }

    #[test]
    fn repaint_feedback_on_cached_bar_skips_unchanged_bar() {
        let _guard = crate::render::GLASS_STATICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let rest = Feedback::default();
        let mut buffer = ChromeBuffer {
            origin: (0, 0),
            width: W,
            active: true,
            maximized: false,
            rounded: false,
            feedback: rest,
            title: "Terminal".to_owned(),
            backdrop: None,
            buffer: MemoryRenderBuffer::from_slice(
                &bar(rest),
                Fourcc::Argb8888,
                (W, TITLEBAR_HEIGHT),
                1,
                Transform::Normal,
                None,
            ),
        };
        // Same feedback: no-op, feedback stays put.
        buffer.repaint_feedback(rest);
        assert_eq!(buffer.feedback, rest);
        let hover = Feedback {
            hover: Some(Button::Close),
            pressed: None,
        };
        buffer.repaint_feedback(hover);
        assert_eq!(buffer.feedback, hover);
    }

    #[test]
    fn repaint_feedback_on_narrow_bar_stays_inside_buffer() {
        let _guard = crate::render::GLASS_STATICS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Narrower than the three controls: their spans start left of 0 or end past the bar.
        let narrow = 10;
        let rest = Feedback::default();
        let mut buffer = ChromeBuffer {
            origin: (0, 0),
            width: narrow,
            active: true,
            maximized: false,
            rounded: false,
            feedback: rest,
            title: String::new(),
            backdrop: None,
            buffer: MemoryRenderBuffer::from_slice(
                &chrome_pixels_over(narrow, true, false, false, rest, "", None),
                Fourcc::Argb8888,
                (narrow, TITLEBAR_HEIGHT),
                1,
                Transform::Normal,
                None,
            ),
        };
        for button in [Button::Close, Button::Minimize, Button::Maximize] {
            let feedback = Feedback {
                hover: Some(button),
                pressed: Some(button),
            };
            buffer.repaint_feedback(feedback);
            buffer.repaint_feedback(rest);
        }
        assert_eq!(buffer.feedback, rest);
    }

    #[test]
    fn owned_span_never_leaves_the_bar_texture() {
        // Every control on every narrow bar: the damaged columns must stay inside the texture.
        for w in 1..=64 {
            for slot in control_slots(w, false) {
                if let Some((x0, x1)) = owned_span(slot.x0, slot.x1, w) {
                    assert!(0 <= x0 && x0 < x1 && x1 <= w, "w={w} span={x0}..{x1}");
                }
            }
        }
        assert_eq!(owned_span(-22, 10, 10), Some((0, 10)));
        assert_eq!(owned_span(12, 40, 10), None);
    }
}

//! Frame chrome: pure scene plan plus native effect intents.
//!
//! Pure value objects. No X calls, no x11rb, no `get_geometry` reply in the
//! paint path. Live Xft/XRender/XShape handles stay inside
//! `flamewm-render-x11`'s [`ExternalDecorationRenderer`]; this module only
//! plans skin-exact values (J06 freeze) and describes effects.

use flamewm_render_core::outline::OutlineStrip;
use flamewm_skin::palette::Rgb;
use flamewm_skin::recipes::decoration::ChromeTokens;
use flamewm_skin::recipes::window_chrome::WINDOW_CHROME;
use flamewm_skin::typography::{FontWeight, Typography};
use flamewm_ui_core::surface::{SurfaceFill, blend_surface_over_rgb};

use super::model::FrameControl;

/// Canonical title typeface (skin `Typography::RWR_0_0_9`).
pub const TITLE_FONT_FAMILY: &str = Typography::RWR_0_0_9.family;
/// Canonical title size: 12px semibold.
/// Canonical title weight: semibold / 600.
pub const TITLE_FONT_WEIGHT: i32 = 600;

/// Pure chrome scene plan for one frame.
///
/// Carries geometry + asset intent for the live render: centered title
/// origin (`title_x` via skin `center_title_x`), the native `_NET_WM_ICON`
/// selection (`icon`, pure/cached asset only, no catalog lookup), its skin
/// destination (`icon_dest`).
#[derive(Debug, Clone, PartialEq)]
pub struct ChromeScene {
    pub frame_w: u32,
    pub frame_h: u32,
    pub titlebar_height: u16,
    pub title: String,
    pub title_family: &'static str,
    pub title_size_px: u16,
    pub title_weight: FontWeight,
    pub title_color: (u8, u8, u8),
    pub bg: (u8, u8, u8),
    /// Straight 8-bit alpha of the titlebar fill (`surfaceOpacity`). Text stays opaque.
    pub bg_alpha: u8,
    pub title_width: i32,
    pub title_x: f32,
    pub icon_present: bool,
    pub icon: Option<crate::chrome::IconImage>,
    pub icon_dest: Option<(f32, f32, f32)>,
    pub icon_edge: u16,
    pub baseline: f32,
    pub hover: Option<FrameControl>,
    pub pressed: Option<FrameControl>,
    pub active: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub floating: bool,
    pub radius: u16,
    pub tokens: ChromeTokens,
}

/// Plan a chrome scene from frame inputs using SKIN values and style tokens.
/// Only a `floating` window gets the rounded mask.
#[must_use]
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub fn plan_scene(
    frame_w: u32,
    frame_h: u32,
    title: &str,
    active: bool,
    hover: Option<FrameControl>,
    pressed: Option<FrameControl>,
    maximized: bool,
    fullscreen: bool,
    floating: bool,
    tokens: &ChromeTokens,
) -> ChromeScene {
    plan_scene_with_titlebar(
        frame_w,
        frame_h,
        WINDOW_CHROME.metrics.titlebar,
        title,
        active,
        hover,
        pressed,
        maximized,
        fullscreen,
        floating,
        tokens,
    )
}

/// The physical titlebar height is the same frame-extents metric passed to
/// native child placement and ICCCM/EWMH publishing.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn plan_scene_with_titlebar(
    frame_w: u32,
    frame_h: u32,
    titlebar_height: u16,
    title: &str,
    active: bool,
    hover: Option<FrameControl>,
    pressed: Option<FrameControl>,
    maximized: bool,
    fullscreen: bool,
    floating: bool,
    tokens: &ChromeTokens,
) -> ChromeScene {
    let title_style = Typography::RWR_0_0_9.title;
    let bg = tokens.titlebar_bg;
    let text = tokens.ink().text;
    let radius =
        super::geometry_shape::scaled_corner_radius(floating, tokens.style, titlebar_height);
    let logical_h = u32::from(WINDOW_CHROME.metrics.titlebar);
    let scaled = |value: u32| (value * u32::from(titlebar_height) + logical_h / 2) / logical_h;
    let available = crate::chrome::live_title_available_width(frame_w, titlebar_height);
    let title = flamewm_skin::recipes::window_chrome::elide_title(
        title,
        available.saturating_mul(i32::from(WINDOW_CHROME.metrics.titlebar))
            / i32::from(titlebar_height.max(1)),
        |ch| crate::chrome::title_text_width(&ch.to_string()),
    );
    let title_width = i32::try_from(scaled(crate::chrome::title_text_width(&title).max(0) as u32))
        .unwrap_or(i32::MAX);
    let title_x = crate::chrome::live_title_x_for_height(frame_w, title_width, titlebar_height);
    ChromeScene {
        frame_w,
        frame_h,
        titlebar_height,
        title,
        title_family: Typography::RWR_0_0_9.family,
        title_size_px: u16::try_from(scaled(u32::from(title_style.size_px)))
            .unwrap_or(u16::MAX)
            .max(1),
        title_weight: title_style.weight,
        title_color: rgb_channels(text),
        bg: rgb_channels(bg),
        bg_alpha: tokens.titlebar_alpha,
        title_width,
        title_x,
        icon_present: false,
        icon: None,
        icon_dest: Some(crate::chrome::icon_dest_rect(titlebar_height)),
        icon_edge: crate::chrome::icon_slot_for(titlebar_height),
        baseline: (f32::from(titlebar_height) - f32::from(u16::try_from(scaled(11)).unwrap_or(11)))
            .max(11.0),
        hover,
        pressed,
        active,
        maximized,
        fullscreen,
        floating,
        radius,
        tokens: *tokens,
    }
}

fn rgb_channels(color: Rgb) -> (u8, u8, u8) {
    (
        ((color.0 >> 16) & 0xff) as u8,
        ((color.0 >> 8) & 0xff) as u8,
        (color.0 & 0xff) as u8,
    )
}

/// Attach the resolved native `_NET_WM_ICON` selection to a planned scene.
/// Pure/cached asset only: the raster is scaled at paint time into the skin
/// icon slot. No application catalog lookup.
#[must_use]
pub fn with_icon(mut scene: ChromeScene, icon: Option<crate::chrome::IconImage>) -> ChromeScene {
    scene.icon_present = icon.is_some();
    scene.icon = icon;
    scene
}

/// Native effect intents executed against [`ExternalDecorationRenderer`].
/// Descriptions only; pure fns never touch a live X connection.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub enum ChromeEffect {
    Retarget {
        frame_xid: u64,
        w: u32,
        h: u32,
    },
    FillTitlebar {
        w: u32,
        h: u32,
    },
    DrawTitle {
        font: &'static str,
        size_px: f32,
        weight: i32,
        x: f32,
        baseline: f32,
        text: String,
    },
    BlitIcon,
    BlitControls {
        hover: Option<FrameControl>,
        pressed: Option<FrameControl>,
    },
    PaintOutline {
        alpha: u8,
        radius: u32,
    },
    ApplyShape {
        radius: u32,
    },
    Flush,
}

/// Pure effect list for a scene. Nonempty title always yields `DrawTitle`;
/// empty title may skip it. No `get_geometry`, no x11rb.
#[cfg(test)]
#[must_use]
pub fn effects_for(frame_xid: u64, scene: &ChromeScene) -> Vec<ChromeEffect> {
    let mut effects = Vec::new();
    effects.push(ChromeEffect::Retarget {
        frame_xid,
        w: scene.frame_w,
        h: scene.frame_h,
    });
    effects.push(ChromeEffect::FillTitlebar {
        w: scene.frame_w,
        h: u32::from(scene.titlebar_height),
    });
    if !scene.title.is_empty() {
        effects.push(ChromeEffect::DrawTitle {
            font: TITLE_FONT_FAMILY,
            size_px: f32::from(scene.title_size_px),
            weight: TITLE_FONT_WEIGHT,
            x: scene.title_x,
            baseline: scene.baseline,
            text: scene.title.clone(),
        });
    }
    if scene.icon_present {
        effects.push(ChromeEffect::BlitIcon);
    }
    effects.push(ChromeEffect::BlitControls {
        hover: scene.hover,
        pressed: scene.pressed,
    });
    if outline_on(scene) {
        effects.push(ChromeEffect::PaintOutline {
            alpha: scene.tokens.surface_outline_alpha,
            radius: u32::from(scene.radius),
        });
    }
    effects.push(ChromeEffect::ApplyShape {
        radius: u32::from(scene.radius),
    });
    effects.push(ChromeEffect::Flush);
    effects
}

/// Opaque RGBA titlebar fill for `scene`: the surface colour over the desktop layer.
fn titlebar_fill_pixels(
    scene: &ChromeScene,
    height: u32,
    origin: (i32, i32),
    root: (u32, u32),
) -> Vec<u8> {
    super::titlebar_backdrop::titlebar_pixels(
        origin,
        root,
        (scene.frame_w, height),
        scene.bg,
        scene.bg_alpha,
        scene.tokens.titlebar_blur_percent,
        u32::from(scene.radius),
    )
}

/// One outline blit over the frame: `(x, y, width, height, rgba)`, the shared strip contract.
pub type OutlineBlit = OutlineStrip;

/// The client's own outermost pixels, one column/row thick, straight RGBA. Read back from the
/// client window so the ring composites over what the window actually drew instead of over the
/// frame's material when no compositing manager can blend an overlay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientEdges {
    /// `1 x client_h` column of the client's left edge.
    pub left: Vec<u8>,
    /// `1 x client_h` column of the client's right edge.
    pub right: Vec<u8>,
    /// `client_w x 1` row of the client's bottom edge.
    pub bottom: Vec<u8>,
}

impl ClientEdges {
    fn column(pixels: &[u8], row: usize) -> Option<[u8; 3]> {
        let at = row.checked_mul(4)?;
        let px = pixels.get(at..at + 4)?;
        Some([px[0], px[1], px[2]])
    }

    fn pixel(pixels: &[u8], col: usize) -> Option<[u8; 3]> {
        Self::column(pixels, col)
    }
}

/// The outline ring exists for this scene: enabled token with a real band, real 2x2+ frame.
fn outline_on(scene: &ChromeScene) -> bool {
    scene.tokens.surface_outline_alpha > 0
        && scene.tokens.surface_outline_width > 0
        && scene.frame_w >= 2
        && scene.frame_h >= 2
}

/// One smoothed antialiased ring (`windowOutlineWidth` px) around the whole frame.
///
/// The pixels are the shared [`flamewm_render_core::outline`] strips, unchanged: the same bytes
/// Wayland paints, so X11 never re-derives the arc or its coverage. Only the composite base
/// differs per backend. The ring colour is `surface_outline_rgb` at `surface_outline_alpha`, and
/// each pixel's alpha stays `ring_alpha(alpha, coverage)` as produced by the shared raster.
///
/// Base rule, documented because it changes the bytes:
/// * titlebar rows already hold the surface material, so their strips are blitted as-is and the
///   ring composites onto that material exactly like the Wayland overlay does.
/// * rows below the titlebar band sit on the client, so each strip builds its own material base
///   with [`super::titlebar_backdrop::titlebar_pixels`] (square corners: it is a material fill,
///   not a shape mask) and pre-blends the ring colour into it, keeping the ring alpha. Blitting
///   that over the client fades the arc out of the window material instead of out of the frame's
///   `background_pixel`, which would leave a dark fringe around every arc.
#[must_use]
pub fn outline_blits(
    scene: &ChromeScene,
    origin: (i32, i32),
    root: (u32, u32),
    client_edges: Option<&ClientEdges>,
) -> Vec<OutlineBlit> {
    if !outline_on(scene) {
        return Vec::new();
    }
    let titlebar_h = u32::from(scene.titlebar_height);
    let fill = SurfaceFill {
        rgb: scene.tokens.surface_outline_rgb.0,
        alpha: scene.tokens.surface_outline_alpha,
        width: scene.tokens.surface_outline_width,
    };
    flamewm_render_core::outline::outline_strips(
        scene.frame_w,
        scene.frame_h,
        u32::from(scene.radius),
        fill.width,
        fill.rgb,
        fill.alpha,
    )
    .into_iter()
    .map(|(x, y, w, h, mut rgba)| {
        if y + h > titlebar_h {
            let base = super::titlebar_backdrop::titlebar_pixels(
                (origin.0 + x as i32, origin.1 + y as i32),
                root,
                (w, h),
                scene.bg,
                scene.bg_alpha,
                scene.tokens.titlebar_blur_percent,
                0,
            );
            let content_rows = titlebar_h.saturating_sub(y) as usize * w as usize;
            for (index, (pixel, material)) in rgba
                .chunks_exact_mut(4)
                .zip(base.chunks_exact(4))
                .enumerate()
                .skip(content_rows)
            {
                if pixel[3] == 0 {
                    continue;
                }
                // Prefer the client's own edge pixel: the ring then blends over the window's final
                // pixels, exactly like the Wayland overlay does, without a compositing manager.
                // Strip-local indices are mapped back into frame coordinates first.
                let local_x = x + (index % w as usize) as u32;
                let local_y = y + index as u32 / w;
                let client_px = client_edges.and_then(|edges| {
                    let row = local_y.saturating_sub(titlebar_h) as usize;
                    if local_x == 0 {
                        ClientEdges::column(&edges.left, row)
                    } else if local_x + 1 == scene.frame_w {
                        ClientEdges::column(&edges.right, row)
                    } else if local_y + 1 == scene.frame_h {
                        ClientEdges::pixel(&edges.bottom, local_x as usize)
                    } else {
                        None
                    }
                });
                let base_rgb = match client_px {
                    Some([r, g, b]) => u32::from_be_bytes([0, r, g, b]),
                    None => u32::from_be_bytes([0, material[0], material[1], material[2]]),
                };
                let blended = blend_surface_over_rgb(
                    base_rgb,
                    SurfaceFill {
                        rgb: fill.rgb,
                        alpha: pixel[3],
                        width: 1,
                    },
                );
                pixel[0] = (blended >> 16) as u8;
                pixel[1] = (blended >> 8) as u8;
                pixel[2] = blended as u8;
                // The base is already in these channels, so the result is opaque: keeping the
                // coverage as alpha would blend the ring over the frame's background a second
                // time and darken it. Arc pixels stay antialiased because the blend itself is
                // coverage-weighted.
                pixel[3] = 255;
            }
        }
        (x, y, w, h, rgba)
    })
    .collect()
}

/// Execute a scene against the renderer. Passes 12.0/600 to `draw_title`;
/// blits the native icon raster and the cached control glyphs at exact
/// skin control geometry (38px buttons, 16px glyphs, icon slot at 6px
/// left); title is centered with skin `center_title_x`. Pure/cached
/// assets only: no application catalog lookup.
pub fn render(
    renderer: &mut flamewm_render_x11::ExternalDecorationRenderer,
    frame_xid: u64,
    scene: &ChromeScene,
    origin: (i32, i32),
    root: (u32, u32),
    paint_ring: bool,
    client_edges: Option<&ClientEdges>,
) -> Result<(), String> {
    use flamewm_render_core::{Color, Rect};
    // Order is the live paint contract: retarget, fill, icon blit, title
    // draw, control blits, outline ring, shape, flush.
    // Each fallible paint step's result is deferred (not `?`-early-returned)
    // so the title style params still reach the draw-target contract
    // (`draw_title` records them before touching the display) even on
    // headless hosts where retarget/fill already failed. Live behavior is
    // unchanged: any failure is still returned as an error.
    let retarget_result = {
        let _guard = flamewm_profiler::start("wm.chrome.render.retarget");
        renderer.retarget(frame_xid, scene.frame_w, scene.frame_h)
    };
    let titlebar_h = f32::from(scene.titlebar_height);
    // No compositor: the surface material (`surfaceColor` @ `surfaceOpacity` over the blurred
    // wallpaper behind the frame) is baked into opaque pixels and blitted. An opaque blit needs
    // no XRender, so hosts without it paint the same fill.
    let fill_result = {
        let _guard = flamewm_profiler::start("wm.chrome.render.fill");
        let pixels_guard = flamewm_profiler::start("wm.chrome.render.fill.pixels");
        let mut pixels = titlebar_fill_pixels(scene, titlebar_h as u32, origin, root);
        let plates = crate::chrome::control_backplates_for(
            scene.frame_w,
            scene.titlebar_height,
            scene.radius,
            scene.hover,
            scene.pressed,
            &scene.tokens,
        );
        crate::chrome::composite_control_backplates(
            &mut pixels,
            scene.frame_w,
            scene.titlebar_height,
            &plates,
        );
        drop(pixels_guard);
        let _blit_guard = flamewm_profiler::start("wm.chrome.render.fill.blit");
        renderer.blit_rgba(
            &pixels,
            scene.frame_w,
            titlebar_h as u32,
            Rect {
                x: 0.0,
                y: 0.0,
                width: scene.frame_w as f32,
                height: titlebar_h,
            },
        )
    };
    let _icon_guard = flamewm_profiler::start("wm.chrome.render.icon");
    let icon_result = match (&scene.icon, scene.icon_dest) {
        (Some(icon), Some((dest_x, dest_y, dest_edge))) => {
            match crate::chrome::native_icon_rgba(icon, u32::from(scene.icon_edge)) {
                Some(raster) => renderer.blit_rgba(
                    &raster.pixels,
                    raster.width,
                    raster.height,
                    Rect {
                        x: dest_x,
                        y: dest_y,
                        width: dest_edge,
                        height: dest_edge,
                    },
                ),
                None => Ok(()),
            }
        }
        _ => Ok(()),
    };
    let _title_guard = flamewm_profiler::start("wm.chrome.render.title");
    let draw_result = if scene.title.is_empty() {
        Ok(())
    } else {
        renderer.draw_title(
            &scene.title,
            TITLE_FONT_FAMILY,
            f32::from(scene.title_size_px),
            TITLE_FONT_WEIGHT,
            scene.title_x,
            scene.baseline,
            Color {
                r: scene.title_color.0,
                g: scene.title_color.1,
                b: scene.title_color.2,
                a: 255,
            },
        )
    };
    let _controls_guard = flamewm_profiler::start("wm.chrome.render.controls");
    let controls_result = crate::chrome::control_blits_for_height(
        scene.frame_w,
        scene.titlebar_height,
        scene.maximized,
        scene.fullscreen,
        scene.hover,
        scene.pressed,
        &scene.tokens,
    )
    .into_iter()
    .try_for_each(|blit| {
        renderer.blit_rgba(
            &blit.raster.pixels,
            blit.raster.width,
            blit.raster.height,
            Rect {
                x: blit.dest_x,
                y: blit.dest_y,
                width: blit.dest_w,
                height: blit.dest_h,
            },
        )
    });
    // One outline owner: the shared ring strips, painted last over the finished frame pixels
    // (titlebar fill, icon, title, controls) and before the shape mask clips the corners.
    let _outline_guard = flamewm_profiler::start("wm.chrome.render.outline");
    // A compositing manager hosts the ring as an ARGB overlay over the client's own pixels, so
    // the frame band must stay clear of it: one ring owner per frame.
    let outline_result = if paint_ring {
        outline_blits(scene, origin, root, client_edges)
    } else {
        Vec::new()
    }
    .into_iter()
    .try_for_each(|(x, y, w, h, pixels)| {
        renderer.blit_rgba(
            &pixels,
            w,
            h,
            Rect {
                x: x as f32,
                y: y as f32,
                width: w as f32,
                height: h as f32,
            },
        )
    });
    let shape_guard = flamewm_profiler::start("wm.chrome.render.shape");
    let shape_result = renderer.apply_shape(u32::from(scene.radius));
    drop(shape_guard);
    let _flush_guard = flamewm_profiler::start("wm.chrome.render.flush");
    renderer.flush();
    retarget_result?;
    fill_result?;
    icon_result?;
    draw_result?;
    controls_result?;
    outline_result?;
    shape_result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_skin::palette::PANEL;

    /// Straight-alpha ring painted onto a zeroed frame buffer: the raw blit bytes.
    fn ringed(scene: &ChromeScene, origin: (i32, i32), root: (u32, u32)) -> (Vec<u8>, u32) {
        ringed_with(scene, origin, root, None)
    }

    fn ringed_with(
        scene: &ChromeScene,
        origin: (i32, i32),
        root: (u32, u32),
        edges: Option<&ClientEdges>,
    ) -> (Vec<u8>, u32) {
        let (w, h) = (scene.frame_w, scene.frame_h);
        let mut frame = vec![0_u8; w as usize * h as usize * 4];
        for (x, y, bw, bh, pixels) in outline_blits(scene, origin, root, edges) {
            for row in 0..bh as usize {
                let src = row * bw as usize * 4;
                let dst = ((y as usize + row) * w as usize + x as usize) * 4;
                let len = bw as usize * 4;
                frame[dst..dst + len].copy_from_slice(&pixels[src..src + len]);
            }
        }
        (frame, w)
    }

    fn pixel_at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let at = (y as usize * width as usize + x as usize) * 4;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    }

    /// Full-opacity white ring: any pre-blend is visible as a non-white channel.
    fn white_outline() -> ChromeTokens {
        ChromeTokens {
            surface_outline_rgb: Rgb(0x00ff_ffff),
            surface_outline_alpha: 255,
            ..ChromeTokens::default()
        }
    }

    /// With the client's own edge pixels available the ring composites over them, so the band
    /// pixel is the outline over the window's final pixels (what the user sees in the window)
    /// instead of over the frame's material. Missing sides fall back per side.
    #[test]
    fn outline_ring_blends_over_the_clients_own_edge_pixels() {
        let scene = plan_scene(
            120,
            100,
            "app",
            true,
            None,
            None,
            false,
            false,
            true,
            &ChromeTokens::default(),
        );
        let (titlebar, frame_h) = (
            u32::from(scene.titlebar_height) as usize,
            scene.frame_h as usize,
        );
        let client_h = frame_h - titlebar;
        // Client edges are solid green; the frame material is the default surface colour.
        let green = [0u8, 255, 0, 255].repeat(client_h);
        let bottom = [0u8, 255, 0, 255].repeat(scene.frame_w as usize);
        let edges = ClientEdges {
            left: green.clone(),
            right: green,
            bottom,
        };
        let (frame, w) = ringed_with(&scene, (0, 0), (800, 600), Some(&edges));
        let opacity = scene.tokens.surface_outline_alpha;
        let mid = (frame_h / 2) as u32;
        let alpha = pixel_at(&frame, w, 0, mid)[3];
        assert!(alpha > 0, "the left band carries the ring");
        // 50% white over full green: red and blue rise from 0 to ~127, green stays at 255.
        let px = pixel_at(&frame, w, 0, mid);
        let half = u32::from(opacity);
        let expect_red = ((255 * half + 127) / 255) as u8;
        assert_eq!(
            px[0], expect_red,
            "red channel = outline over the client pixel"
        );
        assert_eq!(
            px[2], expect_red,
            "blue channel = outline over the client pixel"
        );
        assert_eq!(px[1], 255, "green channel keeps the client's own value");
        // The bottom edge blends over the client too.
        let px = pixel_at(&frame, w, w / 2, (frame_h - 1) as u32);
        assert_eq!(px[0], expect_red);
        // The titlebar rows keep the material base: no client edge there.
        let px = pixel_at(&frame, w, 0, 1);
        assert_ne!(px[1], 255, "titlebar rows are not client pixels");
    }

    #[test]
    fn outline_ring_covers_the_whole_frame_with_shared_coverage() {
        use flamewm_render_core::outline::{ring_alpha, ring_coverage};
        let scene = plan_scene(
            120,
            100,
            "app",
            true,
            None,
            None,
            false,
            false,
            true,
            &ChromeTokens::default(),
        );
        let (frame, w) = ringed(&scene, (0, 0), (800, 600));
        let (h, radius) = (scene.frame_h, u32::from(scene.radius));
        let thickness = scene.tokens.surface_outline_width;
        let opacity = scene.tokens.surface_outline_alpha;
        // Titlebar rows carry the shared alpha (the bar holds the material); client rows bake
        // their base, so they are either absent (coverage 0) or opaque.
        let titlebar = u32::from(scene.titlebar_height);
        for y in 0..h {
            for x in 0..w {
                let coverage = ring_coverage(w, h, radius, thickness, x, y);
                let alpha = pixel_at(&frame, w, x, y)[3];
                if y < titlebar {
                    assert_eq!(alpha, ring_alpha(opacity, coverage), "{x},{y}");
                } else {
                    let expected = if coverage == 0 { 0 } else { 255 };
                    assert_eq!(alpha, expected, "{x},{y}");
                }
            }
        }
        // One continuous ring around titlebar AND client: all four sides.
        for (x, y) in [(w / 2, 0), (w / 2, h - 1), (0, h / 2), (w - 1, h / 2)] {
            assert!(pixel_at(&frame, w, x, y)[3] > 0, "no ring at {x},{y}");
        }
        // The arcs stay antialiased instead of a stair-stepped mask edge.
        let full = ring_alpha(opacity, 255);
        let partial = (0..u32::from(scene.titlebar_height))
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let alpha = pixel_at(&frame, w, x, y)[3];
                alpha > 0 && alpha < full
            })
            .count();
        assert!(
            partial > 3,
            "titlebar arcs must be antialiased, got {partial}"
        );
        // Client-row arcs blend through the base channels instead (opaque, not solid white).
        let blended = (u32::from(scene.titlebar_height)..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let pixel = pixel_at(&frame, w, x, y);
                pixel[3] == 255 && pixel[..3] != [0xff, 0xff, 0xff]
            })
            .count();
        assert!(blended > 3, "client arcs must blend, got {blended}");
        // Interior and the space outside the ideal arc stay untouched.
        assert_eq!(pixel_at(&frame, w, 60, 50), [0, 0, 0, 0]);
        assert_eq!(pixel_at(&frame, w, 0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel_at(&frame, w, 0, h - 1), [0, 0, 0, 0]);
    }

    #[test]
    fn outline_pre_blends_client_rows_into_the_surface_material() {
        let scene = plan_scene(
            120,
            100,
            "app",
            true,
            None,
            None,
            false,
            false,
            true,
            &white_outline(),
        );
        let (frame, w) = ringed(&scene, (0, 0), (800, 600));
        let titlebar_h = u32::from(scene.titlebar_height);
        // Straight edges and full coverage keep the configured ring colour exactly.
        assert_eq!(pixel_at(&frame, w, 60, 0), [0xff, 0xff, 0xff, 255]);
        assert_eq!(pixel_at(&frame, w, 0, 50), [0xff, 0xff, 0xff, 255]);
        // Titlebar rows carry the material in their own fill: the ring stays raw there.
        let (tx, ty) = (0..titlebar_h.min(24))
            .flat_map(|x| (0..24).map(move |y| (x, y)))
            .find(|&(x, y)| {
                let alpha = pixel_at(&frame, w, x, y)[3];
                alpha > 0 && alpha < 255
            })
            .expect("an antialiased titlebar-row pixel");
        assert_eq!(&pixel_at(&frame, w, tx, ty)[..3], &[0xff, 0xff, 0xff]);
        // Client rows carry their base in the channels, so they are opaque and blend out of the
        // window material toward the ring colour instead of fading over the frame background.
        let (cx, cy) = (0..w)
            .flat_map(|x| (titlebar_h..scene.frame_h).map(move |y| (x, y)))
            .find(|&(x, y)| {
                let pixel = pixel_at(&frame, w, x, y);
                pixel[3] == 255 && pixel[..3] != [0xff, 0xff, 0xff]
            })
            .expect("an antialiased client-row pixel");
        let pixel = pixel_at(&frame, w, cx, cy);
        let material = super::super::titlebar_backdrop::titlebar_pixels(
            (cx as i32, cy as i32),
            (800, 600),
            (1, 1),
            scene.bg,
            scene.bg_alpha,
            scene.tokens.titlebar_blur_percent,
            0,
        );
        assert_eq!(pixel[3], 255, "client-row pixels are opaque");
        for (channel, base) in pixel[..3].iter().zip(&material) {
            assert!(
                *channel > *base,
                "faded toward the ring, out of the window material"
            );
        }
    }

    #[test]
    fn outline_emits_nothing_when_disabled_or_degenerate() {
        let tokens = ChromeTokens {
            surface_outline_alpha: 0,
            ..ChromeTokens::default()
        };
        let scene = plan_scene(
            120, 100, "app", true, None, None, false, false, true, &tokens,
        );
        assert!(outline_blits(&scene, (0, 0), (800, 600), None).is_empty());
        assert!(
            !effects_for(1, &scene)
                .iter()
                .any(|effect| matches!(effect, ChromeEffect::PaintOutline { .. }))
        );
        let tiny = plan_scene(
            1,
            100,
            "app",
            true,
            None,
            None,
            false,
            false,
            true,
            &ChromeTokens::default(),
        );
        assert!(outline_blits(&tiny, (0, 0), (800, 600), None).is_empty());
    }

    #[test]
    fn outline_paints_after_controls_and_before_the_shape_mask() {
        let scene = plan_scene(
            120,
            100,
            "a",
            true,
            None,
            None,
            false,
            false,
            true,
            &ChromeTokens::default(),
        );
        let effects = effects_for(1, &scene);
        let outline = effects.iter().position(|effect| {
            matches!(
                effect,
                ChromeEffect::PaintOutline { alpha, radius }
                    if *alpha == scene.tokens.surface_outline_alpha
                        && *radius == u32::from(scene.radius)
            )
        });
        let controls = effects
            .iter()
            .position(|effect| matches!(effect, ChromeEffect::BlitControls { .. }));
        let shape = effects
            .iter()
            .position(|effect| matches!(effect, ChromeEffect::ApplyShape { .. }));
        let (Some(controls), Some(outline), Some(shape)) = (controls, outline, shape) else {
            panic!("missing paint effect: {effects:?}");
        };
        assert!(
            controls < outline,
            "ring paints over the final control pixels"
        );
        assert!(
            outline < shape,
            "ring paints before the shape clips the arcs"
        );
    }

    #[test]
    fn titlebar_fill_no_longer_bakes_a_contour() {
        let tokens = ChromeTokens {
            surface_outline_alpha: 255,
            ..ChromeTokens::default()
        };
        let scene = plan_scene(
            120, 100, "app", true, None, None, false, false, true, &tokens,
        );
        let fill = titlebar_fill_pixels(&scene, 31, (0, 0), (800, 600));
        // The material, not the outline colour: the ring is a separate blit now.
        let material = super::super::titlebar_backdrop::titlebar_pixels(
            (0, 0),
            (800, 600),
            (120, 31),
            scene.bg,
            scene.bg_alpha,
            scene.tokens.titlebar_blur_percent,
            0,
        );
        for (x, y) in [(60, 0), (60, 30)] {
            assert_eq!(pixel_at(&fill, 120, x, y), pixel_at(&material, 120, x, y));
        }
        for (x, y) in [(0, 20), (60, 0), (119, 20), (60, 30)] {
            let pixel = pixel_at(&fill, 120, x, y);
            assert_eq!(pixel[3], 255, "material stays opaque at {x},{y}");
            assert!(
                pixel[0] != 0xff || pixel[1] != 0xff || pixel[2] != 0xff,
                "outline baked into the fill at {x},{y}"
            );
        }
    }

    #[test]
    fn plan_uses_skin_values_exactly() {
        let scene = plan_scene(
            400,
            300,
            "app",
            true,
            None,
            None,
            false,
            false,
            true,
            &flamewm_skin::recipes::decoration::ChromeTokens::default(),
        );
        assert_eq!(scene.bg, (0x00, 0x00, 0x00));
        assert_eq!(scene.bg_alpha, 191);
        assert_eq!(scene.tokens.titlebar_blur_percent, 80);
        // Unavailable renderer (no XRender, no display): flat opaque fill, no error.
        let px = titlebar_fill_pixels(&scene, 2, (0, 0), (800, 600));
        assert_eq!(px.len(), scene.frame_w as usize * 2 * 4);
        assert!(px.chunks_exact(4).all(|p| p[3] == 255));
        assert_eq!(PANEL.0, 0x001c_1c1c);
        assert_eq!(flamewm_skin::palette::TEXT.0, 0x00f1_f2f3);
        assert_eq!(scene.title_color, (0xf1, 0xf2, 0xf3));
        assert_eq!(scene.title_family, "IBM Plex Sans");
        assert_eq!(scene.title_family, Typography::RWR_0_0_9.family);
        assert_eq!(scene.title_size_px, 12);
        assert_eq!(scene.title_size_px, Typography::RWR_0_0_9.title.size_px);
        assert_eq!(scene.title_weight, FontWeight::Semibold);
        assert_eq!(scene.title_weight, Typography::RWR_0_0_9.title.weight);
        assert_eq!(scene.icon_edge, 13);
        assert_eq!(scene.icon_edge, WINDOW_CHROME.metrics.icon);
        assert_eq!(scene.baseline, 20.0);
        assert_eq!(scene.radius, 12);
    }

    #[test]
    fn nonempty_title_requires_draw_title_effect() {
        let scene = plan_scene(
            400,
            300,
            "hello",
            true,
            None,
            None,
            false,
            false,
            true,
            &flamewm_skin::recipes::decoration::ChromeTokens::default(),
        );
        let effects = effects_for(7, &scene);
        let draw = effects.iter().find_map(|effect| match effect {
            ChromeEffect::DrawTitle {
                font,
                size_px,
                weight,
                ..
            } => Some((*font, *size_px, *weight)),
            _ => None,
        });
        assert_eq!(draw, Some(("IBM Plex Sans", 12.0, 600)));
    }

    #[test]
    fn empty_title_may_skip_draw_title() {
        let scene = plan_scene(
            400,
            300,
            "",
            true,
            None,
            None,
            false,
            false,
            true,
            &flamewm_skin::recipes::decoration::ChromeTokens::default(),
        );
        let effects = effects_for(7, &scene);
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, ChromeEffect::DrawTitle { .. }))
        );
    }

    #[test]
    fn maximized_and_fullscreen_radius_zero() {
        let maximized = plan_scene(
            400,
            300,
            "a",
            true,
            None,
            None,
            true,
            false,
            false,
            &flamewm_skin::recipes::decoration::ChromeTokens::default(),
        );
        assert_eq!(maximized.radius, 0);
        let fullscreen = plan_scene(
            400,
            300,
            "a",
            true,
            None,
            None,
            false,
            true,
            false,
            &flamewm_skin::recipes::decoration::ChromeTokens::default(),
        );
        assert_eq!(fullscreen.radius, 0);
        let floating = plan_scene(
            400,
            300,
            "a",
            true,
            None,
            None,
            false,
            false,
            true,
            &flamewm_skin::recipes::decoration::ChromeTokens::default(),
        );
        assert_eq!(floating.radius, 12);
    }

    #[test]
    fn squared_style_is_rectangular_in_every_state() {
        let squared = flamewm_skin::recipes::decoration::ChromeTokens {
            style: flamewm_skin::recipes::decoration::DecorationStyle::Squared,
            ..Default::default()
        };
        for (floating, maximized, fullscreen) in [
            (true, false, false),
            (false, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let scene = plan_scene(
                400, 300, "a", true, None, None, maximized, fullscreen, floating, &squared,
            );
            assert_eq!(
                scene.radius, 0,
                "floating={floating} max={maximized} full={fullscreen}"
            );
        }
    }

    #[test]
    fn hover_role_only_changes_correct_control() {
        let base = plan_scene(
            400,
            300,
            "a",
            true,
            None,
            None,
            false,
            false,
            true,
            &flamewm_skin::recipes::decoration::ChromeTokens::default(),
        );
        let close = plan_scene(
            400,
            300,
            "a",
            true,
            Some(FrameControl::Close),
            None,
            false,
            false,
            true,
            &flamewm_skin::recipes::decoration::ChromeTokens::default(),
        );
        let close_effects = effects_for(1, &close);
        let base_effects = effects_for(1, &base);
        let close_controls = close_effects.iter().find_map(|effect| match effect {
            ChromeEffect::BlitControls { hover, .. } => Some(*hover),
            _ => None,
        });
        let base_controls = base_effects.iter().find_map(|effect| match effect {
            ChromeEffect::BlitControls { hover, .. } => Some(*hover),
            _ => None,
        });
        assert_eq!(close_controls, Some(Some(FrameControl::Close)));
        assert_eq!(base_controls, Some(None));
        // Only the hover payload differs; radius/bg/title style unchanged.
        assert_eq!(close.radius, base.radius);
        assert_eq!(close.bg, base.bg);
        assert_eq!(close.title_family, base.title_family);
    }

    #[test]
    fn render_passes_title_style_through_contract() {
        let mut renderer =
            flamewm_render_x11::ExternalDecorationRenderer::unavailable("no DISPLAY in test");
        let scene = plan_scene(
            400,
            300,
            "title",
            true,
            None,
            None,
            false,
            false,
            true,
            &flamewm_skin::recipes::decoration::ChromeTokens::default(),
        );
        let result = render(&mut renderer, 42, &scene, (0, 0), (800, 600), true, None);
        assert!(result.is_err());
        // Style params reached the draw-target contract before display touch.
        assert_eq!(renderer.last_title_style(), Some((12.0, 600)));
    }
}

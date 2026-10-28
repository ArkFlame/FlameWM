//! Frame chrome: pure scene plan plus native effect intents.
//!
//! Pure value objects. No X calls, no x11rb, no `get_geometry` reply in the
//! paint path. Live Xft/XRender/XShape handles stay inside
//! `flamewm-render-x11`'s [`ExternalDecorationRenderer`]; this module only
//! plans skin-exact values (J06 freeze) and describes effects.

use flamewm_skin::palette::{Rgb, TEXT};
use flamewm_skin::recipes::decoration::ChromeTokens;
use flamewm_skin::recipes::window_chrome::WINDOW_CHROME;
use flamewm_skin::typography::{FontWeight, Typography};

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
    let text = TEXT;
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
        scene.frame_w,
        height,
        scene.bg,
        scene.bg_alpha,
        scene.tokens.titlebar_blur_percent,
    )
}

/// Render the 1px contour inside the titlebar's native pixels. Its colour is the
/// same Surfaces token as panel/popups; no X11 round trip or overlay window.
fn composite_surface_contour(
    pixels: &mut [u8],
    width: u32,
    titlebar_height: u32,
    frame_height: u32,
    radius: u16,
    tokens: &ChromeTokens,
) {
    if width < 2 || titlebar_height == 0 || frame_height < 2 || tokens.surface_border_alpha == 0 {
        return;
    }
    let Some(total) = (width as usize)
        .checked_mul(titlebar_height as usize)
        .and_then(|n| n.checked_mul(4))
    else {
        return;
    };
    if pixels.len() < total {
        return;
    }
    let stroke = flamewm_ui_core::surface::SurfaceFill {
        rgb: tokens.surface_border_rgb.0,
        alpha: tokens.surface_border_alpha,
    };
    let r = u32::from(radius).min(width / 2).min(frame_height / 2);
    // Top perimeter and both side arcs. There is no horizontal border at the bottom of
    // the titlebar: that is the interior of the complete client frame.
    for y in 0..titlebar_height {
        let reach = if y <= r { r.saturating_add(2) } else { 1 };
        for x in 0..width {
            if y > 0 && x >= reach && x < width.saturating_sub(reach) {
                continue;
            }
            let coverage =
                flamewm_ui_core::surface::inner_contour_coverage(width, frame_height, r, x, y);
            if coverage == 0 {
                continue;
            }
            let alpha = (u16::from(stroke.alpha) * u16::from(coverage) + 127) / 255;
            let at = (y as usize * width as usize + x as usize) * 4;
            let base = u32::from_be_bytes([0, pixels[at], pixels[at + 1], pixels[at + 2]]);
            let blended = flamewm_ui_core::surface::blend_surface_over_rgb(
                base,
                flamewm_ui_core::surface::SurfaceFill {
                    alpha: alpha as u8,
                    ..stroke
                },
            );
            pixels[at] = (blended >> 16) as u8;
            pixels[at + 1] = (blended >> 8) as u8;
            pixels[at + 2] = blended as u8;
        }
    }
}

/// The native titlebar occupies only the first rows of the frame. Paint the
/// lower rounded arc on the *same* parent drawable, behind the shaped client;
/// otherwise the two bottom corner contours disappear even though the
/// top corner contour is present in `composite_surface_contour`.
///
/// This uses the shared 1px coverage function and emits only a small RGBA
/// strip (at most radius+1 rows), not a full-frame bitmap. The source is
/// transparent away from the stroke, so PictOpOver preserves the client and
/// backdrop rather than painting a black rectangle in the rounded notch.
fn bottom_contour_strip(
    width: u32,
    height: u32,
    radius: u16,
    tokens: &ChromeTokens,
) -> Option<(u32, Vec<u8>)> {
    if width < 2 || height < 2 || tokens.surface_border_alpha == 0 {
        return None;
    }
    let radius = u32::from(radius).min(width / 2).min(height / 2);
    if radius == 0 {
        return None;
    }
    let rows = radius.saturating_add(1).min(height);
    let bytes = (width as usize)
        .checked_mul(rows as usize)?
        .checked_mul(4)?;
    let mut pixels = vec![0_u8; bytes];
    let rgb = tokens.surface_border_rgb.0;
    for local_y in 0..rows {
        let y = height - rows + local_y;
        for x in 0..width {
            let coverage =
                flamewm_ui_core::surface::inner_contour_coverage(width, height, radius, x, y);
            if coverage == 0 {
                continue;
            }
            let alpha = (u16::from(tokens.surface_border_alpha) * u16::from(coverage) + 127) / 255;
            if alpha == 0 {
                continue;
            }
            let at = (local_y as usize * width as usize + x as usize) * 4;
            pixels[at] = (rgb >> 16) as u8;
            pixels[at + 1] = (rgb >> 8) as u8;
            pixels[at + 2] = rgb as u8;
            pixels[at + 3] = alpha as u8;
        }
    }
    Some((rows, pixels))
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
) -> Result<(), String> {
    use flamewm_render_core::{Color, Rect};
    // Order is the live paint contract: retarget, fill, icon blit, title
    // draw, control blits, shape, flush.
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
        composite_surface_contour(
            &mut pixels,
            scene.frame_w,
            titlebar_h as u32,
            scene.frame_h,
            scene.radius,
            &scene.tokens,
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
    // Bottom arc shares the titlebar contour color/coverage. Only the
    // lower radius rows are uploaded; XRender blends transparent pixels away.
    let bottom_result = if let Some((rows, pixels)) =
        bottom_contour_strip(scene.frame_w, scene.frame_h, scene.radius, &scene.tokens)
    {
        renderer.blit_rgba(
            &pixels,
            scene.frame_w,
            rows,
            Rect {
                x: 0.0,
                y: (scene.frame_h - rows) as f32,
                width: scene.frame_w as f32,
                height: rows as f32,
            },
        )
    } else {
        Ok(())
    };
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
    bottom_result?;
    shape_result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_skin::palette::PANEL;

    #[test]
    fn bottom_contour_draws_both_rounded_corners_and_centre_edge() {
        let tokens = ChromeTokens::default();
        let w = 120;
        let h = 100;
        let radius = 12;
        let (rows, pixels) = bottom_contour_strip(w, h, radius, &tokens).expect("bottom strip");
        let alpha = |x: usize, y: usize| pixels[(y * w as usize + x) * 4 + 3];
        assert_eq!(rows, 13);
        assert_eq!(
            alpha(0, 12),
            0,
            "bottom-left outer corner must stay clipped"
        );
        assert_eq!(
            alpha(119, 12),
            0,
            "bottom-right outer corner must stay clipped"
        );
        assert!(alpha(11, 12) > 0, "bottom-left arc must join bottom edge");
        assert!(alpha(108, 12) > 0, "bottom-right arc must join bottom edge");
        assert!(alpha(60, 12) > 0, "bottom edge must be continuous");
        assert_eq!(alpha(60, 0), 0, "inner area stays transparent");
        let mut disabled = tokens;
        disabled.surface_border_alpha = 0;
        assert!(bottom_contour_strip(w, h, radius, &disabled).is_none());
        assert!(bottom_contour_strip(w, h, 0, &tokens).is_none());
    }

    #[test]
    fn titlebar_contour_follows_the_rounded_corner_instead_of_ending_at_radius() {
        let width = 100_u32;
        let mut pixels = vec![0_u8; width as usize * 31 * 4];
        let tokens = ChromeTokens::default();
        composite_surface_contour(&mut pixels, width, 31, 150, 12, &tokens);
        let red = |x: usize, y: usize| pixels[(y * width as usize + x) * 4];
        assert_eq!(red(0, 0), 0, "the clipped outer corner stays empty");
        assert!(red(11, 0) > 0, "the top arc must join the straight border");
        assert!(red(0, 20) > 0, "the side contour must remain visible");
        assert_eq!(red(30, 15), 0, "the centre stays free of border paint");
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
        assert_eq!(scene.bg, (0x19, 0x1b, 0x1d));
        assert_eq!(scene.bg_alpha, 127);
        assert_eq!(scene.tokens.titlebar_blur_percent, 80);
        // Unavailable renderer (no XRender, no display): flat opaque fill, no error.
        let px = titlebar_fill_pixels(&scene, 2, (0, 0), (800, 600));
        assert_eq!(px.len(), scene.frame_w as usize * 2 * 4);
        assert!(px.chunks_exact(4).all(|p| p[3] == 255));
        assert_eq!(PANEL.0, 0x001b_1e20);
        assert_eq!(TEXT.0, 0x00f1_f2f3);
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
        let result = render(&mut renderer, 42, &scene, (0, 0), (800, 600));
        assert!(result.is_err());
        // Style params reached the draw-target contract before display touch.
        assert_eq!(renderer.last_title_style(), Some((12.0, 600)));
    }
}

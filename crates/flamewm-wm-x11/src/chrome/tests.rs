//! Window chrome unit tests.

use super::*;
use flamewm_skin::recipes::decoration::corner_radius;

fn put(snapshot: &mut SettingsSnapshot, key: &str, value: SettingValue) {
    snapshot.values.insert(key.to_owned(), value);
}

#[test]
fn circle_hover_has_fractional_edge_alpha_and_preserves_centre() {
    let mut glyph = flamewm_image_core::RgbaImage::from_rgba8(16, 16, vec![0_u8; 16 * 16 * 4])
        .expect("valid glyph buffer");
    paint_fill_behind(&mut glyph, [241, 242, 243, 255], FillMask::Circle);
    assert_eq!(glyph.pixels[(8 * 16 + 8) * 4 + 3], 255);
    assert_eq!(glyph.pixels[3], 0);
    assert!(
        glyph.pixels.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255),
        "the hover disk must anti-alias its circumference"
    );
}

#[test]
fn settings_color_rejects_sign_and_prefix_forms() {
    assert_eq!(parse_settings_color("#+12345"), None);
    assert_eq!(parse_settings_color("#-12345"), None);
    assert_eq!(parse_settings_color("#0x1234"), None);
    assert_eq!(parse_settings_color("#123456"), Some(Rgb(0x12_3456)));
}

#[test]
fn center_formula_matches_flame_contract() {
    // desired=(400-90)/2=155 inside free region.
    assert_eq!(recipe::center_title_x(400, 90, 22, 304, 6), 155);
    // Desired below minimum but free region inverted: sorted clamp keeps desired (20).
    assert_eq!(recipe::center_title_x(400, 360, 22, 304, 6), 20);
    // Title wider than the free region: sorted clamp keeps desired (10).
    assert_eq!(recipe::center_title_x(200, 180, 22, 104, 6), 10);
}

#[test]
fn title_measurement_uses_proportional_advance() {
    assert_eq!(title_text_width(""), 0);
    assert_eq!(title_text_width("ab"), 14);
    assert_eq!(title_text_width("あ"), 12);
}

#[test]
fn net_wm_icon_parser_accepts_valid_argb() {
    let cardinals = vec![
        2,
        1,
        0xff00_0000,
        0xff00_ff00, // 2x1
        1,
        1,
        0x80ff_0000, // 1x1
    ];
    let icon = parse_net_wm_icon(&cardinals).expect("valid payload");
    assert_eq!((icon.width, icon.height), (2, 1));
    assert_eq!(icon.argb, vec![0xff00_0000, 0xff00_ff00]);
}

#[test]
fn net_wm_icon_parser_rejects_invalid_payload() {
    assert!(parse_net_wm_icon(&[2, 2, 1, 2, 3]).is_none());
    assert!(parse_net_wm_icon(&[0, 1, 0]).is_none());
    assert!(parse_net_wm_icon(&[1, 1]).is_none());
    assert!(parse_net_wm_icon(&[1, 1, 0, 99]).is_none());
}

#[test]
fn icon_scales_into_slot_with_alpha_preserved() {
    let icon = IconImage {
        width: 2,
        height: 2,
        argb: vec![0xff00_0000, 0x0000_0000, 0x80ff_0000, 0xff00_ff00],
    };
    let scaled = scale_icon_to_slot(&icon, 4).expect("scaled");
    assert_eq!((scaled.width, scaled.height), (4, 4));
    assert_eq!(scaled.argb.len(), 16);
    assert!(scaled.argb.contains(&0x0000_0000));
    assert!(scaled.argb.contains(&0x80ff_0000));
}

#[test]
fn settings_map_to_skin_tokens_and_bad_values_fall_back() {
    let mut snapshot = SettingsSnapshot::new(1);
    let unset = chrome_tokens_from_settings(&snapshot);
    assert!(
        unset
            .buttons
            .iter()
            .all(|button| button.shape == ButtonShape::Circle)
    );
    put(
        &mut snapshot,
        "titlebarButtonStyle",
        SettingValue::Text(String::new()),
    );
    assert!(
        chrome_tokens_from_settings(&snapshot)
            .buttons
            .iter()
            .all(|button| button.shape == ButtonShape::Circle)
    );
    let text = |value: &str| SettingValue::Text(value.to_owned());
    put(&mut snapshot, "decoration", text("squared"));
    put(&mut snapshot, "surfaceColor", text("#202326"));
    put(&mut snapshot, "surfaceOpacity", SettingValue::Percent(50));
    put(&mut snapshot, "titlebarButtonStyle", text("square"));
    put(&mut snapshot, "titlebarCloseHoverColor", text("#00ff00"));
    put(&mut snapshot, "titlebarMinimizeColor", text("#zz"));
    let tokens = chrome_tokens_from_settings(&snapshot);
    assert_eq!(tokens.style, DecorationStyle::Squared);
    assert_eq!(tokens.titlebar_bg, Rgb(0x0020_2326));
    assert_eq!(tokens.titlebar_alpha, 128);
    assert!(
        tokens
            .buttons
            .iter()
            .all(|button| button.shape == ButtonShape::Square)
    );
    assert_eq!(tokens.buttons[2].hover, Some(Rgb(0x0000_ff00)));
    assert_eq!(tokens.buttons[0].glyph, None);
    put(&mut snapshot, "titlebarButtonStyle", text("bogus"));
    let tokens = chrome_tokens_from_settings(&snapshot);
    assert!(
        tokens
            .buttons
            .iter()
            .all(|button| button.shape == ButtonShape::Circle)
    );
}

#[test]
fn surface_opacity_maps_to_titlebar_alpha() {
    let mut snapshot = SettingsSnapshot::new(1);
    let mut alpha = |percent: u8| {
        put(
            &mut snapshot,
            "surfaceOpacity",
            SettingValue::Percent(percent),
        );
        chrome_tokens_from_settings(&snapshot).titlebar_alpha
    };
    assert_eq!(alpha(0), 0);
    assert_eq!(alpha(50), 128);
    assert_eq!(alpha(100), 255);
    snapshot.values.remove("surfaceOpacity");
    // Absent glass key: the regular glass default (12%) is 31.
    assert_eq!(chrome_tokens_from_settings(&snapshot).titlebar_alpha, 31);
}

#[test]
fn titlebar_color_setting_is_ignored() {
    let mut snapshot = SettingsSnapshot::new(1);
    put(
        &mut snapshot,
        "titlebarColor",
        SettingValue::Text("#202326".to_owned()),
    );
    let tokens = chrome_tokens_from_settings(&snapshot);
    assert_eq!(
        tokens.titlebar_bg,
        Rgb(flamewm_ui_core::glass::DARK.tint_rgb)
    );
    assert_eq!(tokens.titlebar_alpha, 31);
}

#[test]
fn square_glyph_has_no_inset_fill_circle_keeps_legacy_disk() {
    let edge = u32::from(WINDOW_CHROME.metrics.control_glyph_edge);
    let look = |shape| ControlLook {
        button: ButtonTokens {
            shape,
            ..ButtonTokens::default()
        },
        titlebar: WINDOW_CHROME.titlebar_background,
    };
    let square = control_glyph_raster(
        ControlRole::Close,
        edge,
        ControlMaterial::Hover,
        look(ButtonShape::Square),
    )
    .unwrap();
    assert_eq!(square.pixels[((8 * edge + 1) * 4 + 3) as usize], 0);
    let circle = control_glyph_raster(
        ControlRole::Close,
        edge,
        ControlMaterial::Hover,
        look(ButtonShape::Circle),
    )
    .unwrap();
    assert_eq!(circle.pixels[((8 * edge + 1) * 4 + 3) as usize], 255);
}

#[test]
fn windows11_square_hover_coverage_at_100_135_200_percent() {
    use crate::frame::layout::{
        controls_width_for_titlebar, layout_frame_children, resize_thickness_for_titlebar,
    };
    use crate::frame::model::FrameControl;
    for (height, radius) in [(31_u16, 12_u16), (42, 16), (62, 24)] {
        for decoration in [DecorationStyle::Rounded, DecorationStyle::Squared] {
            let mut tokens = ChromeTokens {
                style: decoration,
                ..Default::default()
            };
            for button in &mut tokens.buttons {
                button.shape = ButtonShape::Square;
            }
            let full = i32::from(height);
            let cells = layout_frame_children(
                600,
                400,
                full,
                controls_width_for_titlebar(full),
                resize_thickness_for_titlebar(full),
            )
            .controls;
            for (index, control) in [
                FrameControl::Minimize,
                FrameControl::MaximizeRestore,
                FrameControl::Close,
            ]
            .into_iter()
            .enumerate()
            {
                let outer_radius = if decoration == DecorationStyle::Rounded {
                    radius
                } else {
                    0
                };
                let plates = control_backplates_for(
                    600,
                    height,
                    outer_radius,
                    Some(control),
                    None,
                    &tokens,
                );
                assert_eq!(plates.len(), 1);
                let plate = &plates[0];
                assert_eq!(plate.control, control);
                assert_eq!(plate.x, cells[index].x);
                assert_eq!(
                    plate.x + i32::from(plate.width),
                    cells[index].x + cells[index].w
                );
                assert_eq!(plate.y, 0);
                assert_eq!(plate.height, height);
                assert_eq!(plate.corners.top_left, 0);
                assert_eq!(
                    plate.corners.top_right,
                    if control == FrameControl::Close {
                        outer_radius
                    } else {
                        0
                    }
                );
                assert_eq!(plate.corners.bottom_right, plate.corners.bottom_left);
                assert!(plate.corners.bottom_right > 0);
                let at = |x: usize, y: usize| plate.coverage[y * usize::from(plate.width) + x];
                let w = usize::from(plate.width);
                let h = usize::from(height);
                assert_eq!(at(0, 0), 255, "top left is square");
                assert_eq!(at(w / 2, 0), 255, "top edge is full");
                assert_eq!(at(w / 2, h - 1), 255, "bottom edge between curves is full");
                assert_eq!(at(0, h - 1), 0, "bottom-left curve cuts the corner");
                assert_eq!(at(w - 1, h - 1), 0, "bottom-right curve cuts the corner");
                assert_eq!(
                    at(w - 1, 0),
                    if control == FrameControl::Close && outer_radius > 0 {
                        0
                    } else {
                        255
                    }
                );
                assert!(
                    plate.coverage.iter().any(|&a| a > 0 && a < 255),
                    "curve requires antialiasing"
                );
                assert_eq!(
                    plate.rgb,
                    if control == FrameControl::Close {
                        [0xe8, 0x11, 0x23]
                    } else {
                        [0xf1, 0xf2, 0xf3]
                    }
                );
            }
        }
    }
}

#[test]
fn frame_backplates_blend_over_wallpaper_and_leave_adjacent_pixels_unchanged() {
    use crate::frame::model::FrameControl;
    let mut tokens = ChromeTokens::default();
    tokens.buttons[2].shape = ButtonShape::Square;
    let plates = control_backplates_for(400, 31, 12, Some(FrameControl::Close), None, &tokens);
    let mut pixels = [30, 40, 50, 255].repeat(400 * 31);
    composite_control_backplates(&mut pixels, 400, 31, &plates);
    let pixel = |x: usize, y: usize| &pixels[(y * 400 + x) * 4..(y * 400 + x) * 4 + 4];
    assert_eq!(
        pixel(361, 15),
        &[30, 40, 50, 255],
        "adjacent maximize cell untouched"
    );
    assert_eq!(
        pixel(362, 15),
        &[0xe8, 0x11, 0x23, 255],
        "close starts at exact cell boundary"
    );
    assert_eq!(
        pixel(380, 30),
        &[0xe8, 0x11, 0x23, 255],
        "bottom edge filled"
    );
    assert_eq!(
        pixel(399, 0),
        &[30, 40, 50, 255],
        "outer rounded corner clipped"
    );
    assert_eq!(
        pixel(399, 15),
        &[0xe8, 0x11, 0x23, 255],
        "right edge filled below outer corner"
    );
    assert!(
        pixels.chunks_exact(4).all(|p| p[3] == 255),
        "opaque wallpaper composite keeps opaque alpha"
    );
    let mut clean = [30, 40, 50, 255].repeat(400 * 31);
    composite_control_backplates(
        &mut clean,
        400,
        31,
        &control_backplates_for(400, 31, 12, None, None, &tokens),
    );
    assert!(clean.chunks_exact(4).all(|p| p == [30, 40, 50, 255]));
}

#[test]
fn hot_button_settings_update_color_and_reuse_geometry_cache() {
    use crate::frame::model::FrameControl;
    let mut tokens = ChromeTokens::default();
    tokens.buttons[0].shape = ButtonShape::Square;
    let a = control_backplates_for(400, 31, 12, Some(FrameControl::Minimize), None, &tokens);
    tokens.buttons[0].hover = Some(Rgb(0x0022_4466));
    let b = control_backplates_for(400, 31, 12, Some(FrameControl::Minimize), None, &tokens);
    assert_eq!(b[0].rgb, [0x22, 0x44, 0x66]);
    assert!(Arc::ptr_eq(&a[0].coverage, &b[0].coverage));
    tokens.buttons[0].pressed = Some(Rgb(0x0010_2030));
    let pressed =
        control_backplates_for(400, 31, 12, None, Some(FrameControl::Minimize), &tokens);
    assert_eq!(pressed[0].rgb, [0x10, 0x20, 0x30]);
    tokens.buttons[0].shape = ButtonShape::Circle;
    assert!(
        control_backplates_for(400, 31, 12, Some(FrameControl::Minimize), None, &tokens)
            .is_empty()
    );
}

#[test]
fn narrow_frame_backplates_tile_without_leaking_beyond_frame() {
    use crate::frame::model::FrameControl;
    let mut tokens = ChromeTokens::default();
    for button in &mut tokens.buttons {
        button.shape = ButtonShape::Square;
    }
    for width in [100_u32, 160, 400] {
        let plates: Vec<_> = [
            FrameControl::Minimize,
            FrameControl::MaximizeRestore,
            FrameControl::Close,
        ]
        .into_iter()
        .flat_map(|control| control_backplates_for(width, 31, 12, Some(control), None, &tokens))
        .collect();
        assert_eq!(plates.len(), 3);
        for pair in plates.windows(2) {
            assert_eq!(pair[0].x + i32::from(pair[0].width), pair[1].x);
        }
        assert_eq!(plates[2].x + i32::from(plates[2].width), width as i32);
        let mut background = [42_u8, 42, 42, 255].repeat(width as usize * 31);
        composite_control_backplates(&mut background, width, 31, &plates);
        assert_eq!(background.len(), width as usize * 31 * 4);
    }
}

#[test]
fn explicit_skin_default_tokens_reproduce_default_glyph_pixels() {
    let edge = u32::from(WINDOW_CHROME.metrics.control_glyph_edge);
    let explicit = ControlLook {
        button: ButtonTokens {
            shape: ButtonShape::Default,
            glyph: None,
            hover: Some(WINDOW_CHROME.close_hover),
            pressed: None,
        },
        titlebar: WINDOW_CHROME.titlebar_background,
    };
    let default = control_glyph_raster(
        ControlRole::Close,
        edge,
        ControlMaterial::Hover,
        ControlLook::default(),
    )
    .expect("close hover raster");
    let explicit =
        control_glyph_raster(ControlRole::Close, edge, ControlMaterial::Hover, explicit)
            .expect("explicit close hover raster");
    assert_eq!(default.pixels, explicit.pixels);
}

#[test]
fn style_tokens_never_move_control_hit_geometry() {
    let mut tokens = ChromeTokens {
        style: DecorationStyle::Squared,
        ..ChromeTokens::default()
    };
    for button in &mut tokens.buttons {
        button.shape = ButtonShape::Square;
    }
    let default = control_blits_for(400, false, false, None, None, &ChromeTokens::default());
    let styled = control_blits_for(400, false, false, None, None, &tokens);
    let dests = |blits: &[ControlBlit]| {
        blits
            .iter()
            .map(|blit| (blit.dest_x, blit.dest_y, blit.dest_w, blit.dest_h))
            .collect::<Vec<_>>()
    };
    assert_eq!(dests(&default), dests(&styled));
}

#[test]
fn policy_uses_skin_controls_and_rectangular_maximized() {
    assert_eq!(
        ControlPolicy::for_state(true, false).roles[1],
        ControlRole::Restore
    );
    assert_eq!(
        ControlPolicy::for_state(false, false).roles[1],
        ControlRole::Maximize
    );
    assert_eq!(corner_radius(true, DecorationStyle::Rounded), 24);
    assert_eq!(corner_radius(false, DecorationStyle::Rounded), 0);
}

#[test]
fn decoration_setting_parses_rounded_squared_and_legacy_ios() {
    let style_of = |value: &str| {
        let mut snapshot = SettingsSnapshot::new(1);
        put(
            &mut snapshot,
            "decoration",
            SettingValue::Text(value.to_owned()),
        );
        chrome_tokens_from_settings(&snapshot).style
    };
    assert_eq!(style_of("ios"), DecorationStyle::Rounded);
    assert_eq!(style_of("rounded"), DecorationStyle::Rounded);
    assert_eq!(style_of("squared"), DecorationStyle::Squared);
    assert_eq!(style_of("bogus"), DecorationStyle::Rounded);
}

#[test]
fn squircle_mask_rounds_square_tiles_and_keeps_round_icons() {
    let edge = 40_u32;
    let opaque = 0xff20_2020_u32;
    let mut tile = vec![opaque; (edge * edge) as usize];
    squircle_mask_opaque_tile(&mut tile, edge, edge);
    let alpha = |x: u32, y: u32| tile[(y * edge + x) as usize] >> 24;
    // Sharp corner pixel is outside the 22% squircle: fully transparent.
    assert_eq!(alpha(0, 0), 0);
    assert_eq!(alpha(39, 39), 0);
    // Edge midpoints and centre stay opaque.
    assert_eq!(alpha(20, 0), 255);
    assert_eq!(alpha(0, 20), 255);
    assert_eq!(alpha(20, 20), 255);
    // Colour is preserved where the mask keeps the pixel.
    assert_eq!(tile[(20 * edge + 20) as usize] & 0x00ff_ffff, 0x0020_2020);
    // A round icon (inscribed circle, transparent corners) lies inside the squircle: unchanged.
    let mut round = vec![0_u32; (edge * edge) as usize];
    for y in 0..edge {
        for x in 0..edge {
            let (dx, dy) = (x as f32 + 0.5 - 20.0, y as f32 + 0.5 - 20.0);
            if (dx * dx + dy * dy).sqrt() <= 19.0 {
                round[(y * edge + x) as usize] = opaque;
            }
        }
    }
    let before = round.clone();
    squircle_mask_opaque_tile(&mut round, edge, edge);
    assert_eq!(round, before);
}

#[test]
fn native_icon_converts_to_straight_rgba() {
    let icon = IconImage {
        width: 1,
        height: 1,
        argb: vec![0x80ff_0000],
    };
    let rgba = native_icon_rgba(&icon, 2).expect("rgba");
    assert_eq!((rgba.width, rgba.height), (2, 2));
    assert_eq!(rgba.pixels, [255, 0, 0, 128].repeat(4));
}

#[test]
fn icon_slot_and_baseline_follow_titlebar_height() {
    assert_eq!(icon_slot_for(31), recipe::icon_slot_edge());
    assert_eq!(title_baseline(31), 20.0);
    assert_eq!(title_baseline(10), 11.0);
}

#[test]
fn shared_glyph_raster_matches_owned_output() {
    let owned = control_glyph_raster(
        ControlRole::Close,
        16,
        ControlMaterial::Hover,
        ControlLook::default(),
    )
    .expect("owned close glyph");
    let shared = control_glyph_raster_shared(
        ControlRole::Close,
        16,
        ControlMaterial::Hover,
        ControlLook::default(),
    )
    .expect("shared close glyph");
    assert_eq!((shared.width, shared.height), (owned.width, owned.height));
    assert_eq!(shared.pixels, owned.pixels);
}

#[test]
fn installed_control_fallback_is_visible_for_every_role() {
    for role in [
        ControlRole::Minimize,
        ControlRole::Maximize,
        ControlRole::Restore,
        ControlRole::Close,
    ] {
        let raster =
            fallback_control_glyph(role, 16, [242, 242, 242, 255]).expect("native fallback");
        assert_eq!((raster.width, raster.height), (16, 16));
        assert!(raster.pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
    }
}

#[test]
fn control_glyphs_render_from_breeze_assets() {
    for role in [
        ControlRole::Minimize,
        ControlRole::Maximize,
        ControlRole::Restore,
        ControlRole::Close,
    ] {
        let image =
            control_glyph_raster(role, 16, ControlMaterial::Rest, ControlLook::default())
                .expect("breeze control svg");
        assert_eq!((image.width, image.height), (16, 16));
        assert!(image.pixels.chunks_exact(4).any(|pixel| pixel[3] > 0));
    }
    assert!(
        control_glyph_raster(
            ControlRole::Close,
            0,
            ControlMaterial::Rest,
            ControlLook::default()
        )
        .is_none()
    );
}

#[test]
fn hovering_close_selects_hover_material_only_for_close() {
    use crate::frame::model::FrameControl;
    let edge = u32::from(WINDOW_CHROME.metrics.control_glyph_edge);
    // Blits carry no material field, so the material each control was
    // rendered with is observed through its cached glyph raster.
    assert_eq!(
        ControlMaterial::for_pointer(false, false),
        ControlMaterial::Rest
    );
    assert_eq!(
        ControlMaterial::for_pointer(true, false),
        ControlMaterial::Hover
    );
    let rest = control_blits_for(400, false, false, None, None, &ChromeTokens::default());
    let hovered = control_blits_for(
        400,
        false,
        false,
        Some(FrameControl::Close),
        None,
        &ChromeTokens::default(),
    );
    assert_eq!(rest.len(), 3);
    assert_eq!(hovered.len(), 3);
    let expected = [
        (FrameControl::Minimize, ControlMaterial::Rest),
        (FrameControl::MaximizeRestore, ControlMaterial::Rest),
        (FrameControl::Close, ControlMaterial::Hover),
    ];
    for (index, (control, material)) in expected.iter().enumerate() {
        assert_eq!(hovered[index].control, *control);
        let raster =
            control_glyph_raster(hovered[index].role, edge, *material, ControlLook::default())
                .expect("control raster");
        assert_eq!(
            *hovered[index].raster, raster,
            "{control:?} must use {material:?} material on close hover"
        );
        let (h, r) = (&hovered[index], &rest[index]);
        assert_eq!(
            (h.dest_x, h.dest_y, h.dest_w, h.dest_h),
            (r.dest_x, r.dest_y, r.dest_w, r.dest_h),
            "hover must keep the {control:?} destination geometry"
        );
    }
    for (index, blit) in rest.iter().enumerate() {
        let raster = control_glyph_raster(
            blit.role,
            edge,
            ControlMaterial::Rest,
            ControlLook::default(),
        )
        .expect("control raster");
        assert_eq!(
            *blit.raster, raster,
            "no hover must select Rest material for every control"
        );
        assert_eq!(blit.control, hovered[index].control);
    }
}

#[test]
fn close_hover_and_pressed_rasters_differ_from_rest() {
    let edge = u32::from(WINDOW_CHROME.metrics.control_glyph_edge);
    let rest = control_glyph_raster(
        ControlRole::Close,
        edge,
        ControlMaterial::Rest,
        ControlLook::default(),
    )
    .expect("close rest raster");
    for material in [ControlMaterial::Hover, ControlMaterial::Pressed] {
        let raster =
            control_glyph_raster(ControlRole::Close, edge, material, ControlLook::default())
                .expect("close material raster");
        assert_ne!(
            raster.pixels, rest.pixels,
            "close {material:?} must not be byte-identical to rest"
        );
    }
}

#[test]
fn close_hover_paints_skin_red_disk_behind_glyph() {
    let edge = u32::from(WINDOW_CHROME.metrics.control_glyph_edge);
    let raster = control_glyph_raster(
        ControlRole::Close,
        edge,
        ControlMaterial::Hover,
        ControlLook::default(),
    )
    .expect("close hover raster");
    // (1, 8) is inside the edge disk and clear of the glyph strokes.
    let offset = ((8 * edge + 1) * 4) as usize;
    let red = WINDOW_CHROME.close_hover.0;
    assert_eq!(
        &raster.pixels[offset..offset + 4],
        &[(red >> 16) as u8, (red >> 8) as u8, red as u8, 255]
    );
}

#[test]
fn minimize_rest_raster_keeps_flame_default_scheme() {
    let edge = u32::from(WINDOW_CHROME.metrics.control_glyph_edge);
    let raster = control_glyph_raster(
        ControlRole::Minimize,
        edge,
        ControlMaterial::Rest,
        ControlLook::default(),
    )
    .expect("minimize rest raster");
    let bytes = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../")
            .join(ControlRole::Minimize.asset_path()),
    )
    .expect("minimize asset");
    let expected = flamewm_image_core::svg::render_with_color_scheme(
        &bytes,
        edge,
        edge,
        &flamewm_image_core::SvgColorScheme::flame_default(),
    )
    .expect("minimize expected");
    assert_eq!(raster.pixels, expected.pixels);
}

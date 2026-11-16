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
fn decorated_body_backdrop_spans_the_configured_client_extent() {
    // Configured 640x420 body under a titlebar: a shorter buffer must still sit on the full body.
    let rect = Rect::new(100, 100 + TITLEBAR_HEIGHT, 640, 420);
    let frame = Rect::new(100, 100, 640, 420 + TITLEBAR_HEIGHT);
    let body = client_backdrop_rect(rect, 1.0);
    assert_eq!((body.loc.x, body.loc.y), (100, 100 + TITLEBAR_HEIGHT));
    assert_eq!((body.size.w, body.size.h), (640, 420));
    assert_eq!(body.loc.y + body.size.h, frame.y + frame.h);
    let scaled = client_backdrop_rect(rect, 1.25);
    assert_eq!((scaled.size.w, scaled.size.h), (800, 525));
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
    let deco = |mode: Mode| client_corner_radii_for(mode, true, 300, 200, DecorationStyle::Rounded);
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
        let available =
            chrome::title_available_width(0, width - chrome::controls_width(), chrome::TITLE_PAD);
        let text = chrome::elide_title("A very long application window title", available, |_| 6);
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

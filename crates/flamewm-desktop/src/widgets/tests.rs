use super::sync::{ellipsize, fitted_font_size, history_pixels};
use super::*;

#[test]
fn sampler_widgets_arm_the_sample_timer_without_a_clock_tick() {
    let mut store = WidgetStore::default();
    store
        .add(
            WidgetKind::Cpu,
            Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
        )
        .expect("cpu widget fits");
    let mut state = WidgetState::new(store, PathBuf::new());
    let now = Instant::now();
    state.arm(now);
    assert_eq!(
        state.due, None,
        "a CPU widget must not force a per-second tick"
    );
    assert_eq!(
        state.sample_due,
        Some(now),
        "a CPU widget samples on the sample timer"
    );
}

/// Idle samplers re-read raw counters every tick, but only the drawn values reach the
/// document. Equal drawn values must compare equal so an idle desktop stops repainting.
#[test]
fn sample_key_compares_drawn_values_not_raw_counters() {
    let mut state = WidgetState::new(WidgetStore::default(), PathBuf::new());
    state.samples.memory = Some(MemoryUsage {
        total_kib: 8 * 1_048_576,
        available_kib: 4 * 1_048_576,
    });
    state.samples.net_rx_history = VecDeque::from([100u64, 200]);
    state.samples.net_tx_history = VecDeque::from([50u64, 25]);
    let idle = state.sample_key();

    // Same percent, same formatted GiB text; raw KiB differs. Rates differ in absolute
    // bytes but scale to the same graph points.
    state.samples.memory = Some(MemoryUsage {
        total_kib: 8 * 1_048_576 + 1,
        available_kib: 4 * 1_048_576 + 1,
    });
    state.samples.net_rx_history = VecDeque::from([200u64, 400]);
    state.samples.net_tx_history = VecDeque::from([100u64, 50]);
    assert_eq!(
        idle,
        state.sample_key(),
        "samples that draw the same pixels must not force a repaint"
    );

    state.samples.memory = Some(MemoryUsage {
        total_kib: 8 * 1_048_576,
        available_kib: 2 * 1_048_576,
    });
    assert_ne!(idle, state.sample_key(), "changed memory text repaints");

    state.samples.memory = Some(MemoryUsage {
        total_kib: 8 * 1_048_576,
        available_kib: 4 * 1_048_576,
    });
    state.samples.net_rx_history = VecDeque::from([200u64, 100]);
    assert_ne!(idle, state.sample_key(), "changed graph shape repaints");
}

fn alpha_at(pixels: &[u8], size: u32, x: u32, y: u32) -> u8 {
    pixels[((y * size + x) * 4 + 3) as usize]
}

fn red_at(pixels: &[u8], size: u32, x: u32, y: u32) -> u8 {
    pixels[((y * size + x) * 4) as usize]
}

#[test]
fn ring_is_transparent_inside_and_outside_the_band() {
    let size = 64;
    let pixels = ring_pixels(size, 8.0, 50, Rgb::new(255, 0, 0), Rgb::new(0, 0, 255));
    assert_eq!(pixels.len(), (size * size * 4) as usize);
    assert_eq!(alpha_at(&pixels, size, 32, 32), 0);
    assert_eq!(alpha_at(&pixels, size, 0, 0), 0);
    assert!(alpha_at(&pixels, size, 32, 3) > 200);
}

#[test]
fn ring_fills_clockwise_from_the_top() {
    let size = 64;
    let pixels = ring_pixels(size, 8.0, 25, Rgb::new(255, 0, 0), Rgb::new(0, 0, 255));
    assert!(
        red_at(&pixels, size, 50, 12) > 200,
        "first quarter is the ring color"
    );
    assert!(
        red_at(&pixels, size, 12, 50) < 60,
        "last quarter is the track color"
    );
    let empty = ring_pixels(size, 8.0, 0, Rgb::new(255, 0, 0), Rgb::new(0, 0, 255));
    assert!(red_at(&empty, size, 50, 12) < 60);
}

#[test]
fn slot_numbers_are_bounded() {
    assert_eq!(slot_number("widget.1.move"), Some(1));
    assert_eq!(slot_number("widget.8.media.play"), Some(8));
    assert_eq!(slot_number("widget.9.move"), None);
    assert_eq!(slot_number("widget.menu.lock"), None);
}

#[test]
fn panels_center_inside_the_work_area() {
    assert_eq!(
        panel_rect(Rect::new(0, 40, 1000, 600), (560, 520)),
        Rect::new(220, 80, 560, 520)
    );
    assert_eq!(
        panel_rect(Rect::new(0, 0, 400, 300), (560, 520)),
        Rect::new(0, 0, 400, 300)
    );
}

fn wall_time(hour: u8, minute: u8, second: u8) -> WallTime {
    WallTime {
        year: 2026,
        month: 10,
        day: 28,
        weekday: 3,
        hour,
        minute,
        second,
    }
}

#[test]
fn analog_hands_sweep_clockwise_from_twelve() {
    let at_three = hand_turns(wall_time(3, 0, 0));
    assert_eq!(at_three.hour, 0.25);
    assert_eq!(at_three.minute, 0.0);
    assert_eq!(at_three.second, 0.0);
    // 15:30:30 uses the 12-hour dial and carries minutes and seconds into the hour hand.
    let turns = hand_turns(wall_time(15, 30, 30));
    assert!((turns.hour - (3.0 + 0.5 + 30.0 / 3600.0) / 12.0).abs() < 1e-6);
    assert!((turns.minute - 30.5 / 60.0).abs() < 1e-6);
    assert!((turns.second - 0.5).abs() < 1e-6);
}

#[test]
fn segment_coverage_is_full_on_the_axis_and_zero_far_away() {
    let from = (0.0, 0.0);
    let to = (10.0, 0.0);
    assert_eq!(segment_coverage(5.0, 0.0, from, to, 1.0), 1.0);
    assert_eq!(segment_coverage(5.0, 9.0, from, to, 1.0), 0.0);
    assert_eq!(segment_coverage(20.0, 0.0, from, to, 1.0), 0.0);
}

#[test]
fn analog_minute_hand_points_at_the_minute_mark() {
    // 12:15:00: the minute hand points at 3, the hour hand is still near 12.
    let size = 96;
    let hands = Rgb::new(10, 200, 30);
    let accent = Rgb::new(250, 0, 0);
    let pixels = analog_face_pixels(size, wall_time(12, 15, 0), hands, accent);
    assert_eq!(pixels.len(), (size * size * 4) as usize);
    let center = size / 2;
    // Between the hub and the hand tips (hour hand ends at 0.5R, minute at 0.78R).
    let reach = (size as f32 * 0.3) as u32;
    let index = ((center * size + center + reach) * 4) as usize;
    assert_eq!(pixels[index + 3], 255);
    assert!(
        pixels[index] < 60 && pixels[index + 1] > 150,
        "{:?}",
        &pixels[index..index + 4]
    );
    // Nothing is drawn beyond the dial edge.
    assert_eq!(pixels[3], 0);
}

#[test]
fn analog_second_hand_uses_the_accent_color() {
    let size = 96;
    let hands = Rgb::new(10, 200, 30);
    let accent = Rgb::new(250, 0, 0);
    // 12:00:15: the second hand points at 3 and the minute hand is at 12.
    let pixels = analog_face_pixels(size, wall_time(12, 0, 15), hands, accent);
    let center = size / 2;
    // Between the hub and the hand tips (hour hand ends at 0.5R, minute at 0.78R).
    let reach = (size as f32 * 0.3) as u32;
    let index = ((center * size + center + reach) * 4) as usize;
    // The hand is under 1px wide, so the sampled row only partly covers it.
    assert!(pixels[index + 3] > 100);
    assert!(
        pixels[index] > 200 && pixels[index + 1] < 60,
        "{:?}",
        &pixels[index..index + 4]
    );
}

#[test]
fn compact_clock_font_fits_the_time_string_inside_one_cell() {
    // 104 px cell, 16 px padding: "01:45" (5 glyphs) must fit the 72 px inner width.
    let size = fitted_font_size(40.0, 5, 104.0);
    assert!(size * 0.62 * 5.0 <= 72.0 + 0.5);
    assert_eq!(fitted_font_size(20.0, 5, 340.0), 20.0);
    assert_eq!(fitted_font_size(40.0, 8, 40.0), 9.0);
}

#[test]
fn weather_city_name_fits_inside_a_two_cell_card() {
    let inner = (216.0 - WIDGET_PADDING * 2.0) * 0.9;
    let city = ellipsize("Buenos Aires, Argentina Pampeana", 15.0, true, inner);
    assert!(city.ends_with('…'));
    assert!(estimate_text_width(&city, 15.0, true) <= inner);
}

#[test]
fn ellipsize_keeps_text_that_fits_and_trims_the_rest() {
    assert_eq!(ellipsize("ok", 12.0, false, 500.0), "ok");
    let trimmed = ellipsize("a very long custom sentence", 14.0, false, 60.0);
    assert!(trimmed.ends_with('…'));
    assert!(estimate_text_width(&trimmed, 14.0, false) <= 60.0);
}

#[test]
fn opacity_maps_to_alpha() {
    assert_eq!(percent_to_alpha(0), 0);
    assert_eq!(percent_to_alpha(70), 179);
    assert_eq!(percent_to_alpha(140), 255);
}

#[test]
fn edge_drag_moves_only_the_dragged_edges() {
    let bounds = Rect::new(0, 0, 1000, 800);
    let original = Rect::new(100, 100, 200, 100);
    let right = Edges::from_name("e").expect("edge");
    assert_eq!(
        edge_drag_rect(original, right, 50, 999, bounds),
        Rect::new(100, 100, 250, 100)
    );
    let left = Edges::from_name("w").expect("edge");
    assert_eq!(
        edge_drag_rect(original, left, 50, 0, bounds),
        Rect::new(150, 100, 150, 100)
    );
    // Dragging a left edge past the right edge keeps one pixel instead of inverting.
    assert_eq!(edge_drag_rect(original, left, 900, 0, bounds).width, 1);
    let corner = Edges::from_name("nw").expect("edge");
    assert_eq!(
        edge_drag_rect(original, corner, -20, -30, bounds),
        Rect::new(80, 70, 220, 130)
    );
}

fn settings_with(values: &[(&str, flamewm_api::settings::SettingValue)]) -> SettingsSnapshot {
    let mut settings = SettingsSnapshot::new(1);
    for (key, value) in values {
        settings.values.insert((*key).to_owned(), value.clone());
    }
    settings
}

#[test]
fn inheriting_card_takes_the_global_surface_fill_blur_and_radius() {
    let mut widget = Widget::new(1, WidgetKind::Clock, Point::new(0, 0));
    assert!(widget.set_value("surface", 1));
    let surface = widget_surface(&widget, &SettingsSnapshot::new(1));
    // Translucent default: black at 75% with the regular glass blur (70%).
    assert_eq!(
        surface.fill,
        UiColor {
            r: 0,
            g: 0,
            b: 0,
            a: 191
        }
    );
    assert_eq!(surface.blur, Some(70));
    assert_eq!(surface.radius, 18.0);
}

#[test]
fn card_without_custom_surface_ignores_its_own_colour_and_opacity() {
    let mut widget = Widget::new(1, WidgetKind::Memory, Point::new(0, 0));
    assert!(widget.set_value("surface", 1));
    assert!(widget.inherits_surface());
    assert!(widget.set_value("backgroundOpacity", 90));
    let inherited = widget_surface(&widget, &SettingsSnapshot::new(1));
    assert_eq!(
        inherited.blur,
        Some(70),
        "translucent default keeps the glass blur"
    );
    assert_eq!(inherited.fill.a, 191, "translucent default is 75%");
}

#[test]
fn custom_surface_uses_its_own_colour_and_opacity_with_global_blur() {
    let mut widget = Widget::new(1, WidgetKind::Cpu, Point::new(0, 0));
    assert!(widget.set_value("surface", 1));
    assert!(widget.set_value("customSurface", 1));
    assert!(widget.set_value("background", 0));
    assert!(widget.set_value("backgroundOpacity", 70));
    // Glass opt-in: the global blur and the card's own opacity only show with reduceTransparency off.
    let glass = settings_with(&[(
        "reduceTransparency",
        flamewm_api::settings::SettingValue::Boolean(false),
    )]);
    let surface = widget_surface(&widget, &glass);
    assert_eq!(surface.fill.a, 179);
    assert_eq!(surface.blur, Some(70));
    assert_eq!(surface.radius, WIDGET_RADIUS);
    // A global blur change follows every card; the custom colour stays put.
    let no_blur = settings_with(&[(
        "surfaceBlurPercent",
        flamewm_api::settings::SettingValue::Percent(0),
    )]);
    let global = widget_surface(&widget, &no_blur);
    assert_eq!(global.blur, None);
    assert_eq!(global.fill, surface.fill);
}

#[test]
fn reduce_transparency_makes_inheriting_cards_opaque_without_blur() {
    let mut widget = Widget::new(1, WidgetKind::Clock, Point::new(0, 0));
    assert!(widget.set_value("surface", 1));
    let reduced = settings_with(&[(
        "reduceTransparency",
        flamewm_api::settings::SettingValue::Boolean(true),
    )]);
    let surface = widget_surface(&widget, &reduced);
    assert_eq!(surface.fill.a, 255);
    assert_eq!(surface.blur, None);
}

#[test]
fn card_without_surface_draws_no_fill_and_requests_no_backdrop() {
    // Default: no surface, so no fill and no backdrop capture, whatever the global surface is.
    for kind in WidgetKind::ALL {
        let widget = Widget::new(1, kind, Point::new(0, 0));
        assert!(!widget.has_surface(), "{kind:?} defaults to no surface");
        let surface = widget_surface(&widget, &SettingsSnapshot::new(1));
        assert_eq!(surface.fill.a, 0, "{kind:?} fill is fully transparent");
        assert_eq!(surface.blur, None, "{kind:?} requests no backdrop blur");
    }
    // Even a custom colour and opacity draw nothing while `surface` is off.
    let mut custom = Widget::new(1, WidgetKind::Cpu, Point::new(0, 0));
    assert!(custom.set_value("customSurface", 1));
    assert!(custom.set_value("backgroundOpacity", 90));
    let surface = widget_surface(&custom, &SettingsSnapshot::new(1));
    assert_eq!((surface.fill.a, surface.blur), (0, None));
}

#[test]
fn dragged_card_rises_above_content_until_release() {
    assert_eq!(dragged_layer(false), UiLayer::Floating);
    assert!(dragged_layer(false).z_index() > UiLayer::Content.z_index());
    assert!(dragged_layer(false).z_index() < UiLayer::Overlay.z_index());
    assert_eq!(dragged_layer(true), UiLayer::Content);
}

#[test]
fn graph_image_is_rebuilt_only_when_its_inputs_change() {
    use flamewm_ui_x11::test_support::RecordingUiDoc;
    let mut widget = Widget::new(1, WidgetKind::History, Point::new(0, 0));
    widget.rect = Rect::new(0, 0, 300, 160);
    let accent = Rgb {
        red: 9,
        green: 9,
        blue: 9,
    };
    let mut samples = Samples {
        cpu_history: VecDeque::from(vec![10, 20, 30]),
        ..Samples::default()
    };
    let mut graph = None;
    let mut doc = RecordingUiDoc::default();
    sync_history(&mut doc, &mut graph, &samples, accent, 0, &widget).expect("first");
    assert!(doc.images.contains_key("widget-1-history-image"));
    // Unchanged samples and size: no image is rebuilt or uploaded.
    doc.images.clear();
    sync_history(&mut doc, &mut graph, &samples, accent, 0, &widget).expect("same");
    assert!(doc.images.is_empty());
    // A new sample scrolls the graph, so the image is uploaded again.
    samples.cpu_history.push_back(40);
    sync_history(&mut doc, &mut graph, &samples, accent, 0, &widget).expect("changed");
    assert!(doc.images.contains_key("widget-1-history-image"));
    // Resizing the card changes the key as well.
    doc.images.clear();
    widget.rect = Rect::new(0, 0, 320, 160);
    sync_history(&mut doc, &mut graph, &samples, accent, 0, &widget).expect("resized");
    assert!(doc.images.contains_key("widget-1-history-image"));
}

#[test]
fn drop_target_ghost_is_an_outline_without_fill() {
    use flamewm_ui_x11::test_support::RecordingUiDoc;
    let metrics = GridMetrics::for_work_area(Rect::new(0, 0, 1920, 1080), 100);
    let accent = Rgb {
        red: 9,
        green: 9,
        blue: 9,
    };
    let mut doc = RecordingUiDoc::default();
    let cell = Placement::left(1, 1, Span::SINGLE);
    sync_ghost(&mut doc, metrics, accent, Some((cell, true))).expect("ghost");
    assert!(doc.borders.contains_key("widget-ghost"));
    assert!(!doc.backgrounds.contains_key("widget-ghost"));
    assert_eq!(doc.visible.get("widget-ghost"), Some(&true));
}

#[test]
fn history_graph_draws_each_enabled_series_at_its_sample_height() {
    let mut cpu = VecDeque::new();
    cpu.push_back(100u8);
    cpu.push_back(0u8);
    let sky = Rgb {
        red: 1,
        green: 2,
        blue: 3,
    };
    let pixels = history_pixels(8, 5, &[(&cpu, sky)]);
    assert_eq!(pixels.len(), 8 * 5 * 4);
    // History is right-aligned: the newest samples sit in the right column,
    // 100% on the top row and 0% on the bottom row.
    let top_right = ((7) * 4) as usize;
    assert_eq!(&pixels[top_right..top_right + 4], &[1, 2, 3, 255]);
    let bottom_right = ((4 * 8 + 7) * 4) as usize;
    assert_eq!(&pixels[bottom_right..bottom_right + 4], &[1, 2, 3, 255]);
    // Left-most column is untouched while history is shorter than the graph.
    assert_eq!(&pixels[0..4], &[0, 0, 0, 0]);
    // An empty series and an empty pixel stay transparent.
    let empty = history_pixels(8, 5, &[(&VecDeque::new(), sky)]);
    assert!(empty.iter().all(|byte| *byte == 0));
}

#[test]
fn net_dev_totals_skip_loopback_and_autoscale_rates() {
    let text = "Inter-|   Receive                                                |  Transmit\n\
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
lo:  500 1 0 0 0 0 0 0  500 1 0 0 0 0 0 0\n\
  eth0: 1000 2 0 0 0 0 0 0  300 2 0 0 0 0 0 0\n\
  wlan0: 24 1 0 0 0 0 0 0  76 1 0 0 0 0 0 0\n";
    assert_eq!(parse_net_dev(text), Some((1024, 376)));
    assert_eq!(parse_net_dev("garbage"), None);
    let mut rates = VecDeque::new();
    push_rate(&mut rates, 50);
    push_rate(&mut rates, 200);
    assert_eq!(scale_rates(&rates), VecDeque::from([25u8, 100]));
}

#[test]
fn settings_menu_rows_fit_every_kind_with_all_overrides_on() {
    // Every override row on: the menu must still fit its fixed row pool.
    let all_on = std::collections::BTreeMap::from([
        ("surface".to_owned(), 1),
        ("customSurface".to_owned(), 1),
        ("customAccent".to_owned(), 1),
    ]);
    for kind in WidgetKind::ALL {
        let rows = kind.visible_options(&all_on).len();
        assert!(rows <= CONFIG_ROWS, "{kind:?} needs {rows} rows");
    }
}

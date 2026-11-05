use super::*;

pub(super) fn sync_analog(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    slot: usize,
    widget: &Widget,
    time: WallTime,
) -> Result<(), String> {
    let id = format!("widget-{}", slot + 1);
    let side = (widget.rect.width.min(widget.rect.height) as f32 - WIDGET_PADDING * 2.0)
        .max(48.0)
        .floor();
    let size = side as u32;
    // Absolute children are placed from the widget's content box, so the padding is subtracted.
    let left = ((widget.rect.width as f32 - side) / 2.0 - WIDGET_PADDING).floor();
    let top = ((widget.rect.height as f32 - side) / 2.0 - WIDGET_PADDING).floor();
    document.position(&format!("{id}-analog"), left, top)?;
    document.size(&format!("{id}-analog"), side, side)?;
    let hands = widget.color("textColor");
    let accent = widget.color_or_accent("accentColor", projection_accent(state));
    let key = FaceKey {
        size,
        second: u32::from(time.hour) * 3600 + u32::from(time.minute) * 60 + u32::from(time.second),
        hands,
        accent,
    };
    if state.widgets.faces[slot] != Some(key) {
        for number in 1..=12 {
            let numeral = format!("{id}-analog-n{number}");
            document.foreground(&numeral, color(hands, 255))?;
            document.font_size(&numeral, (side * 0.11).clamp(9.0, 22.0))?;
        }
        document.image_rgba8(
            &format!("{id}-analog-image"),
            RuntimeImage {
                source: format!("{id}-analog-face"),
                width: size,
                height: size,
                pixels: analog_face_pixels(size, time, hands, accent),
            },
        )?;
        state.widgets.faces[slot] = Some(key);
    }
    Ok(())
}

pub(super) fn sync_clock(
    document: &mut impl UiDocumentAccess,
    id: &str,
    widget: &Widget,
    time: WallTime,
    accent: Rgb,
) -> Result<(), String> {
    // One-cell cards keep only the time, fitted to the card width.
    let compact = widget.rect.width < COMPACT_CARD_WIDTH;
    let show_time = widget.flag("showTime");
    let show_date = widget.flag("showDate") && !compact;
    let side_by_side = widget.flag("sideBySide") && !compact;
    let time_text = format_time(time, widget.flag("use24Hour"), widget.flag("showSeconds"));
    let time_size = fitted_font_size(
        widget.value("timeSize") as f32,
        time_text.chars().count(),
        widget.rect.width as f32,
    );
    let date_text = format_date(time, widget.flag("showWeekday"), widget.flag("showYear"));
    let time_color = color(widget.color("textColor"), 255);
    let date_color = color(widget.color_or_accent("accentColor", accent), 255);
    let weight = if widget.flag("bold") { 700 } else { 400 };
    let align = widget.align();
    document.visible(&format!("{id}-clock-stack"), !side_by_side)?;
    document.visible(&format!("{id}-clock-row"), side_by_side)?;
    for (time_id, date_id) in [
        (format!("{id}-clock-time"), format!("{id}-clock-date")),
        (
            format!("{id}-clock-row-time"),
            format!("{id}-clock-row-date"),
        ),
    ] {
        document.text_ref(&time_id, &time_text)?;
        document.text_ref(&date_id, &date_text)?;
        document.font_size(&time_id, time_size)?;
        document.font_size(&date_id, WIDGET_DATE_SIZE)?;
        document.font_weight(&time_id, weight)?;
        document.foreground(&time_id, time_color)?;
        document.foreground(&date_id, date_color)?;
        document.visible(&time_id, show_time)?;
        document.visible(&date_id, show_date)?;
    }
    document.visible(&format!("{id}-clock-time-line"), show_time)?;
    document.visible(&format!("{id}-clock-date-line"), show_date)?;
    for line in [
        format!("{id}-clock-time-line"),
        format!("{id}-clock-date-line"),
        format!("{id}-clock-row"),
    ] {
        set_alignment(document, &line, align)?;
    }
    Ok(())
}

pub(super) fn sync_gauge(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    slot: usize,
    widget: &Widget,
) -> Result<(), String> {
    let id = format!("widget-{}", slot + 1);
    let samples = &state.widgets.samples;
    // A ring picks its metric from the widget option; the CPU and RAM gauges fix theirs.
    let metric = match widget.kind {
        WidgetKind::Ring if widget.value("metric") == 0 => WidgetKind::Cpu,
        WidgetKind::Ring => WidgetKind::Memory,
        kind => kind,
    };
    let (title, percent, detail) = match metric {
        WidgetKind::Cpu => (
            "CPU",
            samples.cpu_percent,
            match samples.cpu_count {
                0 => String::new(),
                1 => "1 core".to_owned(),
                count => format!("{count} cores"),
            },
        ),
        _ => (
            "Memory",
            samples.memory.and_then(MemoryUsage::percent),
            samples.memory.map_or_else(String::new, |memory| {
                format!(
                    "{} / {} GiB",
                    format_gib(memory.used_kib()),
                    format_gib(memory.total_kib)
                )
            }),
        ),
    };
    let percent_text = percent.map_or_else(|| "--%".to_owned(), |value| format!("{value}%"));
    // One-cell gauges stack the title above the ring; side-by-side needs room for both.
    let side_by_side = widget.flag("sideBySide") && widget.rect.width >= COMPACT_CARD_WIDTH;
    let show_title = widget.flag("showTitle");
    let show_percent = widget.flag("showPercent");
    let show_details = widget.flag("showDetails");
    let text_color = color(widget.color("textColor"), 255);
    let caption_height = if !side_by_side && show_title {
        22.0
    } else {
        0.0
    };
    let inner_width = widget.rect.width as f32 - WIDGET_PADDING * 2.0;
    let inner_height = widget.rect.height as f32 - WIDGET_PADDING * 2.0 - caption_height;
    let ring_size = if side_by_side {
        inner_height.min(inner_width * 0.45)
    } else {
        inner_height.min(inner_width)
    }
    .max(24.0)
    .floor();
    document.size(&format!("{id}-gauge-ring"), ring_size, ring_size)?;
    document.visible(&format!("{id}-gauge-copy"), side_by_side)?;
    document.visible(&format!("{id}-gauge-caption"), !side_by_side && show_title)?;
    document.visible(&format!("{id}-gauge-center"), !side_by_side && show_percent)?;
    document.visible(&format!("{id}-gauge-title"), show_title)?;
    document.visible(&format!("{id}-gauge-value"), show_percent)?;
    document.visible(
        &format!("{id}-gauge-detail"),
        show_details && !detail.is_empty(),
    )?;
    document.text_ref(&format!("{id}-gauge-title"), title)?;
    document.text_ref(&format!("{id}-gauge-caption"), title)?;
    document.text_ref(&format!("{id}-gauge-value"), &percent_text)?;
    document.text_ref(&format!("{id}-gauge-percent"), &percent_text)?;
    document.text(&format!("{id}-gauge-detail"), detail)?;
    document.font_size(
        &format!("{id}-gauge-percent"),
        (ring_size * 0.22).clamp(10.0, 40.0),
    )?;
    for part in ["title", "caption", "value", "detail", "percent"] {
        document.foreground(&format!("{id}-gauge-{part}"), text_color)?;
    }
    let key = RingKey {
        size: ring_size as u32,
        thickness: widget.value("thickness") as u32,
        percent: percent.unwrap_or(0),
        ring: widget.color_or_accent("ringColor", projection_accent(state)),
        track: widget.color("trackColor"),
    };
    if state.widgets.rings[slot] != Some(key) {
        document.image_rgba8(
            &format!("{id}-gauge-image"),
            RuntimeImage {
                source: format!("{id}-ring-{}-{}", key.size, key.percent),
                width: key.size,
                height: key.size,
                pixels: ring_pixels(
                    key.size,
                    key.thickness as f32,
                    key.percent,
                    key.ring,
                    key.track,
                ),
            },
        )?;
        state.widgets.rings[slot] = Some(key);
    }
    Ok(())
}

/// Draws one line per series into a straight RGBA8 buffer. Values are percentages
/// (0..=100) spread across the full width, one per tick.
pub(super) fn history_pixels(width: u32, height: u32, series: &[(&VecDeque<u8>, Rgb)]) -> Vec<u8> {
    let (width, height) = (width.max(2), height.max(2));
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let max_x = (width - 1) as f32;
    let max_y = (height - 1) as f32;
    for (values, color) in series {
        let count = values.len();
        if count == 0 {
            continue;
        }
        let point = |index: usize, value: u8| {
            let x = if count == 1 {
                max_x
            } else {
                index as f32 * max_x / (HISTORY_LEN - 1) as f32
            };
            let y = max_y - f32::from(value.min(100)) / 100.0 * max_y;
            (x, y)
        };
        let points: Vec<(f32, f32)> = values
            .iter()
            .enumerate()
            .map(|(index, value)| point(index + HISTORY_LEN - count, *value))
            .collect();
        for pair in points.windows(2) {
            let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
            let steps = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0) as u32;
            for step in 0..=steps {
                let t = step as f32 / steps as f32;
                let x = (x0 + (x1 - x0) * t).round() as u32;
                let y = (y0 + (y1 - y0) * t).round() as u32;
                let index = ((y.min(height - 1) * width + x.min(width - 1)) * 4) as usize;
                pixels[index] = color.red;
                pixels[index + 1] = color.green;
                pixels[index + 2] = color.blue;
                pixels[index + 3] = 255;
            }
        }
    }
    pixels
}

pub(super) fn sync_history(
    document: &mut impl UiDocumentAccess,
    graph: &mut Option<GraphKey>,
    samples: &Samples,
    accent: Rgb,
    slot: usize,
    widget: &Widget,
) -> Result<(), String> {
    let mut series: Vec<(&VecDeque<u8>, Rgb)> = Vec::with_capacity(2);
    if widget.flag("showMemory") {
        series.push((&samples.memory_history, widget.color("memoryColor")));
    }
    if widget.flag("showCpu") {
        series.push((
            &samples.cpu_history,
            widget.color_or_accent("cpuColor", accent),
        ));
    }
    sync_graph(document, graph, slot, widget, "history", &series)
}

pub(super) fn sync_network(
    document: &mut impl UiDocumentAccess,
    graph: &mut Option<GraphKey>,
    samples: &Samples,
    accent: Rgb,
    slot: usize,
    widget: &Widget,
) -> Result<(), String> {
    let rx = scale_rates(&samples.net_rx_history);
    let tx = scale_rates(&samples.net_tx_history);
    let mut series: Vec<(&VecDeque<u8>, Rgb)> = Vec::with_capacity(2);
    if widget.flag("showTx") {
        series.push((&tx, widget.color("txColor")));
    }
    if widget.flag("showRx") {
        series.push((&rx, widget.color_or_accent("rxColor", accent)));
    }
    sync_graph(document, graph, slot, widget, "network", &series)
}

/// Shared sparkline path: sizes `widget-N-<prefix>-image` to the card and draws `series`.
/// The image is rebuilt only when its size, colours or samples differ from `graph`.
fn sync_graph(
    document: &mut impl UiDocumentAccess,
    graph: &mut Option<GraphKey>,
    slot: usize,
    widget: &Widget,
    prefix: &'static str,
    series: &[(&VecDeque<u8>, Rgb)],
) -> Result<(), String> {
    let id = format!("widget-{}-{prefix}-image", slot + 1);
    let width = (widget.rect.width as f32 - WIDGET_PADDING * 2.0)
        .max(2.0)
        .floor() as u32;
    let height = (widget.rect.height as f32 - WIDGET_PADDING * 2.0)
        .max(2.0)
        .floor() as u32;
    document.size(&id, width as f32, height as f32)?;
    let key = GraphKey {
        prefix,
        width,
        height,
        series: series
            .iter()
            .map(|(values, color)| (values.iter().copied().collect(), *color))
            .collect(),
    };
    if graph.as_ref() == Some(&key) {
        return Ok(());
    }
    document.image_rgba8(
        &id,
        RuntimeImage {
            source: format!("{id}-{width}x{height}"),
            width,
            height,
            pixels: history_pixels(width, height, series),
        },
    )?;
    *graph = Some(key);
    Ok(())
}

pub(super) const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
pub(super) const WEEKDAY_INITIALS: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];
/// Shown in place of data that is not available; keeps single-text-node targets valid.
pub(super) const EMPTY_TEXT: &str = "\u{200b}";

/// Forecast from the shell's shared feed, the same payload and options the tray reads. Without
/// a published feed the widget shows its "No data" state; unset or off states use the tray's text.
pub(super) fn sync_weather(
    document: &mut impl UiDocumentAccess,
    id: &str,
    widget: &Widget,
    weather: &flamewm_api::weather::WeatherFeedSnapshot,
    accent: Rgb,
) -> Result<(), String> {
    let text = color(widget.color("textColor"), 255);
    let accent = color(widget.color_or_accent("accentColor", accent), 255);
    let view = flamewm_desktop_core::weather_feed::weather_widget_view(weather);
    // City name is one line at 15px semibold: ellipsise it to the card's inner width.
    // The width estimate reads semibold ~8% narrow, so keep a 10% margin.
    let city_width = (widget.rect.width as f32 - WIDGET_PADDING * 2.0) * 0.9;
    document.text(
        &format!("{id}-weather-city"),
        ellipsize(&view.location, 15.0, true, city_width),
    )?;
    document.text_ref(&format!("{id}-weather-temp"), &view.temperature)?;
    document.text_ref(&format!("{id}-weather-condition"), &view.condition_text)?;
    document.text_ref(&format!("{id}-weather-range"), &view.range)?;
    document.foreground(&format!("{id}-weather-city"), text)?;
    document.foreground(&format!("{id}-weather-temp"), text)?;
    document.foreground(&format!("{id}-weather-condition"), text)?;
    document.foreground(&format!("{id}-weather-range"), text)?;
    document.text(
        &format!("{id}-weather-attr"),
        view.attribution.unwrap_or(EMPTY_TEXT).to_owned(),
    )?;
    document.visible(&format!("{id}-weather-attr"), view.attribution.is_some())?;
    // Cards under 300 px wide (one 2x2 cell) have no room beside the city name.
    document.visible(&format!("{id}-weather-right"), widget.rect.width >= 300)?;
    document.visible(&format!("{id}-weather-hours"), widget.flag("showHourly"))?;
    for slot in 0..flamewm_desktop_core::weather_feed::WEATHER_STRIP_DAYS {
        let hour = slot + 1;
        let (day, high) = match view.days.get(slot) {
            Some(day) => (day.weekday, day.high.clone()),
            None => (EMPTY_TEXT, EMPTY_TEXT.to_owned()),
        };
        document.text_ref(&format!("{id}-weather-hour-{hour}-time"), day)?;
        document.text(&format!("{id}-weather-hour-{hour}-temp"), high)?;
        document.foreground(&format!("{id}-weather-hour-{hour}-time"), accent)?;
        document.foreground(&format!("{id}-weather-hour-{hour}-temp"), text)?;
    }
    Ok(())
}

pub(super) fn sync_calendar(
    document: &mut impl UiDocumentAccess,
    id: &str,
    widget: &Widget,
    time: WallTime,
    accent: Rgb,
) -> Result<(), String> {
    let text = color(widget.color("textColor"), 255);
    let clear = color(widget.color("background"), 0);
    let marker = color(widget.color_or_accent("accentColor", accent), 255);
    let ink = color(flamewm_desktop_core::sticky::Rgb::new(16, 16, 16), 255);
    let month = usize::from(time.month.clamp(1, 12)) - 1;
    document.visible(&format!("{id}-cal-month"), widget.flag("showMonth"))?;
    document.text(
        &format!("{id}-cal-month"),
        MONTH_NAMES[month].to_uppercase(),
    )?;
    document.foreground(&format!("{id}-cal-month"), text)?;
    for (column, initial) in WEEKDAY_INITIALS.iter().enumerate() {
        let cell = format!("{id}-cal-wd-{column}");
        document.text(&cell, (*initial).to_owned())?;
        document.foreground(&cell, text)?;
    }
    let weeks = flamewm_desktop_core::widgets::calendar_weeks(time.year, u32::from(time.month));
    for (row, days) in weeks.iter().enumerate() {
        for (column, day) in days.iter().enumerate() {
            let cell = format!("{id}-cal-{row}-{column}");
            let today = *day != 0 && *day == u32::from(time.day);
            let label = if *day == 0 {
                EMPTY_TEXT.to_owned()
            } else {
                day.to_string()
            };
            document.text(&cell, label)?;
            document.foreground(&cell, if today { ink } else { text })?;
            document.background(&cell, if today { marker } else { clear })?;
        }
    }
    Ok(())
}

pub(super) fn sync_media(
    document: &mut impl UiDocumentAccess,
    id: &str,
    widget: &Widget,
    media: &MediaSnapshot,
    accent: Rgb,
) -> Result<(), String> {
    let available =
        media.availability == ServiceAvailability::Available && !media.active_bus_name.is_empty();
    let title = if available && !media.title.is_empty() {
        media.title.clone()
    } else if available {
        "Unknown track".to_owned()
    } else {
        "Nothing playing".to_owned()
    };
    let player = if available && !media.identity.is_empty() {
        media.identity.to_uppercase()
    } else {
        "MEDIA".to_owned()
    };
    document.text(&format!("{id}-media-player"), player)?;
    document.text(&format!("{id}-media-title"), title)?;
    document.text_ref(&format!("{id}-media-artist"), &media.artist)?;
    document.visible(&format!("{id}-media-player"), widget.flag("showPlayer"))?;
    document.visible(
        &format!("{id}-media-artist"),
        widget.flag("showArtist") && available && !media.artist.is_empty(),
    )?;
    document.visible(
        &format!("{id}-media-controls"),
        widget.flag("showControls") && available,
    )?;
    let playing = media.playback == PlaybackState::Playing;
    document.visible(&format!("{id}-media-play-icon"), !playing)?;
    document.visible(&format!("{id}-media-pause-icon"), playing)?;
    document.visible(&format!("{id}-media-prev"), media.can_previous)?;
    document.visible(&format!("{id}-media-next"), media.can_next)?;
    let text = color(widget.color("textColor"), 255);
    document.foreground(&format!("{id}-media-title"), text)?;
    document.foreground(&format!("{id}-media-artist"), text)?;
    document.foreground(
        &format!("{id}-media-player"),
        color(widget.color_or_accent("accentColor", accent), 255),
    )
}

/// Cards narrower than this (one grid cell) use their compact layout.
pub(super) const COMPACT_CARD_WIDTH: i32 = 150;

/// Font size for `chars` glyphs (about 0.62 em each) across a card's inner width,
/// never above `configured` and never below 9 px.
pub(super) fn fitted_font_size(configured: f32, chars: usize, card_width: f32) -> f32 {
    let inner = card_width - WIDGET_PADDING * 2.0;
    let fit = inner / (chars.max(1) as f32 * 0.62);
    configured.min(fit).max(9.0).floor()
}

/// Shortens `text` with a trailing ellipsis until it fits `max_width`.
pub(super) fn ellipsize(text: &str, size: f32, bold: bool, max_width: f32) -> String {
    if estimate_text_width(text, size, bold) <= max_width {
        return text.to_owned();
    }
    let mut kept: Vec<char> = text.chars().collect();
    while kept.pop().is_some() {
        let candidate: String = kept.iter().collect::<String>() + "…";
        if estimate_text_width(&candidate, size, bold) <= max_width {
            return candidate;
        }
    }
    "…".to_owned()
}

pub(super) fn sync_custom(
    document: &mut impl UiDocumentAccess,
    id: &str,
    widget: &Widget,
    placeholders: &Placeholders,
) -> Result<(), String> {
    let base_color = widget.color("textColor");
    let base = SpanStyle {
        color: Some(base_color),
        background: None,
        bold: widget.flag("bold"),
    };
    let lines = fit_lines(parse_markup(&widget.template, base));
    let inner_width = widget.rect.width as f32 - WIDGET_PADDING * 2.0;
    let configured = widget.value("textSize") as f32;
    // Shrink the text when the widest line would overflow the card, down to 9 px.
    let widest = lines
        .iter()
        .map(|spans| {
            spans
                .iter()
                .map(|span| {
                    let text = expand_placeholders(&span.text, placeholders);
                    estimate_text_width(&text, configured, span.style.bold)
                })
                .sum::<f32>()
        })
        .fold(0.0_f32, f32::max);
    let size = if widest > inner_width {
        (configured * inner_width / widest)
            .floor()
            .clamp(9.0, configured)
    } else {
        configured
    };
    let align = widget.align();
    for line_index in 0..MAX_TEMPLATE_LINES {
        let line_id = format!("{id}-line-{}", line_index + 1);
        let line = lines.get(line_index);
        document.visible(&line_id, line.is_some())?;
        set_alignment(document, &line_id, align)?;
        for span_index in 0..MAX_LINE_SPANS {
            let span_id = format!("{line_id}-span-{}", span_index + 1);
            let Some(span) = line.and_then(|spans| spans.get(span_index)) else {
                document.visible(&span_id, false)?;
                continue;
            };
            let text = ellipsize(
                &expand_placeholders(&span.text, placeholders),
                size,
                span.style.bold,
                inner_width,
            );
            document.size(
                &span_id,
                estimate_text_width(&text, size, span.style.bold),
                (size * 1.3).ceil(),
            )?;
            document.text(
                &span_id,
                if text.is_empty() {
                    "\u{200b}".to_owned()
                } else {
                    display_text(&text)
                },
            )?;
            document.font_size(&span_id, size)?;
            document.font_weight(&span_id, if span.style.bold { 700 } else { 400 })?;
            document.foreground(&span_id, color(span.style.color.unwrap_or(base_color), 255))?;
            match span.style.background {
                Some(background) => document.background(&span_id, color(background, 255))?,
                None => document.background_clear(&span_id)?,
            }
            document.visible(&span_id, true)?;
        }
    }
    Ok(())
}

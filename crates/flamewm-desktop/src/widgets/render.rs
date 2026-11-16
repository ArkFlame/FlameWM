use super::*;

/// Values one widget card hands the document: fill, backdrop blur and corner radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WidgetSurface {
    pub(crate) fill: UiColor,
    pub(crate) blur: Option<u8>,
    pub(crate) radius: f32,
}

/// Cards without `surface` (the default) draw no fill and request no backdrop blur, so their
/// content sits on the wallpaper. Inheriting cards take the global surface fill and blur;
/// others use their own options.
pub(crate) fn widget_surface(widget: &Widget, settings: &SettingsSnapshot) -> WidgetSurface {
    let radius = WIDGET_RADIUS;
    if !widget.has_surface() {
        return WidgetSurface {
            fill: UiColor {
                r: 0,
                g: 0,
                b: 0,
                a: 0,
            },
            blur: None,
            radius,
        };
    }
    let blur = projection::surface_blur_percent(settings);
    let blur = (blur > 0).then_some(blur);
    if widget.inherits_surface() {
        let fill = flamewm_ui_core::surface::surface_fill(settings);
        WidgetSurface {
            fill: UiColor {
                r: (fill.rgb >> 16) as u8,
                g: (fill.rgb >> 8) as u8,
                b: fill.rgb as u8,
                a: fill.alpha,
            },
            blur,
            radius,
        }
    } else {
        WidgetSurface {
            fill: color(
                widget.color("background"),
                percent_to_alpha(widget.value("backgroundOpacity")),
            ),
            blur,
            radius,
        }
    }
}

pub(crate) fn assign_layers(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    for slot in 1..=MAX_WIDGETS {
        document.layer(&format!("widget-{slot}"), UiLayer::Content)?;
    }
    // Drag ghost sits above every widget slot while a gesture is live.
    document.layer("widget-ghost", UiLayer::Overlay)?;
    for index in 0..WIDGET_OVERLAY_CELLS {
        document.layer(&format!("widget-slot-{index}"), UiLayer::Overlay)?;
    }
    document.layer("widget-menu", UiLayer::Popover)?;
    document.layer("widget-gallery", UiLayer::Popover)?;
    document.layer("widget-config", UiLayer::Popover)
}

pub(crate) fn hide_panels(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    for id in ["widget-menu", "widget-gallery", "widget-config"] {
        document.visible(id, false)?;
        projection::clear_menu_backdrop(document, id)?;
    }
    Ok(())
}

pub(super) fn set_alignment(
    document: &mut impl UiDocumentAccess,
    line: &str,
    align: TextAlign,
) -> Result<(), String> {
    document.visible(&format!("{line}-lead"), align != TextAlign::Left)?;
    document.visible(&format!("{line}-trail"), align != TextAlign::Right)
}

pub(super) fn widget_placeholders(state: &DesktopState, time: WallTime) -> Placeholders {
    let widgets = &state.widgets;
    Placeholders {
        username: widgets.username.clone(),
        hostname: widgets.hostname.clone(),
        time: format_time(time, true, false),
        date: format_date(time, false, true),
        weekday: time.weekday_name().to_owned(),
        cpu_percent: widgets.samples.cpu_percent,
        memory_percent: widgets.samples.memory.and_then(MemoryUsage::percent),
        uptime: widgets
            .samples
            .uptime_seconds
            .map(format_uptime)
            .unwrap_or_default(),
    }
}

pub(crate) fn sync_widgets(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    // Covered: skip every write and never store the key, so the resume sync recomputes it.
    if state.widgets.obscured {
        return Ok(());
    }
    let time = local_wall_time();
    let placeholders = widget_placeholders(state, time);
    let widgets = state.widgets.store.widgets();
    // Wall time only reaches the document through time-showing widgets; without one, a
    // second-by-second clock must not force a sync.
    let needs_time = widgets.iter().any(|widget| {
        matches!(
            widget.kind,
            WidgetKind::Clock | WidgetKind::Calendar | WidgetKind::Custom
        )
    });
    let mut key_placeholders = placeholders.clone();
    if !needs_time {
        key_placeholders.time.clear();
        key_placeholders.date.clear();
        key_placeholders.weekday.clear();
    }
    let key = SyncKey {
        widgets: widgets.to_vec(),
        settings: state.settings.clone(),
        weather: state.weather.clone(),
        media: state.widgets.media.clone(),
        editing: state
            .widgets
            .editing
            .as_ref()
            .map(|edit| (edit.widget, edit.buffer.clone())),
        placeholders: key_placeholders,
        time: needs_time.then_some(time),
        samples: state.widgets.sample_key(),
    };
    if state.widgets.sync_key.as_ref() == Some(&key) {
        return Ok(());
    }
    for slot in 0..MAX_WIDGETS {
        let id = format!("widget-{}", slot + 1);
        let Some(widget) = state.widgets.store.widgets().get(slot).cloned() else {
            document.visible(&id, false)?;
            state.widgets.rings[slot] = None;
            state.widgets.graphs[slot] = None;
            state.widgets.faces[slot] = None;
            continue;
        };
        document.position(&id, widget.rect.x as f32, widget.rect.y as f32)?;
        document.size(&id, widget.rect.width as f32, widget.rect.height as f32)?;
        let surface = widget_surface(&widget, &state.settings);
        document.background(&id, surface.fill)?;
        document.corner_radius(&id, surface.radius)?;
        document.node_backdrop(&id, surface.blur)?;
        for edge in WIDGET_EDGES {
            document.visible(&format!("{id}-edge-{edge}"), !widget.locked)?;
        }
        let analog = is_analog(&widget);
        document.visible(
            &format!("{id}-clock"),
            widget.kind == WidgetKind::Clock && !analog,
        )?;
        document.visible(&format!("{id}-analog"), analog)?;
        document.visible(
            &format!("{id}-gauge"),
            matches!(
                widget.kind,
                WidgetKind::Cpu | WidgetKind::Memory | WidgetKind::Ring
            ),
        )?;
        document.visible(&format!("{id}-history"), widget.kind == WidgetKind::History)?;
        document.visible(&format!("{id}-network"), widget.kind == WidgetKind::Network)?;
        document.visible(&format!("{id}-media"), widget.kind == WidgetKind::Media)?;
        document.visible(&format!("{id}-weather"), widget.kind == WidgetKind::Weather)?;
        document.visible(
            &format!("{id}-calendar"),
            widget.kind == WidgetKind::Calendar,
        )?;
        let editing = state
            .widgets
            .editing
            .as_ref()
            .filter(|edit| edit.widget == widget.id)
            .map(|edit| edit.buffer.clone());
        document.visible(
            &format!("{id}-custom"),
            widget.kind == WidgetKind::Custom && editing.is_none(),
        )?;
        document.visible(&format!("{id}-editor"), editing.is_some())?;
        let accent = projection_accent(state);
        match widget.kind {
            WidgetKind::Clock if analog => sync_analog(document, state, slot, &widget, time)?,
            WidgetKind::Clock => sync_clock(document, &id, &widget, time, accent)?,
            WidgetKind::Cpu | WidgetKind::Memory | WidgetKind::Ring => {
                sync_gauge(document, state, slot, &widget)?;
            }
            WidgetKind::Media => sync_media(document, &id, &widget, &state.widgets.media, accent)?,
            WidgetKind::Weather => sync_weather(document, &id, &widget, &state.weather, accent)?,
            WidgetKind::Calendar => sync_calendar(document, &id, &widget, time, accent)?,
            WidgetKind::History => sync_history(
                document,
                &mut state.widgets.graphs[slot],
                &state.widgets.samples,
                accent,
                slot,
                &widget,
            )?,
            WidgetKind::Network => sync_network(
                document,
                &mut state.widgets.graphs[slot],
                &state.widgets.samples,
                accent,
                slot,
                &widget,
            )?,
            WidgetKind::Custom => {
                if let Some(buffer) = editing {
                    document.text(
                        &format!("{id}-editor"),
                        projection::caret_text(&buffer, true),
                    )?;
                } else {
                    sync_custom(document, &id, &widget, &placeholders)?;
                }
            }
        }
        document.visible(&id, true)?;
    }
    state.widgets.sync_key = Some(key);
    Ok(())
}

pub(super) fn projection_accent(state: &DesktopState) -> Rgb {
    let accent = crate::entry::accent_color(&state.settings);
    Rgb::new(accent.r, accent.g, accent.b)
}

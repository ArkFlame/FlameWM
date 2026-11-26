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
    widget_surface_from(widget, &surface_key(settings))
}

/// Resolves the settings-derived inputs once per sync. `surface_fill` reads several settings
/// keys and `surface_blur_percent` another, so the per-slot chrome pass reuses this instead of
/// repeating the lookups for every card.
fn surface_key(settings: &SettingsSnapshot) -> SurfaceKey {
    let blur = projection::surface_blur_percent(settings);
    let fill = flamewm_ui_core::surface::surface_fill(settings);
    SurfaceKey {
        blur: (blur > 0).then_some(blur),
        fill: UiColor {
            r: (fill.rgb >> 16) as u8,
            g: (fill.rgb >> 8) as u8,
            b: fill.rgb as u8,
            a: fill.alpha,
        },
    }
}

fn widget_surface_from(widget: &Widget, surface: &SurfaceKey) -> WidgetSurface {
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
    if widget.inherits_surface() {
        WidgetSurface {
            fill: surface.fill,
            blur: surface.blur,
            radius,
        }
    } else {
        WidgetSurface {
            fill: color(
                widget.color("background"),
                percent_to_alpha(widget.value("backgroundOpacity")),
            ),
            blur: surface.blur,
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

/// True for widget kinds whose drawn output reads the wall clock.
fn uses_wall_time(widget: &Widget) -> bool {
    matches!(
        widget.kind,
        WidgetKind::Clock | WidgetKind::Calendar | WidgetKind::Custom
    )
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
    // Wall time only reaches the document through time-showing widgets; without one, a
    // second-by-second clock must not force a sync.
    let needs_time = state.widgets.store.widgets().iter().any(uses_wall_time);
    // Compare the stored key field by field before formatting anything: an unchanged turn then
    // costs no settings/widget snapshot clone, no date/time formatting and no allocation.
    let time_key = needs_time.then_some(time);
    if let Some(key) = state.widgets.sync_key.as_ref() {
        if sync_key_matches(key, state, state.widgets.store.widgets(), time_key) {
            return Ok(());
        }
        // Only the wall clock differs. Every card's chrome and every non-time widget's content
        // already match the document, so redraw just the time-reading slots and refresh the
        // stored time instead of cloning the settings, widgets, weather and media snapshots.
        // This is the steady state of a seconds clock: one second costs one clock slot, not a
        // full-document rewrite of every widget.
        if needs_time && sync_key_inputs_match(key, state, state.widgets.store.widgets()) {
            let accent = projection_accent(state);
            let placeholders = widget_placeholders_if_used(state, time);
            for slot in 0..MAX_WIDGETS {
                let Some(widget) = state.widgets.store.widgets().get(slot).cloned() else {
                    continue;
                };
                if !uses_wall_time(&widget) {
                    continue;
                }
                let id = format!("widget-{}", slot + 1);
                let editing = editing_buffer(state, &widget);
                sync_card_content(
                    document,
                    state,
                    slot,
                    &id,
                    &widget,
                    time,
                    accent,
                    placeholders.as_ref(),
                    editing.as_deref(),
                )?;
            }
            if let Some(key) = state.widgets.sync_key.as_mut() {
                key.time = time_key;
            }
            return Ok(());
        }
    }
    let placeholders = widget_placeholders_if_used(state, time);
    let accent = projection_accent(state);
    let surface = surface_key(&state.settings);
    let key = SyncKey {
        widgets: state.widgets.store.widgets().to_vec(),
        surface,
        accent,
        weather: state.weather.clone(),
        media: state.widgets.media.clone(),
        editing: state
            .widgets
            .editing
            .as_ref()
            .map(|edit| (edit.widget, edit.buffer.clone())),
        username: state.widgets.username.clone(),
        hostname: state.widgets.hostname.clone(),
        time: time_key,
        samples: state.widgets.sample_key(),
    };
    for slot in 0..MAX_WIDGETS {
        let id = format!("widget-{}", slot + 1);
        let Some(widget) = state.widgets.store.widgets().get(slot).cloned() else {
            document.visible(&id, false)?;
            state.widgets.rings[slot] = None;
            state.widgets.graphs[slot] = None;
            state.widgets.faces[slot] = None;
            continue;
        };
        let editing = editing_buffer(state, &widget);
        sync_card_chrome(document, &id, &widget, surface, editing.as_deref())?;
        sync_card_content(
            document,
            state,
            slot,
            &id,
            &widget,
            time,
            accent,
            placeholders.as_ref(),
            editing.as_deref(),
        )?;
        document.visible(&id, true)?;
    }
    state.widgets.sync_key = Some(key);
    Ok(())
}

/// Placeholders are only read by custom widgets; formatting them for a clock tick otherwise
/// allocates five strings per second for nothing.
fn widget_placeholders_if_used(state: &DesktopState, time: WallTime) -> Option<Placeholders> {
    state
        .widgets
        .store
        .widgets()
        .iter()
        .any(|widget| widget.kind == WidgetKind::Custom)
        .then(|| widget_placeholders(state, time))
}

/// Live template buffer of `widget`, when it is being edited.
fn editing_buffer(state: &DesktopState, widget: &Widget) -> Option<String> {
    state
        .widgets
        .editing
        .as_ref()
        .filter(|edit| edit.widget == widget.id)
        .map(|edit| edit.buffer.clone())
}

/// Writes the card's geometry, surface and part visibility. Every input here is part of the
/// sync key, so the time-only fast path may skip this whole block.
fn sync_card_chrome(
    document: &mut impl UiDocumentAccess,
    id: &str,
    widget: &Widget,
    surface: SurfaceKey,
    editing: Option<&str>,
) -> Result<(), String> {
    document.position(id, widget.rect.x as f32, widget.rect.y as f32)?;
    document.size(id, widget.rect.width as f32, widget.rect.height as f32)?;
    let surface = widget_surface_from(widget, &surface);
    document.background(id, surface.fill)?;
    document.corner_radius(id, surface.radius)?;
    document.node_backdrop(id, surface.blur)?;
    for edge in WIDGET_EDGES {
        document.visible(&format!("{id}-edge-{edge}"), !widget.locked)?;
    }
    let analog = is_analog(widget);
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
    document.visible(
        &format!("{id}-custom"),
        widget.kind == WidgetKind::Custom && editing.is_none(),
    )?;
    document.visible(&format!("{id}-editor"), editing.is_some())?;
    Ok(())
}

/// Writes the kind-specific content of `widget`. `accent` is resolved once per sync so the
/// per-slot loop does not re-parse the settings accent colour for every card.
fn sync_card_content(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    slot: usize,
    id: &str,
    widget: &Widget,
    time: WallTime,
    accent: Rgb,
    placeholders: Option<&Placeholders>,
    editing: Option<&str>,
) -> Result<(), String> {
    match widget.kind {
        WidgetKind::Clock if is_analog(widget) => {
            sync_analog(document, state, slot, widget, time, accent)?;
        }
        WidgetKind::Clock => sync_clock(document, id, widget, time, accent)?,
        WidgetKind::Cpu | WidgetKind::Memory | WidgetKind::Ring => {
            sync_gauge(document, state, slot, widget, accent)?;
        }
        WidgetKind::Media => sync_media(document, id, widget, &state.widgets.media, accent)?,
        WidgetKind::Weather => sync_weather(document, id, widget, &state.weather, accent)?,
        WidgetKind::Calendar => sync_calendar(document, id, widget, time, accent)?,
        WidgetKind::History => sync_history(
            document,
            &mut state.widgets.graphs[slot],
            &state.widgets.samples,
            accent,
            slot,
            widget,
        )?,
        WidgetKind::Network => sync_network(
            document,
            &mut state.widgets.graphs[slot],
            &state.widgets.samples,
            accent,
            slot,
            widget,
        )?,
        WidgetKind::Custom => {
            if let Some(buffer) = editing {
                document.text(
                    &format!("{id}-editor"),
                    projection::caret_text(buffer, true),
                )?;
            } else if let Some(placeholders) = placeholders {
                sync_custom(document, id, widget, placeholders)?;
            }
        }
    }
    Ok(())
}

pub(super) fn projection_accent(state: &DesktopState) -> Rgb {
    let accent = crate::entry::accent_color(&state.settings);
    Rgb::new(accent.r, accent.g, accent.b)
}

/// Field-wise equality between the last stored sync key and the live inputs. Equivalent to
/// `SyncKey` equality, but compares borrowed state so an unchanged turn clones and formats
/// nothing (the placeholder text is derived from these raw inputs).
fn sync_key_matches(
    key: &SyncKey,
    state: &DesktopState,
    widgets: &[Widget],
    time: Option<WallTime>,
) -> bool {
    key.time == time && sync_key_inputs_match(key, state, widgets)
}

/// `sync_key_matches` without the wall clock: true when the only changed input is time.
fn sync_key_inputs_match(key: &SyncKey, state: &DesktopState, widgets: &[Widget]) -> bool {
    key.widgets.as_slice() == widgets
        && key.surface == surface_key(&state.settings)
        && key.accent == projection_accent(state)
        && key.weather == state.weather
        && key.media == state.widgets.media
        && key
            .editing
            .as_ref()
            .map(|(widget, buffer)| (*widget, buffer.as_str()))
            == state
                .widgets
                .editing
                .as_ref()
                .map(|edit| (edit.widget, edit.buffer.as_str()))
        && key.username == state.widgets.username
        && key.hostname == state.widgets.hostname
        && WidgetState::key_is_current(&key.samples, &state.widgets.samples)
}

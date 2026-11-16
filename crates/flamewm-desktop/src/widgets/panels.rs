use super::*;

pub(crate) fn hide_panels(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    for id in ["widget-menu", "widget-gallery", "widget-config"] {
        document.visible(id, false)?;
        projection::clear_menu_backdrop(document, id)?;
    }
    Ok(())
}

pub(super) fn selected_widget(state: &DesktopState) -> Option<u32> {
    state
        .menu
        .as_ref()
        .filter(|menu| matches!(menu.kind, MenuKind::Widget | MenuKind::WidgetConfig))
        .and_then(|menu| menu.selected_id.as_deref())
        .and_then(|id| id.parse().ok())
}

pub(super) fn open_panel(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    kind: MenuKind,
    anchor: Point,
    widget: Option<u32>,
) -> Result<(), String> {
    let (output, anchor) = pointer_on_output(state, anchor.x as f32, anchor.y as f32);
    state.menu = Some(MenuState {
        kind,
        anchor,
        output,
        selected_id: widget.map(|id| id.to_string()),
    });
    state.pending_delete = None;
    sync_menu(document, state)
}

pub(crate) fn open_gallery(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    anchor: Point,
) -> Result<(), String> {
    commit_edit(state);
    open_panel(document, state, MenuKind::WidgetGallery, anchor, None)
}

pub(super) fn panel_rect(work_area: Rect, size: (i32, i32)) -> Rect {
    let width = size.0.min(work_area.width);
    let height = size.1.min(work_area.height);
    Rect::new(
        work_area.x + (work_area.width - width) / 2,
        work_area.y + (work_area.height - height) / 2,
        width,
        height,
    )
}

pub(crate) fn place_gallery(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    work_area: Rect,
) -> Result<(), String> {
    let placed = panel_rect(work_area, GALLERY_SIZE);
    let count = state.widgets.store.widgets().len();
    document.text(
        "widget-gallery-count",
        if count >= MAX_WIDGETS {
            format!("All {MAX_WIDGETS} widget slots are in use; remove one to add another")
        } else {
            format!("{count} of {MAX_WIDGETS} placed. Right-click a widget to configure it")
        },
    )?;
    let time = local_wall_time();
    document.text("widget-card-clock-time", format_time(time, true, false))?;
    document.text("widget-card-clock-date", format_date(time, true, false))?;
    let accent = projection_accent(state);
    for (kind, percent) in [
        (
            WidgetKind::Cpu,
            state.widgets.samples.cpu_percent.unwrap_or(42),
        ),
        (
            WidgetKind::Memory,
            state
                .widgets
                .samples
                .memory
                .and_then(MemoryUsage::percent)
                .unwrap_or(63),
        ),
    ] {
        let preview = Widget::new(0, kind, Point::new(0, 0));
        document.image_rgba8(
            &format!("widget-card-{}-image", kind.id()),
            RuntimeImage {
                source: format!("widget-card-{}-{percent}", kind.id()),
                width: CARD_RING_SIZE,
                height: CARD_RING_SIZE,
                pixels: ring_pixels(
                    CARD_RING_SIZE,
                    8.0,
                    percent,
                    if kind == WidgetKind::Cpu {
                        accent
                    } else {
                        preview.color_or_accent("ringColor", accent)
                    },
                    preview.color("trackColor"),
                ),
            },
        )?;
    }
    let full = state.widgets.store.is_full();
    for kind in WidgetKind::ALL {
        document.visible(&format!("widget-card-{}-add", kind.id()), !full)?;
    }
    document.position("widget-gallery", placed.x as f32, placed.y as f32)?;
    document.size("widget-gallery", placed.width as f32, placed.height as f32)?;
    projection::project_menu_surface(document, "widget-gallery", &state.settings)?;
    state.menu_rects.push(placed);
    document.visible("widget-gallery", true)
}

pub(crate) fn place_config(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    work_area: Rect,
) -> Result<(), String> {
    let Some(widget) = selected_widget(state).and_then(|id| state.widgets.store.get(id).cloned())
    else {
        state.menu = None;
        return Ok(());
    };
    let options = widget.kind.visible_options(widget.stored_values());
    for row in 0..CONFIG_ROWS {
        let row_id = format!("widget-config-row-{row}");
        let Some(spec) = options.get(row) else {
            document.visible(&row_id, false)?;
            continue;
        };
        let value = widget.value(spec.key);
        document.text_ref(&format!("{row_id}-label"), spec.label)?;
        document.text(&format!("{row_id}-value"), spec.label_for(value))?;
        let is_color = matches!(
            spec.control,
            OptionControl::Color | OptionControl::AccentColor
        );
        document.visible(&format!("{row_id}-swatch"), is_color)?;
        if is_color {
            let swatch = if value == flamewm_widgets_core::widgets::ACCENT_COLOR {
                projection_accent(state)
            } else {
                palette_color(value)
            };
            document.background(&format!("{row_id}-swatch"), color(swatch, 255))?;
        }
        document.visible(&row_id, true)?;
    }
    document.text(
        "widget-config-title",
        format!("{} settings", widget.kind.title()),
    )?;
    let rows = options.len().min(CONFIG_ROWS) as i32;
    let size = (
        CONFIG_WIDTH,
        CONFIG_HEADER_HEIGHT + rows * CONFIG_ROW_HEIGHT + 16,
    );
    let beside = if widget.rect.right() + 12 + size.0 <= work_area.right() {
        widget.rect.right() + 12
    } else {
        widget.rect.x - 12 - size.0
    };
    let placed = Rect::new(beside, widget.rect.y, size.0, size.1).clamp_inside(work_area);
    document.position("widget-config", placed.x as f32, placed.y as f32)?;
    document.size("widget-config", placed.width as f32, placed.height as f32)?;
    projection::project_menu_surface(document, "widget-config", &state.settings)?;
    state.menu_rects.push(placed);
    document.visible("widget-config", true)
}

pub(crate) fn widget_menu_rows(
    document: &mut impl UiDocumentAccess,
    state: &DesktopState,
) -> Result<usize, String> {
    let Some(widget) = selected_widget(state).and_then(|id| state.widgets.store.get(id)) else {
        return Ok(0);
    };
    let custom = widget.kind == WidgetKind::Custom;
    document.visible("widget-row-edit", custom)?;
    document.text(
        "widget-row-lock-label",
        if widget.locked {
            "Unlock Position"
        } else {
            "Lock Position"
        }
        .to_owned(),
    )?;
    Ok(if custom { 5 } else { 4 })
}

use super::*;

pub(crate) fn hide_menus(
    document: &mut impl UiDocumentAccess,
    menu_rects: &mut Vec<Rect>,
) -> Result<(), String> {
    menu_rects.clear();
    document.visible("desktop-menu", false)?;
    document.visible("desktop-entry-menu", false)?;
    document.visible("desktop-convert-wheel", false)?;
    document.visible("convert-wheel-highlight", false)?;
    document.visible("sticky-menu", false)?;
    widgets::hide_panels(document)?;
    projection::clear_menu_backdrop(document, "desktop-menu")?;
    projection::clear_menu_backdrop(document, "desktop-entry-menu")?;
    projection::clear_menu_backdrop(document, "desktop-convert-wheel")?;
    projection::clear_menu_backdrop(document, "sticky-menu")
}

/// Whether any menu or the delete confirm is shown. Gates the root press observer.
pub(crate) fn menus_open(menu_rects: &[Rect]) -> bool {
    !menu_rects.is_empty()
}

/// Whether a root press at logical `(x, y)` lands outside every rect.
pub(crate) fn press_outside_menus(rects: &[Rect], x: f32, y: f32) -> bool {
    let (x, y) = (x.round() as i32, y.round() as i32);
    !rects
        .iter()
        .any(|rect| x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom())
}

/// Closes the open menus and delete confirm when a root press lands outside them.
/// Presses inside a menu belong to its own action path; nothing open means no-op.
pub(crate) fn close_menus_on_outside_press(
    press: &RawPress,
    document: &mut impl UiDocumentAccess,
    menu_rects: &mut Vec<Rect>,
    menu: &mut Option<MenuState>,
    pending_delete: &mut Option<Vec<PathBuf>>,
) -> Result<(), String> {
    if !menus_open(menu_rects) || !press_outside_menus(menu_rects, press.x, press.y) {
        return Ok(());
    }
    // Drop the semantic owner before hiding the nodes. A subsequent document
    // rebuild must not resurrect a menu dismissed on the previous frame.
    *menu = None;
    *pending_delete = None;
    hide_menus(document, menu_rects)?;
    hide_confirm(document, menu_rects)
}

pub(crate) fn hide_confirm(
    document: &mut impl UiDocumentAccess,
    menu_rects: &mut Vec<Rect>,
) -> Result<(), String> {
    menu_rects.clear();
    document.visible("desktop-confirm", false)?;
    projection::clear_menu_backdrop(document, "desktop-confirm")
}

/// One-time notice after the first organized download. The latch is persisted before this runs.
pub(crate) fn show_downloads_notice(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let bounds = state.grid.work_area;
    let (width, height) = (420, 150);
    let x = bounds.x + (bounds.width - width) / 2;
    let y = bounds.y + (bounds.height - height) / 2;
    document.text(
        "downloads-notice-text",
        flamewm_downloads_core::NOTICE_TEXT.to_owned(),
    )?;
    document.position("downloads-notice", x as f32, y as f32)?;
    document.size("downloads-notice", width as f32, height as f32)?;
    projection::project_menu_surface(document, "downloads-notice", &state.settings)?;
    document.visible("downloads-notice", true)
}

pub(crate) fn show_confirm(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let parts = confirm_menu_parts();
    let measured = context_menu_size(&parts, flamewm_ui_core::context_menu::MIN_ROW_WIDTH);
    let size = (measured.width, measured.height);
    let bounds = state
        .pending_delete
        .as_ref()
        .and_then(|paths| paths.first())
        .and_then(|path| state.model.items().find(|item| &item.path == path))
        .map_or(state.grid.work_area, |item| {
            grid_for_output(state, &item.output).work_area
        });
    let center = Point::new(
        bounds.x + (bounds.width - size.0) / 2,
        bounds.y + (bounds.height - size.1) / 2,
    );
    let placed = clamp_menu_anchor(bounds, Rect::new(center.x, center.y, size.0, size.1), size);
    document.text(
        "confirm-text",
        if state
            .pending_delete
            .as_ref()
            .is_some_and(|paths| paths.len() > 1)
        {
            format!(
                "Move {} items to Trash?",
                state.pending_delete.as_ref().map_or(0, Vec::len)
            )
        } else {
            "Move to Trash?".to_owned()
        },
    )?;
    document.position("desktop-confirm", placed.x as f32, placed.y as f32)?;
    document.size("desktop-confirm", placed.width as f32, placed.height as f32)?;
    projection::project_menu_surface(document, "desktop-confirm", &state.settings)?;
    state.menu_rects.push(placed);
    document.visible("desktop-confirm", true)
}

pub(crate) fn sync_menu(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let _total = context_open_total_point().start();
    {
        let _close = context_open_close_point().start();
        hide_menus(document, &mut state.menu_rects)?;
    }
    let Some(menu) = state.menu.as_ref() else {
        return Ok(());
    };
    let menu_output = menu.output.clone();
    let menu_kind = menu.kind;
    let menu_anchor = menu.anchor;
    let selected_id = menu.selected_id.clone();
    let menu_bounds = grid_for_output(state, &menu_output).work_area;
    match menu_kind {
        MenuKind::Blank => {
            let rows = {
                let _project = context_open_project_point().start();
                blank_context_menu(state.sticky.store.enabled())
            };
            let visible: BTreeSet<&str> = rows.iter().map(|action| blank_row_id(*action)).collect();
            for id in BLANK_ROW_IDS {
                document.visible(id, visible.contains(id))?;
            }
            place_menu_parts(
                document,
                state,
                "desktop-menu",
                &menu_output,
                menu_anchor,
                &blank_menu_parts(rows.len()),
            )?;
        }
        MenuKind::Entry => {
            let selected_kind = selected_id
                .as_ref()
                .and_then(|id| state.model.items().find(|item| &item.id == id))
                .map(|item| item.kind);
            let is_trash = selected_kind == Some(DesktopItemKind::TrashPseudo);
            let convertible = selected_id
                .as_ref()
                .and_then(|id| state.model.items().find(|item| &item.id == id))
                .is_some_and(|item| {
                    item.kind == DesktopItemKind::File && !offered_formats(&item.path).is_empty()
                });
            let rows = {
                let _project = context_open_project_point().start();
                entry_context_menu(is_trash, convertible)
            };
            let visible: BTreeSet<&str> = rows.iter().map(|action| entry_row_id(*action)).collect();
            for id in ENTRY_ROW_IDS {
                document.visible(id, visible.contains(id))?;
            }
            place_menu_parts(
                document,
                state,
                "desktop-entry-menu",
                &menu_output,
                menu_anchor,
                &entry_menu_parts(rows.len()),
            )?;
        }
        MenuKind::ConvertWheel => {
            // Recreate hover state on the next movement; never carry a sector
            // from an earlier file or workspace into this wheel.
            state.wheel_hover = None;
            document.visible("convert-wheel-highlight", false)?;
            let path = selected_id
                .as_ref()
                .and_then(|id| state.model.items().find(|item| &item.id == id))
                .map(|item| &item.path);
            set_wheel_slot_labels(document, state.wheel_mode, path.map(PathBuf::as_path))?;
            document.text(
                "convert-wheel-hub-text",
                state.wheel_mode.label().to_owned(),
            )?;
            let size = (328, 328);
            let placed = clamp_menu_anchor(
                menu_bounds,
                Rect::new(menu_anchor.x, menu_anchor.y, size.0, size.1),
                size,
            );
            document.position("desktop-convert-wheel", placed.x as f32, placed.y as f32)?;
            document.size("desktop-convert-wheel", size.0 as f32, size.1 as f32)?;
            // Same Surfaces palette/transparency/blur as Start and other menus;
            // force a round mask because this control is a radial wheel.
            projection::project_menu_surface(document, "desktop-convert-wheel", &state.settings)?;
            document.corner_radius("desktop-convert-wheel", 164.0)?;
            let accent = accent_color(&state.settings);
            document.background("convert-wheel-hub", UiColor { a: 64, ..accent })?;
            for slot in 0..8 {
                document.foreground(
                    &format!("convert-slot-{slot}"),
                    UiColor {
                        r: 244,
                        g: 246,
                        b: 251,
                        a: 255,
                    },
                )?;
            }
            state.menu_rects.push(placed);
            document.visible("desktop-convert-wheel", true)?;
        }
        MenuKind::Widget => {
            let rows = widgets::widget_menu_rows(document, state)?;
            if rows == 0 {
                state.menu = None;
                return Ok(());
            }
            place_menu_parts(
                document,
                state,
                "widget-menu",
                &menu_output,
                menu_anchor,
                &blank_menu_parts(rows),
            )?;
        }
        MenuKind::WidgetGallery => {
            widgets::place_gallery(document, state, menu_bounds)?;
        }
        MenuKind::WidgetConfig => {
            widgets::place_config(document, state, menu_bounds)?;
        }
        MenuKind::Sticky => {
            let parts = {
                let _project = context_open_project_point().start();
                sticky_menu_parts()
            };
            place_menu_parts(
                document,
                state,
                "sticky-menu",
                &menu_output,
                menu_anchor,
                &parts,
            )?;
        }
    }
    Ok(())
}

pub(crate) fn place_menu_parts(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    id: &str,
    output: &OutputId,
    anchor: Point,
    parts: &[MenuPart],
) -> Result<(), String> {
    // Shared C08 contract: size from actual parts, clamp through menu_rect.
    // Position/size land first so the first presented frame already shows
    // the menu above the contents.
    let placed = {
        let _measure = context_open_measure_point().start();
        let _place = context_open_place_point().start();
        parts_menu_rect(grid_for_output(state, output).work_area, anchor, parts)
    };
    document.position(id, placed.x as f32, placed.y as f32)?;
    document.size(id, placed.width as f32, placed.height as f32)?;
    // Fill and decoration shape (Rounded 12 / Squared 0) land together in project_menu_surface.
    projection::project_menu_surface(document, id, &state.settings)?;
    state.menu_rects.push(placed);
    context_open_counter().increment();
    let _present = context_open_present_point().start();
    document.visible(id, true)
}

/// Prefer the next grid cell horizontally; never obscure a real desktop item.
pub(crate) fn preview_cell_for(source: Cell, grid: GridConfig, occupied: &BTreeSet<Cell>) -> Cell {
    let total = grid.columns.max(1).saturating_mul(grid.rows.max(1));
    let start = source
        .row
        .saturating_mul(grid.columns.max(1))
        .saturating_add(source.column);
    for offset in 1..=total {
        let index = (start + offset).rem_euclid(total);
        let cell = Cell::new(
            index.rem_euclid(grid.columns.max(1)),
            index.div_euclid(grid.columns.max(1)),
        );
        if !occupied.contains(&cell) {
            return cell;
        }
    }
    source
}

pub(crate) fn sync_conversion_preview(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let Some(preview) = state.conversion_preview.as_ref() else {
        return Ok(());
    };
    let outcome = preview
        .result
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let Some(outcome) = outcome else {
        return Ok(());
    };
    let preview = state
        .conversion_preview
        .take()
        .expect("preview observed above");
    document.visible("desktop-convert-preview", false)?;
    match outcome {
        Ok(path) => {
            state.model.set_position(
                path,
                StoredPosition {
                    output: preview.output,
                    cell: preview.cell,
                },
            );
            rescan_desktop(state)?;
            refresh_launchers(state);
            if let Err(error) = LayoutStore::persist(&state.layout_path, state.model.positions()) {
                report_action_failure("save converted placement", format!("{error:?}"));
            }
            sync_document(document, state)?;
        }
        Err(error) => report_action_failure("conversion", error),
    }
    Ok(())
}

pub(crate) fn show_conversion_preview(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    destination: &Path,
    output: &OutputId,
    cell: Cell,
) -> Result<(), String> {
    let grid = grid_for_output(state, output);
    let scale = grid.visual_scale_percent();
    let factor = f32::from(scale) / 100.0;
    let point = grid.cell_to_pixel(cell);
    document.position("desktop-convert-preview", point.x as f32, point.y as f32)?;
    document.size("desktop-convert-preview", 82.0 * factor, 82.0 * factor)?;
    document.position("desktop-convert-preview-icon", 18.0 * factor, 5.0 * factor)?;
    document.size("desktop-convert-preview-icon", 44.0 * factor, 44.0 * factor)?;
    document.position("desktop-convert-preview-label", 2.0 * factor, 52.0 * factor)?;
    document.size(
        "desktop-convert-preview-label",
        76.0 * factor,
        30.0 * factor,
    )?;
    for id in [
        "desktop-convert-preview-line-1",
        "desktop-convert-preview-line-2",
    ] {
        document.size(id, 76.0 * factor, 15.0 * factor)?;
        document.font_size(id, 13.0 * factor)?;
    }
    document.position("desktop-convert-preview-status", 0.0, 71.0 * factor)?;
    document.size(
        "desktop-convert-preview-status",
        82.0 * factor,
        11.0 * factor,
    )?;
    document.font_size("desktop-convert-preview-status", 9.0 * factor)?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Converted file");
    let [first, second] =
        flamewm_desktop_core::presentation::desktop_label_lines(name, 76.0 * factor, 13.0 * factor);
    document.text("desktop-convert-preview-line-1", first)?;
    document.text("desktop-convert-preview-line-2", second)?;
    projection::project_conversion_preview_icon_scaled(
        document,
        &mut state.icons,
        destination,
        scale,
    )?;
    document.visible("desktop-convert-preview", true)
}

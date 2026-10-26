use super::*;

pub(super) fn show_drag_ghost(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    x: f32,
    y: f32,
) -> Result<(), String> {
    let Some(ghost) = state.drag_ghost.as_ref() else {
        return Ok(());
    };
    let scale = grid_for_output(state, &ghost.output).visual_scale_percent();
    let ghost = state.drag_ghost.as_mut().expect("observed ghost");
    let factor = f32::from(scale) / 100.0;
    let kind = ghost.kind;
    ghost.visible = true;
    let gx = x - ghost.offset_x;
    let gy = y - ghost.offset_y;
    document.position("desktop-drag-ghost", gx, gy)?;
    document.position("desktop-drag-ghost-icon", 18.0 * factor, 5.0 * factor)?;
    document.size("desktop-drag-ghost-icon", 44.0 * factor, 44.0 * factor)?;
    projection::project_drag_ghost_icon_scaled(
        document,
        &mut state.icons,
        kind,
        ghost.icon_override.as_deref(),
        scale,
    )?;
    let [line1, line2] = flamewm_desktop_core::presentation::desktop_label_lines(
        &ghost.label,
        76.0 * factor,
        13.0 * factor,
    );
    document.size(
        "desktop-drag-ghost",
        projection::DESKTOP_TILE_WIDTH * factor,
        projection::DESKTOP_TILE_HEIGHT * factor,
    )?;
    document.position("desktop-drag-ghost-label", 2.0 * factor, 52.0 * factor)?;
    document.size("desktop-drag-ghost-label", 76.0 * factor, 30.0 * factor)?;
    for id in ["desktop-drag-ghost-label-1", "desktop-drag-ghost-label-2"] {
        document.size(id, 76.0 * factor, 15.0 * factor)?;
        document.font_size(id, 13.0 * factor)?;
    }
    document.size("desktop-drag-ghost-count", 18.0 * factor, 18.0 * factor)?;
    document.font_size("desktop-drag-ghost-count", 11.0 * factor)?;
    document.text("desktop-drag-ghost-label-1", line1)?;
    document.text("desktop-drag-ghost-label-2", line2)?;
    document.text(
        "desktop-drag-ghost-count",
        format!("{}", ghost.count.max(1)),
    )?;
    document.visible("desktop-drag-ghost-count", ghost.count > 1)?;
    document.visible("desktop-drag-ghost", true)?;
    // During a drag the *single* visible item follows the pointer. The
    // originals retain their model/layout slots and are restored on release;
    // leaving them painted makes the drag appear frozen at its start cell.
    set_source_tiles_visible(document, state.model.items(), &ghost.source_ids, false)?;
    Ok(())
}

pub(super) fn sync_drag_ghost(
    document: &mut impl UiDocumentAccess,
    state: &DesktopState,
) -> Result<(), String> {
    let ghost = state.drag_ghost.as_ref().filter(|ghost| ghost.visible);
    document.visible("desktop-drag-ghost", ghost.is_some())?;
    if let Some(ghost) = ghost {
        set_source_tiles_visible(document, state.model.items(), &ghost.source_ids, false)?;
    }
    Ok(())
}

/// Hides or restores the tiles that are being dragged, keyed by item id.
fn set_source_tiles_visible<'a>(
    document: &mut impl UiDocumentAccess,
    items: impl Iterator<Item = &'a flamewm_desktop_core::model::DesktopItem>,
    source_ids: &[String],
    visible: bool,
) -> Result<(), String> {
    for (index, item) in items.take(SLOT_COUNT).enumerate() {
        if source_ids.contains(&item.id) {
            document.visible(&format!("desktop-item-{index}"), visible)?;
        }
    }
    Ok(())
}

pub(super) fn hide_drag_ghost(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if let Some(ghost) = state.drag_ghost.take() {
        set_source_tiles_visible(document, state.model.items(), &ghost.source_ids, true)?;
    }
    // Best effort: count node may not exist in the compiled
    // document; the ghost id itself is authoritative for visibility.
    let _ = document.visible("desktop-drag-ghost-count", false);
    document.visible("desktop-drag-ghost", false)
}

pub(super) fn commit_item_drag(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    drag: &ItemDrag,
    x: f32,
    y: f32,
) -> Result<(), String> {
    let selected_ids = drag
        .originals
        .iter()
        .map(|item| item.id.as_str())
        .collect::<BTreeSet<_>>();
    let anchor = drag
        .originals
        .iter()
        .find(|item| item.id == drag.anchor_id)
        .ok_or_else(|| "drag anchor disappeared".to_owned())?;
    let anchor_output = live_output(state, &anchor.output);
    let anchor_grid = grid_for_output(state, &anchor_output);
    let delta = drag_cell_delta(
        x - drag.start_x,
        y - drag.start_y,
        anchor_grid.cell_width,
        anchor_grid.cell_height,
    );
    let mut updates = BTreeMap::new();
    let projected = projected_cells(state);
    let drag_outputs = drag
        .originals
        .iter()
        .map(|item| live_output(state, &item.output))
        .collect::<BTreeSet<_>>();
    for output in drag_outputs {
        let selected = drag
            .originals
            .iter()
            .filter(|item| live_output(state, &item.output) == output)
            .map(|item| DragItem {
                id: item.id.clone(),
                cell: item.drag_cell,
            })
            .collect::<Vec<_>>();
        let occupied = state
            .model
            .items()
            .enumerate()
            .filter(|(_, item)| !selected_ids.contains(item.id.as_str()))
            .filter(|(_, item)| live_output(state, &item.output) == output)
            .map(|(slot, _)| projected[slot].1)
            .collect::<BTreeSet<_>>();
        if let Ok(output_updates) =
            group_drag_transaction(&selected, &occupied, delta, grid_for_output(state, &output))
        {
            updates.extend(output_updates);
        } else {
            sync_document(document, state)?;
            return Ok(());
        }
    }

    for original in &drag.originals {
        if let Some(cell) = updates.get(&original.id).copied() {
            state.model.set_position(
                original.path.clone(),
                StoredPosition {
                    output: live_output(state, &original.output),
                    cell,
                },
            );
        }
    }
    if let Err(error) = rescan_desktop(state) {
        restore_original_positions(state, drag)?;
        sync_document(document, state)?;
        return Err(format!("{error:?}"));
    }
    refresh_launchers(state);
    if let Err(error) = LayoutStore::persist(&state.layout_path, state.model.positions()) {
        let restore_error = restore_original_positions(state, drag).err();
        sync_document(document, state)?;
        return Err(match restore_error {
            Some(restore_error) => format!("{error:?}; restore failed: {restore_error}"),
            None => format!("{error:?}"),
        });
    }
    sync_document(document, state)
}

fn restore_original_positions(state: &mut DesktopState, drag: &ItemDrag) -> Result<(), String> {
    for original in &drag.originals {
        state.model.set_position(
            original.path.clone(),
            StoredPosition {
                output: original.output.clone(),
                cell: original.cell,
            },
        );
    }
    rescan_desktop(state)
}

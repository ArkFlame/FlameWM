use super::*;

pub(crate) fn open_rename(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let selected = selected_menu_id(state);
    let Some(id) = selected else {
        state.menu = None;
        return sync_menu(document, state);
    };
    let source = state.model.items().find(|item| item.id == id).map(|item| {
        (
            item.id.clone(),
            item.path.clone(),
            item.display_name.clone(),
        )
    });
    let Some((item_id, source_path, buffer)) = source else {
        state.menu = None;
        return sync_menu(document, state);
    };
    state.menu = None;
    hide_menus(document, &mut state.menu_rects)?;
    rename_open_counter().increment();
    state.rename = Some(RenameState {
        item_id,
        source_path,
        buffer,
    });
    sync_rename_editor(document, state)
}

pub(crate) fn sync_rename_editor(
    document: &mut impl UiDocumentAccess,
    state: &DesktopState,
) -> Result<(), String> {
    let Some(rename) = state.rename.as_ref() else {
        let _ = document.visible("desktop-rename", false);
        return Ok(());
    };
    let cells = projected_cells(state);
    let anchor = state
        .model
        .items()
        .enumerate()
        .find(|(_, item)| item.id == rename.item_id)
        .map_or(state.grid.work_area.origin(), |(slot, _)| {
            let (output, cell) = &cells[slot];
            grid_for_output(state, output).cell_to_pixel(*cell)
        });
    // Value plus OK/Cancel on one row.
    let width = 300;
    let height = 44;
    let bounds = state
        .model
        .items()
        .find(|item| item.id == rename.item_id)
        .map_or(state.grid.work_area, |item| {
            grid_for_output(state, &item.output).work_area
        });
    let placed = clamp_menu_anchor(
        bounds,
        Rect::new(anchor.x, anchor.y + 84, width, height),
        (width, height),
    );
    document.position("desktop-rename", placed.x as f32, placed.y as f32)?;
    document.size("desktop-rename", placed.width as f32, placed.height as f32)?;
    document.text("desktop-rename-value", rename.buffer.clone())?;
    document.visible("desktop-rename", true)?;
    Ok(())
}

pub(crate) fn handle_rename_input(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let text = action.text.clone().unwrap_or_default();
    if text == "\n" || text == "\r" {
        return confirm_rename(document, state);
    }
    let Some(rename) = state.rename.as_mut() else {
        return Ok(());
    };
    if text == "\u{8}" || text == "\u{7f}" {
        rename.buffer.pop();
    } else {
        for ch in text.chars() {
            if !ch.is_control() {
                rename.buffer.push(ch);
            }
        }
    }
    sync_rename_editor(document, state)
}

pub(crate) fn confirm_rename(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let Some(rename) = state.rename.clone() else {
        return Ok(());
    };
    let validated = match validate_rename_name(&rename.buffer) {
        Ok(name) => name,
        Err(error) => {
            rename_failure_counter().increment();
            eprintln!("desktop: rename rejected: {error:?}; keeping editor open");
            return Ok(());
        }
    };
    let target = rename
        .source_path
        .parent()
        .map(|parent| parent.join(&validated));
    let Some(target) = target else {
        rename_failure_counter().increment();
        eprintln!("desktop: rename rejected: no parent; keeping editor open");
        return Ok(());
    };
    if let Err(error) = rename_entry_no_replace(&rename.source_path, &target) {
        rename_failure_counter().increment();
        eprintln!("desktop: rename failed: {error:?}; keeping editor open");
        return Ok(());
    }
    state.model.migrate_rename(&rename.source_path, &target);
    if let Err(error) = rescan_desktop(state) {
        rename_failure_counter().increment();
        eprintln!("desktop: rename rescan failed: {error:?}; keeping editor open");
        return Ok(());
    }
    refresh_launchers(state);
    if let Err(error) = LayoutStore::persist(&state.layout_path, state.model.positions()) {
        rename_failure_counter().increment();
        eprintln!("desktop: rename persist failed: {error:?}; keeping editor open");
        return Ok(());
    }
    rename_success_counter().increment();
    state.selection.clear();
    state
        .selection
        .select(target.to_string_lossy().into_owned());
    state.rename = None;
    sync_document(document, state)
}

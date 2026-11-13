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
    open_rename_item(document, state, id)
}

/// Opens the inline rename editor on one item. Shared by the context menu and the F2 key.
pub(crate) fn open_rename_item(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    id: String,
) -> Result<(), String> {
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

/// The one item F2 renames: exactly one renamable candidate among the selected items. The trash
/// pseudo-item is never renamable, so it never blocks a single real selection. `None` otherwise.
/// Precedent: xfdesktop and Plasma desktop (F2 on the sole selected icon), KDE Dolphin (F2 renames
/// the current item only when one item is selected).
pub(crate) fn sole_renamable(
    candidates: impl IntoIterator<Item = (String, bool)>,
) -> Option<String> {
    let mut renamable = candidates
        .into_iter()
        .filter(|(_, renamable)| *renamable)
        .map(|(id, _)| id);
    let first = renamable.next()?;
    renamable.next().is_none().then_some(first)
}

/// F2 target from the desktop selection model.
pub(crate) fn rename_target_for_f2(state: &DesktopState) -> Option<String> {
    sole_renamable(
        state
            .model
            .items()
            .filter(|item| state.selection.selected().contains(&item.id))
            .map(|item| (item.id.clone(), item.kind != DesktopItemKind::TrashPseudo)),
    )
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
    document.text_ref("desktop-rename-value", &rename.buffer)?;
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

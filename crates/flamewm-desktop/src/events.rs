use super::*;

pub(crate) fn handle_event(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    mutation_dispatcher: Option<&Arc<MutationDispatcher>>,
) -> Result<(), String> {
    // The full annular sector is clickable, not just the format's text box.
    // An empty part of the wheel receives the root action; its polar hit test
    // shares geometry with the painted hover sector. Child buttons use the
    // same test, preventing rectangular hit-box spill into neighbouring slices.
    if state
        .menu
        .as_ref()
        .is_some_and(|menu| menu.kind == MenuKind::ConvertWheel)
    {
        // The background is itself a pointer action. Consume its press so the
        // desktop selection code cannot treat it as a click on empty space.
        if action.action == "desktop.convert.surface" && action.phase == UiActionPhase::Press {
            return Ok(());
        }
        // All descendants (including the hub, labels and highlight) resolve the
        // same local polar geometry. Hover and click cannot disagree because a
        // retained rectangular label happens to sit above a different sector.
        // Keyboard: Tab cycles the mode and digits 1-8 pick a segment. Escape falls through.
        if action.action == "keyboard.input" {
            match action.text.as_deref() {
                Some("\t") => {
                    let next = state.wheel_mode.next();
                    return set_wheel_mode(document, state, next);
                }
                Some(text) => {
                    let index = text
                        .parse::<usize>()
                        .ok()
                        .filter(|number| (1..=8).contains(number))
                        .map(|number| number - 1);
                    if let Some(index) = index {
                        let count =
                            wheel_slot_labels(state.wheel_mode, wheel_path(state).as_deref()).len();
                        if index < count {
                            return handle_menu_action(
                                &format!("convert.slot.{index}"),
                                document,
                                state,
                                mutation_dispatcher,
                            );
                        }
                    }
                }
                None => {}
            }
        }
        if matches!(action.phase, UiActionPhase::Hover | UiActionPhase::Motion) {
            let alt = action.modifiers & (1 << 3) != 0;
            if let Some(mode) =
                WheelMode::from_modifiers(action.shift_down(), action.ctrl_down(), alt)
            {
                if mode != state.wheel_mode {
                    set_wheel_mode(document, state, mode)?;
                }
            }
            let sector = wheel_hit_at(state, action.x, action.y);
            update_wheel_hover(document, state, sector)?;
        }
        if action.phase == UiActionPhase::Release
            && pointer_button(action) == PointerButton::Primary
            && wheel_bounds(state).is_some_and(|bounds| {
                action.x >= bounds.x as f32
                    && action.y >= bounds.y as f32
                    && action.x < bounds.right() as f32
                    && action.y < bounds.bottom() as f32
            })
        {
            if let Some(index) = wheel_hit_at(state, action.x, action.y) {
                return handle_menu_action(
                    &format!("convert.slot.{index}"),
                    document,
                    state,
                    mutation_dispatcher,
                );
            }
            if let Some(bounds) = wheel_bounds(state) {
                if convert_wheel::in_hub(action.x - bounds.x as f32, action.y - bounds.y as f32) {
                    let next = state.wheel_mode.next();
                    return set_wheel_mode(document, state, next);
                }
            }
            return Ok(());
        }
    }
    if action.action == "keyboard.input" {
        if action.text.as_deref() == Some("") {
            // Escape also stops file conversions and tools that are still running.
            if active_conversions() > 0 {
                cancel_conversions();
            }
            return cancel_transient(document, state);
        }
        if state.rename.is_some() {
            return handle_rename_input(action, document, state);
        }
        if sticky::is_editing(state) {
            return sticky::handle_text_input(action, document, state);
        }
        if state.widgets.is_editing() {
            return widgets::handle_text_input(action, document, state);
        }
        if action.text.as_deref() == Some("\u{7f}") && state.pending_delete.is_none() {
            let paths = selected_deletable_paths(state);
            if !paths.is_empty() {
                state.pending_delete = Some(paths);
                state.menu = None;
                hide_menus(document, &mut state.menu_rects)?;
                return show_confirm(document, state);
            }
        }
    }
    if action.action == "keyboard.confirm" && state.rename.is_some() {
        return confirm_rename(document, state);
    }
    if widgets::is_widget_action(&action.action) {
        return widgets::handle_action(action, document, state);
    }
    if action.phase == UiActionPhase::Release
        && action.inside
        && pointer_button(action) == PointerButton::Primary
        && is_menu_action(&action.action)
    {
        context_action_counter().increment();
        return handle_menu_action(&action.action, document, state, mutation_dispatcher);
    }
    if matches!(
        action.action.as_str(),
        "desktop.menu.surface"
            | "desktop.entry-menu.surface"
            | "desktop-convert-wheel"
            | "desktop.confirm.surface"
    ) {
        return Ok(());
    }
    if action.action == "desktop.rename.surface" {
        return Ok(());
    }
    if action.action == "desktop.surface"
        && pointer_button(action) == PointerButton::Primary
        && matches!(action.phase, UiActionPhase::Motion | UiActionPhase::Release)
    {
        let anchor_index = state.item_drag.as_ref().map(|drag| drag.anchor_index);
        if let Some(index) = anchor_index {
            return handle_item(action, document, state, index);
        }
    }
    if action.action == "desktop.surface" {
        return handle_surface(action, document, state);
    }
    if action.action == "downloads.notice.ok" {
        return document.visible("downloads-notice", false);
    }
    if action.action.starts_with("sticky.") {
        return sticky::handle_action(action, document, state);
    }
    if let Some(raw) = action.action.strip_prefix("desktop.item.") {
        if let Ok(index) = raw.parse::<usize>() {
            return handle_item(action, document, state, index);
        }
    }
    Ok(())
}

pub(crate) fn is_menu_action(action: &str) -> bool {
    matches!(
        action,
        "menu.terminal"
            | "menu.folder"
            | "menu.note"
            | "menu.widgets"
            | "menu.desktop"
            | "menu.settings"
            | "entry.empty-trash"
            | "entry.rename"
            | "rename.confirm"
            | "rename.cancel"
            | "entry.shortcut"
            | "entry.convert"
            | "entry.delete"
            | "confirm.trash"
            | "confirm.cancel"
            | "convert.slot.0"
            | "convert.slot.1"
            | "convert.slot.2"
            | "convert.slot.3"
            | "convert.slot.4"
            | "convert.slot.5"
            | "convert.slot.6"
            | "convert.slot.7"
            | "sticky.menu.yellow"
            | "sticky.menu.green"
            | "sticky.menu.pink"
            | "sticky.menu.blue"
            | "sticky.menu.bold"
            | "sticky.menu.delete"
    )
}

pub(crate) fn cancel_transient(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    end_rubber_gesture(state);
    state.item_drag = None;
    state.last_click = None;
    state.menu = None;
    state.pending_delete = None;
    state.rename = None;
    sticky::cancel_edit_and_gesture(state);
    widgets::cancel_transient(state);
    hide_drag_ghost(document, state)?;
    set_selection_overlay(document, state, None)?;
    hide_menus(document, &mut state.menu_rects)?;
    hide_confirm(document, &mut state.menu_rects)?;
    sync_selection_visuals(document, state)?;
    sync_rename_editor(document, state)?;
    sticky::sync_sticky(document, state)?;
    widgets::sync_widgets(document, state)
}

pub(crate) fn handle_surface(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let button = pointer_button(action);
    // Right-click never starts a selection drag: the blank desktop menu only
    // opens on Secondary release at the pointer.
    if button == PointerButton::Secondary {
        if action.phase != UiActionPhase::Release {
            return Ok(());
        }
        context_secondary_counter().increment();
        let (output, anchor) = pointer_on_output(state, action.x, action.y);
        sticky::commit_edit(state);
        state.menu = Some(MenuState {
            kind: MenuKind::Blank,
            anchor,
            output,
            selected_id: None,
        });
        state.pending_delete = None;
        hide_confirm(document, &mut state.menu_rects)?;
        end_rubber_gesture(state);
        set_selection_overlay(document, state, None)?;
        state.selection.clear();
        sync_selection_visuals(document, state)?;
        return sync_menu(document, state);
    }
    if button != PointerButton::Primary {
        return Ok(());
    }
    match action.phase {
        UiActionPhase::Press => {
            // Dismiss both the presentation and its semantic owner. Otherwise
            // a later document re-projection can resurrect an invisible menu.
            state.menu = None;
            hide_menus(document, &mut state.menu_rects)?;
            hide_confirm(document, &mut state.menu_rects)?;
            state.pending_delete = None;
            state.last_click = None;
            sticky::commit_edit(state);
            let (output, start) = pointer_on_output(state, action.x, action.y);
            let bounds = grid_for_output(state, &output).work_area;
            state.item_drag = None;
            state.selection_output = Some(output);
            state.selection_start = Some(start);
            selection_begin_counter().increment();
            state.selection.clear();
            state
                .selection
                .set_rubber(normalized_selection_rect(start, start, bounds));
            state.rubber_items = rubber_snapshot(state);
            set_selection_overlay(document, state, None)?;
            sync_selection_visuals(document, state)?;
        }
        UiActionPhase::Motion => {
            if state.selection_start.is_some() {
                let bounds = rubber_bounds(state);
                state.rubber_pending = Some(pointer_point(action, bounds));
            }
        }
        UiActionPhase::Release => {
            state.rubber_pending = None;
            if let Some(start) = state.selection_start {
                let bounds = rubber_bounds(state);
                let point = pointer_point(action, bounds);
                let rect = normalized_selection_rect(start, point, bounds);
                state.selection.set_rubber(rect);
                apply_rubber_hits(document, state, rect)?;
            }
            end_rubber_gesture(state);
            selection_end_counter().increment();
            set_selection_overlay(document, state, None)?;
        }
        UiActionPhase::Hover => {}
    }
    Ok(())
}

pub(crate) fn handle_item(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    index: usize,
) -> Result<(), String> {
    let button = pointer_button(action);
    // Entry Secondary only opens the entry menu on release after selecting
    // the target; it never enters the open/double-click or drag paths.
    if button == PointerButton::Secondary {
        if action.phase != UiActionPhase::Release {
            return Ok(());
        }
        context_secondary_counter().increment();
        let Some(item) = state.model.items().nth(index) else {
            return Ok(());
        };
        let item_id = item.id.clone();
        let output = live_output(state, &item.output);
        state.selection.select_exclusive(item_id.clone());
        sticky::commit_edit(state);
        state.item_drag = None;
        state.selection_start = None;
        state.selection.clear_rubber();
        set_selection_overlay(document, state, None)?;
        state.last_click = None;
        state.menu = Some(MenuState {
            kind: MenuKind::Entry,
            anchor: pointer_point(action, grid_for_output(state, &output).work_area),
            output,
            selected_id: Some(item_id),
        });
        state.pending_delete = None;
        hide_confirm(document, &mut state.menu_rects)?;
        sync_selection_visuals(document, state)?;
        return sync_menu(document, state);
    }
    if button != PointerButton::Primary {
        return Ok(());
    }
    match action.phase {
        UiActionPhase::Press => {
            // Dismiss both the presentation and its semantic owner. Otherwise
            // a later document re-projection can resurrect an invisible menu.
            state.menu = None;
            hide_menus(document, &mut state.menu_rects)?;
            hide_confirm(document, &mut state.menu_rects)?;
            state.pending_delete = None;
            sticky::commit_edit(state);
            let Some(item) = state.model.items().nth(index) else {
                return Ok(());
            };
            let item_id = item.id.clone();
            if !state.selection.selected().contains(&item_id) {
                state.selection.clear();
                state.selection.select(item_id.clone());
            }
            if double_click_pressed(state, &item_id, action.x, action.y) {
                state.last_click = None;
                state.item_drag = None;
                sync_selection_visuals(document, state)?;
                return open_item(state, index);
            }
            state.last_click = Some(ClickRecord {
                id: item_id.clone(),
                x: action.x,
                y: action.y,
                at: Instant::now(),
            });
            let projected = projected_cells(state);
            let originals = state
                .model
                .items()
                .enumerate()
                .filter(|(_, candidate)| state.selection.selected().contains(&candidate.id))
                .map(|(slot, candidate)| OriginalItem {
                    id: candidate.id.clone(),
                    path: candidate.path.clone(),
                    output: candidate.output.clone(),
                    cell: candidate.cell,
                    drag_cell: projected[slot].1,
                })
                .collect();
            state.item_drag = Some(ItemDrag {
                anchor_index: index,
                anchor_id: item_id,
                start_x: action.x,
                start_y: action.y,
                originals,
                moved: false,
            });
            state.selection_start = None;
            state.selection_output = None;
            sync_selection_visuals(document, state)?;
        }
        UiActionPhase::Motion => {
            let Some(item) = state.model.items().nth(index) else {
                return Ok(());
            };
            let Some(drag) = state.item_drag.as_ref() else {
                return Ok(());
            };
            if drag.anchor_index != index || drag.anchor_id != item.id {
                return Ok(());
            }
            if !drag.moved && !threshold_passed(action.x - drag.start_x, action.y - drag.start_y) {
                return Ok(());
            }
            // Threshold transition: capture icon+label once, then project the
            // ghost; canonical desktop-item-N nodes stay untouched.
            if state.drag_ghost.is_none() {
                let (output, cell) = projected_cells(state)[index].clone();
                let origin = grid_for_output(state, &output).cell_to_pixel(cell);
                state.drag_ghost = Some(DragGhost {
                    output,
                    source_ids: drag.originals.iter().map(|item| item.id.clone()).collect(),
                    kind: item.kind,
                    icon_override: launcher_icon(state, item.kind, &item.path),
                    label: desktop_item_label(state, item).to_owned(),
                    offset_x: drag.start_x - origin.x as f32,
                    offset_y: drag.start_y - origin.y as f32,
                    count: drag.originals.len(),
                    visible: false,
                });
            }
            if let Some(drag_state) = state.item_drag.as_mut() {
                drag_state.moved = true;
            }
            state.last_click = None;
            if state
                .drag_ghost
                .as_ref()
                .is_some_and(|ghost| !ghost.visible)
            {
                drag_ghost_show_counter().increment();
            } else {
                drag_ghost_motion_counter().increment();
            }
            show_drag_ghost(document, state, action.x, action.y)?;
        }
        UiActionPhase::Release => {
            let Some(drag) = state.item_drag.take() else {
                hide_drag_ghost(document, state)?;
                return Ok(());
            };
            if drag.anchor_index != index {
                state.item_drag = Some(drag);
                return Ok(());
            }
            let moved =
                drag.moved || threshold_passed(action.x - drag.start_x, action.y - drag.start_y);
            hide_drag_ghost(document, state)?;
            if moved {
                commit_item_drag(document, state, &drag, action.x, action.y)?;
            }
        }
        UiActionPhase::Hover => {}
    }
    Ok(())
}

/// Single click selects only. A second press on the same item within
/// 500ms and 4px opens; any drag cancels the chain. No single-click timer.
pub(crate) fn double_click_pressed(state: &DesktopState, id: &str, x: f32, y: f32) -> bool {
    let Some(previous) = state.last_click.as_ref() else {
        return false;
    };
    let delta_ms = previous.at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    double_click_opens(&previous.id, previous.x, previous.y, 0, id, x, y, delta_ms)
}

pub(crate) fn open_item(state: &DesktopState, index: usize) -> Result<(), String> {
    let Some(item) = state.model.items().nth(index) else {
        return Ok(());
    };
    let outcome = open_item_inner(state, item.kind, &item.path);
    if let Err(error) = &outcome {
        // Double-click/open boundary: spawn intent failure is logged and
        // counted; the event loop stays alive. Child exit after a successful
        // spawn is not a Flame error and never reaches this path.
        report_action_failure("open item", error.clone());
        spawn_failure_counter().increment();
    }
    Ok(())
}

pub(crate) fn open_item_inner(
    state: &DesktopState,
    kind: DesktopItemKind,
    path: &Path,
) -> Result<(), String> {
    match launcher_open_kind(kind == DesktopItemKind::DesktopLauncher) {
        LauncherOpen::DesktopEntry => {
            let launchable = state.launchers.get(path).is_some_and(|meta| meta.valid);
            if !launchable {
                return Err(format!(
                    "desktop entry is not launchable: {}",
                    path.display()
                ));
            }
            let entry = DesktopEntry::from_file(path)
                .map_err(|error| format!("{error:?}"))?
                .ok_or_else(|| format!("desktop entry is not launchable: {}", path.display()))?;
            entry
                .launch(&ApplicationLaunchOptions::default())
                .map_err(|error| format!("{error:?}"))
        }
        LauncherOpen::RegularFile if kind == DesktopItemKind::Directory => {
            flamewm_applications::open_directory(path).map_err(|error| format!("{error:?}"))
        }
        LauncherOpen::RegularFile => flamewm_applications::open_target(path.as_os_str())
            .map_err(|error| format!("{error:?}")),
    }
}

/// General settings: the settings app's default page, with no `--page` deep link.
pub(crate) fn general_settings() -> CommandIntent {
    CommandIntent {
        program: "flamewm-settings".to_owned(),
        argv: Vec::new(),
        cwd: None,
    }
}

pub(crate) fn spawn_intent(intent: &CommandIntent) -> Result<(), String> {
    flamewm_applications::launch_command(&intent.program, &intent.argv, intent.cwd.as_deref())
        .map_err(|error| format!("spawn {}: {error:?}", intent.program))
}

/// Raw X button translated once at the event boundary (ui-x11 helper).
pub(crate) fn pointer_button(action: &UiActionEvent) -> PointerButton {
    pointer_button_from_raw(action.button)
}

/// Applies the coalesced rubber position once per flush: hit diff, overlay and root publish.
pub(crate) fn apply_pending_rubber(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let Some(point) = state.rubber_pending.take() else {
        return Ok(());
    };
    let Some(start) = state.selection_start else {
        return Ok(());
    };
    let rect = normalized_selection_rect(start, point, rubber_bounds(state));
    // Same rubber rect means same hit set and overlay: nothing to do.
    if state.selection.rubber() == Some(rect) {
        return Ok(());
    }
    state.selection.set_rubber(rect);
    apply_rubber_hits(document, state, rect)?;
    let visible = rubber_visible(
        point.x as f32 - start.x as f32,
        point.y as f32 - start.y as f32,
    );
    if visible {
        selection_visible_counter().increment();
    }
    set_selection_overlay(document, state, visible.then_some(rect))
}

pub(crate) fn pointer_point(action: &UiActionEvent, bounds: Rect) -> Point {
    let max_x = bounds.right().saturating_sub(1);
    let max_y = bounds.bottom().saturating_sub(1);
    Point::new(
        (action.x.round() as i32).clamp(bounds.x, max_x),
        (action.y.round() as i32).clamp(bounds.y, max_y),
    )
}

use super::*;

pub(super) fn slot_number(action: &str) -> Option<usize> {
    action
        .strip_prefix("widget.")?
        .split('.')
        .next()?
        .parse::<usize>()
        .ok()
        .filter(|slot| (1..=MAX_WIDGETS).contains(slot))
}

pub(super) fn slot_widget(state: &DesktopState, action: &str) -> Option<Widget> {
    let slot = slot_number(action)?;
    state.widgets.store.widgets().get(slot - 1).cloned()
}

pub(crate) fn is_widget_action(action: &str) -> bool {
    action.starts_with("widget.")
}

pub(crate) fn handle_action(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let name = action.action.as_str();
    if name.ends_with(".surface") {
        return Ok(());
    }
    let primary_release = action.phase == UiActionPhase::Release
        && action.inside
        && pointer_button(action) == PointerButton::Primary;
    if let Some(rest) = name.strip_prefix("widget.menu.") {
        if primary_release {
            return handle_menu(rest, action, document, state);
        }
        return Ok(());
    }
    if let Some(rest) = name.strip_prefix("widget.gallery.") {
        if primary_release {
            return handle_gallery(rest, document, state);
        }
        return Ok(());
    }
    if let Some(rest) = name.strip_prefix("widget.config.") {
        if primary_release {
            return handle_config(rest, document, state);
        }
        return Ok(());
    }
    let Some(widget) = slot_widget(state, name) else {
        return Ok(());
    };
    if action.phase == UiActionPhase::Release && pointer_button(action) == PointerButton::Secondary
    {
        commit_edit(state);
        state.widgets.gesture = None;
        return open_panel(
            document,
            state,
            MenuKind::Widget,
            Point::new(action.x.round() as i32, action.y.round() as i32),
            Some(widget.id),
        );
    }
    if let Some(control) = name.split_once(".media.").map(|(_, control)| control) {
        if primary_release {
            media_action(control, state);
        }
        return Ok(());
    }
    if pointer_button(action) != PointerButton::Primary {
        return Ok(());
    }
    let mode = match name
        .rsplit_once(".resize-")
        .and_then(|(_, edge)| Edges::from_name(edge))
    {
        Some(edges) => GestureMode::Resize(edges),
        None => GestureMode::Move,
    };
    match action.phase {
        UiActionPhase::Press => {
            state.widgets.pending_pointer = None;
            if state.menu.is_some() {
                state.menu = None;
                hide_menus(document, &mut state.menu_rects)?;
            }
            if state
                .widgets
                .editing
                .as_ref()
                .is_some_and(|edit| edit.widget != widget.id)
            {
                commit_edit(state);
                sync_widgets(document, state)?;
            }
            if widget.locked || state.widgets.is_editing() {
                return Ok(());
            }
            state.widgets.gesture = Some(Gesture {
                widget: widget.id,
                mode,
                start_x: action.x,
                start_y: action.y,
                original: widget.rect,
                moved: false,
            });
        }
        UiActionPhase::Motion => {
            // Coalesced: only the latest pointer position is applied, once per flush.
            if state
                .widgets
                .gesture
                .is_some_and(|g| g.widget == widget.id && g.mode == mode)
            {
                state.widgets.pending_pointer = Some((action.x, action.y));
            }
        }
        UiActionPhase::Release => {
            state.widgets.pending_pointer = None;
            if let Some(gesture) = state
                .widgets
                .gesture
                .filter(|g| g.widget == widget.id && g.mode == mode)
            {
                drag_to(document, state, gesture, (action.x, action.y), true)?;
            }
        }
        UiActionPhase::Hover => {}
    }
    Ok(())
}

pub(super) fn handle_menu(
    rest: &str,
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let Some(id) = selected_widget(state) else {
        state.menu = None;
        return hide_menus(document, &mut state.menu_rects);
    };
    match rest {
        "configure" => {
            let anchor = state
                .widgets
                .store
                .get(id)
                .map_or(Point::new(0, 0), |widget| widget.rect.origin());
            return open_panel(document, state, MenuKind::WidgetConfig, anchor, Some(id));
        }
        "edit" => {
            if let Some(widget) = state.widgets.store.get(id) {
                state.widgets.editing = Some(TemplateEdit {
                    widget: id,
                    buffer: widget.template.clone(),
                });
            }
        }
        "lock" => {
            state.widgets.store.toggle_lock(id);
        }
        "gallery" => {
            state.menu = None;
            hide_menus(document, &mut state.menu_rects)?;
            return open_gallery(
                document,
                state,
                Point::new(action.x.round() as i32, action.y.round() as i32),
            );
        }
        "remove" => {
            if state
                .widgets
                .editing
                .as_ref()
                .is_some_and(|edit| edit.widget == id)
            {
                state.widgets.editing = None;
            }
            state.widgets.store.remove(id);
        }
        _ => return Ok(()),
    }
    state.menu = None;
    hide_menus(document, &mut state.menu_rects)?;
    refresh_after_change(document, state)
}

pub(super) fn handle_gallery(
    rest: &str,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if rest == "close" {
        state.menu = None;
        return hide_menus(document, &mut state.menu_rects);
    }
    let Some(kind) = rest.strip_prefix("add.").and_then(WidgetKind::from_id) else {
        return Ok(());
    };
    let (bounds, scale) =
        state
            .menu
            .as_ref()
            .map_or((state.grid.work_area, state.grid.scale_percent), |menu| {
                let grid = crate::entry::grid_for_output(state, &menu.output);
                (grid.work_area, grid.scale_percent)
            });
    let metrics = GridMetrics::for_work_area(bounds, i32::from(scale));
    if state.widgets.store.add_on_grid(kind, metrics).is_none() {
        return Ok(());
    }
    refresh_after_change(document, state)?;
    if kind == WidgetKind::Media {
        refresh_media(state);
        sync_widgets(document, state)?;
    }
    sync_menu(document, state)
}

pub(super) fn handle_config(
    rest: &str,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if rest == "close" {
        state.menu = None;
        return hide_menus(document, &mut state.menu_rects);
    }
    let Some((row, direction)) = rest.split_once('.') else {
        return Ok(());
    };
    let direction = match direction {
        "up" => 1,
        "down" => -1,
        _ => return Ok(()),
    };
    let Some(row) = row.parse::<usize>().ok() else {
        return Ok(());
    };
    let Some(id) = selected_widget(state) else {
        return Ok(());
    };
    let Some(spec) = state.widgets.store.get(id).and_then(|widget| {
        widget
            .kind
            .visible_options(widget.stored_values())
            .get(row)
            .copied()
    }) else {
        return Ok(());
    };
    let Some(widget) = state.widgets.store.get_mut(id) else {
        return Ok(());
    };
    widget.step_value(spec.key, direction);
    let slot = state
        .widgets
        .store
        .widgets()
        .iter()
        .position(|widget| widget.id == id);
    if let Some(slot) = slot {
        state.widgets.rings[slot] = None;
    }
    refresh_after_change(document, state)?;
    sync_menu(document, state)
}

pub(crate) fn commit_edit(state: &mut DesktopState) {
    let Some(edit) = state.widgets.editing.take() else {
        return;
    };
    if let Some(widget) = state.widgets.store.get_mut(edit.widget) {
        widget.template = edit.buffer;
        state.widgets.persist();
    }
}

pub(crate) fn cancel_transient(state: &mut DesktopState) {
    commit_edit(state);
    state.widgets.gesture = None;
}

pub(crate) fn handle_text_input(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let Some(text) = action.text.as_deref() else {
        return Ok(());
    };
    let Some(edit) = state.widgets.editing.as_mut() else {
        return Ok(());
    };
    if text.is_empty() {
        commit_edit(state);
        return refresh_after_change(document, state);
    }
    if text == "\u{8}" || text == "\u{7f}" {
        edit.buffer.pop();
    } else {
        for ch in text
            .chars()
            .map(|ch| if ch == '\r' { '\n' } else { ch })
            .filter(|ch| *ch == '\n' || !ch.is_control())
        {
            if edit.buffer.chars().count() >= MAX_TEMPLATE_CHARS {
                break;
            }
            edit.buffer.push(ch);
        }
    }
    sync_widgets(document, state)
}

pub(super) fn media_action(control: &str, state: &mut DesktopState) {
    let snapshot = state.widgets.media.clone();
    if snapshot.active_bus_name.is_empty() {
        return;
    }
    let bus_name = snapshot.active_bus_name.clone();
    let action = match control {
        "prev" => SystemAction::Previous { bus_name },
        "next" => SystemAction::Next { bus_name },
        "play" => SystemAction::PlayPause { bus_name },
        _ => return,
    };
    let Some(provider) = state.widgets.mpris.as_mut() else {
        return;
    };
    if let Err(error) = provider.perform_action(action, &snapshot) {
        super::super::log_limit::emit(
            "media widget action failed",
            format_args!("desktop: media widget action failed: {}", error.message),
        );
        return;
    }
    if let Err(error) = provider.on_ready() {
        super::super::log_limit::emit(
            "media widget flush failed",
            format_args!("desktop: media widget flush failed: {}", error.message),
        );
    }
}

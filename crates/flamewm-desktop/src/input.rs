use super::*;

/// Desktop keyboard actions are delivered as typed key events, not `keyboard.input`
/// action strings. Suppress Delete in rename/sticky editors and modifier chords.
pub(crate) fn handle_desktop_key(
    key: &UiKeyEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if key.phase != UiKeyPhase::Press
        || key.key != "Delete"
        || !key.modifiers.is_empty()
        || state.rename.is_some()
        || sticky::is_editing(state)
        || state.widgets.is_editing()
    {
        return Ok(());
    }
    let paths = selected_deletable_paths(state);
    if paths.is_empty() {
        return Ok(());
    }
    state.pending_delete = Some(paths);
    state.menu = None;
    hide_menus(document, &mut state.menu_rects)?;
    show_confirm(document, state)
}

pub(crate) fn wheel_bounds(state: &DesktopState) -> Option<Rect> {
    let menu = state
        .menu
        .as_ref()
        .filter(|menu| menu.kind == MenuKind::ConvertWheel)?;
    Some(clamp_menu_anchor(
        grid_for_output(state, &menu.output).work_area,
        Rect::new(menu.anchor.x, menu.anchor.y, 328, 328),
        (328, 328),
    ))
}

pub(crate) fn wheel_hit_at(state: &DesktopState, x: f32, y: f32) -> Option<usize> {
    let bounds = wheel_bounds(state)?;
    let index = convert_wheel::sector_at(x - bounds.x as f32, y - bounds.y as f32)?;
    let path = state
        .menu
        .as_ref()?
        .selected_id
        .as_ref()
        .and_then(|id| state.model.items().find(|item| &item.id == id))
        .map(|item| &item.path)?;
    (index < wheel_slot_labels(state.wheel_mode, Some(path)).len()).then_some(index)
}

/// Labels for the eight segments of the current mode. Actions need no file.
pub(crate) fn wheel_slot_labels(mode: WheelMode, path: Option<&Path>) -> Vec<String> {
    match (mode, path) {
        (WheelMode::Actions, _) => WheelAction::ALL
            .iter()
            .map(|action| action.label().to_owned())
            .collect(),
        (_, None) => Vec::new(),
        (WheelMode::Convert, Some(path)) => offered_formats(path)
            .iter()
            .map(|format| format.to_ascii_uppercase())
            .collect(),
        (WheelMode::Tools, Some(path)) => offered_tools(path)
            .iter()
            .map(|tool| tool.label().to_owned())
            .collect(),
    }
}

pub(crate) fn set_wheel_slot_labels(
    document: &mut impl UiDocumentAccess,
    mode: WheelMode,
    path: Option<&Path>,
) -> Result<(), String> {
    let labels = wheel_slot_labels(mode, path);
    for slot in 0..8 {
        let id = format!("convert-slot-{slot}");
        let label = labels.get(slot);
        document.visible(&id, label.is_some())?;
        if let Some(label) = label {
            document.text(&id, label.clone())?;
        }
    }
    Ok(())
}

pub(crate) fn wheel_path(state: &DesktopState) -> Option<PathBuf> {
    state
        .menu
        .as_ref()?
        .selected_id
        .as_ref()
        .and_then(|id| state.model.items().find(|item| &item.id == id))
        .map(|item| item.path.clone())
}

/// Switches the open wheel's mode: relabels every segment and clears the hover.
pub(crate) fn set_wheel_mode(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    mode: WheelMode,
) -> Result<(), String> {
    state.wheel_mode = mode;
    state.wheel_hover = None;
    document.visible("convert-wheel-highlight", false)?;
    let path = wheel_path(state);
    set_wheel_slot_labels(document, mode, path.as_deref())?;
    document.text("convert-wheel-hub-text", mode.label().to_owned())
}

/// Runs a Tools or Actions segment. Convert segments are handled by the caller.
pub(crate) fn run_wheel_slot(
    slot: usize,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    mutation_dispatcher: Option<&Arc<MutationDispatcher>>,
) -> Result<(), String> {
    let Some(path) = wheel_path(state) else {
        return Ok(());
    };
    match state.wheel_mode {
        WheelMode::Convert => Ok(()),
        WheelMode::Tools => {
            let Some(tool) = offered_tools(&path).get(slot).copied() else {
                return Ok(());
            };
            state.menu = None;
            sync_menu(document, state)?;
            // Tools write beside the source; the desktop picks up the new file
            // through its file watcher. The worker never touches UI state.
            let spawned = std::thread::Builder::new()
                .name("flamewm-tool".to_owned())
                .spawn(move || {
                    if let Err(error) = run_tool(&path, tool) {
                        report_action_failure("file tool", error);
                    }
                });
            if let Err(error) = spawned {
                report_action_failure("start file tool", error.to_string());
            }
            Ok(())
        }
        WheelMode::Actions => {
            let Some(action) = WheelAction::ALL.get(slot).copied() else {
                return Ok(());
            };
            match action {
                WheelAction::Open => {
                    let selected = state
                        .menu
                        .as_ref()
                        .and_then(|menu| menu.selected_id.clone());
                    let index = state
                        .model
                        .items()
                        .position(|item| Some(&item.id) == selected.as_ref());
                    state.menu = None;
                    sync_menu(document, state)?;
                    if let Some(index) = index {
                        open_item(state, index)?;
                    }
                    Ok(())
                }
                WheelAction::Reveal => {
                    let parent = path.parent().map(Path::to_path_buf);
                    state.menu = None;
                    sync_menu(document, state)?;
                    if let Some(parent) = parent {
                        if let Err(error) = flamewm_applications::open_directory(&parent) {
                            report_action_failure("reveal item", format!("{error:?}"));
                        }
                    }
                    Ok(())
                }
                WheelAction::Rename => {
                    handle_menu_action("entry.rename", document, state, mutation_dispatcher)
                }
                WheelAction::Shortcut => {
                    handle_menu_action("entry.shortcut", document, state, mutation_dispatcher)
                }
                WheelAction::Trash => {
                    handle_menu_action("entry.delete", document, state, mutation_dispatcher)
                }
            }
        }
    }
}

pub(crate) fn update_wheel_hover(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    sector: Option<usize>,
) -> Result<(), String> {
    if state.wheel_hover == sector {
        return Ok(());
    }
    state.wheel_hover = sector;
    if let Some(index) = sector {
        let accent = accent_color(&state.settings);
        document.image_rgba8(
            "convert-wheel-highlight",
            RuntimeImage {
                source: format!("convert-wheel-sector-{index}"),
                width: convert_wheel::WHEEL_SIZE,
                height: convert_wheel::WHEEL_SIZE,
                pixels: convert_wheel::highlight_pixels(index, [accent.r, accent.g, accent.b]),
            },
        )?;
        document.visible("convert-wheel-highlight", true)?;
    } else {
        document.visible("convert-wheel-highlight", false)?;
    }
    Ok(())
}

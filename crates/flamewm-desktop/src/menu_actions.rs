use super::*;

pub(crate) fn handle_menu_action(
    action: &str,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    mutation_dispatcher: Option<&Arc<MutationDispatcher>>,
) -> Result<(), String> {
    match action {
        "menu.terminal" => {
            let intent = open_terminal(&terminal_program(), &state.directory);
            if let Err(error) = spawn_intent(&intent) {
                report_action_failure("open terminal", error);
                spawn_failure_counter().increment();
            }
            state.menu = None;
            sync_menu(document, state)?;
        }
        "menu.folder" => {
            let path = create_new_folder(&state.directory).map_err(|error| format!("{error:?}"))?;
            rescan_desktop(state)?;
            refresh_launchers(state);
            let id = path.to_string_lossy().into_owned();
            state.selection.clear();
            state.selection.select(id);
            state.menu = None;
            sync_document(document, state)?;
        }
        "menu.note" => {
            let anchor = state.menu.as_ref().map(|menu| menu.anchor);
            let anchor = anchor.unwrap_or_else(|| {
                grid_for_output(state, &state.default_output)
                    .work_area
                    .origin()
            });
            sticky::create_note(state, anchor);
            state.menu = None;
            sync_document(document, state)?;
        }
        "menu.widgets" => {
            let anchor = state.menu.as_ref().map_or_else(
                || {
                    grid_for_output(state, &state.default_output)
                        .work_area
                        .origin()
                },
                |menu| menu.anchor,
            );
            state.menu = None;
            hide_menus(document, &mut state.menu_rects)?;
            widgets::open_gallery(document, state, anchor)?;
        }
        "menu.desktop" => {
            insert_workspace_after(mutation_dispatcher, state)?;
            state.menu = None;
            sync_menu(document, state)?;
        }
        "menu.settings" => {
            if let Err(error) = spawn_intent(&general_settings()) {
                report_action_failure("open flamewm settings", error);
                spawn_failure_counter().increment();
            }
            state.menu = None;
            sync_menu(document, state)?;
        }
        "entry.convert" => {
            let supported = state
                .menu
                .as_ref()
                .filter(|menu| menu.kind == MenuKind::Entry)
                .and_then(|menu| menu.selected_id.as_ref())
                .and_then(|id| state.model.items().find(|item| &item.id == id))
                .is_some_and(|item| {
                    item.kind == DesktopItemKind::File && !offered_formats(&item.path).is_empty()
                });
            if !supported {
                return Ok(());
            }
            state.wheel_mode = WheelMode::default();
            if let Some(menu) = state.menu.as_mut() {
                menu.kind = MenuKind::ConvertWheel;
            }
            sync_menu(document, state)?;
        }
        value if value.starts_with("convert.slot.") => {
            let Some(slot) = value
                .strip_prefix("convert.slot.")
                .and_then(|text| text.parse::<usize>().ok())
                .filter(|slot| *slot < 8)
            else {
                return Ok(());
            };
            if state.wheel_mode != WheelMode::Convert {
                return run_wheel_slot(slot, document, state, mutation_dispatcher);
            }
            let path = state
                .menu
                .as_ref()
                .filter(|menu| menu.kind == MenuKind::ConvertWheel)
                .and_then(|menu| menu.selected_id.as_ref())
                .and_then(|id| state.model.items().find(|item| &item.id == id))
                .filter(|item| item.kind == DesktopItemKind::File)
                .map(|item| item.path.clone());
            if let Some(path) = path {
                if let Some(format) = offered_formats(&path).get(slot).copied() {
                    // At most one visible prediction. The worker is independent;
                    // previous conversions keep running without a synthetic tile.
                    let predicted =
                        flamewm_desktop_core::conversion::predicted_destination(&path, format);
                    let source = state
                        .model
                        .items()
                        .find(|item| item.path == path)
                        .map(|item| (item.output.clone(), item.cell));
                    if let (Ok(destination), Some((output, source_cell))) = (predicted, source) {
                        let grid = grid_for_output(state, &output);
                        let occupied = state
                            .model
                            .items()
                            .filter(|item| item.output == output)
                            .map(|item| item.cell)
                            .collect::<BTreeSet<_>>();
                        let cell = preview_cell_for(source_cell, grid, &occupied);
                        let result = Arc::new(Mutex::new(None));
                        let worker_result = Arc::clone(&result);
                        let worker_dirty = Arc::clone(&state.dirty);
                        let format = format.to_owned();
                        match std::thread::Builder::new()
                            .name("flamewm-convert".to_owned())
                            .spawn(move || {
                                let outcome = convert_file(&path, &format);
                                *worker_result
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                    Some(outcome);
                                worker_dirty.store(true, Ordering::Release);
                            }) {
                            Ok(_) => {
                                state.conversion_preview = Some(ConversionPreview {
                                    output: output.clone(),
                                    cell,
                                    destination: destination.clone(),
                                    result,
                                });
                                show_conversion_preview(
                                    document,
                                    state,
                                    &destination,
                                    &output,
                                    cell,
                                )?;
                            }
                            Err(error) => {
                                report_action_failure("start conversion", error.to_string());
                            }
                        }
                    }
                }
            }
            state.menu = None;
            sync_menu(document, state)?;
        }
        "entry.shortcut" => {
            create_shortcut(state)?;
            state.menu = None;
            sync_document(document, state)?;
        }
        "entry.rename" => {
            open_rename(document, state)?;
        }
        "entry.delete" => {
            let path = state
                .menu
                .as_ref()
                .and_then(|menu| menu.selected_id.as_ref())
                .and_then(|id| state.model.items().find(|item| &item.id == id))
                .map(|item| item.path.clone());
            state.menu = None;
            let Some(path) = path else {
                return sync_menu(document, state);
            };
            hide_menus(document, &mut state.menu_rects)?;
            state.pending_delete = Some(vec![path]);
            show_confirm(document, state)?;
        }
        "confirm.trash" => {
            if let Some(paths) = state.pending_delete.take() {
                for path in &paths {
                    if let Err(error) = move_entry_to_trash(path) {
                        // Partial multi-selection failures remain visible, never
                        // fall back to irreversible removal.
                        report_action_failure("move to trash", error);
                    }
                }
                rescan_desktop(state)?;
                refresh_launchers(state);
                state.selection.clear();
            }
            hide_confirm(document, &mut state.menu_rects)?;
            sync_document(document, state)?;
        }
        "confirm.cancel" => {
            state.pending_delete = None;
            hide_confirm(document, &mut state.menu_rects)?;
        }
        "rename.confirm" => {
            if state.rename.is_some() {
                return confirm_rename(document, state);
            }
        }
        "rename.cancel" => {
            state.rename = None;
            sync_rename_editor(document, state)?;
        }
        "entry.empty-trash" => {
            empty_home_trash()?;
            state.menu = None;
            sync_menu(document, state)?;
        }
        "sticky.menu.delete" | "sticky.menu.yellow" | "sticky.menu.green" | "sticky.menu.pink"
        | "sticky.menu.blue" | "sticky.menu.bold" => {
            sticky::apply_menu_action(action, state)?;
            state.menu = None;
            sync_document(document, state)?;
        }
        _ => {}
    }
    Ok(())
}

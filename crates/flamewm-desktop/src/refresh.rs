use super::*;

pub(crate) fn rescan_desktop(state: &mut DesktopState) -> Result<(), String> {
    state
        .model
        .rescan_with_grids(
            &state.directory,
            &state.default_output,
            &state.output_grids,
            state.grid,
        )
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
}

pub(crate) fn selected_menu_id(state: &DesktopState) -> Option<String> {
    state
        .menu
        .as_ref()
        .and_then(|menu| menu.selected_id.clone())
}

pub(crate) fn update_gauges(state: &DesktopState) {
    model_gauge().set((state.model.items().count() as u64).saturating_mul(256));
    launcher_gauge().set((state.launchers.len() as u64).saturating_mul(320));
    icon_gauge().set(state.icons.memory_estimate_bytes() as u64);
}

pub(crate) fn query_workspaces() -> WorkspaceSnapshot {
    let fallback = WorkspaceSnapshot {
        revision: 0,
        count: 1,
        active_index: 0,
        last_index: None,
        names: Vec::new(),
    };
    match control_client() {
        Ok(client) => match client.call(&ControlRequest::GetWorkspaces) {
            Ok(ControlResponse::Workspaces(snapshot)) => snapshot,
            _ => fallback,
        },
        Err(_) => fallback,
    }
}

/// Startup read of the shell's cached forecast. Live changes arrive as `WeatherFeedChanged`;
/// an unreachable host leaves the widget in its "No data" state.
pub(crate) fn query_weather_feed() -> flamewm_api::weather::WeatherFeedSnapshot {
    match control_client() {
        Ok(client) => match client.call(&ControlRequest::GetWeatherFeed) {
            Ok(ControlResponse::WeatherFeed(snapshot)) => snapshot,
            Ok(response) => {
                eprintln!("desktop: startup GetWeatherFeed returned {response:?}; no forecast");
                flamewm_api::weather::WeatherFeedSnapshot::default()
            }
            Err(error) => {
                eprintln!("desktop: startup GetWeatherFeed failed: {}", error.message);
                flamewm_api::weather::WeatherFeedSnapshot::default()
            }
        },
        Err(error) => {
            eprintln!("desktop: weather feed subscription degraded: {error}");
            flamewm_api::weather::WeatherFeedSnapshot::default()
        }
    }
}

pub(crate) fn query_settings() -> SettingsSnapshot {
    let fallback = SettingsSnapshot::new(0);
    match control_client() {
        Ok(client) => match client.call(&ControlRequest::GetSettings) {
            Ok(ControlResponse::Settings(snapshot)) => snapshot,
            Ok(response) => {
                eprintln!("desktop: startup GetSettings returned {response:?}; using defaults");
                fallback
            }
            Err(error) => {
                eprintln!(
                    "desktop: startup GetSettings failed: {}; using defaults",
                    error.message
                );
                fallback
            }
        },
        Err(error) => {
            eprintln!("desktop: startup settings subscription degraded: {error}; using defaults");
            fallback
        }
    }
}

pub(crate) fn monitor_rects(geometries: &[(OutputId, Rect)]) -> Vec<Rect> {
    geometries.iter().map(|(_, rect)| *rect).collect()
}

/// Logs a best-effort subscription or registration that failed; the desktop keeps running without it.
pub(crate) fn warn_degraded<T, E: std::fmt::Display>(subscription: &str, result: Result<T, E>) {
    if let Err(error) = result {
        eprintln!("desktop: degraded {subscription} subscription: {error}");
    }
}

/// Resolves wallpaper settings and requests a root canvas fitted independently per output unless
/// the user opts into the legacy shared canvas.
pub(crate) fn request_wallpaper(
    loader: &mut wallpaper_loader::WallpaperLoader,
    snapshot: &SettingsSnapshot,
    monitors: &[Rect],
) -> Result<u64, wallpaper_loader::LoaderClosed> {
    let source = wallpaper_loader::source_for_settings(
        &setting_text(snapshot, "wallpaperMode"),
        &setting_text(snapshot, "wallpaper"),
        &setting_text(snapshot, "wallpaperColor"),
    );
    match source {
        wallpaper_loader::WallpaperSource::Solid(rgb) => loader.request_solid(rgb),
        wallpaper_loader::WallpaperSource::Image(path) => {
            let share = setting_bool(snapshot, "shareWallpaper", false);
            loader.request_layout(
                path,
                wallpaper_fit::parse(&setting_text(snapshot, "wallpaperFit")),
                monitors,
                share,
            )
        }
    }
}

/// Re-projects every item under a new icon theme generation. The rebuild runs on the icon
/// worker; the pending flag is polled on each UI tick until that generation is installed.
/// Results queued for the previous generation never match a re-projected slot key.
pub(crate) fn refresh_icon_theme(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if !state.icon_theme_pending || !state.icons.apply_icon_theme(&state.icon_theme) {
        return Ok(());
    }
    state.icon_theme_pending = false;
    sync_document(document, state)
}

/// Re-projects the weather widgets when the shell publishes a new feed.
pub(crate) fn refresh_weather_projection(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if !state.weather_visual_dirty {
        return Ok(());
    }
    state.weather_visual_dirty = false;
    widgets::sync_widgets(document, state)
}

/// Per-turn refresh of every projection; the first failure stops the pass, never the desktop.
pub(crate) fn refresh_all(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    refresh_settings_projection(document, state)?;
    refresh_weather_projection(document, state)?;
    refresh_icon_theme(document, state)?;
    refresh_workspace_projection(document, state)?;
    refresh_if_changed(document, state)?;
    apply_pending_icon_results(document, state)?;
    sync_conversion_preview(document, state)?;
    sticky::poll_persist(document, state)
}

pub(crate) fn refresh_settings_projection(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if !state.settings_visual_dirty {
        return Ok(());
    }
    loop {
        match state.wallpaper_loader.poll() {
            wallpaper_loader::PollOutcome::Idle => break,
            wallpaper_loader::PollOutcome::Failed { generation } => {
                if let Some(diagnostic) = &state.wallpaper_loader.state().diagnostic {
                    eprintln!(
                        "desktop: wallpaper generation {generation} failed for {}: {}",
                        diagnostic.path.display(),
                        diagnostic.message
                    );
                }
            }
            wallpaper_loader::PollOutcome::Applied { .. }
            | wallpaper_loader::PollOutcome::Stale { .. } => {}
        }
    }
    if icon_theme_changed(&state.icon_theme, &state.settings) {
        state.icon_theme = setting_text(&state.settings, "iconTheme");
        state.icon_theme_pending = true;
    }
    // SettingsSnapshotChanged is the live authority for stickyNotesEnabled,
    // DesktopSelectionFillOpacity, and DesktopEntryScalePercent; wallpaper
    // stays on the bounded loader path.
    state.sticky.notes_enabled = setting_bool(&state.settings, "stickyNotesEnabled", true);
    let entry_scale = desktop_entry_scale(&state.settings);
    let entry_scale_changed = entry_scale != state.entry_scale_percent;
    if entry_scale_changed {
        state.entry_scale_percent = entry_scale;
        state.grid = GridConfig::compute_with_entry_scale(
            state.grid.work_area,
            state.grid.scale_percent,
            entry_scale,
        );
        for grid in state.output_grids.values_mut() {
            *grid = GridConfig::compute_with_entry_scale(
                grid.work_area,
                grid.scale_percent,
                entry_scale,
            );
        }
    }
    projection::project_global_font_defaults(document, &state.settings)?;
    projection::project_background(document, &state.wallpaper_loader.state().presentation)?;
    if let Err(error) = project_accent_tokens(document, &state.settings) {
        // Optional visual tokens must never terminate the desktop at startup.
        eprintln!("desktop: accent tokens not projected: {error}");
    }
    sync_selection_visuals(document, state)?;
    sticky::sync_sticky(document, state)?;
    for id in projection::MENU_SURFACE_IDS {
        projection::project_menu_surface(document, id, &state.settings)?;
    }
    // Inheriting widget cards read the same global surface fill and blur as the menus above.
    widgets::sync_widgets(document, state)?;
    // Redraw the active radial hover with the current accent after live theme
    // changes; clearing the stamp forces regeneration of the one segment only.
    if let Some(sector) = state.wheel_hover.take() {
        update_wheel_hover(document, state, Some(sector))?;
    }
    sync_cursor_theme(state);
    flamewm_ui_x11::apply_render_acceleration(&state.settings);
    if entry_scale_changed {
        sync_document(document, state)?;
    }
    state.settings_visual_dirty = false;
    Ok(())
}

/// Reloads the live cursor owners when the resolved `cursorTheme` changed. A failed reload
/// keeps the previous theme (rollback) and is retried at the next settings snapshot.
pub(crate) fn sync_cursor_theme(state: &mut DesktopState) {
    if !state.settings.values.contains_key("cursorTheme") {
        return;
    }
    let search_path = std::env::var("XCURSOR_PATH").unwrap_or_default();
    let requested = setting_text(&state.settings, "cursorTheme");
    let Some(theme) = flamewm_session_core::cursor_theme_reload(
        state.applied_cursor_theme.as_deref(),
        &requested,
        &search_path,
    ) else {
        return;
    };
    let errors = flamewm_ui_x11::apply_cursor_theme(&theme);
    if errors.is_empty() {
        state.applied_cursor_theme = Some(theme);
    }
    for error in errors {
        eprintln!("desktop: cursor theme reload failed: {error}");
    }
}

/// Paths of the selected items that deletion may remove (the trash pseudo-item never is).
pub(crate) fn selected_deletable_paths(state: &DesktopState) -> Vec<PathBuf> {
    state
        .model
        .items()
        .filter(|item| {
            state.selection.selected().contains(&item.id)
                && item.kind != DesktopItemKind::TrashPseudo
        })
        .map(|item| item.path.clone())
        .collect()
}

pub(crate) fn state_path(name: &str) -> Result<PathBuf, String> {
    let base = env::var_os("XDG_STATE_HOME").map_or_else(
        || PathBuf::from(env::var_os("HOME").unwrap()).join(".local/state"),
        PathBuf::from,
    );
    Ok(base.join(name))
}

pub(crate) fn refresh_if_changed(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    // One rescan per burst: reactor drains inotify, reduce consumes flag once.
    if !state.dirty.swap(false, Ordering::Acquire) {
        return Ok(());
    }
    inotify_rescan_counter().increment();
    let _span = rescan_point().start();
    rescan_desktop(state)?;
    refresh_launchers(state);
    sync_document(document, state)?;
    update_gauges(state);
    Ok(())
}

/// A workspace transition must never carry a menu (including the delete
/// confirmation or a conversion wheel) onto a different desktop. This is a
/// presentation-only transaction; it does not rescan the filesystem.
pub(crate) fn clear_workspace_menu_state(
    menu: &mut Option<MenuState>,
    pending_delete: &mut Option<Vec<PathBuf>>,
) {
    *menu = None;
    *pending_delete = None;
}

pub(crate) fn dismiss_workspace_menus(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    clear_workspace_menu_state(&mut state.menu, &mut state.pending_delete);
    hide_menus(document, &mut state.menu_rects)?;
    hide_confirm(document, &mut state.menu_rects)
}

pub(crate) fn refresh_workspace_projection(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if !state.workspace_visual_dirty {
        return Ok(());
    }
    let _project = workspace_project_point().start();
    // Context menus are transient to the workspace where they were opened.
    // Do not reproject a stale desktop/file/sticky menu on the newly active one.
    dismiss_workspace_menus(document, state)?;
    sticky::sync_sticky(document, state)?;
    state.workspace_visual_dirty = false;
    state.workspace_present_pending = true;
    Ok(())
}

/// Launcher metadata cache keyed by path, refreshed after every rescan.
/// The Icon= value wins; per-kind defaults apply otherwise. Parsing stays in
/// the applications owner; this cache only records the outcome.
pub(crate) fn refresh_launchers(state: &mut DesktopState) {
    state.launchers.clear();
    for item in state.model.items() {
        if item.kind != DesktopItemKind::DesktopLauncher {
            continue;
        }
        let meta = match DesktopEntry::from_file(&item.path) {
            Ok(Some(entry)) => LauncherMeta {
                icon: entry.icon().to_owned(),
                name: flamewm_applications::desktop_file_display_name(&item.path)
                    .unwrap_or_else(|| entry.name().to_owned()),
                valid: true,
            },
            Ok(None) | Err(_) => LauncherMeta {
                icon: String::new(),
                name: flamewm_applications::desktop_file_display_name(&item.path)
                    .unwrap_or_default(),
                valid: false,
            },
        };
        state.launchers.insert(item.path.clone(), meta);
    }
}

pub(crate) fn desktop_item_label<'a>(
    state: &'a DesktopState,
    item: &'a flamewm_desktop_core::model::DesktopItem,
) -> &'a str {
    if item.kind == DesktopItemKind::DesktopLauncher {
        if let Some(label) = state
            .launchers
            .get(&item.path)
            .filter(|entry| !entry.name.trim().is_empty())
            .map(|entry| entry.name.as_str())
        {
            return label;
        }
    }
    &item.display_name
}

pub(crate) fn launcher_icon(
    state: &DesktopState,
    kind: DesktopItemKind,
    path: &Path,
) -> Option<String> {
    state
        .launchers
        .get(path)
        .filter(|meta| meta.valid && !meta.icon.trim().is_empty())
        .map(|meta| meta.icon.clone())
        .or_else(|| {
            (kind == DesktopItemKind::File).then(|| projection::file_icon_name(path).to_owned())
        })
}

pub(crate) fn sync_document(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let _span = sync_point().start();
    render_sync_counter().increment();
    state.icons.refresh_theme();
    apply_ready_icons(document, state)?;
    project_slots(document, state)?;
    // Model or grid may have changed under an open rubber gesture: re-freeze item rects and
    // drop selections of removed items before syncing visuals (removed slots are already cleared).
    if state.selection_start.is_some() {
        state.rubber_items = rubber_snapshot(state);
        drop_missing_selected(&mut state.selection, &state.rubber_items);
    }
    sync_selection_visuals(document, state)?;
    sync_menu(document, state)?;
    sync_rename_editor(document, state)?;
    sync_drag_ghost(document, state)?;
    sticky::sync_sticky(document, state)?;
    widgets::sync_widgets(document, state)?;
    update_gauges(state);
    Ok(())
}

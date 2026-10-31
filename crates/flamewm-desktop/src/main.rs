use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flamewm_api::applications::ApplicationLaunchOptions;
#[cfg(test)]
use flamewm_api::settings::SettingValue;
use flamewm_api::settings::SettingsSnapshot;
use flamewm_api::workspace::WorkspaceSnapshot;
use flamewm_api::{OutputId, Point, Rect};
use flamewm_applications::DesktopEntry;
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::{ControlClient, ControlSignalClient, MutationDispatcher};
use flamewm_dbus_reactor::BusKind;
use flamewm_desktop_core::conversion::{
    active_conversions, cancel_conversions, convert_file, offered_formats, offered_tools, run_tool,
};
use flamewm_desktop_core::file_actions::{
    CommandIntent, EntryMenuAction, LauncherOpen, create_new_folder, entry_context_menu,
    launcher_open_kind, open_terminal, rename_entry_no_replace, shortcut_destination,
    terminal_program, validate_rename_name,
};
use flamewm_desktop_core::layout::{
    Cell, DragItem, GridConfig, drag_cell_delta, group_drag_transaction, threshold_passed,
};
use flamewm_desktop_core::model::{
    DesktopDirResolver, DesktopItemKind, DesktopModel, StoredPosition,
};
use flamewm_desktop_core::persistence::LayoutStore;
use flamewm_desktop_core::presentation::{
    BlankDesktopAction, blank_context_menu, blank_menu_parts, clamp_menu_anchor,
    confirm_menu_parts, context_menu_size, entry_menu_parts, parts_menu_rect, sticky_menu_parts,
};
use flamewm_desktop_core::selection::{SelectionModel, double_click_opens, rubber_visible};
use flamewm_desktop_core::sticky_persistence;
use flamewm_desktop_core::trash::{Trash, TrashLocation, TrashScope};
use flamewm_integrations_linux::icons::IconKey;
use flamewm_profiler::{CounterPoint, MemoryGauge, ProfilePoint, report_window};
use flamewm_reactor::{FdAction, Reactor};
use flamewm_ui_core::context_menu::MenuPart;
use flamewm_ui_x11::PointerButton;
use flamewm_ui_x11::{
    RawPress, RawPressWatch, RuntimeImage, UiActionEvent, UiActionPhase, UiColor,
    UiControllerEvent, UiDocumentAccess, UiKeyEvent, UiKeyPhase, UiWindowConfig, UiWindowRole,
    WindowGeometrySlot, decode_document, pointer_button_from_raw, root_geometry,
    run_with_controller_events_role_with_presses_and_reactor_geometry,
};

macro_rules! cached {
    ($ty:ident, $name:expr) => {{
        static CELL: ::std::sync::OnceLock<$ty> = ::std::sync::OnceLock::new();
        CELL.get_or_init(|| $ty::new($name))
    }};
}

macro_rules! point_fn {
    ($name:ident, $ty:ident, $key:expr) => {
        pub(crate) fn $name() -> &'static $ty {
            cached!($ty, $key)
        }
    };
}

mod convert_wheel;
use convert_wheel::{WheelAction, WheelMode};
mod downloads;
mod drag;
mod icon_names;
mod icon_raster;
mod install_root;
mod output_layout;
mod projection;
mod rubber;
mod settings_values;
mod slots;
mod sticky;
#[path = "../ui_contract.rs"]
mod ui_contract;
mod wallpaper_fit;
#[allow(clippy::duplicate_mod)]
mod wallpaper_loader;
mod wallpaper_stamp;
mod widgets;
mod workspace_sync;

use drag::{commit_item_drag, hide_drag_ghost, show_drag_ghost, sync_drag_ghost};
use install_root::packaged_root;
#[cfg(test)]
use install_root::{installed_data_root, resolve_packaged_root};
#[cfg(test)]
use output_layout::PROJECTED_CELLS_CALLS;
use output_layout::{
    ProjectedCellsCache, desktop_surface_rect, grid_for_output, live_output, pointer_on_output,
    projected_cells, refresh_output_layout, resolve_output_layout,
};
#[cfg(test)]
use output_layout::{
    grids_for_outputs, normalize_outputs, output_at_in, pointer_point_for_grid,
    subtract_panel_strip,
};
use rubber::{
    RubberItem, apply_rubber_hits, drop_missing_selected, end_rubber_gesture,
    normalized_selection_rect, rubber_bounds, rubber_snapshot, set_selection_overlay,
    sync_selection_visuals,
};
#[cfg(test)]
use settings_values::{ACCENT_TOKEN_ALPHAS, accent_token_values};
use settings_values::{
    accent_color, desktop_entry_scale, icon_theme_changed, project_accent_tokens, setting_bool,
    setting_text,
};
use slots::{SlotInputs, apply_pending_icon_results, apply_ready_icons, project_slots};

mod profiling;
use profiling::*;
fn report_action_failure(context: &str, error: String) {
    action_failure_counter().increment();
    eprintln!("desktop: {context} failed: {error}; keeping event loop alive");
}

mod refresh;
use refresh::*;
mod state;
use state::*;
pub fn main() -> Result<(), String> {
    // Before any thread starts: one malloc arena for the whole process (see the helper docs).
    flamewm_ui_x11::return_large_frees_to_system();
    flamewm_profiler::init_process("flamewm-desktop");
    flamewm_debug::init_process("desktop");
    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?);
    let user_dirs = home.join(".config/user-dirs.dirs");
    let user_dirs = fs::read_to_string(user_dirs).ok();
    let directory = DesktopDirResolver::resolve(&home, user_dirs.as_deref());
    fs::create_dir_all(&directory).map_err(|error| format!("create desktop directory: {error}"))?;

    let (root_width, root_height) = root_geometry()?;
    let surface = desktop_surface_rect(root_width, root_height);
    let settings = query_settings();
    let entry_scale_percent = desktop_entry_scale(&settings);
    let (output_geometries, output_grids, default_output) =
        resolve_output_layout(surface, entry_scale_percent);
    let grid = output_grids
        .get(&default_output)
        .copied()
        .unwrap_or_else(|| GridConfig::compute_with_entry_scale(surface, 100, entry_scale_percent));
    let mut wallpaper_loader = wallpaper_loader::WallpaperLoader::new(
        surface.width as u32,
        surface.height as u32,
        packaged_root(),
    )?;
    request_wallpaper(
        &mut wallpaper_loader,
        &settings,
        &monitor_rects(&output_geometries),
    )
    .map_err(|_| "wallpaper loader closed during startup".to_owned())?;
    let mut model = DesktopModel::default();
    {
        let _span = scan_point().start();
        model
            .rescan_with_grids(&directory, &default_output, &output_grids, grid)
            .map_err(|error| format!("{error:?}"))?;
    }
    let layout_path = state_path("desktop-layout.state")?;
    if let Ok(text) = fs::read_to_string(&layout_path) {
        for (path, position) in LayoutStore::parse(&text).map_err(|error| format!("{error:?}"))? {
            model.set_position(path, position);
        }
        model
            .rescan_with_grids(&directory, &default_output, &output_grids, grid)
            .map_err(|error| format!("{error:?}"))?;
    }
    let sticky_path = sticky_persistence::state_path().map_err(|error| format!("{error:?}"))?;
    let sticky = sticky_persistence::load(&sticky_path).map_err(|error| format!("{error:?}"))?;
    let widget_path =
        flamewm_desktop_core::widgets::state_path().map_err(|error| format!("{error:?}"))?;
    // A missing widget state file means a fresh profile: it gets the default grid layout.
    let widget_fresh = !widget_path.exists();
    let widget_store = flamewm_desktop_core::widgets::load(&widget_path).unwrap_or_else(|error| {
        eprintln!("desktop: widget state unreadable, starting empty: {error:?}");
        flamewm_desktop_core::widgets::WidgetStore::default()
    });
    let dirty = Arc::new(AtomicBool::new(false));
    let icons_pending = Arc::new(AtomicBool::new(false));
    let downloads_dir = flamewm_downloads_core::downloads_dir(&home, user_dirs.as_deref());
    let mut downloads = downloads::Organizer::new(
        downloads_dir,
        state_path("flamewm-downloads-organizer.state")?,
    )
    .map_err(|error| format!("init downloads organizer: {error}"))?;
    downloads.set_enabled(setting_bool(&settings, "organizeDownloads", false));
    let inotify = inotify::Inotify::init().map_err(|error| format!("init inotify: {error}"))?;
    inotify
        .watches()
        .add(
            &directory,
            inotify::WatchMask::CREATE
                | inotify::WatchMask::DELETE
                | inotify::WatchMask::MOVED_FROM
                | inotify::WatchMask::MOVED_TO
                | inotify::WatchMask::CLOSE_WRITE
                | inotify::WatchMask::ATTRIB,
        )
        .map_err(|error| format!("watch desktop directory: {error}"))?;
    let workspace = query_workspaces();
    let state_cell = std::rc::Rc::new(std::cell::RefCell::new({
        let mut state = DesktopState {
            directory,
            model,
            grid,
            output_grids,
            entry_scale_percent,
            output_geometries,
            default_output,
            icons: projection::ParallelIconService::spawn(packaged_root()),
            icon_slots: vec![None; SLOT_COUNT],
            slot_inputs: vec![None; SLOT_COUNT],
            projected_cache: None,
            slots_primed: false,
            layout_path,
            sticky: sticky::StickyState::new(
                sticky,
                sticky_path,
                setting_bool(&settings, "stickyNotesEnabled", true),
            ),
            widgets: widgets::WidgetState::new(widget_store, widget_path),
            selection: SelectionModel::default(),
            selection_start: None,
            rubber_pending: None,
            selection_output: None,
            rubber_items: Vec::new(),
            selection_overlay: None,
            item_drag: None,
            launchers: BTreeMap::new(),
            menu: None,
            menu_rects: Vec::new(),
            wheel_hover: None,
            wheel_mode: WheelMode::default(),
            pending_delete: None,
            last_click: None,
            dirty: Arc::clone(&dirty),
            icons_pending: Arc::clone(&icons_pending),
            rename: None,
            drag_ghost: None,
            conversion_preview: None,
            workspace: workspace_sync::WorkspaceAuthority::new(workspace),
            workspace_visual_dirty: false,
            workspace_present_pending: false,
            workspace_present_span: None,
            settings,
            settings_visual_dirty: false,
            weather: query_weather_feed(),
            weather_visual_dirty: true,
            downloads,
            applied_cursor_theme: None,
            icon_theme: String::new(),
            icon_theme_pending: false,
            wallpaper_loader,
        };
        refresh_launchers(&mut state);
        update_gauges(&state);
        state
    }));

    let document = initial_document(&state_cell)?;
    let mut reactor = Reactor::new().map_err(|error| format!("create reactor: {error}"))?;
    if flamewm_profiler::enabled() {
        reactor
            .register_timer(
                Duration::from_secs(flamewm_profiler::profile_interval_secs()),
                true,
                || {
                    let report = report_window();
                    if !report.is_empty() {
                        eprintln!("{report}");
                    }
                },
            )
            .map_err(|error| format!("register profiler timer: {error}"))?;
    }
    reactor
        .register_timer(
            STARTUP_HEAP_TRIM_DELAY,
            false,
            flamewm_ui_x11::release_free_heap,
        )
        .map_err(|error| format!("register startup heap trim: {error}"))?;
    sticky::attach_persist_wake(&mut state_cell.borrow_mut().sticky, &reactor);
    {
        let wake = reactor.owned_wake_deadline();
        let mut state = state_cell.borrow_mut();
        state.widgets.attach_wake(wake);
        let metrics = widgets::widget_metrics(&state);
        if widget_fresh && state.widgets.store.widgets().is_empty() {
            state.widgets.store =
                flamewm_desktop_core::widgets::WidgetStore::with_default_layout(metrics);
            // The wake was armed against the empty store; re-derive the tick deadline.
            state.widgets.rearm_after_store_replace();
            state.widgets.persist();
        } else {
            state.widgets.store.layout_grid(metrics);
        }
    }
    register_media_widget(&mut reactor, &state_cell);
    let panels_dirty = std::rc::Rc::new(std::cell::Cell::new(false));
    register_workspace_signals(&mut reactor, &state_cell, &panels_dirty);
    let workspace_mutations = match MutationDispatcher::start(BusKind::Session, 16) {
        Ok(dispatcher) => Some(dispatcher),
        Err(error) => {
            eprintln!(
                "desktop: degraded workspace mutation dispatcher: {}",
                error.message
            );
            None
        }
    };
    if let Some(dispatcher) = workspace_mutations.as_ref() {
        let dispatcher = Arc::clone(dispatcher);
        let registration = reactor.register_raw_fd_with_action(
            dispatcher.wake_fd(),
            calloop::Interest::READ,
            move |_, _| {
                while let Some(result) = dispatcher.try_recv_result() {
                    if let Err(error) = result.outcome {
                        eprintln!("desktop: workspace mutation failed: {}", error.message);
                    }
                }
                FdAction::Continue
            },
        );
        warn_degraded("workspace mutation", registration);
    }
    register_inotify(&mut reactor, inotify, dirty)?;
    register_downloads(&mut reactor, &state_cell)?;
    register_icon_and_wallpaper_wakes(&mut reactor, &state_cell, &icons_pending);
    let geometry_slot = WindowGeometrySlot::default();
    // Armed while a menu or the delete confirm is open: root presses reach the close path.
    let press_watch = RawPressWatch::default();
    run_with_controller_events_role_with_presses_and_reactor_geometry(
        document,
        UiWindowConfig {
            width: surface.width as u32,
            height: surface.height as u32,
            x: surface.x,
            y: surface.y,
            title: "FlameWM Desktop".to_owned(),
            identity: None,
        },
        UiWindowRole::Desktop,
        &mut reactor,
        std::rc::Rc::clone(&geometry_slot),
        std::rc::Rc::clone(&press_watch),
        {
            let press_cell = std::rc::Rc::clone(&state_cell);
            move |press: &RawPress, document| {
                let mut guard = press_cell.borrow_mut();
                let state = &mut *guard;
                close_menus_on_outside_press(
                    press,
                    document,
                    &mut state.menu_rects,
                    &mut state.menu,
                    &mut state.pending_delete,
                )
            }
        },
        {
            let event_cell = std::rc::Rc::clone(&state_cell);
            let event_watch = std::rc::Rc::clone(&press_watch);
            let mutation_dispatcher = workspace_mutations.clone();
            move |event, document| {
                let state_cell = &event_cell;
                let mut state = state_cell.borrow_mut();
                if let Err(error) = refresh_all(document, &mut state) {
                    eprintln!("desktop: refresh failed (continuing): {error}");
                }
                let result = match event {
                    UiControllerEvent::Action(action) => {
                        handle_event(action, document, &mut state, mutation_dispatcher.as_ref())
                    }
                    UiControllerEvent::Key(key) => handle_desktop_key(key, document, &mut state),
                    UiControllerEvent::Resized { .. } => Ok(()),
                    UiControllerEvent::Drag(_) => Ok(()),
                    UiControllerEvent::Visibility { obscured } => {
                        widgets::set_obscured(document, &mut state, *obscured)
                    }
                };
                event_watch.set(menus_open(&state.menu_rects));
                // A failed handler must not tear down the loop: the desktop would stop answering clicks.
                if let Err(error) = result {
                    eprintln!("desktop: event handler failed (continuing): {error}");
                }
                Ok(())
            }
        },
        {
            let flush_cell = std::rc::Rc::clone(&state_cell);
            let panels_flag = std::rc::Rc::clone(&panels_dirty);
            let reactor_watch = std::rc::Rc::clone(&press_watch);
            move |document| {
                if panels_flag.replace(false) {
                    let mut state = flush_cell.borrow_mut();
                    let (next_outputs, next_grids, next_default) =
                        refresh_output_layout(surface, state.entry_scale_percent);
                    if next_outputs != state.output_geometries
                        || next_grids != state.output_grids
                        || next_default != state.default_output
                    {
                        state.output_geometries = next_outputs;
                        state.output_grids = next_grids;
                        state.default_output = next_default;
                        state.grid = grid_for_output(&state, &state.default_output);
                        let metrics = widgets::widget_metrics(&state);
                        state.widgets.store.layout_grid(metrics);
                        let monitors = monitor_rects(&state.output_geometries);
                        let DesktopState {
                            wallpaper_loader,
                            settings,
                            ..
                        } = &mut *state;
                        if let Err(error) = request_wallpaper(wallpaper_loader, settings, &monitors)
                        {
                            eprintln!("desktop: wallpaper topology refresh failed: {error:?}");
                        }
                        if let Err(error) = rescan_desktop(&mut state) {
                            eprintln!("desktop: output layout refresh failed: {error}");
                        }
                        sync_document(document, &mut state)?;
                        state.settings_visual_dirty = true;
                    }
                }
                let state_cell = &flush_cell;
                let mut state = state_cell.borrow_mut();
                if let Err(error) = apply_pending_rubber(document, &mut state) {
                    eprintln!("desktop: rubber update failed (continuing): {error}");
                }
                if let Err(error) = widgets::apply_pending_drag(document, &mut state) {
                    eprintln!("desktop: widget drag update failed (continuing): {error}");
                }
                reactor_watch.set(menus_open(&state.menu_rects));
                // Live toggle: the setting snapshot is the authority; watch add/remove follows it.
                let organize_wanted = setting_bool(&state.settings, "organizeDownloads", false);
                state.downloads.set_enabled(organize_wanted);
                state.downloads.tick(std::time::Instant::now());
                if state.downloads.take_notice()
                    && let Err(error) = show_downloads_notice(document, &mut state)
                {
                    eprintln!("desktop: downloads notice failed: {error}");
                }
                let _completed_workspace_present = state.workspace_present_span.take();
                if let Err(error) = refresh_all(document, &mut state) {
                    eprintln!("desktop: refresh failed (continuing): {error}");
                }
                if let Err(error) = widgets::poll(document, &mut state) {
                    eprintln!("desktop: widget refresh failed: {error}");
                }
                let result = sticky::flush_persist_if_due(document, &mut state);
                match result {
                    Ok(()) => {
                        if state.workspace_present_pending {
                            state.workspace_present_pending = false;
                            state.workspace_present_span = Some(workspace_present_point().start());
                        }
                    }
                    Err(error) => {
                        eprintln!(
                            "desktop: sticky persist flush failed: {error}; retry stays armed"
                        );
                    }
                }
                Ok(())
            }
        },
    )?;
    sticky::flush_now(&state_cell.borrow());
    Ok(())
}

fn register_media_widget(
    reactor: &mut Reactor,
    state_cell: &std::rc::Rc<std::cell::RefCell<DesktopState>>,
) {
    match flamewm_integrations_linux::MprisProvider::connect() {
        Ok(mut provider) => match provider.start() {
            Ok(()) => {
                let watch = provider.watch();
                let interest = if watch
                    .events
                    .contains(flamewm_api::ports::FdEvents::WRITABLE)
                {
                    calloop::Interest::BOTH
                } else {
                    calloop::Interest::READ
                };
                let media_cell = std::rc::Rc::clone(&state_cell);
                let registration =
                    reactor.register_raw_fd_with_action(watch.fd, interest, move |_, _| {
                        widgets::refresh_media(&mut media_cell.borrow_mut());
                        FdAction::Continue
                    });
                warn_degraded("media widget", registration);
                let mut state = state_cell.borrow_mut();
                state.widgets.mpris = Some(provider);
                widgets::refresh_media(&mut state);
            }
            Err(error) => eprintln!("desktop: media widget unavailable: {}", error.message),
        },
        Err(error) => eprintln!("desktop: media widget unavailable: {}", error.message),
    }
}

fn register_workspace_signals(
    reactor: &mut Reactor,
    state_cell: &std::rc::Rc<std::cell::RefCell<DesktopState>>,
    panels_dirty: &std::rc::Rc<std::cell::Cell<bool>>,
) {
    // Panel strut changes arrive as control signals. The signal handler only
    // sets this flag; the reactor tick re-resolves the work area once per burst.
    // Best-effort workspace signal subscription: startup query already ran.
    // WorkspacesSnapshotChanged is the live workspace authority; the legacy
    // revision-only signal is intentionally not a fetch trigger.
    match ControlSignalClient::connect(BusKind::Session) {
        Ok(client) => {
            let client: &'static ControlSignalClient = Box::leak(Box::new(client));
            let watch = client.watch();
            let watch_fd = watch.fd;
            let watch_readable = watch
                .events
                .contains(flamewm_api::ports::FdEvents::READABLE);
            let watch_writable = watch
                .events
                .contains(flamewm_api::ports::FdEvents::WRITABLE);
            let interest = match (watch_readable, watch_writable) {
                (true, true) => calloop::Interest::BOTH,
                (false, true) => calloop::Interest::WRITE,
                _ => calloop::Interest::READ,
            };
            let workspace_cell = std::rc::Rc::clone(&state_cell);
            let panels_signal_flag = std::rc::Rc::clone(&panels_dirty);
            let registration = reactor.register_raw_fd_with_action(watch_fd, interest, {
                move |_, _| {
                    let state_cell = &workspace_cell;
                    let panels_flag = &panels_signal_flag;
                    let _ = client.on_ready(|signal| {
                        match signal {
                            flamewm_control_wire::ControlSignal::SettingsSnapshotChanged {
                                snapshot,
                            } => {
                                let mut state = state_cell.borrow_mut();
                                let monitors = monitor_rects(&state.output_geometries);
                                if let Err(error) = request_wallpaper(
                                    &mut state.wallpaper_loader,
                                    &snapshot,
                                    &monitors,
                                ) {
                                    eprintln!(
                                        "desktop: wallpaper settings request failed: {error:?}"
                                    );
                                }
                                state.settings = snapshot;
                                state.settings_visual_dirty = true;
                            }
                            flamewm_control_wire::ControlSignal::WorkspacesSnapshotChanged {
                                snapshot,
                            } => {
                                let _signal = workspace_signal_point().start();
                                workspace_snapshot_received_counter().increment();
                                let mut state = state_cell.borrow_mut();
                                // Authoritative apply consumes snapshot.active_index directly.
                                let active_changed = {
                                    let _apply = workspace_apply_point().start();
                                    state.workspace.apply_snapshot(snapshot)
                                };
                                if active_changed {
                                    workspace_change_counter().increment();
                                    state.workspace_visual_dirty = true;
                                }
                            }
                            signal if workspace_sync::is_legacy_workspace_signal(&signal) => {
                                workspace_legacy_ignored_counter().increment();
                            }
                            flamewm_control_wire::ControlSignal::PanelsSnapshotChanged {
                                ..
                            }
                            | flamewm_control_wire::ControlSignal::PanelsChanged { .. }
                            | flamewm_control_wire::ControlSignal::DisplaysSnapshotChanged {
                                ..
                            }
                            | flamewm_control_wire::ControlSignal::DisplayTransactionChanged {
                                ..
                            } => {
                                panels_flag.set(true);
                            }
                            flamewm_control_wire::ControlSignal::WeatherFeedChanged {
                                snapshot,
                            } => {
                                let mut state = state_cell.borrow_mut();
                                state.weather = snapshot;
                                state.weather_visual_dirty = true;
                            }
                            _ => {}
                        }
                        // The next UI turn projects sync_sticky and sync_menu only;
                        // workspace signals never enter filesystem refresh.
                    });
                    FdAction::Continue
                }
            });
            warn_degraded("workspaces signal", registration);
            // ControlSignalClient is Box::leaked above; it lives for the process
            // lifetime and owns the pump fd.
        }
        Err(error) => {
            eprintln!("desktop: degraded workspaces signal subscription: {error:?}");
        }
    }
}

fn register_inotify(
    reactor: &mut Reactor,
    inotify: inotify::Inotify,
    dirty: Arc<AtomicBool>,
) -> Result<(), String> {
    reactor
        .register_fd_with_source_action(inotify, calloop::Interest::READ, move |_, source, _| {
            let mut buffer = [0u8; 4096];
            let mut changed = false;
            inotify_wakeup_counter().increment();
            loop {
                match source.read_events(&mut buffer) {
                    Ok(events) => {
                        if events.count() == 0 {
                            break;
                        }
                        changed = true;
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }
            if changed {
                dirty.store(true, Ordering::Release);
            }
            // Rearm until fd close/IGNORE/Q_OVERFLOW self-delete; level
            // trigger keeps one rescan per burst via reduce below.
            FdAction::Continue
        })
        .map_err(|error| format!("register inotify: {error}"))?;
    Ok(())
}

fn register_downloads(
    reactor: &mut Reactor,
    state_cell: &std::rc::Rc<std::cell::RefCell<DesktopState>>,
) -> Result<(), String> {
    // Downloads organizer: the same reactor owns its watch fd; the wake slot is shared with
    // sticky persistence, so the organizer only ever publishes its own settle deadline.
    let downloads_source = {
        let mut state = state_cell.borrow_mut();
        state.downloads.attach_wake(reactor.wake_deadline());
        state.downloads.take_source()
    };
    if let Some(source) = downloads_source {
        let downloads_queue = state_cell.borrow().downloads.queue();
        reactor
            .register_fd_with_source_action(source, calloop::Interest::READ, move |_, source, _| {
                downloads::read_events(source, &downloads_queue);
                FdAction::Continue
            })
            .map_err(|error| format!("register downloads organizer: {error}"))?;
    }
    Ok(())
}

fn register_icon_and_wallpaper_wakes(
    reactor: &mut Reactor,
    state_cell: &std::rc::Rc<std::cell::RefCell<DesktopState>>,
    icons_pending: &Arc<AtomicBool>,
) {
    // IconService wake FD: worker completions only set the pending flag;
    // the next event/flush tick drains and repaints affected tiles.
    if let Some(icon_fd) = state_cell.borrow().icons.wake_fd() {
        let icon_cell = std::rc::Rc::clone(&state_cell);
        let icon_pending = Arc::clone(&icons_pending);
        let registration =
            reactor.register_raw_fd_with_action(icon_fd, calloop::Interest::READ, move |_, _| {
                icon_cell.borrow_mut().icons.wake_drain();
                icon_pending.store(true, Ordering::Release);
                FdAction::Continue
            });
        warn_degraded("icon completion", registration);
    }
    let wallpaper_fd = state_cell.borrow().wallpaper_loader.wake_fd();
    let wallpaper_cell = std::rc::Rc::clone(&state_cell);
    let registration =
        reactor.register_raw_fd_with_action(wallpaper_fd, calloop::Interest::READ, move |_, _| {
            let mut state = wallpaper_cell.borrow_mut();
            state.wallpaper_loader.wake_drain();
            state.settings_visual_dirty = true;
            FdAction::Continue
        });
    warn_degraded("wallpaper completion", registration);
}

fn initial_document(
    state_cell: &std::rc::Rc<std::cell::RefCell<DesktopState>>,
) -> Result<flamewm_ui_x11::UiDocument, String> {
    let mut document = decode_document(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/flamewm-desktop.rwr"
    )))?;
    ui_contract::require_desktop_nodes(|id| document.has_node(id))?;
    {
        let state = state_cell.borrow();
        projection::project_global_font_defaults(&mut document, &state.settings)?;
        projection::project_background(
            &mut document,
            &state.wallpaper_loader.state().presentation,
        )?;
    }
    flamewm_ui_x11::apply_render_acceleration(&state_cell.borrow().settings);
    projection::assign_document_layers(&mut document)?;
    widgets::assign_layers(&mut document)?;
    projection::initialize_transient_visibility(&mut document)?;
    {
        let _span = sync_point().start();
        sync_document(&mut document, &mut state_cell.borrow_mut())?;
    }
    update_gauges(&state_cell.borrow());
    Ok(document)
}

#[cfg(test)]
#[path = "tests/cursor_reload_tests.rs"]
mod cursor_reload_tests;

#[cfg(test)]
#[path = "tests/accent_token_tests.rs"]
mod accent_token_tests;

mod input;
use input::*;
mod events;
use events::*;
mod menu_actions;
use menu_actions::*;
mod rename;
use rename::*;
mod files;
use files::*;
mod menus;
use menus::*;
#[cfg(test)]
#[path = "tests/work_area_tests.rs"]
mod work_area_tests;

#[cfg(test)]
#[path = "tests/packaged_root_tests.rs"]
mod packaged_root_tests;

#[cfg(test)]
#[path = "tests/general_settings_tests.rs"]
mod general_settings_tests;

#[cfg(test)]
#[path = "tests/icon_theme_tests.rs"]
mod icon_theme_tests;

#[cfg(test)]
#[path = "tests/outside_press_tests.rs"]
mod outside_press_tests;

#[cfg(test)]
#[path = "tests/rename_button_tests.rs"]
mod rename_button_tests;

#[cfg(test)]
#[path = "tests/workspace_menu_lifetime_tests.rs"]
mod workspace_menu_lifetime_tests;

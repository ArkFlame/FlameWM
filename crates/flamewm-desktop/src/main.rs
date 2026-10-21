use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flamewm_api::applications::ApplicationLaunchOptions;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_api::workspace::WorkspaceSnapshot;
use flamewm_api::{OutputId, Point, Rect};
use flamewm_applications::DesktopEntry;
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::{ControlClient, ControlSignalClient, MutationDispatcher};
use flamewm_dbus_reactor::BusKind;
use flamewm_desktop_core::conversion::{convert_file, offered_formats};
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
        fn $name() -> &'static $ty {
            cached!($ty, $key)
        }
    };
}

mod convert_wheel;
mod projection;
mod sticky;
#[path = "../ui_contract.rs"]
mod ui_contract;
mod wallpaper_fit;
mod wallpaper_loader;
mod workspace_sync;

point_fn!(scan_point, ProfilePoint, "desktop.scan");
point_fn!(sync_point, ProfilePoint, "desktop.sync_document");
point_fn!(rescan_point, ProfilePoint, "desktop.rescan");
point_fn!(icon_point, ProfilePoint, "desktop.icon.resolve");
point_fn!(
    inotify_wakeup_counter,
    CounterPoint,
    "desktop.inotify.wakeup"
);
point_fn!(
    inotify_rescan_counter,
    CounterPoint,
    "desktop.inotify.rescan"
);
point_fn!(
    selection_begin_counter,
    CounterPoint,
    "desktop.selection.begin"
);
point_fn!(
    selection_visible_counter,
    CounterPoint,
    "desktop.selection.visible"
);
point_fn!(selection_end_counter, CounterPoint, "desktop.selection.end");
point_fn!(
    context_secondary_counter,
    CounterPoint,
    "desktop.context.secondary"
);
point_fn!(context_open_counter, CounterPoint, "desktop.context.open");
point_fn!(
    context_open_total_point,
    ProfilePoint,
    "desktop.context.open.total"
);
point_fn!(
    context_open_project_point,
    ProfilePoint,
    "desktop.context.open.project"
);
point_fn!(
    context_open_measure_point,
    ProfilePoint,
    "desktop.context.open.measure"
);
point_fn!(
    context_open_place_point,
    ProfilePoint,
    "desktop.context.open.place"
);
point_fn!(
    context_open_present_point,
    ProfilePoint,
    "desktop.context.open.present"
);
point_fn!(
    context_open_close_point,
    ProfilePoint,
    "desktop.context.open.close"
);
point_fn!(
    context_action_counter,
    CounterPoint,
    "desktop.context.action"
);
point_fn!(render_sync_counter, CounterPoint, "desktop.render.sync");
point_fn!(
    drag_ghost_show_counter,
    CounterPoint,
    "desktop.drag.ghost.show"
);
point_fn!(
    drag_ghost_motion_counter,
    CounterPoint,
    "desktop.drag.ghost.motion"
);
point_fn!(rename_open_counter, CounterPoint, "desktop.rename.open");
point_fn!(
    rename_success_counter,
    CounterPoint,
    "desktop.rename.success"
);
point_fn!(
    rename_failure_counter,
    CounterPoint,
    "desktop.rename.failure"
);
point_fn!(
    action_failure_counter,
    CounterPoint,
    "desktop.action.failure"
);
point_fn!(spawn_failure_counter, CounterPoint, "desktop.spawn.failure");
fn report_action_failure(context: &str, error: String) {
    action_failure_counter().increment();
    eprintln!("desktop: {context} failed: {error}; keeping event loop alive");
}

point_fn!(
    workspace_change_counter,
    CounterPoint,
    "desktop.workspace.change"
);
point_fn!(
    workspace_signal_point,
    ProfilePoint,
    "desktop.workspace.signal"
);
point_fn!(
    workspace_apply_point,
    ProfilePoint,
    "desktop.workspace.apply"
);
point_fn!(
    workspace_project_point,
    ProfilePoint,
    "desktop.workspace.project"
);
point_fn!(
    workspace_present_point,
    ProfilePoint,
    "desktop.workspace.present"
);
point_fn!(
    workspace_snapshot_received_counter,
    CounterPoint,
    "desktop.workspace.snapshot_received"
);
point_fn!(
    workspace_legacy_ignored_counter,
    CounterPoint,
    "desktop.workspace.legacy_ignored"
);
point_fn!(model_gauge, MemoryGauge, "desktop.model");
point_fn!(launcher_gauge, MemoryGauge, "desktop.launcher");
point_fn!(icon_gauge, MemoryGauge, "desktop.icon");
fn rescan_desktop(state: &mut DesktopState) -> Result<(), String> {
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

fn selected_menu_id(state: &DesktopState) -> Option<String> {
    state
        .menu
        .as_ref()
        .and_then(|menu| menu.selected_id.clone())
}

fn update_gauges(state: &DesktopState) {
    model_gauge().set((state.model.items().count() as u64).saturating_mul(256));
    launcher_gauge().set((state.launchers.len() as u64).saturating_mul(320));
    icon_gauge().set(state.icons.memory_estimate_bytes() as u64);
}

const SLOT_COUNT: usize = 128;
const BLANK_ROW_IDS: [&str; 5] = [
    "menu-row-terminal",
    "menu-row-folder",
    "menu-row-note",
    "menu-row-desktop",
    "menu-row-settings",
];
const ENTRY_ROW_IDS: [&str; 5] = [
    "menu-row-empty-trash",
    "menu-row-rename",
    "menu-row-shortcut",
    "menu-row-convert",
    "menu-row-delete",
];

struct LauncherMeta {
    icon: String,
    name: String,
    valid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    Blank,
    Entry,
    ConvertWheel,
    Sticky,
}

struct MenuState {
    kind: MenuKind,
    anchor: Point,
    output: OutputId,
    selected_id: Option<String>,
}

struct ClickRecord {
    id: String,
    x: f32,
    y: f32,
    at: Instant,
}

struct DesktopState {
    directory: PathBuf,
    model: DesktopModel,
    grid: GridConfig,
    output_grids: BTreeMap<OutputId, GridConfig>,
    entry_scale_percent: u16,
    output_geometries: Vec<(OutputId, Rect)>,
    default_output: OutputId,
    icons: projection::ParallelIconService,
    icon_slots: Vec<Option<IconKey>>,
    layout_path: PathBuf,
    sticky: sticky::StickyState,
    selection: SelectionModel,
    selection_start: Option<Point>,
    selection_output: Option<OutputId>,
    item_drag: Option<ItemDrag>,
    launchers: BTreeMap<PathBuf, LauncherMeta>,
    menu: Option<MenuState>,
    /// Rects of the menus and confirm currently shown (logical px); empty when none is open.
    menu_rects: Vec<Rect>,
    wheel_hover: Option<usize>,
    pending_delete: Option<Vec<PathBuf>>,
    last_click: Option<ClickRecord>,
    dirty: Arc<AtomicBool>,
    icons_pending: Arc<AtomicBool>,
    rename: Option<RenameState>,
    drag_ghost: Option<DragGhost>,
    conversion_preview: Option<ConversionPreview>,
    workspace: workspace_sync::WorkspaceAuthority,
    workspace_visual_dirty: bool,
    workspace_present_pending: bool,
    workspace_present_span: Option<flamewm_profiler::SpanGuard>,
    settings: SettingsSnapshot,
    settings_visual_dirty: bool,
    /// Resolved cursor theme last loaded into the live cursor owners.
    applied_cursor_theme: Option<String>,
    /// Last `iconTheme` applied from the settings snapshot.
    icon_theme: String,
    /// Set when `iconTheme` changed; cleared once the new theme generation is installed.
    icon_theme_pending: bool,
    wallpaper_loader: wallpaper_loader::WallpaperLoader,
}

#[derive(Clone)]
struct RenameState {
    item_id: String,
    source_path: PathBuf,
    buffer: String,
}

struct ConversionPreview {
    output: OutputId,
    cell: Cell,
    destination: PathBuf,
    result: Arc<Mutex<Option<Result<PathBuf, String>>>>,
}

struct DragGhost {
    output: OutputId,
    source_ids: Vec<String>,
    kind: DesktopItemKind,
    icon_override: Option<String>,
    label: String,
    offset_x: f32,
    offset_y: f32,
    count: usize,
    visible: bool,
}

#[derive(Clone)]
struct ItemDrag {
    anchor_index: usize,
    anchor_id: String,
    start_x: f32,
    start_y: f32,
    originals: Vec<OriginalItem>,
    moved: bool,
}

#[derive(Clone)]
struct OriginalItem {
    id: String,
    path: PathBuf,
    output: OutputId,
    cell: Cell,
    drag_cell: Cell,
}

fn main() -> Result<(), String> {
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
    let wallpaper_monitors = output_geometries
        .iter()
        .map(|(_, rect)| *rect)
        .collect::<Vec<_>>();
    request_wallpaper(&mut wallpaper_loader, &settings, &wallpaper_monitors)
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
    let dirty = Arc::new(AtomicBool::new(false));
    let icons_pending = Arc::new(AtomicBool::new(false));
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
            layout_path,
            sticky: sticky::StickyState::new(
                sticky,
                sticky_path,
                setting_bool(&settings, "stickyNotesEnabled", true),
            ),
            selection: SelectionModel::default(),
            selection_start: None,
            selection_output: None,
            item_drag: None,
            launchers: BTreeMap::new(),
            menu: None,
            menu_rects: Vec::new(),
            wheel_hover: None,
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
            applied_cursor_theme: None,
            icon_theme: String::new(),
            icon_theme_pending: false,
            wallpaper_loader,
        };
        refresh_launchers(&mut state);
        update_gauges(&state);
        state
    }));

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
            state.settings.values.get("wallpaper"),
            &state.wallpaper_loader.state().presentation,
        )?;
    }
    for error in flamewm_ui_x11::apply_cursor_shadow(setting_bool(
        &state_cell.borrow().settings,
        "cursorShadow",
        true,
    )) {
        eprintln!("desktop: cursor shadow: {error}");
    }
    projection::assign_document_layers(&mut document)?;
    projection::initialize_transient_visibility(&mut document)?;
    {
        let _span = sync_point().start();
        sync_document(&mut document, &mut state_cell.borrow_mut())?;
    }
    update_gauges(&state_cell.borrow());
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
    sticky::attach_persist_wake(&mut state_cell.borrow_mut().sticky, &reactor);
    // Panel strut changes arrive as control signals. The signal handler only
    // sets this flag; the reactor tick re-resolves the work area once per burst.
    let panels_dirty = std::rc::Rc::new(std::cell::Cell::new(false));
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
                                let monitors = state
                                    .output_geometries
                                    .iter()
                                    .map(|(_, rect)| *rect)
                                    .collect::<Vec<_>>();
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
                            _ => {}
                        }
                        // The next UI turn projects sync_sticky and sync_menu only;
                        // workspace signals never enter filesystem refresh.
                    });
                    FdAction::Continue
                }
            });
            if let Err(error) = registration {
                eprintln!("desktop: degraded workspaces signal subscription: {error}");
            }
            // ControlSignalClient is Box::leaked above; it lives for the process
            // lifetime and owns the pump fd.
        }
        Err(error) => {
            eprintln!("desktop: degraded workspaces signal subscription: {error:?}");
        }
    }
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
        if let Err(error) = registration {
            eprintln!("desktop: degraded workspace mutation subscription: {error}");
        }
    }
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
        if let Err(error) = registration {
            eprintln!("desktop: degraded icon completion subscription: {error}");
        }
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
    if let Err(error) = registration {
        eprintln!("desktop: degraded wallpaper completion subscription: {error}");
    }
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
                refresh_settings_projection(document, &mut state)?;
                refresh_icon_theme(document, &mut state)?;
                refresh_workspace_projection(document, &mut state)?;
                refresh_if_changed(document, &mut state)?;
                apply_pending_icon_results(document, &mut state)?;
                sync_conversion_preview(document, &mut state)?;
                sticky::poll_persist(document, &mut state)?;
                let result = match event {
                    UiControllerEvent::Action(action) => {
                        handle_event(action, document, &mut state, mutation_dispatcher.as_ref())
                    }
                    UiControllerEvent::Key(key) => handle_desktop_key(key, document, &mut state),
                };
                event_watch.set(menus_open(&state.menu_rects));
                result
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
                        state.grid = state
                            .output_grids
                            .get(&state.default_output)
                            .copied()
                            .unwrap_or(state.grid);
                        let monitors = state
                            .output_geometries
                            .iter()
                            .map(|(_, rect)| *rect)
                            .collect::<Vec<_>>();
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
                reactor_watch.set(menus_open(&state.menu_rects));
                let _completed_workspace_present = state.workspace_present_span.take();
                refresh_settings_projection(document, &mut state)?;
                refresh_icon_theme(document, &mut state)?;
                refresh_workspace_projection(document, &mut state)?;
                refresh_if_changed(document, &mut state)?;
                apply_pending_icon_results(document, &mut state)?;
                sync_conversion_preview(document, &mut state)?;
                sticky::poll_persist(document, &mut state)?;
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

fn query_workspaces() -> WorkspaceSnapshot {
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

fn query_settings() -> SettingsSnapshot {
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

/// Resolves wallpaper settings and requests a root canvas fitted independently per output unless
/// the user opts into the legacy shared canvas.
fn request_wallpaper(
    loader: &mut wallpaper_loader::WallpaperLoader,
    snapshot: &SettingsSnapshot,
    monitors: &[Rect],
) -> Result<u64, wallpaper_loader::LoaderClosed> {
    let values = &snapshot.values;
    let text = |key: &str| match values.get(key) {
        Some(SettingValue::Text(value)) => value.clone(),
        _ => String::new(),
    };
    let source = wallpaper_loader::source_for_settings(
        &text("wallpaperMode"),
        &text("wallpaper"),
        &text("wallpaperColor"),
    );
    match source {
        wallpaper_loader::WallpaperSource::Solid(rgb) => loader.request_solid(rgb),
        wallpaper_loader::WallpaperSource::Image(path) => {
            let share = setting_bool(snapshot, "shareWallpaper", false);
            loader.request_layout(
                path,
                wallpaper_fit::parse(&text("wallpaperFit")),
                monitors,
                share,
            )
        }
    }
}

fn setting_text<'a>(snapshot: &'a SettingsSnapshot, key: &str) -> String {
    match snapshot.values.get(key) {
        Some(SettingValue::Text(value)) => value.clone(),
        _ => String::new(),
    }
}

fn setting_bool(snapshot: &SettingsSnapshot, key: &str, default: bool) -> bool {
    match snapshot.values.get(key) {
        Some(SettingValue::Boolean(value)) => *value,
        _ => default,
    }
}

/// True when the snapshot's `iconTheme` differs from the last applied value.
fn icon_theme_changed(previous: &str, snapshot: &SettingsSnapshot) -> bool {
    setting_text(snapshot, "iconTheme") != previous
}

fn setting_percent(snapshot: &SettingsSnapshot, key: &str, default: u8) -> u8 {
    match snapshot.values.get(key) {
        Some(SettingValue::Percent(value)) => *value,
        _ => default,
    }
}

fn setting_integer(snapshot: &SettingsSnapshot, key: &str, default: i64) -> i64 {
    match snapshot.values.get(key) {
        Some(SettingValue::Integer(value)) => *value,
        _ => default,
    }
}

fn desktop_entry_scale(snapshot: &SettingsSnapshot) -> u16 {
    setting_integer(snapshot, "DesktopEntryScalePercent", 100).clamp(50, 200) as u16
}

fn percent_alpha(percent: u8) -> u8 {
    ((u16::from(percent) * 255) / 100) as u8
}

/// Neutral grey until the settings snapshot supplies `accent` (mirrors the shell default).
const NEUTRAL_ACCENT: u32 = 0x7f878e;
/// Accent-derived tokens of `ui/shell/flamewm.css`, with the alpha each keeps in its `:root`.
const ACCENT_TOKEN_ALPHAS: [(&str, u8); 9] = [
    ("--accent", 0xff),
    ("--accent-soft", 0x38),
    ("--accent-strong", 0xdd),
    ("--desktop-hover", 0x26),
    ("--desktop-hover-border", 0x4d),
    ("--desktop-selected", 0x47),
    ("--desktop-selected-border", 0xa6),
    ("--selection-fill", 0x33),
    ("--snap-fill", 0x33),
];

fn accent_color(snapshot: &SettingsSnapshot) -> UiColor {
    let hex = match snapshot.values.get("accent") {
        Some(SettingValue::Text(value)) => value.strip_prefix('#').unwrap_or(value),
        _ => "",
    };
    let rgb = if hex.len() == 6 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        u32::from_str_radix(hex, 16).unwrap_or(NEUTRAL_ACCENT)
    } else {
        NEUTRAL_ACCENT
    };
    UiColor {
        r: (rgb >> 16) as u8,
        g: (rgb >> 8) as u8,
        b: rgb as u8,
        a: 255,
    }
}

/// Accent-derived token values for the desktop; `accentOpacity` scales them like the shell does.
fn accent_token_values(snapshot: &SettingsSnapshot) -> Vec<(&'static str, UiColor)> {
    let accent = accent_color(snapshot);
    let opacity = percent_alpha(setting_percent(snapshot, "accentOpacity", 100));
    ACCENT_TOKEN_ALPHAS
        .into_iter()
        .map(|(name, alpha)| {
            let scaled = ((u16::from(alpha) * u16::from(opacity)) / 255) as u8;
            (
                name,
                UiColor {
                    a: scaled,
                    ..accent
                },
            )
        })
        .collect()
}

fn project_accent_tokens(
    document: &mut impl UiDocumentAccess,
    snapshot: &SettingsSnapshot,
) -> Result<(), String> {
    for (name, color) in accent_token_values(snapshot) {
        document.color_variable(name, color)?;
    }
    Ok(())
}

#[cfg(test)]
mod cursor_reload_tests {
    use flamewm_session_core::cursor_theme_reload;

    #[test]
    fn cursor_reload_only_when_resolved_theme_changes() {
        let root =
            std::env::temp_dir().join(format!("flamewm-desktop-cursor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Custom").join("cursors")).expect("theme");
        let search = root.display().to_string();
        let first = cursor_theme_reload(None, "Custom", &search);
        assert_eq!(first.as_deref(), Some("Custom"));
        assert_eq!(
            cursor_theme_reload(first.as_deref(), "Custom", &search),
            None
        );
        // An uninstalled request resolves to the same fallback as the applied theme: no reload.
        let fallback = flamewm_session_core::resolve_cursor_theme("Missing", &search);
        assert_eq!(
            cursor_theme_reload(Some(&fallback), "Missing", &search),
            None
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod accent_token_tests {
    use super::*;

    #[test]
    fn desktop_tokens_follow_live_accent_and_fall_back_to_neutral() {
        let mut snapshot = SettingsSnapshot::new(1);
        assert_eq!(accent_color(&snapshot).r, 0x7f);
        snapshot.values.insert(
            "accent".to_owned(),
            SettingValue::Text("#9b59b6".to_owned()),
        );
        let tokens = accent_token_values(&snapshot);
        let value = |name: &str| tokens.iter().find(|(key, _)| *key == name).map(|(_, c)| *c);
        assert_eq!(
            value("--desktop-hover"),
            Some(UiColor {
                r: 0x9b,
                g: 0x59,
                b: 0xb6,
                a: 0x26
            })
        );
        assert_eq!(value("--snap-fill").map(|c| c.a), Some(0x33));
        assert_eq!(value("--accent"), Some(accent_color(&snapshot)));
        assert_eq!(tokens.len(), ACCENT_TOKEN_ALPHAS.len());
    }

    #[test]
    fn embedded_desktop_document_has_all_required_convert_nodes() {
        let document = decode_document(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/flamewm-desktop.rwr"
        )))
        .expect("decode embedded compiled RWR");
        ui_contract::require_desktop_nodes(|id| document.has_node(id))
            .expect("embedded desktop UI must satisfy runtime ID contract");
    }

    #[test]
    fn desktop_compiled_document_declares_every_accent_token() {
        let mut document = decode_document(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/flamewm-desktop.rwr"
        )))
        .expect("decode compiled desktop document");
        project_accent_tokens(&mut document, &SettingsSnapshot::new(1))
            .expect("every accent token is declared in ui/desktop :root");
    }
}

/// Re-projects every item under a new icon theme generation. The rebuild runs on the icon
/// worker; the pending flag is polled on each UI tick until that generation is installed.
/// Results queued for the previous generation never match a re-projected slot key.
fn refresh_icon_theme(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if !state.icon_theme_pending || !state.icons.apply_icon_theme(&state.icon_theme) {
        return Ok(());
    }
    state.icon_theme_pending = false;
    sync_document(document, state)
}

fn refresh_settings_projection(
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
    let wallpaper = state.settings.values.get("wallpaper");
    projection::project_global_font_defaults(document, &state.settings)?;
    projection::project_background(
        document,
        wallpaper,
        &state.wallpaper_loader.state().presentation,
    )?;
    if let Err(error) = project_accent_tokens(document, &state.settings) {
        // Optional visual tokens must never terminate the desktop at startup.
        eprintln!("desktop: accent tokens not projected: {error}");
    }
    sync_selection_visuals(document, state)?;
    sticky::sync_sticky(document, state)?;
    for id in projection::MENU_SURFACE_IDS {
        projection::project_menu_surface(document, id, &state.settings)?;
    }
    // Redraw the active radial hover with the current accent after live theme
    // changes; clearing the stamp forces regeneration of the one segment only.
    if let Some(sector) = state.wheel_hover.take() {
        update_wheel_hover(document, state, Some(sector))?;
    }
    sync_cursor_theme(state);
    for error in
        flamewm_ui_x11::apply_cursor_shadow(setting_bool(&state.settings, "cursorShadow", true))
    {
        eprintln!("desktop: cursor shadow toggle: {error}");
    }
    if entry_scale_changed {
        sync_document(document, state)?;
    }
    state.settings_visual_dirty = false;
    Ok(())
}

/// Reloads the live cursor owners when the resolved `cursorTheme` changed. A failed reload
/// keeps the previous theme (rollback) and is retried at the next settings snapshot.
fn sync_cursor_theme(state: &mut DesktopState) {
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

/// Root geometry comes from the actual X root through the X11 runtime owner.
/// The root-sized surface paints wallpaper under panels; per-output grids subtract panel geometry.
/// The desktop surface spans the whole root at origin 0,0 so the wallpaper paints under panels
/// and struts. Icon layout and selection stay in the work area (`GridConfig::work_area`).
fn desktop_surface_rect(root_width: u32, root_height: u32) -> Rect {
    Rect::new(0, 0, root_width.max(1) as i32, root_height.max(1) as i32)
}

fn resolve_output_layout(
    surface: Rect,
    entry_scale_percent: u16,
) -> (
    Vec<(OutputId, Rect)>,
    BTreeMap<OutputId, GridConfig>,
    OutputId,
) {
    let outputs = control_client()
        .and_then(|client| match client.call(&ControlRequest::GetDisplays) {
            Ok(ControlResponse::Displays(snapshot)) => Ok(snapshot
                .outputs
                .into_iter()
                .filter(|output| output.connected)
                .collect::<Vec<_>>()),
            Ok(response) => Err(format!("GetDisplays returned {response:?}")),
            Err(error) => Err(error.message),
        })
        .unwrap_or_else(|error| {
            eprintln!("desktop: degraded monitor geometry: {error}; using full root geometry");
            Vec::new()
        });
    let scales = outputs
        .iter()
        .map(|output| (output.id.clone(), output.shell_scale_percent))
        .collect::<BTreeMap<_, _>>();
    let raw = outputs
        .iter()
        .map(|output| (output.id.clone(), output.geometry, output.primary))
        .collect::<Vec<_>>();
    let (mut geometries, origin_x, origin_y) = normalize_outputs(&raw, surface);
    let default_output = outputs
        .iter()
        .find(|output| output.primary && geometries.iter().any(|(id, _)| id == &output.id))
        .map(|output| output.id.clone())
        .or_else(|| geometries.first().map(|(id, _)| id.clone()))
        .unwrap_or_else(|| OutputId::new("default"));
    if geometries.is_empty() {
        geometries.push((default_output.clone(), surface));
    }
    let panels = match panel_geometries() {
        Ok(panels) => panels,
        Err(error) => {
            eprintln!("desktop: degraded work areas: {error}; using monitor bounds");
            Vec::new()
        }
    };
    let panels = panels
        .into_iter()
        .map(|(id, rect)| {
            (
                id,
                Rect::new(
                    rect.x.saturating_sub(origin_x),
                    rect.y.saturating_sub(origin_y),
                    rect.width,
                    rect.height,
                ),
            )
        })
        .collect::<Vec<_>>();
    let grids = grids_for_outputs(&geometries, &panels, &scales, entry_scale_percent);
    (geometries, grids, default_output)
}

fn grids_for_outputs(
    outputs: &[(OutputId, Rect)],
    panels: &[(OutputId, Rect)],
    scales: &BTreeMap<OutputId, u16>,
    entry_scale_percent: u16,
) -> BTreeMap<OutputId, GridConfig> {
    outputs
        .iter()
        .map(|(output, geometry)| {
            let area = panels
                .iter()
                .filter(|(panel_output, _)| panel_output == output)
                .fold(*geometry, |area, (_, panel)| {
                    subtract_panel_strip(area, *panel)
                });
            (
                output.clone(),
                GridConfig::compute_with_entry_scale(
                    area,
                    scales.get(output).copied().unwrap_or(100),
                    entry_scale_percent,
                ),
            )
        })
        .collect()
}

fn normalize_outputs(
    raw: &[(OutputId, Rect, bool)],
    surface: Rect,
) -> (Vec<(OutputId, Rect)>, i32, i32) {
    let origin_x = raw.iter().map(|(_, rect, _)| rect.x).min().unwrap_or(0);
    let origin_y = raw.iter().map(|(_, rect, _)| rect.y).min().unwrap_or(0);
    let outputs = raw
        .iter()
        .filter_map(|(id, rect, _)| {
            Rect::new(
                rect.x.saturating_sub(origin_x),
                rect.y.saturating_sub(origin_y),
                rect.width,
                rect.height,
            )
            .intersection(surface)
            .map(|rect| (id.clone(), rect))
        })
        .collect();
    (outputs, origin_x, origin_y)
}

fn refresh_output_layout(
    surface: Rect,
    entry_scale_percent: u16,
) -> (
    Vec<(OutputId, Rect)>,
    BTreeMap<OutputId, GridConfig>,
    OutputId,
) {
    resolve_output_layout(surface, entry_scale_percent)
}

fn grid_for_output(state: &DesktopState, output: &OutputId) -> GridConfig {
    state
        .output_grids
        .get(output)
        .copied()
        .unwrap_or(state.grid)
}

fn output_at(state: &DesktopState, point: Point) -> OutputId {
    output_at_in(&state.output_geometries, &state.default_output, point)
}

fn output_at_in(outputs: &[(OutputId, Rect)], fallback: &OutputId, point: Point) -> OutputId {
    outputs
        .iter()
        .find(|(_, rect)| rect.contains(point))
        .or_else(|| {
            outputs.iter().min_by_key(|(_, rect)| {
                let x = point.x.clamp(rect.x, rect.right().saturating_sub(1));
                let y = point.y.clamp(rect.y, rect.bottom().saturating_sub(1));
                (i64::from(point.x) - i64::from(x)).pow(2)
                    + (i64::from(point.y) - i64::from(y)).pow(2)
            })
        })
        .map_or_else(|| fallback.clone(), |(id, _)| id.clone())
}

fn pointer_on_output(state: &DesktopState, x: f32, y: f32) -> (OutputId, Point) {
    let raw = Point::new(x.round() as i32, y.round() as i32);
    let output = output_at(state, raw);
    let point = pointer_point_for_grid(raw, grid_for_output(state, &output));
    (output, point)
}

fn pointer_point_for_grid(point: Point, grid: GridConfig) -> Point {
    let bounds = grid.work_area;
    Point::new(
        point.x.clamp(bounds.x, bounds.right().saturating_sub(1)),
        point.y.clamp(bounds.y, bounds.bottom().saturating_sub(1)),
    )
}

/// Projected `(output, cell)` for every model item, aligned by index with
/// `state.model.items()` (so `cells[i]` belongs to the i-th item).
fn projected_cells(state: &DesktopState) -> Vec<(OutputId, Cell)> {
    let items: Vec<_> = state.model.items().collect();
    let mut result: Vec<Option<(OutputId, Cell)>> = vec![None; items.len()];
    let mut occupied = BTreeMap::<OutputId, BTreeSet<Cell>>::new();
    for (slot, item) in items
        .iter()
        .enumerate()
        .filter(|(_, item)| state.output_grids.contains_key(&item.output))
    {
        let grid = grid_for_output(state, &item.output);
        let cell = grid.clamp_cell(item.cell);
        occupied
            .entry(item.output.clone())
            .or_default()
            .insert(cell);
        result[slot] = Some((item.output.clone(), cell));
    }
    let fallback = state.default_output.clone();
    let grid = grid_for_output(state, &fallback);
    for (slot, _) in items
        .iter()
        .enumerate()
        .filter(|(_, item)| !state.output_grids.contains_key(&item.output))
    {
        let cells = occupied.entry(fallback.clone()).or_default();
        let cell = flamewm_desktop_core::model::first_free(cells, grid);
        cells.insert(cell);
        result[slot] = Some((fallback.clone(), cell));
    }
    result
        .into_iter()
        .map(|entry| entry.expect("every model item is projected"))
        .collect()
}

fn panel_geometries() -> Result<Vec<(OutputId, Rect)>, String> {
    let client = ControlClient::connect(BusKind::Session).map_err(|error| format!("{error:?}"))?;
    match client.call(&ControlRequest::GetPanels) {
        Ok(ControlResponse::Panels(snapshot)) => Ok(snapshot
            .panels
            .into_iter()
            .filter(|panel| panel.visible)
            .map(|panel| (panel.output, panel.geometry))
            .collect()),
        Ok(response) => Err(format!("GetPanels returned {response:?}")),
        Err(error) => Err(error.message.clone()),
    }
}

fn subtract_panel_strip(mut area: Rect, panel: Rect) -> Rect {
    let Some(intersection) = area.intersection(panel) else {
        return area;
    };
    if intersection.width >= area.width.saturating_sub(16) {
        if intersection.y <= area.y + area.height / 2 {
            let bottom = intersection.y + intersection.height;
            let shift = bottom - area.y;
            if shift > 0 && shift < area.height {
                area.y = bottom;
                area.height -= shift;
            }
        } else {
            let height = intersection.y - area.y;
            if height > 0 {
                area.height = height;
            }
        }
        return area;
    }
    if intersection.height >= area.height.saturating_sub(16) {
        if intersection.x <= area.x + area.width / 2 {
            let right = intersection.x + intersection.width;
            let shift = right - area.x;
            if shift > 0 && shift < area.width {
                area.x = right;
                area.width -= shift;
            }
        } else {
            let width = intersection.x - area.x;
            if width > 0 {
                area.width = width;
            }
        }
    }
    area
}

fn state_path(name: &str) -> Result<PathBuf, String> {
    let base = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").unwrap()).join(".local/state"));
    Ok(base.join(name))
}

fn refresh_if_changed(
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
fn clear_workspace_menu_state(
    menu: &mut Option<MenuState>,
    pending_delete: &mut Option<Vec<PathBuf>>,
) {
    *menu = None;
    *pending_delete = None;
}

fn dismiss_workspace_menus(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    clear_workspace_menu_state(&mut state.menu, &mut state.pending_delete);
    hide_menus(document, &mut state.menu_rects)?;
    hide_confirm(document, &mut state.menu_rects)
}

fn refresh_workspace_projection(
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
fn refresh_launchers(state: &mut DesktopState) {
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

fn desktop_item_label<'a>(
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

fn launcher_icon(state: &DesktopState, kind: DesktopItemKind, path: &Path) -> Option<String> {
    state
        .launchers
        .get(path)
        .filter(|meta| meta.valid && !meta.icon.trim().is_empty())
        .map(|meta| meta.icon.clone())
        .or_else(|| {
            (kind == DesktopItemKind::File).then(|| projection::file_icon_name(path).to_owned())
        })
}

fn sync_document(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let _span = sync_point().start();
    render_sync_counter().increment();
    state.icons.refresh_theme();
    apply_ready_icons(document, state)?;
    let items: Vec<_> = state.model.items().take(SLOT_COUNT).collect();
    let cells = projected_cells(state);
    for index in 0..SLOT_COUNT {
        let item = items.get(index).copied();
        if let Some(item) = item {
            let (output, cell) = &cells[index];
            let output_grid = grid_for_output(state, output);
            let point = output_grid.cell_to_pixel(*cell);
            let kind = item.kind;
            let icon = launcher_icon(state, kind, &item.path);
            let label = desktop_item_label(state, item).to_owned();
            {
                let _icon = icon_point().start();
                let key = projection::project_item(
                    document,
                    &mut state.icons,
                    index,
                    kind,
                    &label,
                    point,
                    icon.as_deref(),
                    output_grid.visual_scale_percent(),
                )?;
                state.icon_slots[index] = Some(key);
            }
        } else {
            if index < state.icon_slots.len() {
                state.icon_slots[index] = None;
            }
            projection::clear_item(document, index)?;
        }
    }
    sync_selection_visuals(document, state)?;
    sync_menu(document, state)?;
    sync_rename_editor(document, state)?;
    sync_drag_ghost(document, state)?;
    sticky::sync_sticky(document, state)?;
    update_gauges(state);
    Ok(())
}

/// Wake-driven icon completion: the IconService wake FD sets the pending
/// flag (no inotify/click dependency); this drains the service and applies
/// each result only to slots whose tracked [`IconKey`] still matches, then
/// redraws each affected tile surface exactly once via the document marks.
fn apply_pending_icon_results(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if !state.icons_pending.swap(false, Ordering::Acquire) {
        return Ok(());
    }
    apply_ready_icons(document, state)
}
/// A completed icon result applies only to the slot still expecting its exact key
/// (including theme generation), so results for a replaced theme are dropped.
fn slot_expects_key(slot: Option<&IconKey>, key: &IconKey) -> bool {
    slot.is_some_and(|expected| expected == key)
}

fn apply_ready_icons(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let ready = state.icons.drain_ready();
    if ready.is_empty() {
        return Ok(());
    }
    for (key, raster) in ready {
        let Some(raster) = raster else { continue };
        for index in 0..SLOT_COUNT {
            let slot = state.icon_slots.get(index).and_then(|slot| slot.as_ref());
            if !slot_expects_key(slot, &key) {
                continue;
            }
            let Some(item) = state.model.items().nth(index) else {
                continue;
            };
            let kind = item.kind;
            let Some(expected) = state.icon_slots[index].clone() else {
                continue;
            };
            state
                .icons
                .apply_if_current(document, index, kind, &expected, &key, raster.clone())?;
        }
    }
    // A live prediction shares the same async icon cache as real desktop tiles.
    if let Some(preview) = state.conversion_preview.as_ref() {
        let key = projection::ParallelIconService::icon_key(
            DesktopItemKind::File,
            Some(projection::file_icon_name(&preview.destination)),
        );
        if let Some(raster) = state.icons.cached(&key) {
            document.image_rgba8(
                "desktop-convert-preview-icon",
                flamewm_ui_x11::RuntimeImage {
                    source: raster.source,
                    width: raster.width,
                    height: raster.height,
                    pixels: raster.pixels,
                },
            )?;
        }
    }
    Ok(())
}

/// Desktop keyboard actions are delivered as typed key events, not `keyboard.input`
/// action strings. Suppress Delete in rename/sticky editors and modifier chords.
fn handle_desktop_key(
    key: &UiKeyEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if key.phase != UiKeyPhase::Press
        || key.key != "Delete"
        || !key.modifiers.is_empty()
        || state.rename.is_some()
        || sticky::is_editing(state)
    {
        return Ok(());
    }
    let paths = state
        .model
        .items()
        .filter(|item| {
            state.selection.selected().contains(&item.id)
                && item.kind != DesktopItemKind::TrashPseudo
        })
        .map(|item| item.path.clone())
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return Ok(());
    }
    state.pending_delete = Some(paths);
    state.menu = None;
    hide_menus(document, &mut state.menu_rects)?;
    show_confirm(document, state)
}

fn wheel_bounds(state: &DesktopState) -> Option<Rect> {
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

fn wheel_hit_at(state: &DesktopState, x: f32, y: f32) -> Option<usize> {
    let bounds = wheel_bounds(state)?;
    let index = convert_wheel::sector_at(x - bounds.x as f32, y - bounds.y as f32)?;
    let path = state
        .menu
        .as_ref()?
        .selected_id
        .as_ref()
        .and_then(|id| state.model.items().find(|item| &item.id == id))
        .map(|item| &item.path)?;
    (index < offered_formats(path).len()).then_some(index)
}

fn update_wheel_hover(
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

fn handle_event(
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
        if matches!(action.phase, UiActionPhase::Hover | UiActionPhase::Motion) {
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
            return Ok(());
        }
    }
    if action.action == "keyboard.input" {
        if action.text.as_deref() == Some("") {
            return cancel_transient(document, state);
        }
        if state.rename.is_some() {
            return handle_rename_input(action, document, state);
        }
        if sticky::is_editing(state) {
            return sticky::handle_text_input(action, document, state);
        }
        if action.text.as_deref() == Some("\u{7f}") && state.pending_delete.is_none() {
            let paths = state
                .model
                .items()
                .filter(|item| {
                    state.selection.selected().contains(&item.id)
                        && item.kind != DesktopItemKind::TrashPseudo
                })
                .map(|item| item.path.clone())
                .collect::<Vec<_>>();
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

fn is_menu_action(action: &str) -> bool {
    matches!(
        action,
        "menu.terminal"
            | "menu.folder"
            | "menu.note"
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

fn cancel_transient(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    state.selection_start = None;
    state.selection_output = None;
    state.selection.clear_rubber();
    state.item_drag = None;
    state.last_click = None;
    state.menu = None;
    state.pending_delete = None;
    state.rename = None;
    sticky::cancel_edit_and_gesture(state);
    hide_drag_ghost(document, state)?;
    document.visible("desktop-selection", false)?;
    hide_menus(document, &mut state.menu_rects)?;
    hide_confirm(document, &mut state.menu_rects)?;
    sync_selection_visuals(document, state)?;
    sync_rename_editor(document, state)?;
    sticky::sync_sticky(document, state)
}

fn handle_surface(
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
        state.selection_start = None;
        state.selection_output = None;
        state.selection.clear_rubber();
        document.visible("desktop-selection", false)?;
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
            document.visible("desktop-selection", false)?;
            sync_selection_visuals(document, state)?;
        }
        UiActionPhase::Motion => {
            if let Some(start) = state.selection_start {
                let bounds = state
                    .selection_output
                    .as_ref()
                    .map_or(state.grid.work_area, |output| {
                        grid_for_output(state, output).work_area
                    });
                let point = pointer_point(action, bounds);
                let rect = normalized_selection_rect(start, point, bounds);
                state.selection.set_rubber(rect);
                update_selection_from_rubber(state);
                if rubber_visible(
                    point.x as f32 - start.x as f32,
                    point.y as f32 - start.y as f32,
                ) {
                    selection_visible_counter().increment();
                    set_selection_overlay(document, rect)?;
                } else {
                    document.visible("desktop-selection", false)?;
                }
                sync_selection_visuals(document, state)?;
            }
        }
        UiActionPhase::Release => {
            if let Some(start) = state.selection_start {
                let bounds = state
                    .selection_output
                    .as_ref()
                    .map_or(state.grid.work_area, |output| {
                        grid_for_output(state, output).work_area
                    });
                let point = pointer_point(action, bounds);
                let rect = normalized_selection_rect(start, point, bounds);
                state.selection.set_rubber(rect);
                update_selection_from_rubber(state);
            }
            state.selection_start = None;
            state.selection_output = None;
            state.selection.clear_rubber();
            selection_end_counter().increment();
            document.visible("desktop-selection", false)?;
            sync_selection_visuals(document, state)?;
        }
        UiActionPhase::Hover => {}
    }
    Ok(())
}

fn handle_item(
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
        let output = if state.output_grids.contains_key(&item.output) {
            item.output.clone()
        } else {
            state.default_output.clone()
        };
        state.selection.select_exclusive(item_id.clone());
        sticky::commit_edit(state);
        state.item_drag = None;
        state.selection_start = None;
        state.selection.clear_rubber();
        document.visible("desktop-selection", false)?;
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
            let item_id = item.id.clone();
            let item_label = desktop_item_label(state, item).to_owned();
            let item_kind = item.kind;
            let item_path = item.path.clone();
            let anchor_snapshot = state.item_drag.clone();
            let Some(drag) = anchor_snapshot else {
                return Ok(());
            };
            if drag.anchor_index != index || drag.anchor_id != item_id {
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
                    kind: item_kind,
                    icon_override: launcher_icon(state, item_kind, &item_path),
                    label: item_label,
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
fn double_click_pressed(state: &DesktopState, id: &str, x: f32, y: f32) -> bool {
    let Some(previous) = state.last_click.as_ref() else {
        return false;
    };
    let delta_ms = previous.at.elapsed().as_millis().min(u64::MAX as u128) as u64;
    double_click_opens(&previous.id, previous.x, previous.y, 0, id, x, y, delta_ms)
}

fn open_item(state: &DesktopState, index: usize) -> Result<(), String> {
    let Some(item) = state.model.items().nth(index) else {
        return Ok(());
    };
    let outcome = open_item_inner(state, item.kind.clone(), &item.path);
    if let Err(error) = &outcome {
        // Double-click/open boundary: spawn intent failure is logged and
        // counted; the event loop stays alive. Child exit after a successful
        // spawn is not a Flame error and never reaches this path.
        report_action_failure("open item", error.clone());
        spawn_failure_counter().increment();
    }
    Ok(())
}

fn open_item_inner(state: &DesktopState, kind: DesktopItemKind, path: &Path) -> Result<(), String> {
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
                .map(|_| ())
                .map_err(|error| format!("{error:?}"))
        }
        LauncherOpen::RegularFile => flamewm_applications::open_target(path.as_os_str())
            .map_err(|error| format!("{error:?}")),
    }
}

/// General settings: the settings app's default page, with no `--page` deep link.
fn general_settings() -> CommandIntent {
    CommandIntent {
        program: "flamewm-settings".to_owned(),
        argv: Vec::new(),
        cwd: None,
    }
}

fn spawn_intent(intent: &CommandIntent) -> Result<(), String> {
    flamewm_applications::launch_command(&intent.program, &intent.argv, intent.cwd.as_deref())
        .map_err(|error| format!("spawn {}: {error:?}", intent.program))
}

fn handle_menu_action(
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
                                report_action_failure("start conversion", error.to_string())
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

fn open_rename(
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

fn sync_rename_editor(
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
        .map(|(slot, _)| {
            let (output, cell) = &cells[slot];
            grid_for_output(state, output).cell_to_pixel(*cell)
        })
        .unwrap_or(state.grid.work_area.origin());
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

fn handle_rename_input(
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

fn confirm_rename(
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

fn control_client() -> Result<ControlClient, String> {
    ControlClient::connect(BusKind::Session).map_err(|error| format!("{error:?}"))
}

fn insert_workspace_after(
    mutation_dispatcher: Option<&Arc<MutationDispatcher>>,
    state: &DesktopState,
) -> Result<(), String> {
    let dispatcher = mutation_dispatcher
        .ok_or_else(|| "workspace mutation dispatcher unavailable".to_owned())?;
    let snapshot = state.workspace.snapshot();
    dispatcher
        .submit(ControlRequest::InsertWorkspaceAfter {
            index: snapshot.count.saturating_sub(1),
            expected_revision: snapshot.revision,
        })
        .map(|_| ())
        .map_err(|error| error.message)
}

/// Collision-safe shortcut: `.desktop` sources are copied with a ` link`
/// suffix, other sources become symlinks. Pure path math plus direct fs
/// calls; no shell strings.
fn create_shortcut(state: &mut DesktopState) -> Result<(), String> {
    let selected = selected_menu_id(state);
    let Some(id) = selected else {
        return Ok(());
    };
    let source = state
        .model
        .items()
        .find(|item| item.id == id)
        .map(|item| item.path.clone());
    let Some(source) = source else {
        return Ok(());
    };
    let file_name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| "shortcut source has no file name".to_owned())?;
    let destination = shortcut_destination(&state.directory, &file_name);
    if source.extension().and_then(|value| value.to_str()) == Some("desktop") {
        fs::copy(&source, &destination)
            .map(|_| ())
            .map_err(|error| format!("copy {}: {error}", destination.display()))?;
    } else {
        std::os::unix::fs::symlink(&source, &destination)
            .map_err(|error| format!("symlink {}: {error}", destination.display()))?;
    }
    rescan_desktop(state)?;
    refresh_launchers(state);
    state.selection.clear();
    state
        .selection
        .select(destination.to_string_lossy().into_owned());
    Ok(())
}

/// Freedesktop Trash replaces irreversible deletion. The user's home is
/// authoritative for XDG_DATA_HOME, and the UID comes from its metadata.
fn move_entry_to_trash(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?);
    let uid = fs::metadata(&home)
        .map_err(|error| format!("stat HOME: {error}"))?
        .uid();
    let data_home = env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    Trash::move_to_trash(path, &home, &data_home, uid)
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
}

fn empty_home_trash() -> Result<(), String> {
    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?);
    let data_home = env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    let location = TrashLocation {
        scope: TrashScope::Home,
        top_directory: None,
        base: data_home.join("Trash"),
    };
    Trash::empty(&location)
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))?;
    Ok(())
}

fn blank_row_id(action: BlankDesktopAction) -> &'static str {
    match action {
        BlankDesktopAction::OpenTerminal => "menu-row-terminal",
        BlankDesktopAction::CreateNewFolder => "menu-row-folder",
        BlankDesktopAction::NewStickyNote => "menu-row-note",
        BlankDesktopAction::AddVirtualDesktop => "menu-row-desktop",
        BlankDesktopAction::DesktopAndWallpaper => "menu-row-settings",
    }
}

fn entry_row_id(action: EntryMenuAction) -> &'static str {
    match action {
        EntryMenuAction::EmptyTrash => "menu-row-empty-trash",
        EntryMenuAction::Rename => "menu-row-rename",
        EntryMenuAction::CreateShortcut => "menu-row-shortcut",
        EntryMenuAction::Convert => "menu-row-convert",
        EntryMenuAction::Delete => "menu-row-delete",
    }
}

fn hide_menus(
    document: &mut impl UiDocumentAccess,
    menu_rects: &mut Vec<Rect>,
) -> Result<(), String> {
    menu_rects.clear();
    document.visible("desktop-menu", false)?;
    document.visible("desktop-entry-menu", false)?;
    document.visible("desktop-convert-wheel", false)?;
    document.visible("convert-wheel-highlight", false)?;
    document.visible("sticky-menu", false)?;
    projection::clear_menu_backdrop(document, "desktop-menu")?;
    projection::clear_menu_backdrop(document, "desktop-entry-menu")?;
    projection::clear_menu_backdrop(document, "desktop-convert-wheel")?;
    projection::clear_menu_backdrop(document, "sticky-menu")
}

/// Whether any menu or the delete confirm is shown. Gates the root press observer.
fn menus_open(menu_rects: &[Rect]) -> bool {
    !menu_rects.is_empty()
}

/// Whether a root press at logical `(x, y)` lands outside every rect.
fn press_outside_menus(rects: &[Rect], x: f32, y: f32) -> bool {
    let (x, y) = (x.round() as i32, y.round() as i32);
    !rects
        .iter()
        .any(|rect| x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom())
}

/// Closes the open menus and delete confirm when a root press lands outside them.
/// Presses inside a menu belong to its own action path; nothing open means no-op.
fn close_menus_on_outside_press(
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

fn hide_confirm(
    document: &mut impl UiDocumentAccess,
    menu_rects: &mut Vec<Rect>,
) -> Result<(), String> {
    menu_rects.clear();
    document.visible("desktop-confirm", false)?;
    projection::clear_menu_backdrop(document, "desktop-confirm")
}

fn show_confirm(
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

fn sync_menu(document: &mut impl UiDocumentAccess, state: &mut DesktopState) -> Result<(), String> {
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
            let formats = path.map_or_else(Vec::new, |path| offered_formats(path));
            for slot in 0..8 {
                let id = format!("convert-slot-{slot}");
                let choice = formats.get(slot).copied();
                document.visible(&id, choice.is_some())?;
                if let Some(choice) = choice {
                    document.text(&id, choice.to_ascii_uppercase())?;
                }
            }
            if let Some(path) = path {
                let size = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
                let label = if size >= 1_000_000 {
                    format!("{:.1} MB", size as f64 / 1_000_000.0)
                } else {
                    format!("{} KB", size / 1000)
                };
                document.text("convert-wheel-hub-text", label)?;
            }
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

fn place_menu_parts(
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
fn preview_cell_for(source: Cell, grid: GridConfig, occupied: &BTreeSet<Cell>) -> Cell {
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

fn sync_conversion_preview(
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

fn show_conversion_preview(
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

fn show_drag_ghost(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    x: f32,
    y: f32,
) -> Result<(), String> {
    let Some(output) = state.drag_ghost.as_ref().map(|ghost| ghost.output.clone()) else {
        return Ok(());
    };
    let scale = grid_for_output(state, &output).visual_scale_percent();
    let ghost = state.drag_ghost.as_mut().expect("observed ghost");
    let factor = f32::from(scale) / 100.0;
    let kind = ghost.kind;
    let icon_override = ghost.icon_override.clone();
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
        icon_override.as_deref(),
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
    for (index, item) in state.model.items().take(SLOT_COUNT).enumerate() {
        if ghost.source_ids.contains(&item.id) {
            document.visible(&format!("desktop-item-{index}"), false)?;
        }
    }
    Ok(())
}

fn sync_drag_ghost(
    document: &mut impl UiDocumentAccess,
    state: &DesktopState,
) -> Result<(), String> {
    let ghost = state.drag_ghost.as_ref().filter(|ghost| ghost.visible);
    document.visible("desktop-drag-ghost", ghost.is_some())?;
    if let Some(ghost) = ghost {
        set_drag_sources_visible(document, state, &ghost.source_ids, false)?;
    }
    Ok(())
}

/// Hides or restores the tiles that are being dragged, keyed by item id.
fn set_drag_sources_visible(
    document: &mut impl UiDocumentAccess,
    state: &DesktopState,
    source_ids: &[String],
    visible: bool,
) -> Result<(), String> {
    for (index, item) in state.model.items().take(SLOT_COUNT).enumerate() {
        if source_ids.contains(&item.id) {
            document.visible(&format!("desktop-item-{index}"), visible)?;
        }
    }
    Ok(())
}

fn hide_drag_ghost(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if let Some(ghost) = state.drag_ghost.take() {
        set_drag_sources_visible(document, state, &ghost.source_ids, true)?;
    }
    // Best effort: count node may not exist in the compiled
    // document; the ghost id itself is authoritative for visibility.
    let _ = document.visible("desktop-drag-ghost-count", false);
    document.visible("desktop-drag-ghost", false)
}

fn commit_item_drag(
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
    let anchor_output = if state.output_grids.contains_key(&anchor.output) {
        anchor.output.clone()
    } else {
        state.default_output.clone()
    };
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
        .map(|item| {
            if state.output_grids.contains_key(&item.output) {
                item.output.clone()
            } else {
                state.default_output.clone()
            }
        })
        .collect::<BTreeSet<_>>();
    for output in drag_outputs {
        let selected = drag
            .originals
            .iter()
            .filter(|item| {
                let item_output = if state.output_grids.contains_key(&item.output) {
                    &item.output
                } else {
                    &state.default_output
                };
                item_output == &output
            })
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
            .filter(|(_, item)| {
                let item_output = if state.output_grids.contains_key(&item.output) {
                    &item.output
                } else {
                    &state.default_output
                };
                item_output == &output
            })
            .map(|(slot, _)| projected[slot].1)
            .collect::<BTreeSet<_>>();
        match group_drag_transaction(&selected, &occupied, delta, grid_for_output(state, &output)) {
            Ok(output_updates) => updates.extend(output_updates),
            Err(_) => {
                sync_document(document, state)?;
                return Ok(());
            }
        }
    }

    for original in &drag.originals {
        if let Some(cell) = updates.get(&original.id).copied() {
            state.model.set_position(
                original.path.clone(),
                StoredPosition {
                    output: if state.output_grids.contains_key(&original.output) {
                        original.output.clone()
                    } else {
                        state.default_output.clone()
                    },
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

fn update_selection_from_rubber(state: &mut DesktopState) {
    let Some(_) = state.selection.rubber() else {
        return;
    };
    let cells = projected_cells(state);
    let item_rects = state.model.items().zip(&cells).map(|(item, (output, cell))| {
        let grid = grid_for_output(state, output);
        let point = grid.cell_to_pixel(*cell);
        (
            item.id.as_str(),
            Rect::new(point.x, point.y, grid.cell_width, grid.cell_height),
        )
    });
    let selected = state.selection.hit_test(item_rects);
    state.selection.clear();
    for id in selected {
        state.selection.select(id);
    }
}

fn sync_selection_visuals(
    document: &mut impl UiDocumentAccess,
    state: &DesktopState,
) -> Result<(), String> {
    let fill_alpha = percent_alpha(setting_percent(
        &state.settings,
        "DesktopSelectionFillOpacity",
        20,
    ));
    let accent = accent_color(&state.settings);
    for (index, item) in state.model.items().enumerate() {
        let id = format!("desktop-item-{index}");
        if state.selection.selected().contains(&item.id) {
            document.background(
                &id,
                UiColor {
                    a: fill_alpha,
                    ..accent
                },
            )?;
            document.border(&id, UiColor { a: 0xdd, ..accent })?;
        } else {
            document.background_clear(&id)?;
            document.border_clear(&id)?;
        }
    }
    Ok(())
}

fn set_selection_overlay(document: &mut impl UiDocumentAccess, rect: Rect) -> Result<(), String> {
    document.visible("desktop-selection", true)?;
    document.position("desktop-selection", rect.x as f32, rect.y as f32)?;
    document.size("desktop-selection", rect.width as f32, rect.height as f32)
}

/// Raw X button translated once at the event boundary (ui-x11 helper).
fn pointer_button(action: &UiActionEvent) -> PointerButton {
    pointer_button_from_raw(action.button)
}

fn pointer_point(action: &UiActionEvent, bounds: Rect) -> Point {
    let max_x = bounds.right().saturating_sub(1);
    let max_y = bounds.bottom().saturating_sub(1);
    Point::new(
        (action.x.round() as i32).clamp(bounds.x, max_x),
        (action.y.round() as i32).clamp(bounds.y, max_y),
    )
}

fn normalized_selection_rect(start: Point, end: Point, bounds: Rect) -> Rect {
    let start_x = start.x.clamp(bounds.x, bounds.right().saturating_sub(1));
    let start_y = start.y.clamp(bounds.y, bounds.bottom().saturating_sub(1));
    let end_x = end.x.clamp(bounds.x, bounds.right().saturating_sub(1));
    let end_y = end.y.clamp(bounds.y, bounds.bottom().saturating_sub(1));
    Rect::new(
        start_x.min(end_x),
        start_y.min(end_y),
        (start_x - end_x).unsigned_abs().max(1) as i32,
        (start_y - end_y).unsigned_abs().max(1) as i32,
    )
}

fn packaged_root() -> PathBuf {
    let manifest_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .expect("flamewm workspace root");
    resolve_packaged_root(env::current_exe().ok().as_deref(), &manifest_root, |root| {
        root.join("assets").is_dir()
    })
}

fn resolve_packaged_root(
    executable: Option<&Path>,
    manifest_root: &Path,
    assets_available: impl FnOnce(&Path) -> bool,
) -> PathBuf {
    executable
        .and_then(installed_data_root)
        .filter(|root| assets_available(root))
        .unwrap_or_else(|| manifest_root.to_path_buf())
}

fn installed_data_root(executable: &Path) -> Option<PathBuf> {
    let bin_dir = executable.parent()?;
    let usr_dir = bin_dir.parent()?;
    (bin_dir.file_name()? == "bin" && usr_dir.file_name()? == "usr")
        .then(|| usr_dir.join("share/flamewm"))
}

#[cfg(test)]
mod work_area_tests {
    use super::{
        desktop_surface_rect, grids_for_outputs, normalize_outputs, output_at_in,
        pointer_point_for_grid, subtract_panel_strip,
    };
    use flamewm_api::{OutputId, Point, Rect};
    use flamewm_desktop_core::layout::GridConfig;
    use std::collections::BTreeMap;

    #[test]
    fn desktop_surface_covers_full_root_under_bottom_strut() {
        let work_area = Rect::new(0, 0, 1280, 756);
        let surface = desktop_surface_rect(1280, 800);

        assert_eq!(surface, Rect::new(0, 0, 1280, 800));
        assert_eq!((surface.width as u32, surface.height as u32), (1280, 800));
        assert!(surface.width >= work_area.width && surface.height > work_area.height);
    }

    #[test]
    fn panel_edges_keep_icons_in_work_area_with_surface_origin_at_root() {
        // The surface sits at root origin, so window-local == screen. Cells must map inside the
        // work area for top, left, bottom and right struts, and hit-testing must invert them.
        let surface = desktop_surface_rect(1280, 800);
        assert_eq!(surface.origin(), flamewm_api::Point::new(0, 0));
        for work_area in [
            Rect::new(0, 44, 1280, 756),
            Rect::new(44, 0, 1236, 800),
            Rect::new(0, 0, 1280, 756),
            Rect::new(0, 0, 1236, 800),
        ] {
            let grid = GridConfig::compute(work_area, 100);
            for column in 0..grid.columns {
                for row in 0..grid.rows {
                    let cell = flamewm_desktop_core::layout::Cell::new(column, row);
                    let point = grid.cell_to_pixel(cell);
                    assert!(work_area.contains(point), "{point:?} outside {work_area:?}");
                    assert!(surface.contains(point));
                    assert_eq!(grid.pixel_to_cell(point), cell);
                }
            }
        }
    }

    #[test]
    fn icon_grid_stays_inside_work_area_while_surface_is_root() {
        let work_area = Rect::new(0, 0, 1280, 756);
        let grid = GridConfig::compute(work_area, 100);
        let last = grid.clamp_cell(flamewm_desktop_core::layout::Cell::new(i32::MAX, i32::MAX));
        let point = grid.cell_to_pixel(last);

        assert!(work_area.contains(point));
        assert_eq!(grid.work_area, work_area);
    }

    #[test]
    fn bottom_panel_strip_leaves_root_minus_panel_height_at_origin() {
        let root = Rect::new(0, 0, 1350, 641);
        let panel = Rect::new(0, 641 - 55, 1350, 55);

        assert_eq!(
            subtract_panel_strip(root, panel),
            Rect::new(0, 0, 1350, 586)
        );
    }

    #[test]
    fn each_output_uses_its_own_taskbar_work_area() {
        let left = OutputId::new("left");
        let right = OutputId::new("right");
        let outputs = [
            (left.clone(), Rect::new(0, 0, 1920, 1080)),
            (right.clone(), Rect::new(1920, 200, 2560, 1440)),
        ];
        let panels = [
            (left.clone(), Rect::new(4, 1036, 1912, 44)),
            (right.clone(), Rect::new(1924, 1596, 2552, 44)),
        ];
        let grids = grids_for_outputs(&outputs, &panels, &BTreeMap::new(), 100);

        assert_eq!(grids[&left].work_area, Rect::new(0, 0, 1920, 1036));
        assert_eq!(grids[&right].work_area, Rect::new(1920, 200, 2560, 1396));
        assert!(grids[&left].work_area.bottom() <= panels[0].1.y);
        assert!(grids[&right].work_area.bottom() <= panels[1].1.y);
    }

    #[test]
    fn distinct_output_scales_keep_independent_grid_and_panel_bounds() {
        let left = OutputId::new("left");
        let right = OutputId::new("right");
        let outputs = [
            (left.clone(), Rect::new(0, 0, 1920, 1080)),
            (right.clone(), Rect::new(1920, 0, 1920, 1080)),
        ];
        let panels = [
            (left.clone(), Rect::new(0, 1030, 1920, 50)),
            (right.clone(), Rect::new(1920, 1030, 1920, 50)),
        ];
        let scales = BTreeMap::from([(left.clone(), 105), (right.clone(), 195)]);
        let grids = grids_for_outputs(&outputs, &panels, &scales, 100);
        assert_eq!(grids[&left].scale_percent, 105);
        assert_eq!(grids[&right].scale_percent, 195);
        assert_ne!(grids[&left].cell_width, grids[&right].cell_width);
        assert_eq!(grids[&left].work_area.bottom(), 1030);
        assert_eq!(grids[&right].work_area.bottom(), 1030);
        for grid in grids.values() {
            let origin = grid.cell_to_pixel(flamewm_desktop_core::layout::Cell::new(0, 0));
            assert!(grid.work_area.contains(origin));
        }
    }

    #[test]
    fn entry_scale_preference_grows_cells_without_changing_output_scale() {
        let output = OutputId::new("primary");
        let outputs = [(output.clone(), Rect::new(0, 0, 1920, 1080))];
        let grids = grids_for_outputs(&outputs, &[], &BTreeMap::new(), 150);
        let grid = grids[&output];
        assert_eq!(grid.scale_percent, 100);
        assert_eq!(grid.entry_scale_percent, 150);
        assert_eq!(grid.visual_scale_percent(), 150);
        assert_eq!(grid.cell_width, 144);
    }

    #[test]
    fn negative_monitor_origin_is_normalized_and_pointer_selects_its_output() {
        let left = OutputId::new("left");
        let right = OutputId::new("right");
        let raw = [
            (left.clone(), Rect::new(-1920, 0, 1920, 1080), false),
            (right.clone(), Rect::new(0, 200, 2560, 1440), true),
        ];
        let root = Rect::new(0, 0, 4480, 1640);
        let (outputs, origin_x, origin_y) = normalize_outputs(&raw, root);

        assert_eq!((origin_x, origin_y), (-1920, 0));
        assert_eq!(outputs[0].1, Rect::new(0, 0, 1920, 1080));
        assert_eq!(outputs[1].1, Rect::new(1920, 200, 2560, 1440));
        assert_eq!(output_at_in(&outputs, &left, Point::new(2500, 300)), right);
        assert_eq!(output_at_in(&outputs, &left, Point::new(1919, 100)), left);
        assert_eq!(output_at_in(&outputs, &left, Point::new(4600, 100)), right);
        let work = Rect::new(1920, 200, 2560, 1396);
        assert_eq!(
            pointer_point_for_grid(Point::new(2500, 1640), GridConfig::compute(work, 100)),
            Point::new(2500, 1595)
        );
    }

    #[test]
    fn panel_height_change_updates_output_grid() {
        let root = Rect::new(0, 0, 1350, 641);
        let before = subtract_panel_strip(root, Rect::new(0, 641 - 44, 1350, 44));
        let after = subtract_panel_strip(root, Rect::new(0, 641 - 55, 1350, 55));

        assert_eq!(before, Rect::new(0, 0, 1350, 597));
        assert_eq!(after, Rect::new(0, 0, 1350, 586));
    }
}

#[cfg(test)]
mod packaged_root_tests {
    use super::{installed_data_root, resolve_packaged_root};
    use std::path::{Path, PathBuf};

    #[test]
    fn installed_binary_maps_to_sibling_share_data_root() {
        let executable = Path::new("/opt/flamewm/usr/bin/flamewm-desktop");

        assert_eq!(
            installed_data_root(executable),
            Some(PathBuf::from("/opt/flamewm/usr/share/flamewm"))
        );
        assert_eq!(
            resolve_packaged_root(Some(executable), Path::new("/workspace/flamewm"), |_| true),
            PathBuf::from("/opt/flamewm/usr/share/flamewm")
        );
    }

    #[test]
    fn unavailable_install_assets_fall_back_to_manifest_root() {
        let executable = Path::new("/opt/flamewm/usr/bin/flamewm-desktop");
        let manifest_root = Path::new("/workspace/flamewm");

        assert_eq!(
            resolve_packaged_root(Some(executable), manifest_root, |_| false),
            manifest_root.to_path_buf()
        );
    }

    #[test]
    fn development_executable_uses_manifest_root_without_cwd_lookup() {
        let executable = Path::new("/workspace/flamewm/target/debug/flamewm-desktop");
        let manifest_root = Path::new("/workspace/flamewm");

        assert_eq!(
            resolve_packaged_root(Some(executable), manifest_root, |_| true),
            manifest_root.to_path_buf()
        );
    }
}

#[cfg(test)]
mod general_settings_tests {
    use super::general_settings;

    #[test]
    fn general_settings_intent_has_no_page_deep_link() {
        let intent = general_settings();
        assert_eq!(intent.program, "flamewm-settings");
        assert!(intent.argv.is_empty());
        assert!(intent.cwd.is_none());
    }

    #[test]
    fn desktop_menu_settings_row_is_flamewm_settings_only() {
        let html = include_str!("../../../ui/desktop/index.html");
        assert_eq!(html.matches("id=\"menu-row-settings\"").count(), 1);
        assert!(html.contains("<span>FlameWM Settings</span>"));
        assert!(html.contains(
            "src=\"assets/breeze/preferences-desktop.svg\"><span>FlameWM Settings</span>"
        ));
        assert!(!html.contains("Desktop and Wallpaper"));
    }
}

#[cfg(test)]
mod icon_theme_tests {
    use super::*;

    fn snapshot_with_theme(theme: Option<&str>) -> SettingsSnapshot {
        let mut snapshot = SettingsSnapshot::new(1);
        if let Some(theme) = theme {
            snapshot
                .values
                .insert("iconTheme".to_owned(), SettingValue::Text(theme.to_owned()));
        }
        snapshot
    }

    #[test]
    fn stale_theme_generation_result_is_rejected_after_reprojection() {
        let current = projection::ParallelIconService::icon_key(DesktopItemKind::File, None);
        let mut stale = current.clone();
        stale.theme_generation = current.theme_generation.wrapping_add(1);
        assert!(slot_expects_key(Some(&current), &current));
        assert!(!slot_expects_key(Some(&current), &stale));
        assert!(!slot_expects_key(None, &current));
    }

    #[test]
    fn icon_theme_change_detects_new_and_removed_values() {
        let papirus = snapshot_with_theme(Some("Papirus"));
        assert!(icon_theme_changed("", &papirus));
        assert!(!icon_theme_changed("Papirus", &papirus));
        assert!(icon_theme_changed("PlagueSur", &papirus));
        assert!(icon_theme_changed("Papirus", &snapshot_with_theme(None)));
        assert!(!icon_theme_changed("", &snapshot_with_theme(None)));
    }
}

#[cfg(test)]
mod outside_press_tests {
    use super::*;
    use flamewm_ui_x11::test_support::RecordingUiDoc;

    fn press(x: f32, y: f32) -> RawPress {
        RawPress {
            x,
            y,
            button: 1,
            time_ms: 0,
        }
    }

    #[test]
    fn classification_is_inside_on_rect_and_outside_past_its_exclusive_edges() {
        let rects = [Rect::new(10, 10, 20, 20)];
        assert!(!press_outside_menus(&rects, 10.0, 10.0));
        assert!(!press_outside_menus(&rects, 29.4, 29.4));
        assert!(press_outside_menus(&rects, 30.0, 15.0));
        assert!(press_outside_menus(&rects, 15.0, 30.0));
        assert!(press_outside_menus(&rects, 9.0, 15.0));
    }

    #[test]
    fn press_inside_any_open_rect_is_not_outside() {
        let rects = [Rect::new(0, 0, 10, 10), Rect::new(50, 50, 10, 10)];
        assert!(!press_outside_menus(&rects, 55.0, 55.0));
        assert!(press_outside_menus(&rects, 30.0, 30.0));
    }

    #[test]
    fn arming_follows_open_menus_only() {
        assert!(!menus_open(&[]));
        assert!(menus_open(&[Rect::new(0, 0, 1, 1)]));
    }

    #[test]
    fn outside_press_closes_menus_and_clears_pending_delete() {
        let mut doc = RecordingUiDoc::default();
        let mut rects = vec![Rect::new(10, 10, 20, 20)];
        let mut pending = Some(vec![PathBuf::from("/tmp/flamewm-pending")]);
        let mut menu = Some(MenuState {
            kind: MenuKind::Entry,
            anchor: Point::new(10, 10),
            output: OutputId::new("default"),
            selected_id: Some("image.png".into()),
        });
        close_menus_on_outside_press(
            &press(100.0, 100.0),
            &mut doc,
            &mut rects,
            &mut menu,
            &mut pending,
        )
        .expect("close");
        assert!(rects.is_empty());
        assert!(
            menu.is_none(),
            "outside press must release the semantic menu, not just hide it"
        );
        assert!(pending.is_none());
        assert_eq!(doc.visible.get("desktop-menu"), Some(&false));
        assert_eq!(doc.visible.get("desktop-confirm"), Some(&false));
    }

    #[test]
    fn inside_press_leaves_menus_and_pending_delete_untouched() {
        let mut doc = RecordingUiDoc::default();
        let mut rects = vec![Rect::new(10, 10, 20, 20)];
        let mut pending = Some(vec![PathBuf::from("/tmp/flamewm-pending")]);
        let mut menu = Some(MenuState {
            kind: MenuKind::ConvertWheel,
            anchor: Point::new(10, 10),
            output: OutputId::new("default"),
            selected_id: Some("image.png".into()),
        });
        close_menus_on_outside_press(
            &press(12.0, 12.0),
            &mut doc,
            &mut rects,
            &mut menu,
            &mut pending,
        )
        .expect("inside");
        assert_eq!(rects, vec![Rect::new(10, 10, 20, 20)]);
        assert!(
            menu.is_some(),
            "press inside the wheel must preserve its action target"
        );
        assert!(pending.is_some());
        assert!(doc.visible.is_empty());
    }

    #[test]
    fn press_while_closed_is_a_noop() {
        let mut doc = RecordingUiDoc::default();
        let mut rects = Vec::new();
        let mut pending = Some(vec![PathBuf::from("/tmp/flamewm-pending")]);
        let mut menu = None;
        close_menus_on_outside_press(
            &press(5.0, 5.0),
            &mut doc,
            &mut rects,
            &mut menu,
            &mut pending,
        )
        .expect("closed");
        assert!(menu.is_none());
        assert!(pending.is_some());
        assert!(doc.visible.is_empty());
    }
}

#[cfg(test)]
mod rename_button_tests {
    use super::is_menu_action;

    #[test]
    fn rename_editor_buttons_are_dispatched_as_menu_actions() {
        assert!(is_menu_action("rename.confirm"));
        assert!(is_menu_action("rename.cancel"));
        assert!(is_menu_action("entry.rename"));
    }
}

#[cfg(test)]
mod workspace_menu_lifetime_tests {
    use super::*;

    #[test]
    fn workspace_transition_clears_opened_entry_menu_and_delete_confirmation() {
        for kind in [
            MenuKind::Blank,
            MenuKind::Entry,
            MenuKind::ConvertWheel,
            MenuKind::Sticky,
        ] {
            let mut menu = Some(MenuState {
                kind,
                anchor: Point::new(100, 200),
                output: OutputId::new("default"),
                selected_id: Some("/tmp/my-image.png".to_owned()),
            });
            let mut pending_delete = Some(vec![PathBuf::from("/tmp/my-image.png")]);
            clear_workspace_menu_state(&mut menu, &mut pending_delete);
            assert!(menu.is_none(), "a menu must not survive workspace changes");
            assert!(
                pending_delete.is_none(),
                "a modal delete intent must not survive either"
            );
        }
    }
}

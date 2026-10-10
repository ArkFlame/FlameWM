use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use flamewm_api::applications::ApplicationLaunchOptions;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_api::workspace::WorkspaceSnapshot;
use flamewm_api::{OutputId, Point, Rect};
use flamewm_applications::DesktopEntry;
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_control_dbus::{ControlClient, ControlSignalClient, MutationDispatcher};
use flamewm_dbus_reactor::BusKind;
use flamewm_desktop_core::file_actions::{
    CommandIntent, EntryMenuAction, LauncherOpen, create_new_folder, desktop_settings,
    entry_context_menu, launcher_open_kind, open_terminal, rename_entry_no_replace,
    shortcut_destination, terminal_program, validate_rename_name,
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
use flamewm_desktop_core::sticky::{
    ResizeCorner, StickyNote, StickyNoteStore, move_rect, resize_rect,
};
use flamewm_desktop_core::sticky_persistence;
use flamewm_desktop_core::trash::{Trash, TrashLocation, TrashScope};
use flamewm_integrations_linux::icons::IconKey;
use flamewm_profiler::{CounterPoint, MemoryGauge, ProfilePoint, report_window};
use flamewm_reactor::{FdAction, Reactor};
use flamewm_ui_core::context_menu::MenuPart;
use flamewm_ui_x11::PointerButton;
use flamewm_ui_x11::{
    UiActionEvent, UiActionPhase, UiColor, UiControllerEvent, UiDocumentAccess, UiWindowConfig,
    UiWindowRole, decode_document, pointer_button_from_raw, root_geometry,
    WindowGeometry, WindowGeometrySlot, run_with_controller_events_role_with_reactor_geometry,
};

mod projection;
mod wallpaper_loader;
mod workspace_sync;

use std::sync::OnceLock;

fn scan_point() -> &'static ProfilePoint {
    static POINT: OnceLock<ProfilePoint> = OnceLock::new();
    POINT.get_or_init(|| ProfilePoint::new("desktop.scan"))
}

fn sync_point() -> &'static ProfilePoint {
    static POINT: OnceLock<ProfilePoint> = OnceLock::new();
    POINT.get_or_init(|| ProfilePoint::new("desktop.sync_document"))
}

fn rescan_point() -> &'static ProfilePoint {
    static POINT: OnceLock<ProfilePoint> = OnceLock::new();
    POINT.get_or_init(|| ProfilePoint::new("desktop.rescan"))
}

fn icon_point() -> &'static ProfilePoint {
    static POINT: OnceLock<ProfilePoint> = OnceLock::new();
    POINT.get_or_init(|| ProfilePoint::new("desktop.icon.resolve"))
}

fn inotify_wakeup_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.inotify.wakeup"))
}

fn inotify_rescan_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.inotify.rescan"))
}

fn selection_begin_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.selection.begin"))
}

fn selection_visible_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.selection.visible"))
}

fn selection_end_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.selection.end"))
}

fn context_secondary_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.context.secondary"))
}

fn context_open_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.context.open"))
}

fn context_open_total_point() -> &'static ProfilePoint {
    static POINT: OnceLock<ProfilePoint> = OnceLock::new();
    POINT.get_or_init(|| ProfilePoint::new("desktop.context.open.total"))
}

fn context_open_project_point() -> &'static ProfilePoint {
    static POINT: OnceLock<ProfilePoint> = OnceLock::new();
    POINT.get_or_init(|| ProfilePoint::new("desktop.context.open.project"))
}

fn context_open_measure_point() -> &'static ProfilePoint {
    static POINT: OnceLock<ProfilePoint> = OnceLock::new();
    POINT.get_or_init(|| ProfilePoint::new("desktop.context.open.measure"))
}

fn context_open_place_point() -> &'static ProfilePoint {
    static POINT: OnceLock<ProfilePoint> = OnceLock::new();
    POINT.get_or_init(|| ProfilePoint::new("desktop.context.open.place"))
}

fn context_open_present_point() -> &'static ProfilePoint {
    static POINT: OnceLock<ProfilePoint> = OnceLock::new();
    POINT.get_or_init(|| ProfilePoint::new("desktop.context.open.present"))
}

fn context_open_close_point() -> &'static ProfilePoint {
    static POINT: OnceLock<ProfilePoint> = OnceLock::new();
    POINT.get_or_init(|| ProfilePoint::new("desktop.context.open.close"))
}

fn context_action_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.context.action"))
}

fn render_sync_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.render.sync"))
}

fn drag_ghost_show_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.drag.ghost.show"))
}

fn drag_ghost_motion_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.drag.ghost.motion"))
}

fn rename_open_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.rename.open"))
}

fn rename_success_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.rename.success"))
}

fn rename_failure_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.rename.failure"))
}

fn action_failure_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.action.failure"))
}

fn spawn_failure_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.spawn.failure"))
}

fn report_action_failure(context: &str, error: String) {
    action_failure_counter().increment();
    eprintln!("desktop: {context} failed: {error}; keeping event loop alive");
}

fn sticky_edit_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.sticky.edit"))
}

fn sticky_move_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.sticky.move"))
}

fn sticky_resize_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.sticky.resize"))
}

fn sticky_persist_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.sticky.persist"))
}

fn workspace_change_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.workspace.change"))
}

fn workspace_signal_point() -> &'static ProfilePoint {
    static POINT: OnceLock<ProfilePoint> = OnceLock::new();
    POINT.get_or_init(|| ProfilePoint::new("desktop.workspace.signal"))
}

fn workspace_apply_point() -> &'static ProfilePoint {
    static POINT: OnceLock<ProfilePoint> = OnceLock::new();
    POINT.get_or_init(|| ProfilePoint::new("desktop.workspace.apply"))
}

fn workspace_project_point() -> &'static ProfilePoint {
    static POINT: OnceLock<ProfilePoint> = OnceLock::new();
    POINT.get_or_init(|| ProfilePoint::new("desktop.workspace.project"))
}

fn workspace_present_point() -> &'static ProfilePoint {
    static POINT: OnceLock<ProfilePoint> = OnceLock::new();
    POINT.get_or_init(|| ProfilePoint::new("desktop.workspace.present"))
}

fn workspace_snapshot_received_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.workspace.snapshot_received"))
}

fn workspace_legacy_ignored_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.workspace.legacy_ignored"))
}

#[allow(dead_code)]
fn workspace_filesystem_rescan_from_workspace_counter() -> &'static CounterPoint {
    static COUNTER: OnceLock<CounterPoint> = OnceLock::new();
    COUNTER.get_or_init(|| CounterPoint::new("desktop.workspace.filesystem_rescan_from_workspace"))
}

fn model_gauge() -> &'static MemoryGauge {
    static GAUGE: OnceLock<MemoryGauge> = OnceLock::new();
    GAUGE.get_or_init(|| MemoryGauge::new("desktop.model"))
}

fn launcher_gauge() -> &'static MemoryGauge {
    static GAUGE: OnceLock<MemoryGauge> = OnceLock::new();
    GAUGE.get_or_init(|| MemoryGauge::new("desktop.launcher"))
}

fn icon_gauge() -> &'static MemoryGauge {
    static GAUGE: OnceLock<MemoryGauge> = OnceLock::new();
    GAUGE.get_or_init(|| MemoryGauge::new("desktop.icon"))
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
const ENTRY_ROW_IDS: [&str; 4] = [
    "menu-row-empty-trash",
    "menu-row-rename",
    "menu-row-shortcut",
    "menu-row-delete",
];

struct LauncherMeta {
    icon: String,
    valid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    Blank,
    Entry,
    Sticky,
}

struct MenuState {
    kind: MenuKind,
    anchor: Point,
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
    icons: projection::ParallelIconService,
    icon_slots: Vec<Option<IconKey>>,
    layout_path: PathBuf,
    sticky: StickyNoteStore,
    sticky_path: PathBuf,
    selection: SelectionModel,
    selection_start: Option<Point>,
    item_drag: Option<ItemDrag>,
    launchers: BTreeMap<PathBuf, LauncherMeta>,
    menu: Option<MenuState>,
    pending_delete: Option<PathBuf>,
    last_click: Option<ClickRecord>,
    dirty: Arc<AtomicBool>,
    icons_pending: Arc<AtomicBool>,
    rename: Option<RenameState>,
    drag_ghost: Option<DragGhost>,
    sticky_edit: Option<StickyEdit>,
    sticky_gesture: Option<StickyGesture>,
    sticky_persist_pending: bool,
    sticky_persist_armed: bool,
    sticky_persist_due: Option<Instant>,
    workspace: workspace_sync::WorkspaceAuthority,
    workspace_visual_dirty: bool,
    workspace_present_pending: bool,
    workspace_present_span: Option<flamewm_profiler::SpanGuard>,
    settings: SettingsSnapshot,
    settings_visual_dirty: bool,
    sticky_notes_enabled: bool,
    wallpaper_loader: wallpaper_loader::WallpaperLoader,
}

#[derive(Clone)]
struct RenameState {
    item_id: String,
    source_path: PathBuf,
    buffer: String,
}

struct DragGhost {
    kind: DesktopItemKind,
    icon_override: Option<String>,
    label: String,
    offset_x: f32,
    offset_y: f32,
    count: usize,
    visible: bool,
}

#[derive(Clone)]
struct StickyEdit {
    note_id: String,
    buffer: String,
}

#[derive(Clone)]
struct StickyGesture {
    note_id: String,
    mode: StickyGestureMode,
    start_x: f32,
    start_y: f32,
    original: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StickyGestureMode {
    Move,
    Resize,
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
}

fn main() -> Result<(), String> {
    flamewm_profiler::init_process("flamewm-desktop");
    flamewm_debug::init_process("desktop");
    let home = PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?);
    let user_dirs = home.join(".config/user-dirs.dirs");
    let user_dirs = fs::read_to_string(user_dirs).ok();
    let directory = DesktopDirResolver::resolve(&home, user_dirs.as_deref());
    fs::create_dir_all(&directory).map_err(|error| format!("create desktop directory: {error}"))?;

    let work_area = resolve_work_area()?;
    let grid = GridConfig::compute(work_area, 100);
    let settings = query_settings();
    let mut wallpaper_loader = wallpaper_loader::WallpaperLoader::new(
        work_area.width.max(1) as u32,
        work_area.height.max(1) as u32,
        packaged_root(),
    )?;
    wallpaper_loader
        .request(setting_text(&settings, "wallpaper"))
        .map_err(|_| "wallpaper loader closed during startup".to_owned())?;
    let mut model = DesktopModel::default();
    let output = OutputId::new("default");
    {
        let _span = scan_point().start();
        model
            .rescan(&directory, &output, grid)
            .map_err(|error| format!("{error:?}"))?;
    }
    let layout_path = state_path("desktop-layout.state")?;
    if let Ok(text) = fs::read_to_string(&layout_path) {
        for (path, position) in LayoutStore::parse(&text).map_err(|error| format!("{error:?}"))? {
            model.set_position(path, position);
        }
        model
            .rescan(&directory, &output, grid)
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
            icons: projection::ParallelIconService::spawn(packaged_root()),
            icon_slots: vec![None; SLOT_COUNT],
            layout_path,
            sticky,
            sticky_path,
            selection: SelectionModel::default(),
            selection_start: None,
            item_drag: None,
            launchers: BTreeMap::new(),
            menu: None,
            pending_delete: None,
            last_click: None,
            dirty: Arc::clone(&dirty),
            icons_pending: Arc::clone(&icons_pending),
            rename: None,
            drag_ghost: None,
            sticky_edit: None,
            sticky_gesture: None,
            sticky_persist_pending: false,
            sticky_persist_armed: false,
            sticky_persist_due: None,
            workspace: workspace_sync::WorkspaceAuthority::new(workspace),
            workspace_visual_dirty: false,
            workspace_present_pending: false,
            workspace_present_span: None,
            sticky_notes_enabled: setting_bool(&settings, "stickyNotesEnabled", true),
            settings,
            settings_visual_dirty: false,
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
    {
        let state = state_cell.borrow();
        projection::project_background(
            &mut document,
            state.settings.values.get("wallpaper"),
            &state.wallpaper_loader.state().presentation,
        )?;
    }
    projection::assign_document_layers(&mut document)?;
    projection::initialize_transient_visibility(&mut document)?;
    {
        let _span = sync_point().start();
        sync_document(&mut document, &mut state_cell.borrow_mut())?;
    }
    update_gauges(&state_cell.borrow());
    let mut reactor = Reactor::new().map_err(|error| format!("create reactor: {error}"))?;
    reactor
        .register_timer(
            Duration::from_secs(flamewm_profiler::profile_interval_secs()),
            true,
            || {
                if flamewm_profiler::enabled() {
                    let report = report_window();
                    if !report.is_empty() {
                        eprintln!("{report}");
                    }
                }
            },
        )
        .map_err(|error| format!("register profiler timer: {error}"))?;
    register_sticky_persist_timer(&mut reactor)?;
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
                                let wallpaper = match snapshot.values.get("wallpaper") {
                                    Some(SettingValue::Text(path)) => path.clone(),
                                    _ => String::new(),
                                };
                                let mut state = state_cell.borrow_mut();
                                if let Err(error) = state.wallpaper_loader.request(wallpaper) {
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
                            flamewm_control_wire::ControlSignal::PanelsSnapshotChanged { .. }
                            | flamewm_control_wire::ControlSignal::PanelsChanged { .. } => {
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
    // Known outer rect of the desktop window (created at the work area); updated
    // whenever a geometry request is issued. Read back by the resize decision.
    let mut window_rect = Rect::new(
        work_area.x,
        work_area.y,
        work_area.width.max(1),
        work_area.height.max(1),
    );
    let geometry_slot = WindowGeometrySlot::default();
    run_with_controller_events_role_with_reactor_geometry(
        document,
        UiWindowConfig {
            width: work_area.width.max(1) as u32,
            height: work_area.height.max(1) as u32,
            x: work_area.x,
            y: work_area.y,
            title: "FlameWM Desktop".to_owned(),
            identity: None,
        },
        UiWindowRole::Desktop,
        &mut reactor,
        std::rc::Rc::clone(&geometry_slot),
        {
            let event_cell = std::rc::Rc::clone(&state_cell);
            let mutation_dispatcher = workspace_mutations.clone();
            move |event, document| {
                let state_cell = &event_cell;
                let mut state = state_cell.borrow_mut();
                refresh_settings_projection(document, &mut state)?;
                refresh_if_changed(document, &mut state)?;
                refresh_workspace_projection(document, &mut state)?;
                apply_pending_icon_results(document, &mut state)?;
                poll_sticky_persist(document, &mut state)?;
                match event {
                    UiControllerEvent::Action(action) => {
                        handle_event(action, document, &mut state, mutation_dispatcher.as_ref())
                    }
                    UiControllerEvent::Key(_) => Ok(()),
                }
            }
        },
        {
            let flush_cell = std::rc::Rc::clone(&state_cell);
            let panels_flag = std::rc::Rc::clone(&panels_dirty);
            let mut current_work_area = work_area;
            let geometry_out = std::rc::Rc::clone(&geometry_slot);
            move |document| {
                if panels_flag.replace(false) {
                    refresh_work_area_after_panel_change(
                        &mut current_work_area,
                        &mut window_rect,
                        &geometry_out,
                    );
                }
                let state_cell = &flush_cell;
                let mut state = state_cell.borrow_mut();
                let _completed_workspace_present = state.workspace_present_span.take();
                refresh_settings_projection(document, &mut state)?;
                refresh_if_changed(document, &mut state)?;
                refresh_workspace_projection(document, &mut state)?;
                apply_pending_icon_results(document, &mut state)?;
                poll_sticky_persist(document, &mut state)?;
                let result = flush_sticky_persist_if_due(document, &mut state);
                if result.is_ok() && state.workspace_present_pending {
                    state.workspace_present_pending = false;
                    state.workspace_present_span = Some(workspace_present_point().start());
                }
                result
            }
        },
    )?;
    flush_sticky_now(&state_cell.borrow());
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

fn setting_percent(snapshot: &SettingsSnapshot, key: &str, default: u8) -> u8 {
    match snapshot.values.get(key) {
        Some(SettingValue::Percent(value)) => *value,
        _ => default,
    }
}

fn percent_alpha(percent: u8) -> u8 {
    ((u16::from(percent) * 255) / 100) as u8
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
    // SettingsSnapshotChanged is the live authority for stickyNotesEnabled and
    // DesktopSelectionFillOpacity; wallpaper stays on the bounded loader path.
    state.sticky_notes_enabled = setting_bool(&state.settings, "stickyNotesEnabled", true);
    let wallpaper = state.settings.values.get("wallpaper");
    projection::project_background(
        document,
        wallpaper,
        &state.wallpaper_loader.state().presentation,
    )?;
    sync_selection_visuals(document, state)?;
    sync_sticky(document, state)?;
    state.settings_visual_dirty = false;
    Ok(())
}

fn refresh_if_changed_direct(state: &mut DesktopState) -> Result<(), String> {
    // Kept as a narrow compatibility seam for the startup/event boundary:
    // workspace visual dirtiness is never filesystem dirtiness.
    state.workspace_visual_dirty = true;
    Ok(())
}

fn register_sticky_persist_timer(reactor: &mut Reactor) -> Result<(), String> {
    // Debounce owner is the reactor tick: the flag is armed by mutations and
    // flushed in poll_sticky_persist/flush_sticky_persist_if_due (<=500ms via
    // a 100ms repeat). No threads.
    reactor
        .register_timer(Duration::from_millis(100), true, || {})
        .map(|_| ())
        .map_err(|error| format!("register sticky persist timer: {error}"))
}

fn poll_sticky_persist(
    _document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if state.sticky_persist_pending && !state.sticky_persist_armed {
        state.sticky_persist_armed = true;
        state.sticky_persist_due = Some(Instant::now() + Duration::from_millis(500));
    }
    Ok(())
}

fn flush_sticky_persist_if_due(
    _document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let Some(due) = state.sticky_persist_due else {
        return Ok(());
    };
    if Instant::now() < due {
        return Ok(());
    }
    state.sticky_persist_due = None;
    state.sticky_persist_pending = false;
    state.sticky_persist_armed = false;
    persist_sticky(state)
}

fn persist_sticky(state: &mut DesktopState) -> Result<(), String> {
    sticky_persist_counter().increment();
    sticky_persistence::persist(&state.sticky_path, &state.sticky)
        .map_err(|error| format!("{error:?}"))
}

fn schedule_sticky_persist(state: &mut DesktopState) {
    state.sticky_persist_pending = true;
}

fn flush_sticky_now(state: &DesktopState) {
    if state.sticky_persist_pending {
        let _ = sticky_persistence::persist(&state.sticky_path, &state.sticky);
    }
}

/// Root geometry comes from the actual X root through the X11 runtime owner.
/// Panel struts are subtracted when Control GetPanels answers; otherwise the
/// full root rect is used with exactly one degraded warning. No constant size.
fn resolve_work_area() -> Result<Rect, String> {
    let (width, height) = root_geometry()?;
    let root = Rect::new(0, 0, width as i32, height as i32);
    match panel_geometries() {
        Ok(panels) => {
            let mut area = root;
            for panel in panels {
                area = subtract_panel_strip(area, panel);
            }
            Ok(area)
        }
        Err(error) => {
            eprintln!("desktop: degraded work area: {error}; using full root geometry");
            Ok(root)
        }
    }
}

fn panel_geometries() -> Result<Vec<Rect>, String> {
    let client = ControlClient::connect(BusKind::Session).map_err(|error| format!("{error:?}"))?;
    match client.call(&ControlRequest::GetPanels) {
        Ok(ControlResponse::Panels(snapshot)) => Ok(snapshot
            .panels
            .into_iter()
            .filter(|panel| panel.visible)
            .map(|panel| panel.geometry)
            .collect()),
        Ok(response) => Err(format!("GetPanels returned {response:?}")),
        Err(error) => Err(error.message.clone()),
    }
}

/// Re-resolves the work area after a panel signal. Errors keep the current
/// area and are reported once per signal burst. When the desktop window's
/// known rect differs from the new work area, a move+size request is written
/// to the geometry slot for the X11 loop to apply.
fn refresh_work_area_after_panel_change(
    current: &mut Rect,
    window: &mut Rect,
    geometry: &WindowGeometrySlot,
) {
    let next = match resolve_work_area() {
        Ok(next) => next,
        Err(error) => {
            eprintln!("desktop: work area refresh after panel change failed: {error}");
            return;
        }
    };
    let Some(area) = changed_work_area(*current, next) else {
        return;
    };
    *current = area;
    let Some(target) = window_resize_request(*window, area) else {
        return;
    };
    geometry.set(Some(WindowGeometry {
        x: target.x,
        y: target.y,
        width: target.width.max(1) as u32,
        height: target.height.max(1) as u32,
    }));
    *window = target;
}

/// Pure work-area change detector: `None` means no resize request.
fn changed_work_area(current: Rect, next: Rect) -> Option<Rect> {
    (current != next).then_some(next)
}

/// Pure window decision: `None` when the window already matches the target
/// rect (position and size), otherwise the rect to request.
fn window_resize_request(window: Rect, target: Rect) -> Option<Rect> {
    (window != target).then_some(target)
}

fn subtract_panel_strip(mut area: Rect, panel: Rect) -> Rect {
    let Some(intersection) = area.intersection(panel) else {
        return area;
    };
    if intersection.width >= area.width - 1 {
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
    if intersection.height >= area.height - 1 {
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
    state
        .model
        .rescan(&state.directory, &OutputId::new("default"), state.grid)
        .map_err(|error| format!("{error:?}"))?;
    refresh_launchers(state);
    sync_document(document, state)?;
    update_gauges(state);
    Ok(())
}

fn refresh_workspace_projection(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    if !state.workspace_visual_dirty {
        return Ok(());
    }
    let _project = workspace_project_point().start();
    sync_sticky(document, state)?;
    sync_menu(document, state)?;
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
        match DesktopEntry::from_file(&item.path) {
            Ok(Some(entry)) => {
                state.launchers.insert(
                    item.path.clone(),
                    LauncherMeta {
                        icon: entry.icon().to_owned(),
                        valid: true,
                    },
                );
            }
            Ok(None) | Err(_) => {
                state.launchers.insert(
                    item.path.clone(),
                    LauncherMeta {
                        icon: String::new(),
                        valid: false,
                    },
                );
            }
        }
    }
}

fn launcher_icon(state: &DesktopState, path: &Path) -> Option<String> {
    state
        .launchers
        .get(path)
        .filter(|meta| meta.valid && !meta.icon.trim().is_empty())
        .map(|meta| meta.icon.clone())
}

fn sync_document(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let _span = sync_point().start();
    render_sync_counter().increment();
    apply_ready_icons(document, state)?;
    for index in 0..SLOT_COUNT {
        let item = state.model.items().nth(index);
        if let Some(item) = item {
            let point = state.grid.cell_to_pixel(item.cell);
            let icon = launcher_icon(state, &item.path);
            let kind = item.kind;
            let label = item.display_name.clone();
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
                )?;
                state.icon_slots[index] = Some(projection::ParallelIconService::icon_key(
                    kind,
                    icon.as_deref(),
                ));
                let _ = key;
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
    sync_sticky(document, state)?;
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
            let matches = state
                .icon_slots
                .get(index)
                .and_then(|slot| slot.as_ref())
                .is_some_and(|expected| expected == &key);
            if !matches {
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
    Ok(())
}

const STICKY_SLOTS: usize = projection::STICKY_SLOT_COUNT;

fn visible_sticky_notes(state: &DesktopState) -> Vec<StickyNote> {
    state
        .sticky
        .notes_for_workspace(state.workspace.active_index())
        .into_iter()
        .take(STICKY_SLOTS)
        .cloned()
        .collect()
}

fn sync_sticky(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let visible = visible_sticky_notes(state);
    let show_any = state.sticky_notes_enabled && state.sticky.enabled() && !visible.is_empty();
    for slot in 0..STICKY_SLOTS {
        let number = slot + 1;
        let id = format!("sticky-{number}");
        if let Some(note) = visible.get(slot) {
            document.visible(&id, show_any)?;
            document.position(&id, note.rect.x as f32, note.rect.y as f32)?;
            document.size(&id, note.rect.width as f32, note.rect.height as f32)?;
            document.background(
                &id,
                UiColor {
                    r: note.background.red,
                    g: note.background.green,
                    b: note.background.blue,
                    a: 255,
                },
            )?;
            let number = slot + 1;
            let text_id = format!("sticky-{number}-text");
            let editing = state
                .sticky_edit
                .as_ref()
                .filter(|edit| edit.note_id == note.id)
                .map(|edit| edit.buffer.clone());
            let text = match editing {
                Some(buffer) => projection::caret_text(&buffer, true),
                None => note.text.clone(),
            };
            document.text(&text_id, text)?;
            document.font_size(&text_id, note.text_size as f32)?;
            document.font_weight(&text_id, if note.bold { 700 } else { 400 })?;
            document.foreground(
                &text_id,
                UiColor {
                    r: note.foreground.red,
                    g: note.foreground.green,
                    b: note.foreground.blue,
                    a: 255,
                },
            )?;
            let number = slot + 1;
            document.visible(&format!("sticky-{number}-drag"), show_any)?;
            for corner in ["nw", "ne", "sw", "se"] {
                document.visible(&format!("sticky-{number}-resize-{corner}"), show_any)?;
            }
        } else {
            document.visible(&id, false)?;
            let number = slot + 1;
            document.text(&format!("sticky-{number}-text"), String::new())?;
            let _ = document.visible(&format!("sticky-{number}-drag"), false);
            for corner in ["nw", "ne", "sw", "se"] {
                let _ = document.visible(&format!("sticky-{number}-resize-{corner}"), false);
            }
        }
    }
    Ok(())
}

fn handle_event(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    mutation_dispatcher: Option<&Arc<MutationDispatcher>>,
) -> Result<(), String> {
    if action.action == "keyboard.input" {
        if action.text.as_deref() == Some("") {
            return cancel_transient(document, state);
        }
        if state.rename.is_some() {
            return handle_rename_input(action, document, state);
        }
        if let Some(edit) = state.sticky_edit.clone() {
            return handle_sticky_text_input(action, document, state, &edit);
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
        "desktop.menu.surface" | "desktop.entry-menu.surface" | "desktop.confirm.surface"
    ) {
        return Ok(());
    }
    if action.action == "desktop.rename.surface" {
        return Ok(());
    }
    if action.action == "desktop.surface" {
        return handle_surface(action, document, state);
    }
    // Sticky Secondary opens the sticky context menu for the exact note under
    // the pointer. This routes BEFORE the generic sticky.* primary dispatch
    // below; otherwise the generic branch shadows it and it is unreachable.
    if action.action.starts_with("sticky.")
        && action.phase == UiActionPhase::Release
        && pointer_button(action) == PointerButton::Secondary
    {
        let visible = visible_sticky_notes(state);
        let slot_index = slot_number(&action.action).and_then(|number| number.checked_sub(1));
        if let Some(position) = slot_index {
            if let Some(note) = visible.get(position) {
                context_secondary_counter().increment();
                let anchor = pointer_point(action, state.grid.work_area);
                state.menu = Some(MenuState {
                    kind: MenuKind::Sticky,
                    anchor,
                    selected_id: Some(note.id.clone()),
                });
                state.pending_delete = None;
                hide_confirm(document)?;
                return sync_menu(document, state);
            }
        }
        return Ok(());
    }
    if action.action == "sticky.move" {
        return Ok(());
    }
    if let Some(raw) = action.action.strip_prefix("desktop.item.") {
        if let Ok(index) = raw.parse::<usize>() {
            return handle_item(action, document, state, index);
        }
    }
    if action.action == "sticky.menu.surface" {
        return Ok(());
    }
    if action.action.starts_with("sticky.") {
        return handle_sticky_slot(action, document, state);
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
            | "entry.shortcut"
            | "entry.delete"
            | "confirm.delete"
            | "confirm.cancel"
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
    state.selection.clear_rubber();
    state.item_drag = None;
    state.last_click = None;
    state.menu = None;
    state.pending_delete = None;
    state.rename = None;
    commit_sticky_edit(state);
    state.sticky_gesture = None;
    hide_drag_ghost(document, state)?;
    document.visible("desktop-selection", false)?;
    hide_menus(document)?;
    hide_confirm(document)?;
    sync_selection_visuals(document, state)?;
    sync_rename_editor(document, state)?;
    sync_sticky(document, state)
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
        let anchor = pointer_point(action, state.grid.work_area);
        commit_sticky_edit(state);
        state.menu = Some(MenuState {
            kind: MenuKind::Blank,
            anchor,
            selected_id: None,
        });
        state.pending_delete = None;
        hide_confirm(document)?;
        state.selection_start = None;
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
            hide_menus(document)?;
            hide_confirm(document)?;
            state.pending_delete = None;
            state.last_click = None;
            commit_sticky_edit(state);
            let start = pointer_point(action, state.grid.work_area);
            state.item_drag = None;
            state.selection_start = Some(start);
            selection_begin_counter().increment();
            state.selection.clear();
            state.selection.set_rubber(normalized_selection_rect(
                start,
                start,
                state.grid.work_area,
            ));
            document.visible("desktop-selection", false)?;
            sync_selection_visuals(document, state)?;
        }
        UiActionPhase::Motion => {
            if let Some(start) = state.selection_start {
                let point = pointer_point(action, state.grid.work_area);
                let rect = normalized_selection_rect(start, point, state.grid.work_area);
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
                let point = pointer_point(action, state.grid.work_area);
                let rect = normalized_selection_rect(start, point, state.grid.work_area);
                state.selection.set_rubber(rect);
                update_selection_from_rubber(state);
            }
            state.selection_start = None;
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
        state.selection.select_exclusive(item_id.clone());
        commit_sticky_edit(state);
        state.item_drag = None;
        state.selection_start = None;
        state.selection.clear_rubber();
        document.visible("desktop-selection", false)?;
        state.last_click = None;
        state.menu = Some(MenuState {
            kind: MenuKind::Entry,
            anchor: pointer_point(action, state.grid.work_area),
            selected_id: Some(item_id),
        });
        state.pending_delete = None;
        hide_confirm(document)?;
        sync_selection_visuals(document, state)?;
        return sync_menu(document, state);
    }
    if button != PointerButton::Primary {
        return Ok(());
    }
    match action.phase {
        UiActionPhase::Press => {
            hide_menus(document)?;
            hide_confirm(document)?;
            state.pending_delete = None;
            commit_sticky_edit(state);
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
            let originals = state
                .model
                .items()
                .enumerate()
                .filter(|(_, candidate)| state.selection.selected().contains(&candidate.id))
                .map(|(_, candidate)| OriginalItem {
                    id: candidate.id.clone(),
                    path: candidate.path.clone(),
                    output: candidate.output.clone(),
                    cell: candidate.cell,
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
            sync_selection_visuals(document, state)?;
        }
        UiActionPhase::Motion => {
            let Some(item) = state.model.items().nth(index) else {
                return Ok(());
            };
            let item_id = item.id.clone();
            let item_label = item.display_name.clone();
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
                state.drag_ghost = Some(DragGhost {
                    kind: item_kind,
                    icon_override: launcher_icon(state, &item_path),
                    label: item_label,
                    offset_x: action.x - drag.start_x,
                    offset_y: action.y - drag.start_y,
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
                .map_err(|error| format!("{error:?}"))?;
            Ok(())
        }
        LauncherOpen::RegularFile => {
            Command::new("xdg-open")
                .arg(path)
                .spawn()
                .map(|_| ())
                .map_err(|error| error.to_string())?;
            Ok(())
        }
    }
}

fn spawn_intent(intent: &CommandIntent) -> Result<(), String> {
    let mut command = Command::new(&intent.program);
    command.args(&intent.argv);
    if let Some(cwd) = &intent.cwd {
        command.current_dir(cwd);
    }
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("spawn {}: {error}", intent.program))?;
    Ok(())
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
            state
                .model
                .rescan(&state.directory, &OutputId::new("default"), state.grid)
                .map_err(|error| format!("{error:?}"))?;
            refresh_launchers(state);
            let id = path.to_string_lossy().into_owned();
            state.selection.clear();
            state.selection.select(id);
            state.menu = None;
            sync_document(document, state)?;
        }
        "menu.note" => {
            let anchor = state.menu.as_ref().map(|menu| menu.anchor);
            let anchor = anchor.unwrap_or(state.grid.work_area.origin());
            if state.sticky.enabled() {
                let workspace = state.workspace.active_index();
                let sequence = state.sticky.count_for_workspace(workspace) + 1;
                let id = format!("sticky-{workspace}-{sequence}");
                if state.sticky.count_for_workspace(workspace) < STICKY_SLOTS {
                    state.sticky.create(StickyNote::new(
                        id,
                        OutputId::new("default"),
                        workspace,
                        anchor.x,
                        anchor.y,
                    ));
                    schedule_sticky_persist(state);
                }
            }
            state.menu = None;
            sync_document(document, state)?;
        }
        "menu.desktop" => {
            insert_workspace_after(mutation_dispatcher, state)?;
            state.menu = None;
            sync_menu(document, state)?;
        }
        "menu.settings" => {
            if let Err(error) = spawn_intent(&desktop_settings()) {
                report_action_failure("open desktop settings", error);
                spawn_failure_counter().increment();
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
            let selected = state
                .menu
                .as_ref()
                .and_then(|menu| menu.selected_id.clone());
            let Some(id) = selected else {
                state.menu = None;
                return sync_menu(document, state);
            };
            let path = state
                .model
                .items()
                .find(|item| item.id == id)
                .map(|item| item.path.clone());
            let Some(path) = path else {
                state.menu = None;
                return sync_menu(document, state);
            };
            state.menu = None;
            hide_menus(document)?;
            state.pending_delete = Some(path);
            show_confirm(document, state)?;
        }
        "confirm.delete" => {
            if let Some(path) = state.pending_delete.take() {
                delete_permanent(&path)?;
                state
                    .model
                    .rescan(&state.directory, &OutputId::new("default"), state.grid)
                    .map_err(|error| format!("{error:?}"))?;
                refresh_launchers(state);
            }
            hide_confirm(document)?;
            sync_document(document, state)?;
        }
        "confirm.cancel" => {
            state.pending_delete = None;
            hide_confirm(document)?;
        }
        "entry.empty-trash" => {
            empty_home_trash()?;
            state.menu = None;
            sync_menu(document, state)?;
        }
        "sticky.menu.delete" => {
            let note_id = state
                .menu
                .as_ref()
                .and_then(|menu| menu.selected_id.clone())
                .or_else(|| state.sticky.notes().first().map(|note| note.id.clone()));
            if let Some(note_id) = note_id {
                state.sticky.remove(&note_id);
                if state
                    .sticky_edit
                    .as_ref()
                    .is_some_and(|edit| edit.note_id == note_id)
                {
                    state.sticky_edit = None;
                }
                sticky_persist_counter().increment();
                sticky_persistence::persist(&state.sticky_path, &state.sticky)
                    .map_err(|error| format!("{error:?}"))?;
            }
            state.menu = None;
            sync_document(document, state)?;
        }
        "sticky.menu.yellow" | "sticky.menu.green" | "sticky.menu.pink" | "sticky.menu.blue" => {
            let preset = match action {
                "sticky.menu.yellow" => 0,
                "sticky.menu.green" => 1,
                "sticky.menu.pink" => 2,
                _ => 3,
            };
            let note_id = state
                .menu
                .as_ref()
                .and_then(|menu| menu.selected_id.clone());
            if let Some(note_id) = note_id {
                state.sticky.set_color(&note_id, preset);
                schedule_sticky_persist(state);
            }
            state.menu = None;
            sync_document(document, state)?;
        }
        "sticky.menu.bold" => {
            let note_id = state
                .menu
                .as_ref()
                .and_then(|menu| menu.selected_id.clone());
            if let Some(note_id) = note_id {
                state.sticky.toggle_bold(&note_id);
                schedule_sticky_persist(state);
            }
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
    let selected = state
        .menu
        .as_ref()
        .and_then(|menu| menu.selected_id.clone());
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
    hide_menus(document)?;
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
    let anchor = state
        .model
        .items()
        .find(|item| item.id == rename.item_id)
        .map(|item| state.grid.cell_to_pixel(item.cell))
        .unwrap_or(state.grid.work_area.origin());
    let width = 220;
    let height = 48;
    let placed = clamp_menu_anchor(
        state.grid.work_area,
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
    if let Err(error) = state
        .model
        .rescan(&state.directory, &OutputId::new("default"), state.grid)
    {
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
    let selected = state
        .menu
        .as_ref()
        .and_then(|menu| menu.selected_id.clone());
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
    state
        .model
        .rescan(&state.directory, &OutputId::new("default"), state.grid)
        .map_err(|error| format!("{error:?}"))?;
    refresh_launchers(state);
    state.selection.clear();
    state
        .selection
        .select(destination.to_string_lossy().into_owned());
    Ok(())
}

/// Permanent delete only runs after the confirmation step armed it.
fn delete_permanent(path: &Path) -> Result<(), String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("stat {}: {error}", path.display()))?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path).map_err(|error| format!("delete {}: {error}", path.display()))?;
    } else {
        fs::remove_file(path).map_err(|error| format!("delete {}: {error}", path.display()))?;
    }
    Ok(())
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
        EntryMenuAction::Delete => "menu-row-delete",
    }
}

fn hide_menus(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    document.visible("desktop-menu", false)?;
    document.visible("desktop-entry-menu", false)?;
    document.visible("sticky-menu", false)?;
    Ok(())
}

fn hide_confirm(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    document.visible("desktop-confirm", false)
}

fn show_confirm(document: &mut impl UiDocumentAccess, state: &DesktopState) -> Result<(), String> {
    let parts = confirm_menu_parts();
    let measured = context_menu_size(&parts, flamewm_ui_core::context_menu::MIN_ROW_WIDTH);
    let size = (measured.width, measured.height);
    let center = Point::new(
        state.grid.work_area.x + (state.grid.work_area.width - size.0) / 2,
        state.grid.work_area.y + (state.grid.work_area.height - size.1) / 2,
    );
    let placed = clamp_menu_anchor(
        state.grid.work_area,
        Rect::new(center.x, center.y, size.0, size.1),
        size,
    );
    document.position("desktop-confirm", placed.x as f32, placed.y as f32)?;
    document.size("desktop-confirm", placed.width as f32, placed.height as f32)?;
    document.visible("desktop-confirm", true)
}

fn sync_menu(document: &mut impl UiDocumentAccess, state: &DesktopState) -> Result<(), String> {
    let _total = context_open_total_point().start();
    {
        let _close = context_open_close_point().start();
        hide_menus(document)?;
    }
    let Some(menu) = state.menu.as_ref() else {
        return Ok(());
    };
    match menu.kind {
        MenuKind::Blank => {
            let rows = {
                let _project = context_open_project_point().start();
                blank_context_menu(state.sticky.enabled())
            };
            let visible: BTreeSet<&str> = rows.iter().map(|action| blank_row_id(*action)).collect();
            for id in BLANK_ROW_IDS {
                document.visible(id, visible.contains(id))?;
            }
            place_menu_parts(
                document,
                state,
                "desktop-menu",
                menu.anchor,
                &blank_menu_parts(rows.len()),
            )?;
        }
        MenuKind::Entry => {
            let selected_kind = menu
                .selected_id
                .as_ref()
                .and_then(|id| state.model.items().find(|item| &item.id == id))
                .map(|item| item.kind);
            let is_trash = selected_kind == Some(DesktopItemKind::TrashPseudo);
            let rows = {
                let _project = context_open_project_point().start();
                entry_context_menu(is_trash)
            };
            let visible: BTreeSet<&str> = rows.iter().map(|action| entry_row_id(*action)).collect();
            for id in ENTRY_ROW_IDS {
                document.visible(id, visible.contains(id))?;
            }
            place_menu_parts(
                document,
                state,
                "desktop-entry-menu",
                menu.anchor,
                &entry_menu_parts(rows.len()),
            )?;
        }
        MenuKind::Sticky => {
            let parts = {
                let _project = context_open_project_point().start();
                sticky_menu_parts()
            };
            place_menu_parts(document, state, "sticky-menu", menu.anchor, &parts)?;
        }
    }
    Ok(())
}

fn place_menu_parts(
    document: &mut impl UiDocumentAccess,
    state: &DesktopState,
    id: &str,
    anchor: Point,
    parts: &[MenuPart],
) -> Result<(), String> {
    // Shared C08 contract: size from actual parts, clamp through menu_rect.
    // Position/size land first so the first presented frame already shows
    // the menu above the contents.
    let placed = {
        let _measure = context_open_measure_point().start();
        let _place = context_open_place_point().start();
        parts_menu_rect(state.grid.work_area, anchor, parts)
    };
    document.position(id, placed.x as f32, placed.y as f32)?;
    document.size(id, placed.width as f32, placed.height as f32)?;
    context_open_counter().increment();
    let _present = context_open_present_point().start();
    document.visible(id, true)
}

fn show_drag_ghost(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    x: f32,
    y: f32,
) -> Result<(), String> {
    let Some(ghost) = state.drag_ghost.as_mut() else {
        return Ok(());
    };
    let kind = ghost.kind;
    let icon_override = ghost.icon_override.clone();
    ghost.visible = true;
    let gx = x - ghost.offset_x;
    let gy = y - ghost.offset_y;
    document.position("desktop-drag-ghost", gx, gy)?;
    projection::project_drag_ghost_icon(
        document,
        &mut state.icons,
        kind,
        icon_override.as_deref(),
    )?;
    let (line1, line2) = split_label_lines(&ghost.label);
    document.size(
        "desktop-drag-ghost",
        projection::DESKTOP_TILE_WIDTH,
        projection::DESKTOP_TILE_HEIGHT,
    )?;
    document.text("desktop-drag-ghost-label-1", line1)?;
    document.text("desktop-drag-ghost-label-2", line2)?;
    document.text(
        "desktop-drag-ghost-count",
        format!("{}", ghost.count.max(1)),
    )?;
    document.visible("desktop-drag-ghost", true)?;
    Ok(())
}

fn split_label_lines(label: &str) -> (String, String) {
    let mut words = label.split_whitespace();
    let mut line1 = String::new();
    let mut line2 = String::new();
    for word in words.by_ref() {
        if line1.len() + word.len() + usize::from(!line1.is_empty()) <= 12 {
            if !line1.is_empty() {
                line1.push(' ');
            }
            line1.push_str(word);
        } else {
            line2 = std::iter::once(word)
                .chain(words)
                .collect::<Vec<_>>()
                .join(" ");
            break;
        }
    }
    (line1, line2)
}

fn sync_drag_ghost(
    document: &mut impl UiDocumentAccess,
    state: &DesktopState,
) -> Result<(), String> {
    let visible = state.drag_ghost.as_ref().is_some_and(|ghost| ghost.visible);
    document.visible("desktop-drag-ghost", visible)
}

fn hide_drag_ghost(
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    state.drag_ghost = None;
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
    let selected = drag
        .originals
        .iter()
        .map(|item| DragItem {
            id: item.id.clone(),
            cell: item.cell,
        })
        .collect::<Vec<_>>();
    let selected_ids = drag
        .originals
        .iter()
        .map(|item| item.id.as_str())
        .collect::<BTreeSet<_>>();
    let occupied = state
        .model
        .items()
        .filter(|item| !selected_ids.contains(item.id.as_str()))
        .map(|item| item.cell)
        .collect::<BTreeSet<_>>();
    let delta = drag_cell_delta(
        x - drag.start_x,
        y - drag.start_y,
        state.grid.cell_width,
        state.grid.cell_height,
    );
    let updates = match group_drag_transaction(&selected, &occupied, delta, state.grid) {
        Ok(updates) => updates,
        Err(_) => {
            sync_document(document, state)?;
            return Ok(());
        }
    };

    for original in &drag.originals {
        if let Some(cell) = updates.get(&original.id).copied() {
            state.model.set_position(
                original.path.clone(),
                StoredPosition {
                    output: original.output.clone(),
                    cell,
                },
            );
        }
    }
    if let Err(error) = state
        .model
        .rescan(&state.directory, &OutputId::new("default"), state.grid)
    {
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
    state
        .model
        .rescan(&state.directory, &OutputId::new("default"), state.grid)
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
}

fn commit_sticky_edit(state: &mut DesktopState) {
    if state.sticky_edit.take().is_some() {
        schedule_sticky_persist(state);
    }
}

fn handle_sticky_text_input(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
    edit: &StickyEdit,
) -> Result<(), String> {
    let note_id = edit.note_id.clone();
    if action.text.as_deref() == Some("") {
        state.sticky_edit = None;
        return sync_sticky(document, state);
    }
    if let Some(text) = action.text.clone() {
        if text == "\u{8}" || text == "\u{7f}" {
            if let Some(current) = state.sticky_edit.as_mut() {
                current.buffer.pop();
            }
        } else {
            for ch in text.chars() {
                if ch == '\n' || !ch.is_control() {
                    if let Some(current) = state.sticky_edit.as_mut() {
                        current.buffer.push(ch);
                    }
                }
            }
        }
        if let Some(buffer) = state
            .sticky_edit
            .as_ref()
            .map(|current| current.buffer.clone())
        {
            state.sticky.set_text(&note_id, buffer);
            sticky_edit_counter().increment();
            schedule_sticky_persist(state);
        }
        return sync_sticky(document, state);
    }
    Ok(())
}

fn handle_sticky_slot(
    action: &UiActionEvent,
    document: &mut impl UiDocumentAccess,
    state: &mut DesktopState,
) -> Result<(), String> {
    let visible = visible_sticky_notes(state);
    let slot_index = slot_number(&action.action).and_then(|number| number.checked_sub(1));
    let Some(position) = slot_index else {
        return Ok(());
    };
    let Some(note) = visible.get(position).cloned() else {
        return Ok(());
    };
    let note_id = note.id.clone();
    // Primary press anywhere on the note body (root sticky.N.move action,
    // title excluded) activates the edit buffer, not just the text node.
    if action.action.ends_with(".move")
        && action.phase == UiActionPhase::Press
        && pointer_button(action) == PointerButton::Primary
    {
        hide_menus(document)?;
        state.menu = None;
        state.sticky_edit = Some(StickyEdit {
            note_id,
            buffer: note.text.clone(),
        });
        sticky_edit_counter().increment();
        return sync_sticky(document, state);
    }
    // Primary click elsewhere (desktop surface or another note) commits the
    // active buffer, persists, and clears sticky_edit.
    if action.action.ends_with(".text") {
        if action.phase == UiActionPhase::Press && pointer_button(action) == PointerButton::Primary
        {
            state.sticky_edit = Some(StickyEdit {
                note_id,
                buffer: note.text.clone(),
            });
            sticky_edit_counter().increment();
            return sync_sticky(document, state);
        }
        return Ok(());
    }
    if action.action.ends_with(".drag") {
        if pointer_button(action) != PointerButton::Primary {
            return Ok(());
        }
        match action.phase {
            UiActionPhase::Press => {
                state.sticky_gesture = Some(StickyGesture {
                    note_id,
                    mode: StickyGestureMode::Move,
                    start_x: action.x,
                    start_y: action.y,
                    original: note.rect,
                });
            }
            UiActionPhase::Motion => {
                if let Some(gesture) = state.sticky_gesture.clone() {
                    if gesture.note_id == note_id {
                        let rect = move_rect(
                            gesture.original,
                            (action.x - gesture.start_x) as i32,
                            (action.y - gesture.start_y) as i32,
                            state.grid.work_area,
                        );
                        state.sticky.set_rect(&note_id, rect);
                        sticky_move_counter().increment();
                        sync_sticky(document, state)?;
                    }
                }
            }
            UiActionPhase::Release => {
                if let Some(gesture) = state.sticky_gesture.take() {
                    if gesture.note_id == note_id {
                        let rect = move_rect(
                            gesture.original,
                            (action.x - gesture.start_x) as i32,
                            (action.y - gesture.start_y) as i32,
                            state.grid.work_area,
                        );
                        state.sticky.set_rect(&note_id, rect);
                        sticky_move_counter().increment();
                        flush_sticky_now(state);
                        sync_sticky(document, state)?;
                    }
                }
            }
            UiActionPhase::Hover => {}
        }
        return Ok(());
    }
    if action.action.contains(".resize-") {
        if pointer_button(action) != PointerButton::Primary {
            return Ok(());
        }
        let corner = if action.action.ends_with("-nw") {
            ResizeCorner::TopLeft
        } else if action.action.ends_with("-ne") {
            ResizeCorner::TopRight
        } else if action.action.ends_with("-sw") {
            ResizeCorner::BottomLeft
        } else {
            ResizeCorner::BottomRight
        };
        match action.phase {
            UiActionPhase::Press => {
                state.sticky_gesture = Some(StickyGesture {
                    note_id,
                    mode: StickyGestureMode::Resize,
                    start_x: action.x,
                    start_y: action.y,
                    original: note.rect,
                });
                state.sticky_gesture.as_mut().map(|gesture| {
                    gesture.mode = StickyGestureMode::Resize;
                });
                let _ = corner;
            }
            UiActionPhase::Motion | UiActionPhase::Release => {
                if let Some(gesture) = state.sticky_gesture.clone() {
                    if gesture.note_id == note_id {
                        let rect = resize_rect(
                            gesture.original,
                            corner,
                            (action.x - gesture.start_x) as i32,
                            (action.y - gesture.start_y) as i32,
                            state.grid.work_area,
                        );
                        state.sticky.set_rect(&note_id, rect);
                        sticky_resize_counter().increment();
                        if action.phase == UiActionPhase::Release {
                            state.sticky_gesture = None;
                            flush_sticky_now(state);
                        }
                        sync_sticky(document, state)?;
                    }
                }
            }
            UiActionPhase::Hover => {}
        }
        return Ok(());
    }
    Ok(())
}

fn slot_number(action: &str) -> Option<usize> {
    action
        .strip_prefix("sticky.")
        .and_then(|rest| rest.split('.').next())
        .and_then(|number| number.parse::<usize>().ok())
}

fn update_selection_from_rubber(state: &mut DesktopState) {
    let Some(_) = state.selection.rubber() else {
        return;
    };
    let item_rects = state
        .model
        .items()
        .map(|item| {
            let point = state.grid.cell_to_pixel(item.cell);
            (
                item.id.clone(),
                Rect::new(
                    point.x,
                    point.y,
                    state.grid.cell_width,
                    state.grid.cell_height,
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let selected = state.selection.hit_test(&item_rects);
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
    for (index, item) in state.model.items().enumerate() {
        let id = format!("desktop-item-{index}");
        if state.selection.selected().contains(&item.id) {
            document.background(
                &id,
                UiColor {
                    r: 239,
                    g: 64,
                    b: 72,
                    a: fill_alpha,
                },
            )?;
            document.border(
                &id,
                UiColor {
                    r: 239,
                    g: 64,
                    b: 72,
                    a: 221,
                },
            )?;
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
    if bin_dir.file_name()? != "bin" {
        return None;
    }
    let usr_dir = bin_dir.parent()?;
    if usr_dir.file_name()? != "usr" {
        return None;
    }
    Some(usr_dir.join("share/flamewm"))
}

#[cfg(test)]
mod work_area_tests {
    use super::{changed_work_area, subtract_panel_strip, window_resize_request};
    use flamewm_api::Rect;

    #[test]
    fn window_already_matching_work_area_requests_no_resize() {
        let area = Rect::new(0, 0, 1350, 586);

        assert_eq!(window_resize_request(area, area), None);
    }

    #[test]
    fn window_mismatching_work_area_requests_resize_to_target() {
        let window = Rect::new(0, 0, 1350, 641);
        let target = Rect::new(0, 0, 1350, 586);

        assert_eq!(window_resize_request(window, target), Some(target));
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
    fn panel_height_change_produces_new_work_area() {
        let root = Rect::new(0, 0, 1350, 641);
        let before = subtract_panel_strip(root, Rect::new(0, 641 - 44, 1350, 44));
        let after = subtract_panel_strip(root, Rect::new(0, 641 - 55, 1350, 55));

        assert_eq!(
            changed_work_area(before, after),
            Some(Rect::new(0, 0, 1350, 586))
        );
    }

    #[test]
    fn unchanged_work_area_produces_no_resize_request() {
        let area = Rect::new(0, 0, 1350, 586);

        assert_eq!(changed_work_area(area, area), None);
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

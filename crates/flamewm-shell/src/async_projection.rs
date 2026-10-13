//! Async projection: labels/visibility immediate, icons cached-or-placeholder.
//! Never resolves icons on the caller thread. Schedules worker requests only.

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

use flamewm_api::session::SessionCapabilities;
use flamewm_shell_core::StartModel;
use flamewm_ui_x11::{RuntimeImage, UiColor, UiDocumentAccess};

use crate::icon_loader::{IconLoader, IconTarget};
use crate::start::StartCategory;
use crate::taskbar::workspaces::{
    project as project_workspaces, slot_position, visible_page, PAGE_SIZE,
};
use crate::ShellSnapshot;

pub const TASK_SLOT_COUNT: usize = 8;
pub const START_APP_SLOT_COUNT: usize = 8;

const ICON_CACHE_ENTRY_BUDGET: usize = 2048;

pub type IconCacheKey = (String, u32, u32);

struct IconCache {
    entry_budget: usize,
    entries: HashMap<IconCacheKey, RuntimeImage>,
    order: VecDeque<IconCacheKey>,
}

impl IconCache {
    fn new(entry_budget: usize) -> Self {
        Self {
            entry_budget: entry_budget.max(1),
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&mut self, key: &IconCacheKey) -> Option<RuntimeImage> {
        let image = self.entries.get(key)?.clone();
        if let Some(position) = self.order.iter().position(|entry| entry == key) {
            self.order.remove(position);
        }
        self.order.push_back(key.clone());
        Some(image)
    }

    fn insert(&mut self, key: IconCacheKey, image: RuntimeImage) {
        if self.entries.contains_key(&key) {
            if let Some(position) = self.order.iter().position(|entry| entry == &key) {
                self.order.remove(position);
            }
        } else if self.entries.len() == self.entry_budget {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, image);
    }
}

fn icon_cache() -> &'static std::sync::Mutex<IconCache> {
    static CACHE: OnceLock<std::sync::Mutex<IconCache>> = OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(IconCache::new(ICON_CACHE_ENTRY_BUDGET)))
}

fn redundant_hover_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.start.redundant_hover"))
}

fn request_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.icon.request"))
}

fn result_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.icon.result"))
}

fn stale_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.icon.stale_drop"))
}

fn queue_full_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.icon.queue_full"))
}

fn high_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.icon.high"))
}

fn low_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.icon.low"))
}

fn coalesced_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.icon.coalesced"))
}

fn dropped_low_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.icon.dropped_low"))
}

fn fallback_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.icon.empty_placeholder"))
}

fn prewarm_counter() -> &'static flamewm_profiler::CounterPoint {
    static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
    C.get_or_init(|| flamewm_profiler::CounterPoint::new("shell.icon.prewarm"))
}

/// Exact icon target binding. A worker completion mutates its node only
/// when the live binding still equals key + generation + view epoch;
/// otherwise it is a stale drop. `view_epoch` is domain-scoped: Start
/// slots use the start epoch (bumped on category/search/rebuild), task
/// slots use the task epoch (bumped on task-slot reprojection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconBinding {
    pub key: IconCacheKey,
    pub target: IconTarget,
    pub generation: u64,
    pub view_epoch: u64,
}

fn bindings() -> &'static std::sync::Mutex<HashMap<IconTarget, IconBinding>> {
    static B: OnceLock<std::sync::Mutex<HashMap<IconTarget, IconBinding>>> = OnceLock::new();
    B.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn start_epoch() -> &'static std::sync::atomic::AtomicU64 {
    static E: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    E.get_or_init(|| std::sync::atomic::AtomicU64::new(0))
}

fn task_epoch() -> &'static std::sync::atomic::AtomicU64 {
    static E: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    E.get_or_init(|| std::sync::atomic::AtomicU64::new(0))
}

/// Bump the start view epoch: category, search query, or model rebuild.
pub fn bump_start_view_epoch() -> u64 {
    start_epoch().fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
}

/// Bump the task view epoch: task-slot reprojection.
pub fn bump_task_view_epoch() -> u64 {
    task_epoch().fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
}

fn current_start_epoch() -> u64 {
    start_epoch().load(std::sync::atomic::Ordering::SeqCst)
}

fn current_task_epoch() -> u64 {
    task_epoch().load(std::sync::atomic::Ordering::SeqCst)
}

fn epoch_for_target(target: IconTarget) -> u64 {
    match target {
        IconTarget::StartSlot(_) => current_start_epoch(),
        IconTarget::TaskSlot(_) => current_task_epoch(),
    }
}

/// Exact image node id for one icon target, using source ids only.
#[must_use]
pub fn icon_node_id_for_target(target: IconTarget) -> String {
    match target {
        IconTarget::StartSlot(n) => format!("start-app-slot-{}-icon", u32::from(n) + 1),
        IconTarget::TaskSlot(n) => format!("task-slot-{}-icon", u32::from(n) + 1),
    }
}

fn record_binding(target: IconTarget, name: &str, w: u32, h: u32, generation: u64) {
    let binding = IconBinding {
        key: (name.to_owned(), w, h),
        target,
        generation,
        view_epoch: epoch_for_target(target),
    };
    if let Ok(mut bindings) = bindings().lock() {
        bindings.insert(target, binding);
    }
}

/// Verify-then-apply one worker completion against the live binding.
/// The caller inserts the raster into the icon cache first (cache is
/// view-independent); this only gates node mutation. On match: mutates
/// exactly the one image node for `target` (existing invalidation drives
/// node/surface dirtiness). On mismatch: stale drop, counted, document
/// untouched. Out-of-range prewarm sentinels (`StartSlot(n)` at or above
/// the slot count, never bound to a node) are cache-only: no counter, no
/// mutation. Returns true when the node was mutated.
pub fn apply_icon_result(
    document: &mut impl UiDocumentAccess,
    target: IconTarget,
    name: &str,
    w: u32,
    h: u32,
    generation: u64,
    image: RuntimeImage,
) -> Result<bool, String> {
    let slot = match target {
        IconTarget::StartSlot(n) => u32::from(n),
        IconTarget::TaskSlot(n) => u32::from(n),
    };
    let range = match target {
        IconTarget::StartSlot(_) => START_APP_SLOT_COUNT as u32,
        IconTarget::TaskSlot(_) => TASK_SLOT_COUNT as u32,
    };
    if slot >= range {
        return Ok(false);
    }
    let expected = IconBinding {
        key: (name.to_owned(), w, h),
        target,
        generation,
        view_epoch: epoch_for_target(target),
    };
    let matches = bindings()
        .lock()
        .ok()
        .and_then(|bindings| bindings.get(&target).cloned())
        .is_some_and(|current| current == expected);
    if !matches {
        stale_counter().increment();
        return Ok(false);
    }
    document.image_rgb8(&icon_node_id_for_target(target), image)?;
    Ok(true)
}

/// Repaints one target's node from the view-independent cache when its live
/// binding names a key whose raster is already cached. Covers a completion
/// dropped as stale after an epoch bump: the raster is cached, so the node
/// must be repainted without waiting for another projection.
pub fn apply_cached_binding(
    document: &mut impl UiDocumentAccess,
    target: IconTarget,
) -> Result<bool, String> {
    let slot = match target {
        IconTarget::StartSlot(n) | IconTarget::TaskSlot(n) => u32::from(n),
    };
    let range = match target {
        IconTarget::StartSlot(_) => START_APP_SLOT_COUNT as u32,
        IconTarget::TaskSlot(_) => TASK_SLOT_COUNT as u32,
    };
    if slot >= range {
        return Ok(false);
    }
    let bound = bindings()
        .lock()
        .ok()
        .and_then(|bindings| bindings.get(&target).map(|binding| binding.key.clone()));
    let Some((name, w, h)) = bound else {
        return Ok(false);
    };
    let Some(image) = cached_image(&name, w, h) else {
        return Ok(false);
    };
    document.image_rgb8(&icon_node_id_for_target(target), image)?;
    Ok(true)
}

#[cfg(test)]
pub fn test_record_binding(target: IconTarget, name: &str, w: u32, h: u32, generation: u64) {
    record_binding(target, name, w, h, generation);
}

#[cfg(test)]
pub fn test_reset_icon_bindings() {
    start_epoch().store(0, std::sync::atomic::Ordering::SeqCst);
    task_epoch().store(0, std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut bindings) = bindings().lock() {
        bindings.clear();
    }
    if let Ok(mut cache) = icon_cache().lock() {
        cache.entries.clear();
        cache.order.clear();
    }
}
#[derive(Debug, Default)]
pub struct StartViewNote {
    last_category: Option<StartCategory>,
    last_query: Option<String>,
}

impl StartViewNote {
    /// Returns true when the view semantically changed and needs reprojection.
    pub fn note_start_view(&mut self, category: StartCategory, query: &str) -> bool {
        if self.last_category == Some(category) && self.last_query.as_deref() == Some(query) {
            redundant_hover_counter().increment();
            return false;
        }
        self.last_category = Some(category);
        self.last_query = Some(query.to_owned());
        true
    }
}

fn cached_image(name: &str, w: u32, h: u32) -> Option<RuntimeImage> {
    icon_cache().lock().ok()?.get(&(name.to_owned(), w, h))
}

pub fn note_icon_result(name: &str, w: u32, h: u32, image: RuntimeImage) {
    if let Ok(mut cache) = icon_cache().lock() {
        cache.insert((name.to_owned(), w, h), image);
    }
    result_counter().increment();
}

/// Drops every caller-side cached raster so a generation bump (theme or
/// palette change) never retains an old icon. Worker service keys already
/// carry the generation; this keeps the projection cache consistent with
/// them without changing call-site signatures.
pub fn invalidate_icon_cache() {
    if let Ok(mut cache) = icon_cache().lock() {
        cache.entries.clear();
        cache.order.clear();
    }
}

pub fn note_loader_stats(loader: &IconLoader) {
    if loader.queue_full_drops > 0 {
        queue_full_counter().increment_by(loader.queue_full_drops);
    }
    if loader.stale_drops > 0 {
        stale_counter().increment_by(loader.stale_drops);
    }
    if loader.high_enqueued > 0 {
        high_counter().increment_by(loader.high_enqueued);
    }
    if loader.low_enqueued > 0 {
        low_counter().increment_by(loader.low_enqueued);
    }
    if loader.coalesced > 0 {
        coalesced_counter().increment_by(loader.coalesced);
    }
    if loader.dropped_low > 0 {
        dropped_low_counter().increment_by(loader.dropped_low);
    }
}

/// Visible-row request: cache hit applies now; miss applies the neutral
/// packaged/system image when a worker already resolved it, else the
/// transparent empty placeholder, and queues one HIGH worker request.
/// Miss order: last-valid (caller cache) -> neutral packaged/system ->
/// empty/transparent. Generic app misses never show brand artwork: the
/// worker drops brand fallback rasters to None, and the caller keeps the
/// last valid image or the transparent placeholder. Never flame-red, never
/// retains a stale icon.
fn schedule_visible(
    document: &mut impl UiDocumentAccess,
    icon_id: &str,
    loader: &mut Option<&mut IconLoader>,
    target: IconTarget,
    name: &str,
    w: u32,
    h: u32,
) -> Result<(), String> {
    if let Some(hit) = cached_image(name, w, h) {
        document.image_rgb8(icon_id, hit)?;
        // The node now shows this key: bind it so a late completion for an
        // earlier key cannot overwrite it.
        if let Some(loader) = loader.as_deref_mut() {
            record_binding(target, name, w, h, loader.generation());
        }
        return Ok(());
    }
    // Miss: last-valid is absent (caller cache missed); pending worker
    // results arrive as neutral packaged/system rasters via
    // `note_icon_result`. Until then apply transparent empty so the row
    // never shows a stale icon and never a flame-red square.
    document.image_rgb8(icon_id, empty_image(w, h))?;
    fallback_counter().increment();
    if let Some(loader) = loader.as_deref_mut() {
        let generation = loader.generation();
        loader.request(target, name, w, h);
        request_counter().increment();
        // Bind this node to key + generation + view epoch; only the
        // matching completion may mutate it.
        record_binding(target, name, w, h, generation);
    }
    Ok(())
}

/// Icon lookup name for one task slot. A window with no desktop entry of
/// its own resolves through a desktop entry whose `StartupWMClass` matches
/// its WM_CLASS-derived name; otherwise the state's name is used unchanged.
fn task_icon_name(
    state: &flamewm_shell_core::TaskVisualState,
    applications: &[flamewm_api::applications::DesktopApplication],
) -> String {
    let has_own_entry = applications
        .iter()
        .any(|application| application.id == state.app_id);
    if has_own_entry {
        return state.icon_name.clone();
    }
    desktop_icon_for_wm_class(applications, &state.icon_name)
        .unwrap_or_else(|| state.icon_name.clone())
}

/// Desktop-entry `Icon=` whose `StartupWMClass` equals one lookup candidate
/// of `name` (case-insensitive). Non-empty icons only.
pub fn desktop_icon_for_wm_class(
    applications: &[flamewm_api::applications::DesktopApplication],
    name: &str,
) -> Option<String> {
    crate::icon_loader::icon_name_candidates(name)
        .iter()
        .find_map(|candidate| {
            applications
                .iter()
                .find(|application| {
                    !application.icon_name.is_empty()
                        && application.startup_wm_class.eq_ignore_ascii_case(candidate)
                })
                .map(|application| application.icon_name.clone())
        })
}

/// Empty transparent placeholder applied synchronously on a visible-row
/// cache miss and hidden-slot initial state. The worker result overwrites
/// it via `note_icon_result` when ready (neutral packaged/system raster,
/// never flame-red, never brand). This is the single placeholder system:
/// no second placeholder path exists.
fn empty_image(w: u32, h: u32) -> RuntimeImage {
    let dim_w = w.max(1);
    let dim_h = h.max(1);
    RuntimeImage {
        source: "flamewm-empty".to_owned(),
        width: dim_w,
        height: dim_h,
        pixels: vec![0; dim_w as usize * dim_h as usize * 4],
    }
}

/// Panel projection without any synchronous icon resolve: task labels,
/// visibility, backgrounds, workspaces, clock, and tray visibility project
/// immediately; icons apply only when cached, otherwise a request is queued.
/// Fine-grained entry points (`project_task_slots_async`,
/// `project_pager_async`, `project_status_tray_async`,
/// `project_panel_geometry`) let each changed domain reproject only the
/// nodes it owns; `project_panel_async` is the full-panel fallback for
/// cold start / snapshot replace.
pub fn project_panel_async(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    mut loader: Option<&mut IconLoader>,
) -> Result<(), String> {
    project_panel_scoped(
        document,
        snapshot,
        loader.as_deref_mut(),
        &PanelScope::full(),
    )
}

/// Subset of panel nodes a changed domain may reproject. Windows owns
/// task slots only, Workspaces owns the pager only, Panels owns geometry/
/// layout + task-slot/pager ownership counts, System owns the status tray
/// visibility only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelScope {
    pub task_slots: bool,
    pub pager: bool,
    pub tray: bool,
    pub geometry: bool,
}

impl PanelScope {
    #[must_use]
    pub fn full() -> Self {
        Self {
            task_slots: true,
            pager: true,
            tray: true,
            geometry: true,
        }
    }

    #[must_use]
    pub fn for_changed(changed: crate::runtime::ChangedDomains) -> Self {
        Self {
            task_slots: changed.windows,
            pager: changed.workspaces,
            tray: changed.system,
            geometry: changed.panels,
        }
    }

    #[must_use]
    pub fn none() -> Self {
        Self {
            task_slots: false,
            pager: false,
            tray: false,
            geometry: false,
        }
    }
}

/// Startup task-icon prime: whether the one-shot task-slot projection may run.
/// It needs live task states to request icons for and a loader to request on.
pub fn task_icon_prime_should_run(
    armed: bool,
    task_state_count: usize,
    loader_present: bool,
) -> bool {
    armed && task_state_count > 0 && loader_present
}

/// Startup task-icon prime: whether the prime stays armed after a run.
/// Disarms only once a run with task states, a loader, and a successful
/// projection has submitted icon requests; an empty snapshot keeps it armed.
pub fn task_icon_prime_still_armed(
    armed: bool,
    task_state_count: usize,
    loader_present: bool,
    projection_ok: bool,
) -> bool {
    armed && !(task_state_count > 0 && loader_present && projection_ok)
}

/// Domain-scoped panel dispatcher for the shell loop (`refresh_dynamic`):
/// WindowsChanged -> task slots only, WorkspacesChanged -> pager only,
/// PanelsChanged -> geometry only, SystemChanged -> tray only. Combined
/// flags compose (e.g. panels+windows projects geometry+slots, never the
/// tray). Empty scope is a no-op returning `Ok`. Clock projects only on
/// the full scope (cold start / snapshot replace via
/// `project_panel_async`); per-tick clock uses `project_clock_at`.
/// Focus-only window updates therefore reproject 8 task slots, not the
/// full panel.
pub fn project_panel_for_changed(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    loader: Option<&mut IconLoader>,
    changed: crate::runtime::ChangedDomains,
) -> Result<(), String> {
    let scope = PanelScope::for_changed(changed);
    if scope == PanelScope::none() {
        return Ok(());
    }
    project_panel_scoped(document, snapshot, loader, &scope)
}

/// WindowsChanged -> task slots only. Resolves task visual states (cheap
/// in-memory join, no fetch) and projects labels/icon/visibility/background
/// for the 8 task slots. Pager, tray, and geometry nodes are untouched, so
/// a focus-only window update never reprojects the full panel.
pub fn project_task_slots_async(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    mut loader: Option<&mut IconLoader>,
) -> Result<(), String> {
    let _span = crate::runtime::shell_span("shell.tasks.project").start();
    bump_task_view_epoch();
    let states = flamewm_shell_core::task_visual_states(
        &snapshot.panels,
        &snapshot.windows,
        &snapshot.applications,
    );
    // Task-slot nodes only: labels, icons, line visibility, backgrounds,
    // slot visibility. Pager/tray/geometry untouched.
    for index in 0..TASK_SLOT_COUNT {
        let slot_id = format!("task-slot-{}", index + 1);
        let icon_id = format!("task-slot-{}-icon", index + 1);
        let label_id = format!("task-slot-{}-label", index + 1);
        let line_id = format!("task-slot-{}-line", index + 1);
        let state = states.get(index);
        if let Some(state) = state {
            let label = snapshot
                .applications
                .iter()
                .find(|application| application.id == state.app_id)
                .map(|application| application.name.as_str())
                .filter(|name| !name.is_empty())
                .unwrap_or(state.app_id.as_str());
            document.text(&label_id, label.to_owned())?;
            schedule_visible(
                document,
                &icon_id,
                &mut loader.as_deref_mut(),
                IconTarget::TaskSlot(index as u8),
                &task_icon_name(state, &snapshot.applications),
                25,
                25,
            )?;
            document.visible(&line_id, state.running)?;
            if state.focused {
                document.background_clear(&slot_id)?;
                document.background(&line_id, crate::projection::task_accent())?;
            } else if state.minimized {
                document.background_clear(&slot_id)?;
                document.background(
                    &line_id,
                    UiColor {
                        r: 135,
                        g: 145,
                        b: 155,
                        a: 255,
                    },
                )?;
            } else {
                document.background_clear(&slot_id)?;
                document.background(
                    &line_id,
                    UiColor {
                        r: 135,
                        g: 145,
                        b: 155,
                        a: 255,
                    },
                )?;
            }
        } else {
            document.text(&label_id, String::new())?;
            document.visible(&line_id, false)?;
            document.background_clear(&slot_id)?;
        }
        document.visible(&slot_id, state.is_some())?;
    }
    Ok(())
}

/// WorkspacesChanged -> pager only. Projects the visible workspace page
/// (labels, visibility, active background) and touches no task, tray, or
/// geometry node.
pub fn project_pager_async(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    let _span = crate::runtime::shell_span("shell.pager.project").start();
    let workspaces = project_workspaces(snapshot.workspaces.as_ref());
    for index in 0..PAGE_SIZE {
        document.visible(&format!("workspace-{}", index + 1), false)?;
    }
    let page: Vec<&crate::taskbar::workspaces::WorkspaceView> = visible_page(&workspaces);
    for workspace in page {
        let Some((row, column)) = slot_position(workspace.index, &workspaces) else {
            continue;
        };
        let slot = row * 2 + column;
        if slot >= PAGE_SIZE {
            continue;
        }
        let button_id = format!("workspace-{}", slot + 1);
        document.text(&button_id, workspace.label.clone())?;
        document.visible(&button_id, true)?;
        if workspace.active {
            document.background(
                &button_id,
                UiColor {
                    r: 239,
                    g: 64,
                    b: 72,
                    a: 56,
                },
            )?;
        } else {
            document.background(
                &button_id,
                UiColor {
                    r: 36,
                    g: 39,
                    b: 42,
                    a: 255,
                },
            )?;
        }
    }
    Ok(())
}

/// SystemChanged -> status tray visibility only. The open popup content
/// projects on its own open path (`refresh_dynamic` reprojects media when
/// open); the panel itself only flips tray visibility nodes.
pub fn project_status_tray_async(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    let status = crate::taskbar::status::project(&snapshot.system).views;
    document.visible(
        "tray-media",
        status.media.visible
            && matches!(
                snapshot.system.media.playback,
                flamewm_api::system::PlaybackState::Playing
                    | flamewm_api::system::PlaybackState::Paused
            ),
    )?;
    document.visible(
        "tray-play-icon",
        matches!(
            status.media.play_pause_icon,
            Some(flamewm_ui_core::IconRole::MediaPlay)
        ),
    )?;
    document.visible(
        "tray-pause-icon",
        matches!(
            status.media.play_pause_icon,
            Some(flamewm_ui_core::IconRole::MediaPause)
        ),
    )?;
    document.visible("tray-volume", status.audio.visible)?;
    document.visible("tray-volume-icon", !status.audio.muted)?;
    document.visible("tray-volume-muted-icon", status.audio.muted)?;
    document.visible("tray-network", status.network.visible)?;
    Ok(())
}

/// PanelsChanged -> geometry/layout ownership counts only (slot/pager
/// visibility for added/removed entries). Content styling stays with the
/// task/pager projectors. Currently geometry is node-identical to the
/// visibility pass, so this reuses the slot/pager visibility tails without
/// labels, icons, or backgrounds.
pub fn project_panel_geometry(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    crate::projection::project_taskbar_height(document, snapshot)?;
    let states = flamewm_shell_core::task_visual_states(
        &snapshot.panels,
        &snapshot.windows,
        &snapshot.applications,
    );
    for index in 0..TASK_SLOT_COUNT {
        document.visible(
            &format!("task-slot-{}", index + 1),
            states.get(index).is_some(),
        )?;
    }
    let workspaces = project_workspaces(snapshot.workspaces.as_ref());
    let page: Vec<&crate::taskbar::workspaces::WorkspaceView> = visible_page(&workspaces);
    let mut visible = [false; PAGE_SIZE];
    for workspace in &page {
        if let Some((row, column)) = slot_position(workspace.index, &workspaces) {
            let slot = row * 2 + column;
            if slot < PAGE_SIZE {
                visible[slot] = true;
            }
        }
    }
    for (index, shown) in visible.iter().enumerate() {
        document.visible(&format!("workspace-{}", index + 1), *shown)?;
    }
    Ok(())
}

fn project_panel_scoped(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    mut loader: Option<&mut IconLoader>,
    scope: &PanelScope,
) -> Result<(), String> {
    if scope.task_slots {
        project_task_slots_async(document, snapshot, loader.as_deref_mut())?;
        // Task slots consumed the loader borrow; re-borrow is not needed
        // because no other scoped pass uses icons. Pass None onward.
        return project_panel_scoped_tail(document, snapshot, scope);
    }
    project_panel_scoped_tail(document, snapshot, scope)
}

fn project_panel_scoped_tail(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    scope: &PanelScope,
) -> Result<(), String> {
    if scope.geometry {
        project_panel_geometry(document, snapshot)?;
    }
    if scope.pager {
        project_pager_async(document, snapshot)?;
    }
    if scope.tray {
        project_status_tray_async(document, snapshot)?;
    }
    if scope.full_clock() {
        crate::projection::project_clock(document)?;
    }
    Ok(())
}

impl PanelScope {
    fn full_clock(&self) -> bool {
        self.task_slots && self.pager && self.tray && self.geometry
    }
}

/// LOW prewarm for rows just outside the visible window. Fire-and-forget:
/// runs after first show, never delays startup, never touches the document.
pub fn prewarm_start_tail(
    model: &StartModel,
    category: StartCategory,
    query: &str,
    mut loader: Option<&mut IconLoader>,
) {
    let Some(loader) = loader.as_deref_mut() else {
        return;
    };
    let applications = crate::projection::start_visible_results(model, category, query);
    let (_, last) = flamewm_shell_core::status::visible_window(applications.len(), 36, 320, 0);
    for application in applications.iter().skip(last).take(8) {
        if cached_image(&application.icon_name, 20, 20).is_some() {
            continue;
        }
        loader.request_low(
            IconTarget::StartSlot(START_APP_SLOT_COUNT as u8),
            &application.icon_name,
            20,
            20,
        );
    }
    prewarm_counter().increment();
}

/// Start unified projection without synchronous resolve: the single
/// flamewm-start.rwr document carries categories, search, app rows, and
/// power rows. Labels/visibility immediate; icons cached-or-Flame-fallback
/// with queued HIGH worker requests. Never resolves on the caller thread.
pub fn project_start_submenu_async(
    document: &mut impl UiDocumentAccess,
    model: &StartModel,
    category: StartCategory,
    query: &str,
    session: &SessionCapabilities,
    mut loader: Option<&mut IconLoader>,
) -> Result<(), String> {
    project_start_unified(
        document,
        model,
        category,
        query,
        session,
        loader.as_deref_mut(),
    )
}

/// Unified Start document projection (J04/J07): categories + search +
/// app rows + power rows in ONE document on the single Start surface.
pub fn project_start_unified(
    document: &mut impl UiDocumentAccess,
    model: &StartModel,
    category: StartCategory,
    query: &str,
    session: &SessionCapabilities,
    mut loader: Option<&mut IconLoader>,
) -> Result<(), String> {
    bump_start_view_epoch(); // Canonical indexed path: borrowed query view from the precomputed
                             // index, then the single presentation-bucket filter. No clone, no
                             // set_query-on-temporary. (Shell StartCategory is the presentation
                             // seam; core filter_view takes the core category type.)
    let applications = crate::projection::start_visible_results(model, category, query);
    document.visible("start-applications", !applications.is_empty())?;
    let (first, last) = flamewm_shell_core::status::visible_window(applications.len(), 36, 320, 0);
    let window = applications
        .get(first..last.min(applications.len()))
        .unwrap_or_default();
    for index in 0..START_APP_SLOT_COUNT {
        let slot = index + 1;
        let button_id = format!("start-app-slot-{slot}");
        let label_id = format!("start-app-slot-{slot}-label");
        let icon_id = format!("start-app-slot-{slot}-icon");
        if let Some(application) = window.get(index) {
            document.text(&label_id, application.name.clone())?;
            document.visible(&button_id, true)?;
            document.visible(&icon_id, true)?;
            schedule_visible(
                document,
                &icon_id,
                &mut loader.as_deref_mut(),
                IconTarget::StartSlot(index as u8),
                &application.icon_name,
                20,
                20,
            )?;
        } else {
            document.text(&label_id, String::new())?;
            document.visible(&button_id, false)?;
            document.visible(&icon_id, false)?;
        }
    }
    let show_session = query.trim().is_empty()
        && category == StartCategory::Power
        && crate::projection::session_available(session);
    document.visible("start-group-power", show_session)?;
    document.visible("start-session-lock", show_session && session.lock)?;
    document.visible("start-session-logout", show_session && session.logout)?;
    document.visible("start-session-suspend", show_session && session.suspend)?;
    document.visible("start-session-reboot", show_session && session.reboot)?;
    document.visible("start-session-shutdown", show_session && session.shutdown)?;
    // Category rail + search label live in the same unified document, so
    // one intrinsic union covers the whole Start surface.
    crate::projection::project_start(document, model, category, query, session)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::applications::DesktopApplication;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeDoc {
        texts: HashMap<String, String>,
        images: Vec<String>,
        visible: HashMap<String, bool>,
    }

    impl UiDocumentAccess for FakeDoc {
        fn text(&mut self, id: &str, text: String) -> Result<(), String> {
            self.texts.insert(id.to_owned(), text);
            Ok(())
        }
        fn image_rgb8(&mut self, id: &str, _image: RuntimeImage) -> Result<(), String> {
            self.images.push(id.to_owned());
            Ok(())
        }
        fn image_rgba8(&mut self, id: &str, _image: RuntimeImage) -> Result<(), String> {
            self.images.push(id.to_owned());
            Ok(())
        }
        fn visible(&mut self, id: &str, v: bool) -> Result<(), String> {
            self.visible.insert(id.to_owned(), v);
            Ok(())
        }
        fn toggle(&mut self, _: &str, _: &str, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn position(&mut self, _: &str, _: f32, _: f32) -> Result<(), String> {
            Ok(())
        }
        fn size(&mut self, _: &str, _: f32, _: f32) -> Result<(), String> {
            Ok(())
        }
        fn background(&mut self, _: &str, _: UiColor) -> Result<(), String> {
            Ok(())
        }
        fn border(&mut self, _: &str, _: UiColor) -> Result<(), String> {
            Ok(())
        }
        fn background_clear(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn foreground(&mut self, _: &str, _: UiColor) -> Result<(), String> {
            Ok(())
        }
        fn foreground_clear(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn border_clear(&mut self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn layer(&mut self, _: &str, _: flamewm_ui_core::style::UiLayer) -> Result<(), String> {
            Ok(())
        }
        fn overflow(
            &mut self,
            _: &str,
            _: flamewm_ui_x11::Overflow,
            _: flamewm_ui_x11::Overflow,
        ) -> Result<(), String> {
            Ok(())
        }
        fn font_size(&mut self, _: &str, _: f32) -> Result<(), String> {
            Ok(())
        }
        fn font_weight(&mut self, _: &str, _: u16) -> Result<(), String> {
            Ok(())
        }
    }

    fn test_image(source: &str) -> RuntimeImage {
        RuntimeImage {
            source: source.to_owned(),
            width: 1,
            height: 1,
            pixels: vec![0, 0, 0, 255],
        }
    }

    fn icon_image(source: &str) -> RuntimeImage {
        RuntimeImage {
            source: source.to_owned(),
            width: 20,
            height: 20,
            pixels: vec![1; 20 * 20 * 4],
        }
    }

    fn icon_test_lock() -> &'static std::sync::Mutex<()> {
        static M: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        M.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[test]
    fn task_icon_prime_stays_armed_until_a_request_round_runs() {
        // Empty snapshot at first tick: nothing to request, stays armed.
        assert!(!task_icon_prime_should_run(true, 0, true));
        assert!(task_icon_prime_still_armed(true, 0, true, true));
        // No loader yet: cannot request, stays armed.
        assert!(!task_icon_prime_should_run(true, 2, false));
        assert!(task_icon_prime_still_armed(true, 2, false, true));
        // Failed projection: stays armed for a retry.
        assert!(task_icon_prime_still_armed(true, 2, true, false));
        // Task states plus loader plus successful projection: disarms.
        assert!(task_icon_prime_should_run(true, 2, true));
        assert!(!task_icon_prime_still_armed(true, 2, true, true));
        // Once disarmed it never re-runs.
        assert!(!task_icon_prime_should_run(false, 2, true));
        assert!(!task_icon_prime_still_armed(false, 2, true, true));
    }

    #[test]
    fn icon_binding_match_mutates_exact_node_only() {
        use crate::icon_loader::IconTarget;
        let _guard = icon_test_lock().lock();
        test_reset_icon_bindings();
        let mut doc = FakeDoc::default();
        test_record_binding(IconTarget::StartSlot(0), "icon-a", 20, 20, 7);
        let applied = apply_icon_result(
            &mut doc,
            IconTarget::StartSlot(0),
            "icon-a",
            20,
            20,
            7,
            icon_image("a"),
        )
        .expect("apply ok");
        assert!(applied, "live binding must mutate");
        assert_eq!(doc.images, vec!["start-app-slot-1-icon".to_owned()]);
    }

    #[test]
    fn icon_binding_stale_on_epoch_bump_touches_nothing() {
        use crate::icon_loader::IconTarget;
        let _guard = icon_test_lock().lock();
        test_reset_icon_bindings();
        let mut doc = FakeDoc::default();
        test_record_binding(IconTarget::StartSlot(0), "icon-a", 20, 20, 7);
        bump_start_view_epoch();
        let applied = apply_icon_result(
            &mut doc,
            IconTarget::StartSlot(0),
            "icon-a",
            20,
            20,
            7,
            icon_image("a"),
        )
        .expect("stale check ok");
        assert!(!applied, "epoch bump must stale the completion");
        assert!(doc.images.is_empty(), "stale drop touches no node");
    }

    #[test]
    fn icon_binding_stale_on_key_or_generation_mismatch() {
        use crate::icon_loader::IconTarget;
        let _guard = icon_test_lock().lock();
        test_reset_icon_bindings();
        let mut doc = FakeDoc::default();
        test_record_binding(IconTarget::TaskSlot(1), "icon-a", 25, 25, 3);
        let icon25 = |source: &str| RuntimeImage {
            source: source.to_owned(),
            width: 25,
            height: 25,
            pixels: vec![1; 25 * 25 * 4],
        };
        let wrong_name = apply_icon_result(
            &mut doc,
            IconTarget::TaskSlot(1),
            "icon-b",
            25,
            25,
            3,
            icon25("b"),
        )
        .expect("stale check ok");
        assert!(!wrong_name, "key mismatch must stale");
        let wrong_gen = apply_icon_result(
            &mut doc,
            IconTarget::TaskSlot(1),
            "icon-a",
            25,
            25,
            4,
            icon25("b"),
        )
        .expect("stale check ok");
        assert!(!wrong_gen, "generation mismatch must stale");
        assert!(doc.images.is_empty(), "stale drops touch no node");
    }

    #[test]
    fn icon_ready_after_epoch_bump_repaints_bound_node() {
        use crate::icon_loader::IconTarget;
        let _guard = icon_test_lock().lock();
        test_reset_icon_bindings();
        let mut doc = FakeDoc::default();
        test_record_binding(IconTarget::TaskSlot(1), "icon-a", 25, 25, 3);
        // A projection bumps the task epoch without re-binding this slot.
        bump_task_view_epoch();
        note_icon_result("icon-a", 25, 25, icon_image("a"));
        let completed = apply_icon_result(
            &mut doc,
            IconTarget::TaskSlot(1),
            "icon-a",
            25,
            25,
            3,
            icon_image("a"),
        )
        .expect("stale check ok");
        assert!(!completed, "epoch bump stales the late completion");
        assert!(doc.images.is_empty());
        let repainted = apply_cached_binding(&mut doc, IconTarget::TaskSlot(1))
            .expect("cached repaint ok");
        assert!(repainted, "cached raster for the bound key must repaint");
        assert_eq!(doc.images, vec!["task-slot-2-icon".to_owned()]);
    }

    #[test]
    fn wm_class_matches_desktop_entry_icon_before_fallback() {
        use flamewm_api::applications::DesktopApplication;
        let app = DesktopApplication {
            id: flamewm_api::DesktopAppId::new("xterm-desktop"),
            name: "XTerm".to_owned(),
            generic_name: String::new(),
            comment: String::new(),
            startup_wm_class: "XTerm".to_owned(),
            argv: Vec::new(),
            keywords: Vec::new(),
            icon_name: "utilities-terminal".to_owned(),
            categories: Vec::new(),
        };
        assert_eq!(
            desktop_icon_for_wm_class(std::slice::from_ref(&app), "mini.xterm").as_deref(),
            Some("utilities-terminal")
        );
        assert_eq!(desktop_icon_for_wm_class(std::slice::from_ref(&app), "gimp"), None);
    }

    #[test]
    fn task_slot_node_id_uses_source_ids() {
        use crate::icon_loader::IconTarget;
        assert_eq!(
            icon_node_id_for_target(IconTarget::TaskSlot(2)),
            "task-slot-3-icon"
        );
        assert_eq!(
            icon_node_id_for_target(IconTarget::StartSlot(0)),
            "start-app-slot-1-icon"
        );
    }

    #[test]
    fn windows_scope_projects_task_slots_only() {
        let _guard = icon_test_lock().lock();
        use crate::runtime::ChangedDomains;
        let snapshot = ShellSnapshot::default();
        let changed = ChangedDomains {
            windows: true,
            ..ChangedDomains::default()
        };
        let scope = PanelScope::for_changed(changed);
        assert_eq!(
            scope,
            PanelScope {
                task_slots: true,
                pager: false,
                tray: false,
                geometry: false,
            }
        );
        let mut doc = FakeDoc::default();
        project_panel_for_changed(&mut doc, &snapshot, None, changed).expect("projects");
        for index in 0..8 {
            assert!(
                doc.visible
                    .contains_key(&format!("task-slot-{}", index + 1)),
                "task slot projected",
            );
        }
        assert!(
            !doc.visible.contains_key("workspace-1"),
            "pager untouched on windows-only change",
        );
        assert!(
            !doc.visible.contains_key("tray-volume"),
            "tray untouched on windows-only change",
        );
    }

    #[test]
    fn workspaces_scope_projects_pager_only() {
        let _guard = icon_test_lock().lock();
        use crate::runtime::ChangedDomains;
        let snapshot = ShellSnapshot::default();
        let changed = ChangedDomains {
            workspaces: true,
            ..ChangedDomains::default()
        };
        let mut doc = FakeDoc::default();
        project_panel_for_changed(&mut doc, &snapshot, None, changed).expect("projects");
        assert!(doc.visible.contains_key("workspace-1"), "pager projected");
        assert!(
            !doc.visible.contains_key("task-slot-1"),
            "task slots untouched on workspaces-only change",
        );
        assert!(
            !doc.visible.contains_key("tray-volume"),
            "tray untouched on workspaces-only change",
        );
    }

    #[test]
    fn system_scope_projects_tray_only() {
        let _guard = icon_test_lock().lock();
        use crate::runtime::ChangedDomains;
        let snapshot = ShellSnapshot::default();
        let changed = ChangedDomains {
            system: true,
            ..ChangedDomains::default()
        };
        let mut doc = FakeDoc::default();
        project_panel_for_changed(&mut doc, &snapshot, None, changed).expect("projects");
        assert!(doc.visible.contains_key("tray-volume"), "tray projected");
        assert!(
            !doc.visible.contains_key("task-slot-1"),
            "task slots untouched on system-only change",
        );
        assert!(
            !doc.visible.contains_key("workspace-1"),
            "pager untouched on system-only change",
        );
    }

    #[test]
    fn panels_scope_projects_geometry_only() {
        let _guard = icon_test_lock().lock();
        use crate::runtime::ChangedDomains;
        let snapshot = ShellSnapshot::default();
        let changed = ChangedDomains {
            panels: true,
            ..ChangedDomains::default()
        };
        let mut doc = FakeDoc::default();
        project_panel_for_changed(&mut doc, &snapshot, None, changed).expect("projects");
        assert!(
            doc.visible.contains_key("task-slot-1"),
            "geometry owns slot count"
        );
        assert!(
            doc.visible.contains_key("workspace-1"),
            "geometry owns pager count"
        );
        assert!(doc.texts.is_empty(), "geometry writes no labels");
        assert!(doc.images.is_empty(), "geometry writes no icons");
    }

    #[test]
    fn empty_scope_is_noop() {
        let _guard = icon_test_lock().lock();
        use crate::runtime::ChangedDomains;
        let snapshot = ShellSnapshot::default();
        let mut doc = FakeDoc::default();
        project_panel_for_changed(&mut doc, &snapshot, None, ChangedDomains::default())
            .expect("noop ok");
        assert!(doc.visible.is_empty(), "empty scope touches nothing");
    }

    #[test]
    fn icon_cache_evicts_least_recently_used_entry_at_budget() {
        let mut cache = IconCache::new(2);
        let first = ("first".to_owned(), 1, 1);
        let second = ("second".to_owned(), 1, 1);
        let third = ("third".to_owned(), 1, 1);
        cache.insert(first.clone(), test_image("first"));
        cache.insert(second.clone(), test_image("second"));
        assert_eq!(cache.get(&first).expect("first cache hit").source, "first");
        cache.insert(third.clone(), test_image("third"));

        assert!(cache.get(&second).is_none(), "least-recent entry evicted");
        assert_eq!(cache.get(&first).expect("first retained").source, "first");
        assert_eq!(cache.get(&third).expect("third retained").source, "third");
    }

    #[test]
    fn async_panel_projects_labels_without_sync_resolve() {
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot::default();
        let mut doc = FakeDoc::default();
        project_panel_async(&mut doc, &snapshot, None).expect("projects");
        // No loader/cache: rows apply the Flame fallback synchronously, so
        // images are present but no worker request could be queued.
        assert!(doc.images.is_empty(), "no task states, no icon nodes");
        for index in 0..8 {
            let id = format!("task-slot-{}", index + 1);
            assert!(doc.visible.contains_key(&id), "slot {id} projected");
        }
    }

    #[test]
    fn visible_row_miss_applies_fallback_now() {
        let _guard = icon_test_lock().lock();
        let model = StartModel::new(vec![flamewm_api::applications::DesktopApplication {
            id: flamewm_api::DesktopAppId::new("org.example.x"),
            name: "X".to_owned(),
            generic_name: String::new(),
            comment: String::new(),
            startup_wm_class: String::new(),
            argv: Vec::new(),
            keywords: Vec::new(),
            icon_name: "missing-icon".to_owned(),
            categories: Vec::new(),
        }]);
        let session = SessionCapabilities::default();
        let mut doc = FakeDoc::default();
        project_start_submenu_async(&mut doc, &model, StartCategory::All, "", &session, None)
            .expect("projects");
        assert!(
            doc.images.contains(&"start-app-slot-1-icon".to_owned()),
            "miss applies Flame fallback now, never retains old icon"
        );
    }

    #[test]
    fn async_start_submenu_hides_empty_slots() {
        let _guard = icon_test_lock().lock();
        let model = StartModel::new(Vec::new());
        let session = SessionCapabilities::default();
        let mut doc = FakeDoc::default();
        project_start_submenu_async(&mut doc, &model, StartCategory::All, "", &session, None)
            .expect("projects");
        assert_eq!(doc.visible.get("start-app-slot-1"), Some(&false));
    }

    fn start_test_app(id: &str, name: &str, category: &str) -> DesktopApplication {
        DesktopApplication {
            id: flamewm_api::DesktopAppId::new(id),
            name: name.to_owned(),
            generic_name: String::new(),
            comment: String::new(),
            startup_wm_class: String::new(),
            argv: Vec::new(),
            keywords: Vec::new(),
            icon_name: "missing-icon".to_owned(),
            categories: vec![category.to_owned()],
        }
    }

    #[test]
    fn search_under_category_finds_other_categories_and_hides_rail() {
        let _guard = icon_test_lock().lock();
        let model = StartModel::new(vec![
            start_test_app("org.example.calc", "Calculator", "Utility"),
            start_test_app("org.example.chess", "Chess", "Game"),
        ]);
        let session = SessionCapabilities::default();
        let mut doc = FakeDoc::default();
        project_start_unified(
            &mut doc,
            &model,
            StartCategory::Games,
            "calc",
            &session,
            None,
        )
        .expect("projects");
        assert_eq!(
            doc.texts.get("start-app-slot-1-label").map(String::as_str),
            Some("Calculator"),
            "search ignores the selected category"
        );
        assert_eq!(doc.visible.get("start-app-slot-2"), Some(&false));
        assert_eq!(doc.visible.get("start-categories"), Some(&false));
        assert_eq!(doc.visible.get("start-group-power"), Some(&false));
    }

    #[test]
    fn start_click_slot_maps_to_the_projected_row() {
        let _guard = icon_test_lock().lock();
        let model = StartModel::new(vec![
            start_test_app("org.example.calc", "Calculator", "Utility"),
            start_test_app("org.example.chess", "Chess", "Game"),
        ]);
        let session = SessionCapabilities::default();
        let mut doc = FakeDoc::default();
        project_start_unified(
            &mut doc,
            &model,
            StartCategory::Games,
            "calc",
            &session,
            None,
        )
        .expect("projects");
        let projected = doc.texts.get("start-app-slot-1-label").cloned();
        assert_eq!(
            crate::projection::start_application_name_for_slot(
                &model,
                StartCategory::Games,
                "calc",
                0
            ),
            projected,
            "click on slot 1 launches the row that was projected"
        );
    }

    #[test]
    fn empty_query_restores_selected_category_view() {
        let _guard = icon_test_lock().lock();
        let model = StartModel::new(vec![
            start_test_app("org.example.calc", "Calculator", "Utility"),
            start_test_app("org.example.chess", "Chess", "Game"),
        ]);
        let session = SessionCapabilities::default();
        let mut doc = FakeDoc::default();
        project_start_unified(&mut doc, &model, StartCategory::Games, "", &session, None)
            .expect("projects");
        assert_eq!(
            doc.texts.get("start-app-slot-1-label").map(String::as_str),
            Some("Chess")
        );
        assert_eq!(doc.visible.get("start-app-slot-2"), Some(&false));
        assert_eq!(doc.visible.get("start-categories"), Some(&true));
    }

    #[test]
    fn start_view_note_gates_redundant_hover() {
        let mut note = StartViewNote::default();
        assert!(note.note_start_view(StartCategory::All, ""));
        assert!(!note.note_start_view(StartCategory::All, ""));
        assert!(note.note_start_view(StartCategory::Games, ""));
    }

    #[test]
    fn no_sync_resolve_calls_in_async_path() {
        let src = include_str!("async_projection.rs");
        // Import of the loader handle is fine; synchronous resolve calls are not.
        let probe = ["prepare", "_", "name", "("].concat();
        let env = ["from", "_", "environment"].concat();
        assert!(!src.contains(&probe), "async path must not resolve sync");
        assert!(
            !src.contains(&env),
            "async path must not construct resolver"
        );
    }

    #[test]
    fn popup_roles_are_distinct() {
        let roles = [
            flamewm_ui_x11::SurfaceRole::PopupMenu,
            flamewm_ui_x11::SurfaceRole::DropdownMenu,
        ];
        assert_ne!(roles[0], roles[1]);
        let _ = flamewm_ui_x11::SurfaceRole::Dock;
    }

    #[test]
    fn eight_slots_project_distinct_ids() {
        let mut ids = std::collections::HashSet::new();
        for index in 0..START_APP_SLOT_COUNT {
            ids.insert(format!("start-app-slot-{}", index + 1));
        }
        assert_eq!(ids.len(), 8);
    }
}

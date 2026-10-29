//! Async projection: labels/visibility immediate, icons cached-or-placeholder.
//! Never resolves icons on the caller thread. Schedules worker requests only.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;

use flamewm_api::session::SessionCapabilities;
use flamewm_api::DesktopAppId;
use flamewm_shell_core::StartModel;
use flamewm_ui_x11::{RuntimeImage, UiColor, UiDocumentAccess};

use crate::icon_loader::{IconLoader, IconTarget};
use crate::projection::{PanelView, TaskbarItems};
use crate::start::StartCategory;
use crate::ShellSnapshot;

pub const TASK_SLOT_COUNT: usize = 8;
pub const START_APP_SLOT_COUNT: usize = 8;

// Retain a large working set across taskbar, Start, and launcher views.
// The icon resolver has already rasterized every image to its requested
// physical display size; no extra downscale or lossy preview is necessary.
const ICON_CACHE_ENTRY_BUDGET: usize = 2048;
/// Maximum retained RGBA8 icon pixel payload (32 MiB, not a fixed allocation).
const ICON_CACHE_BYTE_BUDGET: usize = 32 * 1024 * 1024;

pub type IconCacheKey = (String, u32, u32);

/// Decoded icon raster. The pixel buffer is shared: the cache and every
/// document showing the icon hold one allocation, so a hit clones an `Arc`.
#[derive(Clone)]
pub struct IconRaster {
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<Vec<u8>>,
}

impl From<RuntimeImage> for IconRaster {
    fn from(image: RuntimeImage) -> Self {
        Self {
            source: image.source,
            width: image.width,
            height: image.height,
            pixels: Arc::new(image.pixels),
        }
    }
}

type SizeKey = (u32, u32);

struct IconCacheEntry {
    raster: IconRaster,
    /// Stamp of the most recent insert/get; matches this entry's newest `order` record.
    stamp: u64,
}

/// LRU via lazy recency records: `order` holds `(name, size, stamp)` triples
/// pushed on every insert/get. A record is live only if its stamp equals the
/// entry's current stamp; stale records are skipped on eviction and compacted
/// away once they outnumber live entries, keeping every operation amortized O(1).
/// Entries nest by name so lookups borrow `&str` without allocating.
struct IconCache {
    entry_budget: usize,
    byte_budget: usize,
    bytes: usize,
    len: usize,
    clock: u64,
    entries: HashMap<Arc<str>, HashMap<SizeKey, IconCacheEntry>>,
    order: VecDeque<(Arc<str>, SizeKey, u64)>,
}

impl IconCache {
    fn new(entry_budget: usize) -> Self {
        Self::with_budgets(entry_budget, ICON_CACHE_BYTE_BUDGET)
    }

    fn with_budgets(entry_budget: usize, byte_budget: usize) -> Self {
        Self {
            entry_budget: entry_budget.max(1),
            byte_budget,
            bytes: 0,
            len: 0,
            clock: 0,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn next_stamp(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
        self.len = 0;
    }

    fn is_live(&self, name: &str, size: SizeKey, stamp: u64) -> bool {
        self.entries
            .get(name)
            .and_then(|sizes| sizes.get(&size))
            .is_some_and(|entry| entry.stamp == stamp)
    }

    fn remove(&mut self, name: &str, size: SizeKey) {
        let Some(sizes) = self.entries.get_mut(name) else {
            return;
        };
        if let Some(entry) = sizes.remove(&size) {
            self.bytes -= entry.raster.pixels.len();
            self.len -= 1;
        }
        if sizes.is_empty() {
            self.entries.remove(name);
        }
    }

    /// Membership test that does not touch LRU recency.
    #[cfg(test)]
    fn contains(&self, name: &str, size: SizeKey) -> bool {
        self.entries
            .get(name)
            .is_some_and(|sizes| sizes.contains_key(&size))
    }

    fn get(&mut self, name: &str, size: SizeKey) -> Option<IconRaster> {
        let stamp = self.next_stamp();
        let key = self.entries.get_key_value(name)?.0.clone();
        let entry = self.entries.get_mut(name)?.get_mut(&size)?;
        entry.stamp = stamp;
        let raster = entry.raster.clone();
        self.order.push_back((key, size, stamp));
        self.compact_order();
        Some(raster)
    }

    fn insert(&mut self, name: &str, size: SizeKey, raster: IconRaster) {
        self.remove(name, size);
        let bytes = raster.pixels.len();
        if bytes > self.byte_budget {
            // An oversized raster is never retained; evicting others for it
            // would only churn the cache.
            return;
        }
        while self.len >= self.entry_budget || self.bytes + bytes > self.byte_budget {
            let Some((old_name, old_size, stamp)) = self.order.pop_front() else {
                break;
            };
            if self.is_live(&old_name, old_size, stamp) {
                self.remove(&old_name, old_size);
            }
        }
        let stamp = self.next_stamp();
        // Multiple physical sizes of one icon name share its Arc<str> key.
        // Avoid allocating a duplicate string for each HiDPI variant.
        let key: Arc<str> = self
            .entries
            .get_key_value(name)
            .map_or_else(|| Arc::from(name), |(existing, _)| Arc::clone(existing));
        self.bytes += bytes;
        self.len += 1;
        self.order.push_back((key.clone(), size, stamp));
        self.entries
            .entry(key)
            .or_default()
            .insert(size, IconCacheEntry { raster, stamp });
        self.compact_order();
    }

    /// Drops stale recency records once they dominate, bounding `order` to
    /// roughly twice the live entry count.
    fn compact_order(&mut self) {
        if self.order.len() <= 2 * self.len + 16 {
            return;
        }
        let entries = &self.entries;
        self.order.retain(|(name, size, stamp)| {
            entries
                .get(&**name)
                .and_then(|sizes| sizes.get(size))
                .is_some_and(|entry| entry.stamp == *stamp)
        });
    }
}

fn icon_cache() -> &'static std::sync::Mutex<IconCache> {
    static CACHE: OnceLock<std::sync::Mutex<IconCache>> = OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(IconCache::new(ICON_CACHE_ENTRY_BUDGET)))
}

macro_rules! counter {
    ($($name:ident = $id:literal;)*) => {$(
        fn $name() -> &'static flamewm_profiler::CounterPoint {
            static C: OnceLock<flamewm_profiler::CounterPoint> = OnceLock::new();
            C.get_or_init(|| flamewm_profiler::CounterPoint::new($id))
        }
    )*};
}

counter! {
    redundant_hover_counter = "shell.start.redundant_hover";
    request_counter = "shell.icon.request";
    result_counter = "shell.icon.result";
    stale_counter = "shell.icon.stale_drop";
    queue_full_counter = "shell.icon.queue_full";
    high_counter = "shell.icon.high";
    low_counter = "shell.icon.low";
    coalesced_counter = "shell.icon.coalesced";
    dropped_low_counter = "shell.icon.dropped_low";
    fallback_counter = "shell.icon.empty_placeholder";
}

/// Start and task slot ranges; prewarm sentinels past the range never bind a node.
fn target_in_range(target: IconTarget) -> bool {
    match target {
        IconTarget::StartSlot(n) => u32::from(n) < START_APP_SLOT_COUNT as u32,
        IconTarget::LauncherSlot(n) => usize::from(n) < crate::launcher::LAUNCHER_ROWS,
        IconTarget::TaskSlot(n) => u32::from(n) < TASK_SLOT_COUNT as u32,
    }
}

const TASK_LINE_IDLE: UiColor = UiColor {
    r: 135,
    g: 145,
    b: 155,
    a: 255,
};

fn task_states(view: PanelView<'_>) -> Vec<flamewm_shell_core::TaskVisualState> {
    let snapshot = view.snapshot;
    flamewm_shell_core::task_visual_states_on_output_and_workspace(
        &snapshot.panels,
        &snapshot.windows,
        &snapshot.applications,
        view.panel.map(|panel| &panel.output),
        snapshot
            .workspaces
            .as_ref()
            .map(|workspaces| workspaces.active_index),
    )
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

/// Bindings are keyed by target and raster edge: per-output panels share one
/// target id but may request different edges (`HiDPI` mix), and each must keep
/// its own exact-size binding instead of overwriting the other's.
type BindingKey = (IconTarget, u32);

fn bindings() -> &'static std::sync::Mutex<HashMap<BindingKey, IconBinding>> {
    static B: OnceLock<std::sync::Mutex<HashMap<BindingKey, IconBinding>>> = OnceLock::new();
    B.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

static START_EPOCH: AtomicU64 = AtomicU64::new(0);
static TASK_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Bump the start view epoch: category, search query, or model rebuild.
pub fn bump_start_view_epoch() -> u64 {
    START_EPOCH.fetch_add(1, Ordering::SeqCst) + 1
}

/// Bump the task view epoch: task-slot reprojection.
pub fn bump_task_view_epoch() -> u64 {
    TASK_EPOCH.fetch_add(1, Ordering::SeqCst) + 1
}

fn epoch_for_target(target: IconTarget) -> u64 {
    match target {
        IconTarget::StartSlot(_) | IconTarget::LauncherSlot(_) => {
            START_EPOCH.load(Ordering::SeqCst)
        }
        IconTarget::TaskSlot(_) => TASK_EPOCH.load(Ordering::SeqCst),
    }
}

/// Exact image node id for one icon target, using source ids only.
#[must_use]
pub fn icon_node_id_for_target(target: IconTarget) -> String {
    match target {
        IconTarget::StartSlot(n) => format!("start-app-slot-{}-icon", u32::from(n) + 1),
        IconTarget::LauncherSlot(n) => format!("launcher-row-{}-icon", u32::from(n) + 1),
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
        bindings.insert((target, w), binding);
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
    raster: impl Into<IconRaster>,
) -> Result<bool, String> {
    if !target_in_range(target) {
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
        .and_then(|bindings| bindings.get(&(target, w)).cloned())
        .is_some_and(|current| current == expected);
    if !matches {
        stale_counter().increment();
        return Ok(false);
    }
    paint_raster(document, &icon_node_id_for_target(target), raster.into())?;
    Ok(true)
}

/// Hands the shared pixel buffer to the document; no per-document copy.
fn paint_raster(
    document: &mut impl UiDocumentAccess,
    id: &str,
    raster: IconRaster,
) -> Result<(), String> {
    document.image_rgba8_shared(
        id,
        raster.source,
        raster.width,
        raster.height,
        raster.pixels,
    )
}

/// Repaints one target's node from the view-independent cache when its live
/// binding names a key whose raster is already cached. Covers a completion
/// dropped as stale after an epoch bump: the raster is cached, so the node
/// must be repainted without waiting for another projection.
pub fn apply_cached_binding(
    document: &mut impl UiDocumentAccess,
    target: IconTarget,
    edge: u32,
) -> Result<bool, String> {
    if !target_in_range(target) {
        return Ok(false);
    }
    let bound = bindings().lock().ok().and_then(|bindings| {
        bindings
            .get(&(target, edge))
            .map(|binding| binding.key.clone())
    });
    let Some((name, w, h)) = bound else {
        return Ok(false);
    };
    let Some(raster) = cached_image(&name, w, h) else {
        return Ok(false);
    };
    paint_raster(document, &icon_node_id_for_target(target), raster)?;
    Ok(true)
}

#[cfg(test)]
pub fn test_record_binding(target: IconTarget, name: &str, w: u32, h: u32, generation: u64) {
    record_binding(target, name, w, h, generation);
}

#[cfg(test)]
pub(crate) fn icon_test_lock() -> &'static std::sync::Mutex<()> {
    static M: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
pub fn test_reset_icon_bindings() {
    START_EPOCH.store(0, Ordering::SeqCst);
    TASK_EPOCH.store(0, Ordering::SeqCst);
    if let Ok(mut bindings) = bindings().lock() {
        bindings.clear();
    }
    if let Ok(mut cache) = icon_cache().lock() {
        cache.clear();
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

fn cached_image(name: &str, w: u32, h: u32) -> Option<IconRaster> {
    icon_cache().lock().ok()?.get(name, (w, h))
}

#[cfg(test)]
fn cached_contains(name: &str, w: u32, h: u32) -> bool {
    icon_cache()
        .lock()
        .is_ok_and(|cache| cache.contains(name, (w, h)))
}

pub fn note_icon_result(name: &str, w: u32, h: u32, raster: impl Into<IconRaster>) {
    if let Ok(mut cache) = icon_cache().lock() {
        cache.insert(name, (w, h), raster.into());
    }
    result_counter().increment();
}

/// Drops every caller-side cached raster so a generation bump (theme or
/// palette change) never retains an old icon. Worker service keys already
/// carry the generation; this keeps the projection cache consistent with
/// them without changing call-site signatures.
pub fn invalidate_icon_cache() {
    if let Ok(mut cache) = icon_cache().lock() {
        cache.clear();
    }
}

pub fn note_loader_stats(loader: &IconLoader) {
    let bumps = [
        (queue_full_counter(), loader.queue_full_drops),
        (stale_counter(), loader.stale_drops),
        (high_counter(), loader.high_enqueued),
        (low_counter(), loader.low_enqueued),
        (coalesced_counter(), loader.coalesced),
        (dropped_low_counter(), loader.dropped_low),
    ];
    for (counter, n) in bumps {
        if n > 0 {
            counter.increment_by(n);
        }
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
pub(crate) fn schedule_visible(
    document: &mut impl UiDocumentAccess,
    icon_id: &str,
    loader: &mut Option<&mut IconLoader>,
    target: IconTarget,
    name: &str,
    w: u32,
    h: u32,
) -> Result<(), String> {
    if let Some(hit) = cached_image(name, w, h) {
        paint_raster(document, icon_id, hit)?;
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
    cache: &mut TaskIconCache,
    state: &flamewm_shell_core::TaskVisualState,
    applications: &[flamewm_api::applications::DesktopApplication],
) -> String {
    if let Some(entry) = cache
        .resolved
        .iter()
        .find(|entry| entry.app_id == state.app_id && entry.icon_in == state.icon_name)
    {
        return entry.icon_out.clone();
    }
    let has_own_entry = applications
        .iter()
        .any(|application| application.id == state.app_id);
    let icon_out = if has_own_entry {
        state.icon_name.clone()
    } else {
        desktop_icon_for_wm_class(applications, &state.icon_name)
            .unwrap_or_else(|| state.icon_name.clone())
    };
    cache.resolved.retain(|entry| entry.app_id != state.app_id);
    cache.resolved.push(ResolvedTaskIcon {
        app_id: state.app_id.clone(),
        icon_in: state.icon_name.clone(),
        icon_out: icon_out.clone(),
    });
    icon_out
}

/// One resolved task icon, keyed by `app_id` and the state icon it came from.
struct ResolvedTaskIcon {
    app_id: DesktopAppId,
    icon_in: String,
    icon_out: String,
}

/// Task icon resolutions valid for one applications list. `sync` drops every
/// entry when the list's icon-relevant fields differ from the last refresh.
/// Owned by the caller (app state), passed to each task-slot projection.
#[derive(Default)]
pub struct TaskIconCache {
    /// (id, `icon_name`, `startup_wm_class`) of the list the entries were built from.
    applications: Vec<(DesktopAppId, String, String)>,
    resolved: Vec<ResolvedTaskIcon>,
}

impl TaskIconCache {
    fn sync(&mut self, applications: &[flamewm_api::applications::DesktopApplication]) {
        let unchanged = self.applications.len() == applications.len()
            && self
                .applications
                .iter()
                .zip(applications)
                .all(|((id, icon, wm_class), app)| {
                    *id == app.id && *icon == app.icon_name && *wm_class == app.startup_wm_class
                });
        if unchanged {
            return;
        }
        self.applications = applications
            .iter()
            .map(|app| {
                (
                    app.id.clone(),
                    app.icon_name.clone(),
                    app.startup_wm_class.clone(),
                )
            })
            .collect();
        self.resolved.clear();
    }
}

/// Desktop-entry `Icon=` whose `StartupWMClass` equals one lookup candidate
/// of `name` (case-insensitive). Non-empty icons only.
#[must_use]
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
    view: PanelView<'_>,
    loader: Option<&mut IconLoader>,
    icon_cache: &mut TaskIconCache,
) -> Result<(), String> {
    project_panel_scoped(document, view, loader, icon_cache, PanelScope::full())
}

/// Subset of panel nodes a changed domain may reproject. Windows owns
/// task slots only, Workspaces owns the pager and workspace-filtered tasks, Panels owns geometry/
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
            task_slots: changed.windows || changed.workspaces,
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
#[must_use]
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
#[must_use]
pub fn task_icon_prime_still_armed(
    armed: bool,
    task_state_count: usize,
    loader_present: bool,
    projection_ok: bool,
) -> bool {
    armed && !(task_state_count > 0 && loader_present && projection_ok)
}

/// Domain-scoped panel dispatcher for the shell loop (`refresh_dynamic`):
/// `WindowsChanged` -> task slots only, `WorkspacesChanged` -> pager and task slots,
/// `PanelsChanged` -> geometry only, `SystemChanged` -> tray only. Combined
/// flags compose (e.g. panels+windows projects geometry+slots, never the
/// tray). Empty scope is a no-op returning `Ok`. Clock projects only on
/// the full scope (cold start / snapshot replace via
/// `project_panel_async`); per-tick clock uses `project_clock_at`.
/// Focus-only window updates therefore reproject 8 task slots, not the
/// full panel.
pub fn project_panel_for_changed(
    document: &mut impl UiDocumentAccess,
    view: PanelView<'_>,
    loader: Option<&mut IconLoader>,
    icon_cache: &mut TaskIconCache,
    changed: crate::runtime::ChangedDomains,
) -> Result<(), String> {
    let scope = PanelScope::for_changed(changed);
    if scope == PanelScope::none() {
        return Ok(());
    }
    project_panel_scoped(document, view, loader, icon_cache, scope)
}
/// `WindowsChanged` -> task slots only. Resolves task visual states (cheap
/// in-memory join, no fetch) and projects labels/icon/visibility/background
/// for the 8 task slots. Pager, tray, and geometry nodes are untouched, so
/// a focus-only window update never reprojects the full panel.
pub fn project_task_slots_async(
    document: &mut impl UiDocumentAccess,
    view: PanelView<'_>,
    mut loader: Option<&mut IconLoader>,
    icon_cache: &mut TaskIconCache,
) -> Result<(), String> {
    let _span = crate::runtime::shell_span("shell.tasks.project").start();
    bump_task_view_epoch();
    let snapshot = view.snapshot;
    let states = task_states(view);
    icon_cache.sync(&snapshot.applications);
    // Task-slot nodes only: labels, icons, line visibility, backgrounds,
    // slot visibility. Pager/tray/geometry untouched.
    let (mut slot_id, mut icon_id, mut label_id, mut line_id) =
        (String::new(), String::new(), String::new(), String::new());
    for index in 0..TASK_SLOT_COUNT {
        let n = index + 1;
        slot_id.clear();
        write!(slot_id, "task-slot-{n}").expect("write to String is infallible");
        icon_id.clear();
        write!(icon_id, "task-slot-{n}-icon").expect("write to String is infallible");
        label_id.clear();
        write!(label_id, "task-slot-{n}-label").expect("write to String is infallible");
        line_id.clear();
        write!(line_id, "task-slot-{n}-line").expect("write to String is infallible");
        crate::projection::project_task_progress(
            document,
            index,
            states.get(index).map(|state| &state.id),
            view.task_progress,
        )?;
        if let Some(state) = states.get(index) {
            let label = snapshot
                .applications
                .iter()
                .find(|application| application.id == state.app_id)
                .map(|application| application.name.as_str())
                .filter(|name| !name.is_empty())
                .unwrap_or(state.app_id.as_str());
            document.text(&label_id, label.to_owned())?;
            let px = crate::projection::task_icon_device_px(view.output);
            schedule_visible(
                document,
                &icon_id,
                &mut loader.as_deref_mut(),
                IconTarget::TaskSlot(index as u8),
                &task_icon_name(icon_cache, state, &snapshot.applications),
                px,
                px,
            )?;
            document.visible(&line_id, state.running)?;
            document.background_clear(&slot_id)?;
            let line = if state.focused {
                crate::projection::task_accent()
            } else {
                TASK_LINE_IDLE
            };
            document.background(&line_id, line)?;
        } else {
            document.text(&label_id, String::new())?;
            document.visible(&line_id, false)?;
            document.background_clear(&slot_id)?;
        }
        document.visible(
            &slot_id,
            view.items.activities && states.get(index).is_some(),
        )?;
    }
    crate::projection::project_task_slots_extent(
        document,
        crate::projection::task_slots_rendered(states.len(), view.items),
    )?;
    document.visible("task-slot-insert", view.items.activities)?;
    Ok(())
}

/// `WorkspacesChanged` also refreshes tasks via `PanelScope`; this projection
/// itself owns only pager nodes. Projects the same pager cells, columns and
/// fills as the sync panel pass (`crate::projection::project_pager`).
pub fn project_pager_async(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    let _span = crate::runtime::shell_span("shell.pager.project").start();
    crate::projection::project_pager(document, snapshot.workspaces.as_ref())
}

/// `SystemChanged` -> status tray visibility only. The open popup content
/// projects on its own open path (`refresh_dynamic` reprojects media when
/// open); the panel itself only flips tray visibility nodes.
pub fn project_status_tray_async(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    items: TaskbarItems,
) -> Result<(), String> {
    crate::projection::project_status_tray(document, &snapshot.system, items)
}

/// `PanelsChanged` -> geometry/layout ownership counts only (slot/pager
/// visibility for added/removed entries). Content styling stays with the
/// task/pager projectors. Currently geometry is node-identical to the
/// visibility pass, so this reuses the slot/pager visibility tails without
/// labels, icons, or backgrounds.
pub fn project_panel_geometry(
    document: &mut impl UiDocumentAccess,
    view: PanelView<'_>,
) -> Result<(), String> {
    let snapshot = view.snapshot;
    crate::projection::project_taskbar_height(document, view)?;
    let states = task_states(view);
    for index in 0..TASK_SLOT_COUNT {
        document.visible(
            &format!("task-slot-{}", index + 1),
            view.items.activities && states.get(index).is_some(),
        )?;
    }
    crate::projection::project_task_slots_extent(
        document,
        crate::projection::task_slots_rendered(states.len(), view.items),
    )?;
    document.visible("task-slot-insert", view.items.activities)?;
    crate::projection::project_pager_visibility(document, snapshot.workspaces.as_ref())?;
    crate::projection::project_element_backgrounds(
        document,
        view.items,
        crate::projection::task_slots_rendered(states.len(), view.items),
        crate::projection::tray_any_visible(&snapshot.system, view.items),
        view.items.clock,
    )?;
    Ok(())
}

fn project_panel_scoped(
    document: &mut impl UiDocumentAccess,
    view: PanelView<'_>,
    loader: Option<&mut IconLoader>,
    icon_cache: &mut TaskIconCache,
    scope: PanelScope,
) -> Result<(), String> {
    if scope.task_slots {
        project_task_slots_async(document, view, loader, icon_cache)?;
    }
    if scope.geometry {
        project_panel_geometry(document, view)?;
    }
    if scope.pager {
        project_pager_async(document, view.snapshot)?;
        document.visible(
            "workspace-pager",
            crate::projection::workspace_pager_visible(
                view.items,
                view.snapshot.workspaces.as_ref(),
            ),
        )?;
    }
    if scope.tray {
        project_status_tray_async(document, view.snapshot, view.items)?;
    }
    if scope.task_slots && scope.pager && scope.tray && scope.geometry {
        crate::projection::project_clock(document)?;
    }
    Ok(())
}

/// Unified Start document projection (J04/J07): categories + search +
/// app rows + power rows in ONE document on the single Start surface.
/// Labels/visibility immediate; icons cached-or-Flame-fallback with queued
/// HIGH worker requests. Never resolves on the caller thread.
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
    let (mut button_id, mut label_id, mut icon_id) = (String::new(), String::new(), String::new());
    for index in 0..START_APP_SLOT_COUNT {
        let slot = index + 1;
        button_id.clear();
        write!(button_id, "start-app-slot-{slot}").expect("write to String is infallible");
        label_id.clear();
        write!(label_id, "start-app-slot-{slot}-label").expect("write to String is infallible");
        icon_id.clear();
        write!(icon_id, "start-app-slot-{slot}-icon").expect("write to String is infallible");
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
        && crate::projection::session_available(*session);
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
#[macro_export]
macro_rules! noop_document_overflow {
    () => {
        fn overflow(
            &mut self,
            _: &str,
            _: flamewm_ui_x11::Overflow,
            _: flamewm_ui_x11::Overflow,
        ) -> Result<(), String> {
            Ok(())
        }
    };
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use flamewm_api::applications::DesktopApplication;

    #[derive(Default)]
    pub(crate) struct FakeDoc {
        pub(crate) texts: HashMap<String, String>,
        pub(crate) images: Vec<String>,
        pub(crate) visible: HashMap<String, bool>,
        pub(crate) fills: HashMap<String, UiColor>,
        pub(crate) sizes: HashMap<String, (f32, f32)>,
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
        fn size(&mut self, id: &str, width: f32, height: f32) -> Result<(), String> {
            self.sizes.insert(id.to_owned(), (width, height));
            Ok(())
        }
        fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
            self.fills.insert(id.to_owned(), color);
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
        crate::noop_document_overflow!();
        fn font_size(&mut self, _: &str, _: f32) -> Result<(), String> {
            Ok(())
        }
        fn font_weight(&mut self, _: &str, _: u16) -> Result<(), String> {
            Ok(())
        }
    }

    fn test_image(source: &str) -> RuntimeImage {
        sized_test_image(source, 1)
    }

    fn project_changed(view: PanelView<'_>, changed: crate::runtime::ChangedDomains) -> FakeDoc {
        let mut doc = FakeDoc::default();
        project_panel_for_changed(&mut doc, view, None, &mut TaskIconCache::default(), changed)
            .expect("projects");
        doc
    }

    fn tray_visibility(document: &FakeDoc) -> HashMap<String, bool> {
        document
            .visible
            .iter()
            .filter(|(id, _)| id.starts_with("tray-") || id.as_str() == "clock-button")
            .map(|(id, &visible)| (id.clone(), visible))
            .collect()
    }

    fn project_start_doc(model: &StartModel, category: StartCategory, query: &str) -> FakeDoc {
        let mut doc = FakeDoc::default();
        project_start_unified(
            &mut doc,
            model,
            category,
            query,
            &SessionCapabilities::default(),
            None,
        )
        .expect("projects");
        doc
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
            sized_test_image("a", 20),
        )
        .expect("apply ok");
        assert!(applied, "live binding must mutate");
        assert_eq!(doc.images, vec!["start-app-slot-1-icon".to_owned()]);
    }

    #[test]
    fn icon_binding_stale_on_epoch_bump_touches_nothing() {
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
            sized_test_image("a", 20),
        )
        .expect("stale check ok");
        assert!(!applied, "epoch bump must stale the completion");
        assert!(doc.images.is_empty(), "stale drop touches no node");
    }

    #[test]
    fn icon_binding_stale_on_key_or_generation_mismatch() {
        let _guard = icon_test_lock().lock();
        test_reset_icon_bindings();
        let mut doc = FakeDoc::default();
        test_record_binding(IconTarget::TaskSlot(1), "icon-a", 25, 25, 3);
        let wrong_name = apply_icon_result(
            &mut doc,
            IconTarget::TaskSlot(1),
            "icon-b",
            25,
            25,
            3,
            sized_test_image("b", 25),
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
            sized_test_image("b", 25),
        )
        .expect("stale check ok");
        assert!(!wrong_gen, "generation mismatch must stale");
        assert!(doc.images.is_empty(), "stale drops touch no node");
    }

    #[test]
    fn task_slot_bindings_keep_one_edge_per_output_scale() {
        let _guard = icon_test_lock().lock();
        test_reset_icon_bindings();
        let mut doc = FakeDoc::default();
        // Two outputs share TaskSlot(0) but project different icon edges.
        test_record_binding(IconTarget::TaskSlot(0), "app", 25, 25, 1);
        test_record_binding(IconTarget::TaskSlot(0), "app", 31, 31, 1);
        let at_25 = apply_icon_result(
            &mut doc,
            IconTarget::TaskSlot(0),
            "app",
            25,
            25,
            1,
            sized_test_image("a", 25),
        )
        .expect("edge 25 ok");
        let at_31 = apply_icon_result(
            &mut doc,
            IconTarget::TaskSlot(0),
            "app",
            31,
            31,
            1,
            sized_test_image("a", 31),
        )
        .expect("edge 31 ok");
        assert!(at_25 && at_31, "both edges keep their own binding");
        assert_eq!(doc.images.len(), 2);
    }

    #[test]
    fn icon_ready_after_epoch_bump_repaints_bound_node() {
        let _guard = icon_test_lock().lock();
        test_reset_icon_bindings();
        let mut doc = FakeDoc::default();
        test_record_binding(IconTarget::TaskSlot(1), "icon-a", 25, 25, 3);
        // A projection bumps the task epoch without re-binding this slot.
        bump_task_view_epoch();
        note_icon_result("icon-a", 25, 25, sized_test_image("a", 20));
        let completed = apply_icon_result(
            &mut doc,
            IconTarget::TaskSlot(1),
            "icon-a",
            25,
            25,
            3,
            sized_test_image("a", 20),
        )
        .expect("stale check ok");
        assert!(!completed, "epoch bump stales the late completion");
        assert!(doc.images.is_empty());
        let repainted =
            apply_cached_binding(&mut doc, IconTarget::TaskSlot(1), 25).expect("cached repaint ok");
        assert!(repainted, "cached raster for the bound key must repaint");
        assert_eq!(doc.images, vec!["task-slot-2-icon".to_owned()]);
    }

    #[test]
    fn wm_class_matches_desktop_entry_icon_before_fallback() {
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
        assert_eq!(
            desktop_icon_for_wm_class(std::slice::from_ref(&app), "gimp"),
            None
        );
    }

    #[test]
    fn task_slot_node_id_uses_source_ids() {
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
    fn task_slots_list_only_windows_on_the_shown_panel_output() {
        use flamewm_api::panels::{
            PanelEdge, PanelSnapshot, PanelsSnapshot, TaskEntry, TaskEntryKind,
        };
        use flamewm_api::window::{WindowSnapshot, WindowState};
        use flamewm_api::{DesktopAppId, OutputId, Rect, TaskEntryId, WindowRef, WorkspaceRef};
        let _guard = icon_test_lock().lock();
        let window_on = |id: u64, output: &str| WindowSnapshot {
            reference: WindowRef::new(id, 1),
            title: String::new(),
            app_id: DesktopAppId::new("term"),
            outer_geometry: Rect::new(0, 0, 10, 10),
            restore_geometry: Rect::new(0, 0, 10, 10),
            state: WindowState::Normal,
            sticky: false,
            focused: false,
            workspace: WorkspaceRef {
                index: 0,
                revision: 1,
            },
            output: OutputId::new(output),
            state_generation: 1,
        };
        let task = |id: u64| {
            let window = WindowRef::new(id, 1);
            TaskEntry {
                id: TaskEntryId::window(window),
                app_id: DesktopAppId::new("term"),
                kind: TaskEntryKind::Window { window },
                order_index: id as usize,
            }
        };
        let mut snapshot = ShellSnapshot {
            panels: PanelsSnapshot {
                revision: 1,
                panels: vec![PanelSnapshot {
                    output: OutputId::new("A"),
                    edge: PanelEdge::Bottom,
                    logical_size: 40,
                    visible: true,
                    geometry: Rect::new(0, 760, 1000, 40),
                }],
                tasks: vec![task(1), task(2)],
                pinned_apps: Vec::new(),
            },
            windows: vec![window_on(1, "A"), window_on(2, "B")],
            ..ShellSnapshot::default()
        };
        let mut doc = FakeDoc::default();
        project_task_slots_async(
            &mut doc,
            PanelView::first(&snapshot),
            None,
            &mut TaskIconCache::default(),
        )
        .expect("projects");
        assert_eq!(doc.visible.get("task-slot-1"), Some(&true));
        assert_eq!(doc.visible.get("task-slot-2"), Some(&false));
        // Window 1 moved A->B; window 2 is already on B. Panel A lists neither.
        snapshot.windows[0].output = OutputId::new("B");
        let mut doc = FakeDoc::default();
        project_task_slots_async(
            &mut doc,
            PanelView::first(&snapshot),
            None,
            &mut TaskIconCache::default(),
        )
        .expect("panel A projects");
        assert_eq!(doc.visible.get("task-slot-1"), Some(&false));
        assert_eq!(doc.visible.get("task-slot-2"), Some(&false));
        // Panel B lists both.
        snapshot.panels.panels[0].output = OutputId::new("B");
        let mut doc = FakeDoc::default();
        project_task_slots_async(
            &mut doc,
            PanelView::first(&snapshot),
            None,
            &mut TaskIconCache::default(),
        )
        .expect("panel B projects");
        assert_eq!(doc.visible.get("task-slot-1"), Some(&true));
        assert_eq!(doc.visible.get("task-slot-2"), Some(&true));

        // Same output, different workspaces. A workspace-only snapshot
        // change must replace both the task label AND the click-target slot;
        // minimized windows belong to their original workspace too.
        snapshot.panels.tasks[0].app_id = DesktopAppId::new("first-app");
        snapshot.panels.tasks[1].app_id = DesktopAppId::new("second-app");
        snapshot.windows[0].app_id = DesktopAppId::new("first-app");
        snapshot.windows[1].app_id = DesktopAppId::new("second-app");
        snapshot.windows[0].workspace.index = 0;
        snapshot.windows[1].workspace.index = 1;
        snapshot.windows[1].state = WindowState::Minimized;
        snapshot.workspaces = Some(flamewm_api::workspace::WorkspaceSnapshot {
            revision: 1,
            count: 2,
            active_index: 0,
            last_index: None,
            names: vec![],
        });
        let changed = crate::runtime::ChangedDomains {
            workspaces: true,
            ..Default::default()
        };
        let first = project_changed(PanelView::first(&snapshot), changed);
        assert_eq!(first.visible.get("task-slot-1"), Some(&true));
        assert_eq!(first.visible.get("task-slot-2"), Some(&false));
        assert_eq!(
            first.texts.get("task-slot-1-label").map(String::as_str),
            Some("first-app")
        );
        snapshot
            .workspaces
            .as_mut()
            .expect("workspace snapshot")
            .active_index = 1;
        let second = project_changed(PanelView::first(&snapshot), changed);
        assert_eq!(second.visible.get("task-slot-1"), Some(&true));
        assert_eq!(second.visible.get("task-slot-2"), Some(&false));
        assert_eq!(
            second.texts.get("task-slot-1-label").map(String::as_str),
            Some("second-app")
        );
    }

    #[test]
    fn windows_scope_projects_task_slots_only() {
        use crate::runtime::ChangedDomains;
        let _guard = icon_test_lock().lock();
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
        let doc = project_changed(PanelView::first(&snapshot), changed);
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
    fn workspaces_scope_projects_pager_and_tasks() {
        use crate::runtime::ChangedDomains;
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot::default();
        let changed = ChangedDomains {
            workspaces: true,
            ..ChangedDomains::default()
        };
        let doc = project_changed(PanelView::first(&snapshot), changed);
        assert!(doc.visible.contains_key("workspace-1"), "pager projected");
        assert!(
            doc.visible.contains_key("task-slot-1"),
            "task slots must update on workspace-only change",
        );
        assert!(
            !doc.visible.contains_key("tray-volume"),
            "tray untouched on workspaces-only change",
        );
    }

    #[test]
    fn asynchronous_workspace_count_transition_hides_empty_capsule() {
        use crate::runtime::ChangedDomains;
        let _guard = icon_test_lock().lock();
        let changed = ChangedDomains {
            workspaces: true,
            ..ChangedDomains::default()
        };
        let mut snapshot = ShellSnapshot::default();
        let pager = |snapshot: &ShellSnapshot| project_changed(PanelView::first(snapshot), changed);
        snapshot.workspaces = Some(flamewm_api::workspace::WorkspaceSnapshot {
            revision: 1,
            count: 1,
            active_index: 0,
            last_index: None,
            names: Vec::new(),
        });
        assert_eq!(
            pager(&snapshot).visible.get("workspace-pager"),
            Some(&false)
        );
        snapshot
            .workspaces
            .as_mut()
            .expect("workspace snapshot")
            .count = 2;
        assert_eq!(pager(&snapshot).visible.get("workspace-pager"), Some(&true));
        snapshot
            .workspaces
            .as_mut()
            .expect("workspace snapshot")
            .count = 1;
        assert_eq!(
            pager(&snapshot).visible.get("workspace-pager"),
            Some(&false)
        );
    }

    // REGRESSION: the async pager pass projects the same two-row topology as
    // the sync panel pass. Six workspaces read `1 2 3 / 4 5 6`, so workspace 4
    // (index 3) is the lower cell of the first column (cell 2), not the page
    // slot of an old 2x2 page.
    // MUTATION: keep the fixed 4-slot page in either pager path.
    #[test]
    fn async_pager_projects_the_two_row_topology() {
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot {
            workspaces: Some(flamewm_api::workspace::WorkspaceSnapshot {
                revision: 1,
                count: 6,
                active_index: 3,
                last_index: None,
                names: Vec::new(),
            }),
            ..ShellSnapshot::default()
        };
        let mut doc = FakeDoc::default();
        project_pager_async(&mut doc, &snapshot).expect("pager projects");
        // Labels follow the cell each workspace sits in.
        assert_eq!(doc.texts.get("workspace-1").map(String::as_str), Some("1"));
        assert_eq!(doc.texts.get("workspace-3").map(String::as_str), Some("2"));
        assert_eq!(doc.texts.get("workspace-5").map(String::as_str), Some("3"));
        assert_eq!(doc.texts.get("workspace-2").map(String::as_str), Some("4"));
        assert_eq!(doc.texts.get("workspace-4").map(String::as_str), Some("5"));
        assert_eq!(doc.texts.get("workspace-6").map(String::as_str), Some("6"));
        // Cells 1..=6 are shown, every later cell and column is hidden.
        for cell in 1..=6 {
            assert_eq!(
                doc.visible.get(&format!("workspace-{cell}")),
                Some(&true),
                "cell {cell}"
            );
        }
        for cell in 7..=18 {
            assert_eq!(
                doc.visible.get(&format!("workspace-{cell}")),
                Some(&false),
                "cell {cell}"
            );
        }
        for column in 1..=3 {
            assert_eq!(
                doc.visible.get(&format!("workspace-column-{column}")),
                Some(&true),
                "column {column}"
            );
        }
        for column in 4..=9 {
            assert_eq!(
                doc.visible.get(&format!("workspace-column-{column}")),
                Some(&false),
                "column {column}"
            );
        }
        // Only the active workspace carries the accent fill.
        let active = crate::projection::pager_active_fill();
        assert_eq!(doc.fills.get("workspace-2"), Some(&active));
        for cell in [1, 3, 4, 5, 6] {
            assert_ne!(
                doc.fills.get(&format!("workspace-{cell}")),
                Some(&active),
                "cell {cell} is not the active workspace"
            );
        }
    }

    #[test]
    fn system_scope_projects_tray_only() {
        use crate::runtime::ChangedDomains;
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot::default();
        let changed = ChangedDomains {
            system: true,
            ..ChangedDomains::default()
        };
        let doc = project_changed(PanelView::first(&snapshot), changed);
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
    fn status_tray_matches_sync_full_and_async_system_projection() {
        use flamewm_api::system::{PlaybackState, ServiceAvailability};
        let _guard = icon_test_lock().lock();
        let mut snapshot = ShellSnapshot::default();
        snapshot.system.media.availability = ServiceAvailability::Available;
        snapshot.system.audio.availability = ServiceAvailability::Available;
        snapshot.system.network.availability = ServiceAvailability::Available;

        for (
            playback,
            muted,
            network_available,
            volume_item,
            network_item,
            clock_item,
            media,
            play,
            pause,
        ) in [
            (
                PlaybackState::Playing,
                false,
                true,
                true,
                true,
                true,
                true,
                false,
                true,
            ),
            (
                PlaybackState::Paused,
                true,
                true,
                false,
                false,
                false,
                true,
                true,
                false,
            ),
            (
                PlaybackState::Stopped,
                false,
                false,
                true,
                true,
                true,
                false,
                false,
                false,
            ),
        ] {
            snapshot.system.media.playback = playback;
            snapshot.system.audio.muted = muted;
            snapshot.system.network.availability = if network_available {
                ServiceAvailability::Available
            } else {
                ServiceAvailability::Unavailable
            };
            let items = TaskbarItems {
                volume: volume_item,
                network: network_item,
                clock: clock_item,
                ..TaskbarItems::default()
            };

            let mut sync = FakeDoc::default();
            let mut resolver = flamewm_integrations_linux::icons::IconResolver::new(
                std::path::PathBuf::new(),
                Vec::new(),
                Vec::new(),
                0,
            );
            crate::projection::project_panel(
                &mut sync,
                PanelView::first(&snapshot).with_items(items),
                &mut resolver,
            )
            .expect("sync panel projects");

            let changed = crate::runtime::ChangedDomains {
                system: true,
                ..Default::default()
            };
            let scoped = project_changed(PanelView::first(&snapshot).with_items(items), changed);
            let mut full = FakeDoc::default();
            project_panel_async(
                &mut full,
                PanelView::first(&snapshot).with_items(items),
                None,
                &mut TaskIconCache::default(),
            )
            .expect("async full panel projects");

            let sync_tray = tray_visibility(&sync);
            assert_eq!(tray_visibility(&scoped), sync_tray, "scoped: {playback:?}");
            assert_eq!(tray_visibility(&full), sync_tray, "full: {playback:?}");
            assert_eq!(sync_tray.get("tray-media"), Some(&media));
            assert_eq!(sync_tray.get("tray-play-icon"), Some(&play));
            assert_eq!(sync_tray.get("tray-pause-icon"), Some(&pause));
            assert_eq!(sync_tray.get("tray-volume"), Some(&volume_item));
            assert_eq!(sync_tray.get("tray-volume-icon"), Some(&!muted));
            assert_eq!(sync_tray.get("tray-volume-muted-icon"), Some(&muted));
            assert_eq!(
                sync_tray.get("tray-network"),
                Some(&(network_available && network_item))
            );
            assert_eq!(sync_tray.get("clock-button"), Some(&clock_item));
        }
    }

    #[test]
    fn panels_scope_projects_geometry_only() {
        use crate::runtime::ChangedDomains;
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot::default();
        let changed = ChangedDomains {
            panels: true,
            ..ChangedDomains::default()
        };
        let doc = project_changed(PanelView::first(&snapshot), changed);
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

    // REGRESSION: a power-only SystemChanged reprojects through the tray
    // scope; the battery button must follow the power snapshot there, not
    // only on the sync projection path. A 45% discharge shows the 50 glyph,
    // an 87% reading shows the whole button with the 90 glyph.
    // MUTATION: drop the project_battery_tray call from project_status_tray_async.
    #[test]
    fn system_scope_projects_battery_from_power_snapshot() {
        use crate::runtime::ChangedDomains;
        use flamewm_api::system::{PowerSnapshot, PowerState, ServiceAvailability};
        let _guard = icon_test_lock().lock();
        let mut snapshot = ShellSnapshot::default();
        snapshot.system.power = PowerSnapshot {
            available: ServiceAvailability::Available,
            present: true,
            percent: 45,
            state: PowerState::Discharging,
            cpu_percent: None,
            power_rate_mw: None,
        };
        let changed = ChangedDomains {
            system: true,
            ..ChangedDomains::default()
        };
        let doc = project_changed(PanelView::first(&snapshot), changed);
        assert_eq!(doc.visible.get("tray-battery"), Some(&true));
        assert_eq!(doc.visible.get("tray-battery-050"), Some(&true));
        assert_eq!(doc.visible.get("tray-battery-caution"), Some(&false));

        // Unknown (plugged in, not charging) stays shown by default; only the
        // battery-only-when-relevant setting hides it.
        snapshot.system.power.percent = 87;
        snapshot.system.power.state = PowerState::Unknown;
        let doc = project_changed(PanelView::first(&snapshot), changed);
        assert_eq!(doc.visible.get("tray-battery"), Some(&true));
        let relevant_only = TaskbarItems {
            battery_only_when_relevant: true,
            ..TaskbarItems::default()
        };
        let doc = project_changed(
            PanelView::first(&snapshot).with_items(relevant_only),
            changed,
        );
        assert_eq!(doc.visible.get("tray-battery"), Some(&false));

        // Charging shows the charging glyph, at 80% too.
        snapshot.system.power.percent = 45;
        snapshot.system.power.state = PowerState::Charging;
        let doc = project_changed(PanelView::first(&snapshot), changed);
        assert_eq!(doc.visible.get("tray-battery"), Some(&true));
        assert_eq!(doc.visible.get("tray-battery-050-charging"), Some(&true));
        assert_eq!(doc.visible.get("tray-battery-050"), Some(&false));

        snapshot.system.power.percent = 80;
        let doc = project_changed(PanelView::first(&snapshot), changed);
        assert_eq!(doc.visible.get("tray-battery"), Some(&true));
        assert_eq!(doc.visible.get("tray-battery-080-charging"), Some(&true));
    }

    // A08: each cluster item is its setting AND availability; the clock
    // cluster and workspace pager follow their own settings.
    #[test]
    fn taskbar_item_settings_hide_cluster_items_clock_and_pager() {
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot::default();
        let hidden = TaskbarItems {
            weather: false,
            volume: false,
            network: false,
            battery: false,
            battery_only_when_relevant: false,
            clock: false,
            workspaces: false,
            ..TaskbarItems::default()
        };
        let mut doc = FakeDoc::default();
        project_panel_async(
            &mut doc,
            PanelView::first(&snapshot).with_items(hidden),
            None,
            &mut TaskIconCache::default(),
        )
        .expect("projects");
        for id in [
            "tray-volume",
            "tray-network",
            "tray-battery",
            "clock-button",
            "workspace-pager",
        ] {
            assert_eq!(doc.visible.get(id), Some(&false), "{id}");
        }
    }

    // A03: a theme change drops rasters cached under the old theme, so the next
    // Start or taskbar projection re-requests them instead of reusing old art.
    #[test]
    fn theme_change_invalidates_cached_icons() {
        let _guard = icon_test_lock().lock();
        note_icon_result("firefox", 20, 20, empty_image(20, 20));
        assert!(cached_contains("firefox", 20, 20));
        invalidate_icon_cache();
        assert!(!cached_contains("firefox", 20, 20));
    }

    // A08: status cluster order in the panel template.
    #[test]
    fn panel_status_cluster_order_is_weather_volume_network_battery_clock() {
        let html = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../ui/shell/panel.html"
        ))
        .expect("panel.html");
        let at = |id: &str| {
            html.find(&format!("id=\"{id}\""))
                .unwrap_or_else(|| panic!("{id} missing"))
        };
        let order = [
            "tray-weather",
            "tray-volume",
            "tray-network",
            "tray-battery",
            "clock-button",
        ]
        .map(at);
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
        assert!(at("tray-media") < at("tray-weather"));
        assert!(at("workspace-pager") < at("tray-media"));
    }

    #[test]
    fn empty_scope_is_noop() {
        use crate::runtime::ChangedDomains;
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot::default();
        let doc = project_changed(PanelView::first(&snapshot), ChangedDomains::default());
        assert!(doc.visible.is_empty(), "empty scope touches nothing");
    }

    #[test]
    fn icon_cache_evicts_least_recently_used_entry_at_budget() {
        let mut cache = IconCache::new(2);
        let first = ("first".to_owned(), 1, 1);
        let second = ("second".to_owned(), 1, 1);
        let third = ("third".to_owned(), 1, 1);
        cache.insert(&first.0, (first.1, first.2), test_image("first").into());
        cache.insert(&second.0, (second.1, second.2), test_image("second").into());
        assert_eq!(
            cache
                .get(&first.0, (first.1, first.2))
                .expect("first cache hit")
                .source,
            "first"
        );
        cache.insert(&third.0, (third.1, third.2), test_image("third").into());

        assert!(
            cache.get(&second.0, (second.1, second.2)).is_none(),
            "least-recent entry evicted"
        );
        assert_eq!(
            cache
                .get(&first.0, (first.1, first.2))
                .expect("first retained")
                .source,
            "first"
        );
        assert_eq!(
            cache
                .get(&third.0, (third.1, third.2))
                .expect("third retained")
                .source,
            "third"
        );
    }

    fn sized_test_image(source: &str, side: u32) -> RuntimeImage {
        RuntimeImage {
            source: source.to_owned(),
            width: side,
            height: side,
            pixels: vec![0; side as usize * side as usize * 4],
        }
    }

    #[test]
    fn icon_cache_contains_does_not_touch_lru_order() {
        let mut cache = IconCache::new(2);
        let first = ("first".to_owned(), 1, 1);
        let second = ("second".to_owned(), 1, 1);
        let third = ("third".to_owned(), 1, 1);
        cache.insert(&first.0, (first.1, first.2), test_image("first").into());
        cache.insert(&second.0, (second.1, second.2), test_image("second").into());
        assert!(cache.contains(&first.0, (first.1, first.2)));
        assert!(!cache.contains(&third.0, (third.1, third.2)));
        cache.insert(&third.0, (third.1, third.2), test_image("third").into());

        // `first` was only probed, so it stays the least-recent entry.
        assert!(
            !cache.contains(&first.0, (first.1, first.2)),
            "contains must not refresh recency"
        );
        assert!(cache.contains(&second.0, (second.1, second.2)));
        assert!(cache.contains(&third.0, (third.1, third.2)));
    }

    #[test]
    fn icon_cache_get_refreshes_order_across_many_touches() {
        let mut cache = IconCache::new(3);
        let keys: Vec<IconCacheKey> = ["a", "b", "c"]
            .iter()
            .map(|name| ((*name).to_owned(), 1, 1))
            .collect();
        for key in &keys {
            cache.insert(&key.0, (key.1, key.2), test_image(&key.0).into());
        }
        // Hammer one key so stale recency records pile up and get compacted.
        for _ in 0..100 {
            assert!(cache.get(&keys[0].0, (keys[0].1, keys[0].2)).is_some());
        }
        assert!(cache.order.len() <= 2 * cache.len + 16 + 1);
        // Recency now: b (oldest), c, a. Next insert evicts b.
        cache.insert("d", (1, 1), test_image("d").into());
        assert!(
            !cache.contains(&keys[1].0, (keys[1].1, keys[1].2)),
            "least-recent live entry evicted"
        );
        assert!(cache.contains(&keys[0].0, (keys[0].1, keys[0].2)));
        assert!(cache.contains(&keys[2].0, (keys[2].1, keys[2].2)));
        assert_eq!(cache.bytes, 3 * 4);
    }

    #[test]
    fn default_icon_cache_is_bounded_for_an_idle_shell() {
        let cache = IconCache::new(ICON_CACHE_ENTRY_BUDGET);
        assert_eq!(cache.entry_budget, 2048);
        assert_eq!(cache.byte_budget, 32 * 1024 * 1024);
    }

    #[test]
    fn icon_cache_shares_name_storage_between_icon_sizes() {
        let mut cache = IconCache::with_budgets(10, 4096);
        cache.insert("browser", (16, 16), test_image("small").into());
        cache.insert("browser", (32, 32), test_image("large").into());
        let first = &cache.order[0].0;
        let second = &cache.order[1].0;
        assert!(
            Arc::ptr_eq(first, second),
            "size variants share an icon-name allocation"
        );
        assert_eq!(cache.len, 2);
        assert!(cache.get("browser", (16, 16)).is_some());
        assert!(cache.get("browser", (32, 32)).is_some());
    }

    #[test]
    fn icon_cache_byte_budget_evicts_oldest_when_exceeded() {
        // Entry budget is loose; the 8-byte byte budget must drive eviction.
        let mut cache = IconCache::with_budgets(10, 8);
        let first = ("first".to_owned(), 1, 1);
        let second = ("second".to_owned(), 1, 1);
        let third = ("third".to_owned(), 1, 1);
        cache.insert(&first.0, (first.1, first.2), test_image("first").into());
        cache.insert(&second.0, (second.1, second.2), test_image("second").into());
        cache.insert(&third.0, (third.1, third.2), test_image("third").into());

        assert!(
            cache.get(&first.0, (first.1, first.2)).is_none(),
            "oldest evicted by bytes"
        );
        assert!(
            cache.get(&second.0, (second.1, second.2)).is_some(),
            "second retained"
        );
        assert!(
            cache.get(&third.0, (third.1, third.2)).is_some(),
            "third retained"
        );
        assert!(cache.bytes <= 8, "cache stays under byte budget");
    }

    #[test]
    fn icon_cache_entry_larger_than_byte_budget_is_not_retained() {
        let mut cache = IconCache::with_budgets(10, 8);
        let small = ("small".to_owned(), 1, 1);
        let huge = ("huge".to_owned(), 3, 3);
        cache.insert(&small.0, (small.1, small.2), test_image("small").into());
        cache.insert(
            &huge.0,
            (huge.1, huge.2),
            sized_test_image("huge", 3).into(),
        );

        assert!(
            cache.get(&huge.0, (huge.1, huge.2)).is_none(),
            "oversized entry not retained"
        );
        assert!(
            cache.get(&small.0, (small.1, small.2)).is_some(),
            "oversized insert evicts nothing"
        );
        assert!(cache.bytes <= 8, "cache stays under byte budget");
    }

    #[test]
    fn async_panel_projects_labels_without_sync_resolve() {
        let _guard = icon_test_lock().lock();
        let snapshot = ShellSnapshot::default();
        let mut doc = FakeDoc::default();
        project_panel_async(
            &mut doc,
            PanelView::first(&snapshot),
            None,
            &mut TaskIconCache::default(),
        )
        .expect("projects");
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
        let doc = project_start_doc(&model, StartCategory::All, "");
        assert!(
            doc.images.contains(&"start-app-slot-1-icon".to_owned()),
            "miss applies Flame fallback now, never retains old icon"
        );
    }

    #[test]
    fn async_start_submenu_hides_empty_slots() {
        let _guard = icon_test_lock().lock();
        let doc = project_start_doc(&StartModel::new(Vec::new()), StartCategory::All, "");
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
        let doc = project_start_doc(&model, StartCategory::Games, "calc");
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
        let doc = project_start_doc(&model, StartCategory::Games, "calc");
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
        let doc = project_start_doc(&model, StartCategory::Games, "");
        assert_eq!(
            doc.texts.get("start-app-slot-1-label").map(String::as_str),
            Some("Chess")
        );
        assert_eq!(doc.visible.get("start-app-slot-2"), Some(&false));
        assert_eq!(doc.visible.get("start-categories"), Some(&true));
    }

    #[test]
    fn start_content_pane_keeps_fixed_height_across_search_states() {
        // Search hides the category rail, so the content pane is the only tall
        // child left. Its min-height must equal its max-height (the shared
        // 420px cap) or the Start outer height drops during search and
        // zero-result states. The search bar stays bottom-anchored via auto top margin.
        let css = include_str!("../../../ui/shell/flamewm.css");
        let rule = |selector: &str| -> &str {
            let start = css
                .find(&format!("{selector} {{"))
                .unwrap_or_else(|| panic!("missing rule {selector}"));
            let end = css[start..].find('}').expect("rule end") + start;
            &css[start..end]
        };
        let pane = rule(".start-content-pane");
        assert!(pane.contains("min-height:420px"), "{pane}");
        assert!(pane.contains("max-height:420px"), "{pane}");
        assert!(rule(".start-left-pane").contains("justify-content:flex-end"));
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

    #[test]
    fn task_progress_is_slot_identity_bound_scaled_and_cleared() {
        let id = flamewm_api::TaskEntryId::new("win:7:1");
        let entry = flamewm_api::panels::TaskProgressEntry {
            task_id: id.clone(),
            progress: flamewm_api::panels::TaskProgress::new(50).unwrap(),
        };
        let mut doc = FakeDoc::default();
        crate::projection::project_task_progress(&mut doc, 0, Some(&id), &[entry]).unwrap();
        assert_eq!(doc.visible.get("task-slot-1-progress"), Some(&true));
        assert_eq!(doc.sizes.get("task-slot-1-progress"), Some(&(14.0, 2.0)));

        crate::projection::project_task_progress(&mut doc, 0, None, &[]).unwrap();
        assert_eq!(doc.visible.get("task-slot-1-progress"), Some(&false));
        assert_eq!(doc.sizes.get("task-slot-1-progress"), Some(&(0.0, 2.0)));
        assert_eq!(doc.visible.get("task-slot-2-progress"), None);
    }
}

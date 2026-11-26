use super::*;
use std::time::SystemTime;

/// Once startup (wallpaper decode, icon loads, first paints) has settled, its freed heap is handed
/// back.
pub(crate) const STARTUP_HEAP_TRIM_DELAY: Duration = Duration::from_secs(10);

pub(crate) const SLOT_COUNT: usize = 128;

pub(crate) const BLANK_ROW_IDS: [&str; 6] = [
    "menu-row-terminal",
    "menu-row-folder",
    "menu-row-note",
    "menu-row-widgets",
    "menu-row-desktop",
    "menu-row-settings",
];

pub(crate) const ENTRY_ROW_IDS: [&str; 5] = [
    "menu-row-empty-trash",
    "menu-row-rename",
    "menu-row-shortcut",
    "menu-row-convert",
    "menu-row-delete",
];

pub(crate) struct LauncherMeta {
    pub(crate) icon: String,
    pub(crate) name: String,
    pub(crate) valid: bool,
    /// File identity captured when this entry was parsed; an unchanged stamp reuses the entry
    /// after a rescan instead of re-reading and re-parsing the `.desktop` file.
    pub(crate) stamp: LauncherStamp,
}

/// Cheap file identity for launcher cache reuse (`len` plus last modification time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LauncherStamp {
    pub(crate) len: u64,
    pub(crate) modified: Option<SystemTime>,
}

/// Desktop selection styling last written to the document: fill alpha, accent and the exact
/// selected set. A sync whose inputs match this snapshot issues no document writes.
pub(crate) struct SelectionVisuals {
    pub(crate) fill_alpha: u8,
    pub(crate) accent: UiColor,
    pub(crate) selected: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuKind {
    Blank,
    Entry,
    ConvertWheel,
    Sticky,
    Widget,
    WidgetGallery,
    WidgetConfig,
}

pub(crate) struct MenuState {
    pub(crate) kind: MenuKind,
    pub(crate) anchor: Point,
    pub(crate) output: OutputId,
    pub(crate) selected_id: Option<String>,
}

pub(crate) struct ClickRecord {
    pub(crate) id: String,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) at: Instant,
}

pub(crate) struct DesktopState {
    pub(crate) directory: PathBuf,
    pub(crate) model: DesktopModel,
    pub(crate) grid: GridConfig,
    pub(crate) output_grids: BTreeMap<OutputId, GridConfig>,
    pub(crate) entry_scale_percent: u16,
    pub(crate) output_geometries: Vec<(OutputId, Rect)>,
    pub(crate) default_output: OutputId,
    pub(crate) icons: projection::ParallelIconService,
    pub(crate) icon_slots: Vec<Option<IconKey>>,
    /// Inputs last projected into each slot; equal inputs skip re-projection.
    pub(crate) slot_inputs: Vec<Option<SlotInputs>>,
    /// False until the first slot pass has cleared every empty slot.
    pub(crate) slots_primed: bool,
    /// Last `projected_cells` result with the exact inputs it was computed from.
    pub(crate) projected_cache: Option<ProjectedCellsCache>,
    pub(crate) layout_path: PathBuf,
    pub(crate) sticky: sticky::StickyState,
    pub(crate) widgets: widgets::WidgetState,
    pub(crate) selection: SelectionModel,
    /// Selection styling last projected; equal inputs skip the per-item document writes.
    pub(crate) selection_visuals: Option<SelectionVisuals>,
    pub(crate) selection_start: Option<Point>,
    /// Latest rubber pointer position; applied once per flush so motion never redraws per event.
    pub(crate) rubber_pending: Option<Point>,
    pub(crate) selection_output: Option<OutputId>,
    /// Item rects frozen at rubber-gesture start; empty outside a gesture.
    pub(crate) rubber_items: Vec<RubberItem>,
    /// Overlay rect currently shown; `None` while hidden (initially hidden by the first-paint guard).
    pub(crate) selection_overlay: Option<Rect>,
    pub(crate) item_drag: Option<ItemDrag>,
    /// Latest item-drag pointer; applied once per flush so motion never rebuilds the ghost.
    pub(crate) drag_pending: Option<(f32, f32)>,
    pub(crate) launchers: BTreeMap<PathBuf, LauncherMeta>,
    pub(crate) menu: Option<MenuState>,
    /// Rects of the menus and confirm currently shown (logical px); empty when none is open.
    pub(crate) menu_rects: Vec<Rect>,
    pub(crate) wheel_hover: Option<usize>,
    /// Sector and accent of the highlight raster already uploaded; an equal pair reuses it.
    pub(crate) wheel_highlight: Option<(usize, [u8; 3])>,
    pub(crate) wheel_mode: WheelMode,
    pub(crate) pending_delete: Option<Vec<PathBuf>>,
    pub(crate) last_click: Option<ClickRecord>,
    pub(crate) dirty: Arc<AtomicBool>,
    /// Names the desktop directory watch reported since the last rescan; launcher stamps reuse
    /// cached metadata for entries a burst never touched.
    pub(crate) changed: Arc<ChangedPaths>,
    pub(crate) icons_pending: Arc<AtomicBool>,
    pub(crate) rename: Option<RenameState>,
    pub(crate) drag_ghost: Option<DragGhost>,
    pub(crate) conversion_preview: Option<ConversionPreview>,
    pub(crate) workspace: workspace_sync::WorkspaceAuthority,
    pub(crate) workspace_visual_dirty: bool,
    pub(crate) workspace_present_pending: bool,
    pub(crate) workspace_present_span: Option<flamewm_profiler::SpanGuard>,
    pub(crate) settings: SettingsSnapshot,
    pub(crate) settings_visual_dirty: bool,
    /// Shell-published forecast feed (the same payload the tray reads); revision 0 when none.
    pub(crate) weather: flamewm_api::weather::WeatherFeedSnapshot,
    pub(crate) weather_visual_dirty: bool,
    pub(crate) downloads: downloads::Organizer,
    /// Off-turn trash moves and emptying; completion arrives on the reactor wake fd.
    pub(crate) trash: TrashJobs,
    /// Resolved cursor theme last loaded into the live cursor owners.
    pub(crate) applied_cursor_theme: Option<String>,
    /// Pending backoff for a failed cursor theme reload; `None` after a success.
    pub(crate) cursor_theme_failure: Option<refresh::CursorThemeFailure>,
    /// Last `iconTheme` applied from the settings snapshot.
    pub(crate) icon_theme: String,
    /// Set when `iconTheme` changed; cleared once the new theme generation is installed.
    pub(crate) icon_theme_pending: bool,
    pub(crate) wallpaper_loader: wallpaper_loader::WallpaperLoader,
    /// Non-blocking control queue for derived settings (`wallpaperMeanRgb`). `None` when degraded.
    pub(crate) mutations: Option<std::sync::Arc<flamewm_control_dbus::MutationDispatcher>>,
    /// Loader generation whose mean was last computed; the mean is recomputed only on change.
    pub(crate) wallpaper_mean_generation: Option<u64>,
    /// Mean last published to the settings owner.
    pub(crate) published_wallpaper_mean: Option<u32>,
}

#[derive(Clone)]
pub(crate) struct RenameState {
    pub(crate) item_id: String,
    pub(crate) source_path: PathBuf,
    pub(crate) buffer: String,
}

pub(crate) struct ConversionPreview {
    pub(crate) output: OutputId,
    pub(crate) cell: Cell,
    pub(crate) destination: PathBuf,
    pub(crate) result: Arc<Mutex<Option<Result<PathBuf, String>>>>,
}

pub(crate) struct DragGhost {
    pub(crate) output: OutputId,
    pub(crate) source_ids: Vec<String>,
    pub(crate) kind: DesktopItemKind,
    pub(crate) icon_override: Option<String>,
    pub(crate) label: String,
    pub(crate) offset_x: f32,
    pub(crate) offset_y: f32,
    pub(crate) count: usize,
    pub(crate) visible: bool,
    /// Constant chrome (geometry, labels, icon) already projected; later motion only moves it.
    pub(crate) chrome_projected: bool,
    /// The icon raster currently shown came from the warm cache, so a drain can still replace it.
    pub(crate) icon_resolved: bool,
}

#[derive(Clone)]
pub(crate) struct ItemDrag {
    pub(crate) anchor_index: usize,
    pub(crate) anchor_id: String,
    pub(crate) start_x: f32,
    pub(crate) start_y: f32,
    pub(crate) originals: Vec<OriginalItem>,
    pub(crate) moved: bool,
}

#[derive(Clone)]
pub(crate) struct OriginalItem {
    pub(crate) id: String,
    pub(crate) path: PathBuf,
    pub(crate) output: OutputId,
    pub(crate) cell: Cell,
    pub(crate) drag_cell: Cell,
}

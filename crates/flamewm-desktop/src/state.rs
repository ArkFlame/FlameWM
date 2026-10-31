use super::*;

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
    pub(crate) selection_start: Option<Point>,
    /// Latest rubber pointer position; applied once per flush so motion never redraws per event.
    pub(crate) rubber_pending: Option<Point>,
    pub(crate) selection_output: Option<OutputId>,
    /// Item rects frozen at rubber-gesture start; empty outside a gesture.
    pub(crate) rubber_items: Vec<RubberItem>,
    /// Overlay rect currently shown; `None` while hidden (initially hidden by the first-paint guard).
    pub(crate) selection_overlay: Option<Rect>,
    pub(crate) item_drag: Option<ItemDrag>,
    pub(crate) launchers: BTreeMap<PathBuf, LauncherMeta>,
    pub(crate) menu: Option<MenuState>,
    /// Rects of the menus and confirm currently shown (logical px); empty when none is open.
    pub(crate) menu_rects: Vec<Rect>,
    pub(crate) wheel_hover: Option<usize>,
    pub(crate) wheel_mode: WheelMode,
    pub(crate) pending_delete: Option<Vec<PathBuf>>,
    pub(crate) last_click: Option<ClickRecord>,
    pub(crate) dirty: Arc<AtomicBool>,
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
    /// Resolved cursor theme last loaded into the live cursor owners.
    pub(crate) applied_cursor_theme: Option<String>,
    /// Last `iconTheme` applied from the settings snapshot.
    pub(crate) icon_theme: String,
    /// Set when `iconTheme` changed; cleared once the new theme generation is installed.
    pub(crate) icon_theme_pending: bool,
    pub(crate) wallpaper_loader: wallpaper_loader::WallpaperLoader,
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

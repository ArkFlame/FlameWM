use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Instant;

pub mod control;
mod display_layout;
pub mod file_chooser;
pub mod runtime;
pub mod theme_picker;
pub mod view;

#[cfg(test)]
mod controller;
#[cfg(test)]
mod projection;
#[cfg(test)]
mod ui_contract;

use flamewm_api::panels::PanelEdge;
use flamewm_api::session::SessionAction;
use flamewm_api::settings::SettingValue;
use flamewm_api::{ModeId, OutputId};
use flamewm_control_core::{ControlError, ControlRequest};
use flamewm_integrations_linux::media_store::{MediaKind, MediaStore, import_for_setting};
use flamewm_settings_core::{ControlTransport, DISPLAY_MODE_SLOTS, SettingsClient, SettingsPage};
use flamewm_ui_x11::{
    UiActionEvent, UiActionPhase, UiColor, UiDocumentAccess, UiKeyEvent, UiKeyPhase, UiWindowConfig,
};

const DONATE_URI: &str = "https://paypal.me/LinsaFTW";
const SOURCE_URI: &str = "https://github.com/ArkFlame/flamewm";
const WEBSITE_URI: &str = "https://wm.arkflame.com";

/// Canonical product version: the workspace version inherited by this crate.
const APP_VERSION: &str = env!("FLAMEWM_SETTINGS_VERSION");
/// Authoritative accent for the native FlameWM preset.
const DEFAULT_ACCENT: &str = "#EF4048";
/// Nodes whose compiled colour is the accent and must follow live accent changes.
/// Every `toggle-on` id in ui/settings.html must be listed here (checked by a test).
const ACCENT_NODE_IDS: &[&str] = &[
    "nav-selection-accent",
    "anim-global-on",
    "anim-start-on",
    "anim-window-on",
    "anim-minimize-on",
    "anim-maximize-on",
    "lock-date-on",
    "lock-username-on",
    "lock-suspend-on",
    "sticky-toggle-on",
    "focus-mouse-on",
    "snap-preview-on",
    "taskbar-seconds-on",
    "taskbar-date-on",
    "taskbar-weather-on",
    "taskbar-volume-on",
    "taskbar-network-on",
    "taskbar-battery-on",
    "taskbar-battery-relevant-on",
    "taskbar-clock-on",
    "taskbar-workspaces-on",
    "weather-enabled-on",
    "weather-auto-location-on",
    "time-auto-zone-on",
    "font-toggle-on",
    "wallpaper-share-on",
    "cursor-shadow-on",
];
/// Boolean ON/OFF twins: (setting key, ON id, OFF id, default when the key is absent).
const SETTING_TWINS: &[(&str, &str, &str, bool)] = &[
    (
        "shareWallpaper",
        "wallpaper-share-on",
        "wallpaper-share-off",
        false,
    ),
    (
        "cursorShadow",
        "cursor-shadow-on",
        "cursor-shadow-off",
        true,
    ),
    (
        "taskbarShowWeather",
        "taskbar-weather-on",
        "taskbar-weather-off",
        true,
    ),
    (
        "taskbarShowVolume",
        "taskbar-volume-on",
        "taskbar-volume-off",
        true,
    ),
    (
        "taskbarShowNetwork",
        "taskbar-network-on",
        "taskbar-network-off",
        true,
    ),
    (
        "taskbarShowBattery",
        "taskbar-battery-on",
        "taskbar-battery-off",
        true,
    ),
    (
        "taskbarBatteryOnlyWhenRelevant",
        "taskbar-battery-relevant-on",
        "taskbar-battery-relevant-off",
        false,
    ),
    (
        "taskbarShowClock",
        "taskbar-clock-on",
        "taskbar-clock-off",
        true,
    ),
    (
        "taskbarShowWorkspaces",
        "taskbar-workspaces-on",
        "taskbar-workspaces-off",
        true,
    ),
    (
        "weatherEnabled",
        "weather-enabled-on",
        "weather-enabled-off",
        true,
    ),
    (
        "autoLocationEnabled",
        "weather-auto-location-on",
        "weather-auto-location-off",
        false,
    ),
    (
        "autoTimeZoneEnabled",
        "time-auto-zone-on",
        "time-auto-zone-off",
        false,
    ),
];
/// Inline status line for failed requests. Optional in the compiled markup.
const ERROR_NODE: &str = "settings-error";
/// Version line on the About page. Optional in the compiled markup.
const ABOUT_VERSION_NODE: &str = "about-version";
/// Output rows per page on the Displays page and the Taskbar output list. The
/// Taskbar rows are optional in the compiled markup.
const OUTPUT_PAGE_SLOTS: usize = 4;

/// Process-local monotonic milliseconds for the in-process ControlTransport path.
/// The D-Bus wire adapter replaces this value with the reactor's monotonic clock.
fn monotonic_now_ms() -> u64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

/// Page-step direction for the output and mode pagers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PageStep {
    Prev,
    Next,
}

/// Page cursors for the output list (shared by the Displays and Taskbar pagers) and
/// the mode list. `generation` is the display snapshot generation the cursors were
/// last clamped against; a slot action carrying an older cursor is rejected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DisplayPages {
    /// `None` until the first successful refresh or action stamps the cursors.
    generation: Option<u64>,
    output_page: usize,
    mode_page: usize,
}

/// Node ids for one pager: its two step buttons, their disabled `-off` twins, and the label.
struct PagerNodes {
    prev: &'static str,
    prev_off: &'static str,
    next: &'static str,
    next_off: &'static str,
    label: &'static str,
}

const OUTPUT_PAGER: PagerNodes = PagerNodes {
    prev: "display-output-prev",
    prev_off: "display-output-prev-off",
    next: "display-output-next",
    next_off: "display-output-next-off",
    label: "display-output-page-label",
};

const TASKBAR_OUTPUT_PAGER: PagerNodes = PagerNodes {
    prev: "taskbar-output-prev",
    prev_off: "taskbar-output-prev-off",
    next: "taskbar-output-next",
    next_off: "taskbar-output-next-off",
    label: "taskbar-output-page-label",
};

const MODE_PAGER: PagerNodes = PagerNodes {
    prev: "display-mode-prev",
    prev_off: "display-mode-prev-off",
    next: "display-mode-next",
    next_off: "display-mode-next-off",
    label: "display-mode-page-label",
};

const ICON_THEME_PAGER: PagerNodes = PagerNodes {
    prev: "theme-icon-prev",
    prev_off: "theme-icon-prev-off",
    next: "theme-icon-next",
    next_off: "theme-icon-next-off",
    label: "theme-icon-page-label",
};

const CURSOR_THEME_PAGER: PagerNodes = PagerNodes {
    prev: "theme-cursor-prev",
    prev_off: "theme-cursor-prev-off",
    next: "theme-cursor-next",
    next_off: "theme-cursor-next-off",
    label: "theme-cursor-page-label",
};

fn theme_pager(kind: theme_picker::PickerKind) -> &'static PagerNodes {
    match kind {
        theme_picker::PickerKind::Icon => &ICON_THEME_PAGER,
        theme_picker::PickerKind::Cursor => &CURSOR_THEME_PAGER,
    }
}

fn page_count(len: usize, page_size: usize) -> usize {
    len.div_ceil(page_size)
}

/// Moves one page in `step`, clamped to `0..pages`. A step past either end is a no-op.
fn step_page(page: usize, step: PageStep, pages: usize) -> usize {
    let last = pages.saturating_sub(1);
    let page = page.min(last);
    match step {
        PageStep::Prev => page.saturating_sub(1),
        PageStep::Next => (page + 1).min(last),
    }
}

/// Absolute index of `slot` on `page`, i.e. `page * page_size + slot`. A cursor
/// stamped for another display generation is stale and rejected; a slot outside the
/// page, or an index past `len`, is a legible error. Nothing is applied on error.
fn page_slot_index(
    cursor_generation: u64,
    current_generation: u64,
    page: usize,
    page_size: usize,
    slot: usize,
    len: usize,
    what: &str,
) -> Result<usize, String> {
    if cursor_generation != current_generation {
        return Err(format!(
            "stale {what} page: cursor is for display generation {cursor_generation} but the authoritative generation is {current_generation}; page again"
        ));
    }
    if slot >= page_size {
        return Err(format!(
            "{what} slot {slot} is outside the page of {page_size}"
        ));
    }
    let index = page
        .checked_mul(page_size)
        .and_then(|base| base.checked_add(slot))
        .ok_or_else(|| format!("{what} page {page} overflows the index space"))?;
    if index >= len {
        return Err(format!("no {what} at index {index}; {len} available"));
    }
    Ok(index)
}

/// "1-4 of 5" for the items on `page`; "0 of 0" when there are none.
fn page_label(page: usize, page_size: usize, total: usize) -> String {
    if total == 0 {
        return "0 of 0".to_owned();
    }
    let start = page.saturating_mul(page_size).saturating_add(1).min(total);
    let end = page.saturating_add(1).saturating_mul(page_size).min(total);
    format!("{start}-{end} of {total}")
}

/// Projects one pager. A step button is visible only when it can move, and its
/// `-off` twin is visible exactly when the button is disabled.
fn project_pager(
    document: &mut impl UiDocumentAccess,
    nodes: &PagerNodes,
    page: usize,
    pages: usize,
    label: String,
) -> Result<(), String> {
    let has_prev = page > 0;
    let has_next = page + 1 < pages;
    document.visible(nodes.prev, has_prev)?;
    document.visible(nodes.prev_off, !has_prev)?;
    document.visible(nodes.next, has_next)?;
    document.visible(nodes.next_off, !has_next)?;
    document.text(nodes.label, label)
}

#[must_use]
pub fn normal_window_config(width: u32, height: u32) -> UiWindowConfig {
    UiWindowConfig {
        width,
        height,
        x: 0,
        y: 0,
        title: "System Settings".to_owned(),
        identity: Some(flamewm_ui_x11::X11WindowIdentity {
            instance: "flamewm-settings".to_owned(),
            class: "FlameWM Settings".to_owned(),
        }),
    }
}

/// One curated hotkey row: its markup slot, the shortcut action it edits,
/// its display label, and the optional node that shows its binding.
struct HotkeyRow {
    slot: &'static str,
    action: &'static str,
    label: &'static str,
    node: &'static str,
}

static HOTKEY_ROWS: [HotkeyRow; 9] = [
    HotkeyRow {
        slot: "start",
        action: flamewm_api::shortcuts::ACTION_TOGGLE_START_MENU,
        label: "Toggle Start menu",
        node: "hotkey-key-start",
    },
    HotkeyRow {
        slot: "left",
        action: flamewm_api::shortcuts::ACTION_WORKSPACE_LEFT,
        label: "Desktop left",
        node: "hotkey-key-left",
    },
    HotkeyRow {
        slot: "right",
        action: flamewm_api::shortcuts::ACTION_WORKSPACE_RIGHT,
        label: "Desktop right",
        node: "hotkey-key-right",
    },
    HotkeyRow {
        slot: "up",
        action: flamewm_api::shortcuts::ACTION_WORKSPACE_UP,
        label: "Desktop up",
        node: "hotkey-key-up",
    },
    HotkeyRow {
        slot: "down",
        action: flamewm_api::shortcuts::ACTION_WORKSPACE_DOWN,
        label: "Desktop down",
        node: "hotkey-key-down",
    },
    HotkeyRow {
        slot: "shot-full",
        action: flamewm_api::shortcuts::ACTION_SCREENSHOT_FULL,
        label: "Capture all screens",
        node: "hotkey-key-shot-full",
    },
    HotkeyRow {
        slot: "shot-region",
        action: flamewm_api::shortcuts::ACTION_SCREENSHOT_REGION,
        label: "Capture and annotate region",
        node: "hotkey-key-shot-region",
    },
    HotkeyRow {
        slot: "shot-window",
        action: flamewm_api::shortcuts::ACTION_SCREENSHOT_WINDOW,
        label: "Capture active window",
        node: "hotkey-key-shot-window",
    },
    HotkeyRow {
        slot: "lock",
        action: flamewm_api::shortcuts::ACTION_SESSION_LOCK,
        label: "Lock screen",
        node: "hotkey-key-lock",
    },
];

#[derive(Debug, Clone, Copy)]
enum HotkeyOperation {
    Capture,
    Clear,
    Reset,
}

/// Which setting the shared file chooser writes on completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChooserTarget {
    Wallpaper,
    LockBackground,
    Avatar,
    StartIcon,
}

impl ChooserTarget {
    /// The managed media family the selected file is copied into.
    fn media_kind(self) -> MediaKind {
        match self {
            Self::Wallpaper => MediaKind::DesktopWallpaper,
            Self::LockBackground => MediaKind::LockWallpaper,
            Self::Avatar => MediaKind::Avatar,
            Self::StartIcon => MediaKind::StartIcon,
        }
    }
}

/// Presentation sub-pages that the Appearance nav rows open. They share the core
/// `SettingsPage::Appearance` and are local to this window, never WM state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppearanceTab {
    Theme,
    StartMenu,
    Surfaces,
    Animations,
    LockScreen,
    Screensaver,
}

impl AppearanceTab {
    const ALL: [Self; 6] = [
        Self::Theme,
        Self::StartMenu,
        Self::Surfaces,
        Self::Animations,
        Self::LockScreen,
        Self::Screensaver,
    ];

    /// The markup suffix: nav action `settings.page.<node>` and section `page-<node>`.
    fn node(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::StartMenu => "start-menu",
            Self::Surfaces => "surfaces",
            Self::Animations => "animations",
            Self::LockScreen => "lock-screen",
            Self::Screensaver => "screensaver",
        }
    }

    fn parse(node: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tab| tab.node() == node)
    }
}

/// Every control action the Settings markup may emit. Parsing is pure, so the
/// markup coverage test can check each `data-action` without side effects.
enum SettingsAction {
    Page(SettingsPage),
    AccentSwatch(&'static str),
    /// Shows one Appearance sub-page from the nav.
    AppearanceTab(AppearanceTab),
    /// Opens the Start icon chooser. Import is not wired yet (see `execute`).
    StartIconChoose,
    /// Clears the Start icon and removes its managed copy.
    StartIconClear,
    /// Opens the icon or cursor theme picker on its first page.
    ThemePickerOpen(theme_picker::PickerKind),
    ThemePickerClose,
    ThemePickerPage(theme_picker::PickerKind, PageStep),
    /// Row `slot` on the open picker's current page.
    ThemePickerSelect(theme_picker::PickerKind, usize),
    /// Opens the portal chooser for one archive, or one folder when `folder` is set.
    ThemeInstallChoose {
        folder: bool,
    },
    WallpaperChoose,
    /// Shows the bundled wallpaper; the saved custom path is kept.
    WallpaperDefault,
    /// Shows the solid wallpaper colour; the saved custom path is kept.
    WallpaperClear,
    /// Opens the shared file chooser for the lock-screen background image.
    LockBackgroundChoose,
    /// Shows the solid lock colour; the saved custom path is kept.
    LockBackgroundClear,
    /// Shows the bundled lock background; the saved custom path is kept.
    LockBackgroundDefault,
    /// Opens the shared file chooser for the lock-screen profile picture.
    AvatarChoose,
    AvatarClear,
    /// Starts the lock screen in preview mode through the desktop control bus.
    LockPreview,
    /// Starts the screensaver in preview mode through the desktop control bus.
    ScreensaverPreview,
    /// Writes one fixed integer value to a setting, from an explicit option button.
    IntegerSelect {
        key: &'static str,
        value: i64,
    },
    Toggle(&'static str),
    SelectionOpacity(i8),
    SnapOpacity(i8),
    DesktopEntrySize(i8),
    FontSize(i8),
    TaskbarEdgeCycle,
    TaskbarEdge(PanelEdge),
    TaskbarSizeCycle,
    FontFamilyCycle,
    Hotkey(&'static str, HotkeyOperation),
    /// Output row `slot` on the current output page.
    DisplaySelect(usize),
    DisplayArrange(display_layout::Side),
    /// Mode row `slot` on the current mode page.
    DisplaySlot(usize),
    OutputPage(PageStep),
    ModePage(PageStep),
    DisplayBegin(ModeId),
    DisplayScale(u16),
    DisplayKeep,
    DisplayRevert,
    OpenUri(&'static str),
    /// Opens the shared colour picker for one `#RRGGBB` setting.
    ColorPickerOpen(&'static str),
    ColorPickerClose,
    /// Focuses the picker's hex field so typed keys edit its buffer.
    ColorPickerFocus,
    /// Applies one palette cell (`#RRGGBB`) to the picker's setting at once.
    ColorPickerPick(String),
    /// Validates the hex buffer and applies it; an invalid buffer writes nothing.
    ColorPickerApply,
    /// Sets a `#RRGGBB` setting to one fixed preset value in one transaction.
    ColorSwatch {
        key: &'static str,
        value: &'static str,
    },
    /// Steps a percent setting by 5 within 0..=100.
    PercentStep(&'static str, i8),
    /// Adds `delta` to an integer setting and clamps the result to `minimum..=maximum`.
    IntegerStep {
        key: &'static str,
        delta: i64,
        minimum: i64,
        maximum: i64,
    },
    /// Drag-only slider; the value arrives as event text, see `handle_animation_speed_drag`.
    AnimationSpeedSlider,
    /// Drag-only selected output scale slider; only Release changes runtime scale.
    DisplayScaleSlider,
    /// Advances a text setting through `values`, wrapping.
    TextCycle {
        key: &'static str,
        values: &'static [&'static str],
    },
    /// Writes one fixed text value to a setting, from an explicit option button.
    TextSelect {
        key: &'static str,
        value: &'static str,
    },
    /// A native `keyboard.*` id from the X11 key source. It is not markup, so
    /// it never edits a row; hotkey capture is driven by the key-event path.
    NativeKeyEvent,
}

pub struct SettingsApplication<T: ControlTransport> {
    client: SettingsClient<T>,
    wallpaper_chooser: Option<file_chooser::WallpaperChooserWorker>,
    /// The setting the open file chooser writes when it completes.
    chooser_target: ChooserTarget,
    pending_open: Option<Receiver<Result<(), String>>>,
    /// True only between a hotkey row capture and its committing key release.
    /// Gates key events so a stale capture armed by the core cannot commit later.
    hotkey_armed: bool,
    error: Option<String>,
    pages: DisplayPages,
    themes: theme_picker::ThemeRegistry,
    /// The open theme picker and its current page.
    theme_picker: Option<(theme_picker::PickerKind, usize)>,
    color_picker: Option<ColorPicker>,
    theme_scan: Option<Receiver<theme_picker::ThemeRegistry>>,
    theme_install: Option<Receiver<theme_picker::InstallOutcome>>,
    theme_status: Option<String>,
    appearance_tab: AppearanceTab,
    /// Resolved cursor theme last loaded into this window's live cursor owners.
    applied_cursor_theme: Option<String>,
    /// Pointer origin for an output card; motion remains local, only release
    /// causes a native RandR request.
    monitor_drag: Option<(usize, f32, u64, OutputId)>,
    /// Non-authoritative drag preview, bound to the original output and generation.
    scale_drag: Option<(OutputId, u64, u16)>,
    /// Arrow keys affect scale only after the slider is focused by pointer.
    scale_slider_focused: bool,
}

impl<T: ControlTransport> SettingsApplication<T> {
    pub fn new(transport: T) -> Self {
        Self {
            client: SettingsClient::new(transport),
            wallpaper_chooser: None,
            chooser_target: ChooserTarget::Wallpaper,
            pending_open: None,
            hotkey_armed: false,
            error: None,
            pages: DisplayPages::default(),
            themes: theme_picker::ThemeRegistry::default(),
            theme_picker: None,
            color_picker: None,
            theme_scan: None,
            theme_install: None,
            theme_status: None,
            appearance_tab: AppearanceTab::Theme,
            applied_cursor_theme: None,
            monitor_drag: None,
            scale_drag: None,
            scale_slider_focused: false,
        }
    }

    pub fn client(&self) -> &SettingsClient<T> {
        &self.client
    }
    pub fn client_mut(&mut self) -> &mut SettingsClient<T> {
        &mut self.client
    }

    /// Message from the last failed request, shown inline until the next success.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn refresh(&mut self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        self.client.refresh_all().map_err(control_error)?;
        self.sync_cursor_theme();
        self.reconcile_pages();
        self.sync(document)
    }

    /// Shows a failed request inline. The authoritative snapshots are left as
    /// they were, so the previous values stay on screen.
    pub fn report_error(&mut self, document: &mut impl UiDocumentAccess, message: String) {
        self.error = Some(message);
        self.sync_error(document);
    }

    pub fn handle_action(
        &mut self,
        event: &UiActionEvent,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if event.action == DISPLAY_SCALE_SLIDER {
            return self.handle_display_scale_drag(event, document);
        }
        if event.action == ANIMATION_SPEED_SLIDER {
            return self.handle_animation_speed_drag(event, document);
        }
        if let Some(raw_slot) = event.action.strip_prefix("settings.display.select.") {
            if let Ok(slot) = raw_slot.parse::<usize>() {
                if event.phase == UiActionPhase::Press {
                    self.monitor_drag = self.output_page_slot(slot).and_then(|output| {
                        self.client
                            .displays()
                            .map(|snapshot| (slot, event.x, snapshot.generation, output.id.clone()))
                    });
                    return Ok(());
                }
                if event.phase == UiActionPhase::Motion {
                    if let Some((pressed_slot, origin_x, _, _)) = &self.monitor_drag {
                        if *pressed_slot == slot && (event.x - origin_x).abs() >= 40.0 {
                            // Only visual feedback while dragging, never native RandR state.
                            let accent = self.accent()?;
                            document.border(&format!("display-output-{slot}"), accent)?;
                        }
                    }
                    return Ok(());
                }
                if event.phase == UiActionPhase::Release {
                    let origin = self.monitor_drag.take();
                    if let Some((_pressed_slot, initial_x, generation, pressed_id)) =
                        origin.filter(|(pressed_slot, _, _, _)| *pressed_slot == slot)
                    {
                        let distance = event.x - initial_x;
                        if distance.abs() >= 40.0 {
                            let current_generation =
                                self.client.displays().map_or(0, |s| s.generation);
                            if generation != current_generation
                                || self
                                    .output_page_slot(slot)
                                    .is_none_or(|o| o.id != pressed_id)
                            {
                                return Err("display topology changed during monitor drag".into());
                            }
                            self.client.select_output(&pressed_id);
                            let side = if distance < 0.0 {
                                display_layout::Side::Left
                            } else {
                                display_layout::Side::Right
                            };
                            self.arrange_display(side)?;
                            return self.settle(document);
                        }
                    }
                }
            }
        }
        if event.phase == UiActionPhase::Press {
            self.scale_slider_focused = false;
        }
        if event.phase != UiActionPhase::Release || !event.inside {
            return Ok(());
        }
        if event.action == KEYBOARD_INPUT_ACTION && self.color_picker.is_some() {
            self.color_picker_text(event.text.as_deref().unwrap_or_default())?;
            return self.settle(document);
        }
        let action = parse_action(event.action.as_str())?;
        self.execute(action)?;
        self.settle(document)
    }

    /// Drag preview is display-local and never persists until the pointer releases,
    /// including release outside the original slider bounds.
    fn handle_display_scale_drag(
        &mut self,
        event: &UiActionEvent,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if event.button != 1 {
            return Ok(());
        }
        match event.phase {
            UiActionPhase::Press => {
                let (id, generation, value) = self.scale_selection()?;
                self.scale_drag = Some((id, generation, value));
                self.scale_slider_focused = true;
            }
            UiActionPhase::Motion | UiActionPhase::Release => {
                let Some((id, generation, _)) = self.scale_drag.as_ref().cloned() else {
                    return Ok(());
                };
                let Some(text) = event.text.as_deref() else {
                    if event.phase == UiActionPhase::Release {
                        self.scale_drag = None;
                    }
                    return Ok(());
                };
                let percent = display_scale_from_slider(text)?;
                if event.phase == UiActionPhase::Release {
                    self.scale_drag = None;
                    let (current_id, current_generation, current_scale) = self.scale_selection()?;
                    if current_id != id || current_generation != generation {
                        return Err("display topology changed during scale adjustment".into());
                    }
                    if percent != current_scale {
                        self.client
                            .set_shell_scale(percent)
                            .map_err(control_error)?;
                    }
                    return self.settle(document);
                }
                self.scale_drag = Some((id, generation, percent));
                self.sync_display_scale(document, Some(percent))?;
            }
            UiActionPhase::Hover => {}
        }
        Ok(())
    }

    fn scale_selection(&self) -> Result<(OutputId, u64, u16), String> {
        let snapshot = self
            .client
            .displays()
            .ok_or("display snapshot unavailable")?;
        let output = self.selected_display().ok_or("no selected display")?;
        Ok((
            output.id.clone(),
            snapshot.generation,
            output.shell_scale_percent,
        ))
    }

    /// The native range drag reports the pointer as `text` (0..=100) on every
    /// Motion and on the final Release, even when the pointer left the track.
    fn handle_animation_speed_drag(
        &mut self,
        event: &UiActionEvent,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if !matches!(event.phase, UiActionPhase::Motion | UiActionPhase::Release) {
            return Ok(());
        }
        let Some(text) = event.text.as_deref() else {
            return Ok(());
        };
        let speed = animation_speed_from_slider(text)?;
        if self.integer_setting("animationSpeed").ok() != Some(speed) {
            self.client
                .apply_setting("animationSpeed", SettingValue::Integer(speed))
                .map_err(control_error)?;
        }
        self.settle(document)
    }

    pub fn handle_key_event(
        &mut self,
        event: &UiKeyEvent,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        // Escape cancels an in-progress monitor gesture without ever applying RandR.
        if event.phase == UiKeyPhase::Release
            && event.key == "Escape"
            && self.monitor_drag.take().is_some()
        {
            return self.settle(document);
        }
        if event.phase == UiKeyPhase::Release && !self.hotkey_armed && self.scale_slider_focused {
            if event.key == "Escape" {
                self.scale_slider_focused = false;
                self.scale_drag = None;
                return self.settle(document);
            }
            let delta = match event.key.as_str() {
                "Left" | "Down" => -5,
                "Right" | "Up" => 5,
                "Home" => -100,
                "End" => 100,
                _ => 0,
            };
            if delta != 0 && event.modifiers.is_empty() {
                let (_, _, current) = self.scale_selection()?;
                let target = (i32::from(current) + delta).clamp(100, 200) as u16;
                if current != target {
                    self.client.set_shell_scale(target).map_err(control_error)?;
                }
                return self.settle(document);
            }
        }
        if event.phase == UiKeyPhase::Release && self.hotkey_armed {
            if let Some(binding) = bare_super_binding(event) {
                self.finish_hotkey_capture(binding)?;
            } else if !is_modifier_key(&event.key) {
                let binding = if event.modifiers.is_empty() {
                    event.key.clone()
                } else {
                    format!("{}+{}", event.modifiers.join("+"), event.key)
                };
                self.finish_hotkey_capture(&binding)?;
            }
        }
        self.settle(document)
    }

    pub fn poll_background(&mut self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let mut completion_errors: Vec<String> = self.poll_open_uri().err().into_iter().collect();
        if self.poll_theme_jobs() {
            if let Err(error) = self.sync(document) {
                completion_errors.push(error);
            }
        }
        let result = self
            .wallpaper_chooser
            .as_mut()
            .and_then(file_chooser::WallpaperChooserWorker::poll);
        if let Some(result) = result {
            self.wallpaper_chooser = None;
            let mut wallpaper_changed = false;
            match result {
                Ok(Some(path)) => {
                    let applied = match import_for_setting(self.chooser_target.media_kind(), &path)
                    {
                        Ok(managed) => match self.chooser_target {
                            ChooserTarget::Wallpaper => self
                                .client
                                .set_wallpaper(managed)
                                .map_err(control_error)
                                .and_then(|()| {
                                    self.apply_text("wallpaperMode", "custom".to_owned())
                                }),
                            ChooserTarget::LockBackground => self
                                .apply_text("lockBackgroundImage", managed)
                                .and_then(|()| {
                                    self.apply_text("lockBackgroundMode", "custom".to_owned())
                                }),
                            ChooserTarget::Avatar => self.apply_text("avatarImage", managed),
                            ChooserTarget::StartIcon => self.replace_start_icon(managed),
                        },
                        Err(error) => Err(error),
                    };
                    match applied {
                        Ok(()) => wallpaper_changed = true,
                        Err(error) => completion_errors.push(error),
                    }
                }
                Ok(None) => {}
                Err(error) => completion_errors.push(error),
            }
            if wallpaper_changed {
                self.settle(document)?;
            }
        }
        if completion_errors.is_empty() {
            Ok(())
        } else {
            Err(completion_errors.join("; "))
        }
    }

    /// Clears the inline error and projects the authoritative snapshots. Called
    /// only after a request succeeded.
    /// Reloads this window's cursor owners when the resolved `cursorTheme` changed. A failed
    /// reload keeps the previous theme (rollback) and is retried at the next settings snapshot.
    fn sync_cursor_theme(&mut self) {
        let shadow_enabled = !matches!(
            self.setting_value("cursorShadow"),
            Some(SettingValue::Boolean(false))
        );
        for error in flamewm_ui_x11::apply_cursor_shadow(shadow_enabled) {
            eprintln!("settings: cursor shadow reload failed: {error}");
        }
        let Ok(requested) = self.text_setting("cursorTheme") else {
            return;
        };
        let search_path = std::env::var("XCURSOR_PATH").unwrap_or_default();
        let Some(theme) = flamewm_session_core::cursor_theme_reload(
            self.applied_cursor_theme.as_deref(),
            &requested,
            &search_path,
        ) else {
            return;
        };
        let errors = flamewm_ui_x11::apply_cursor_theme(&theme);
        if errors.is_empty() {
            self.applied_cursor_theme = Some(theme);
        }
        for error in errors {
            eprintln!("settings: cursor theme reload failed: {error}");
        }
    }

    fn settle(&mut self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        self.error = None;
        self.sync_cursor_theme();
        self.reconcile_pages();
        self.sync(document)
    }

    /// Clamps both page cursors to the authoritative snapshots and stamps them with the
    /// current display generation. Runs after every successful refresh or action.
    fn reconcile_pages(&mut self) {
        let generation = self
            .client
            .displays()
            .map_or(0, |snapshot| snapshot.generation);
        let output_pages = page_count(self.output_count(), OUTPUT_PAGE_SLOTS);
        let mode_pages = page_count(self.client.display_mode_count(), DISPLAY_MODE_SLOTS);
        self.pages = DisplayPages {
            generation: Some(generation),
            output_page: self.pages.output_page.min(output_pages.saturating_sub(1)),
            mode_page: self.pages.mode_page.min(mode_pages.saturating_sub(1)),
        };
    }

    fn output_count(&self) -> usize {
        self.client
            .displays()
            .map_or(0, |snapshot| snapshot.outputs.len())
    }

    /// Stable visual left-to-right ordering derived from actual RandR geometry,
    /// not X server enumeration order. Output IDs remain the native authority.
    fn ordered_output_indices(&self) -> Vec<usize> {
        let Some(snapshot) = self.client.displays() else {
            return vec![];
        };
        let mut indices: Vec<_> = (0..snapshot.outputs.len()).collect();
        indices.sort_by_key(|&i| {
            let o = &snapshot.outputs[i];
            (
                !o.geometry.is_valid(),
                if o.geometry.is_valid() {
                    o.geometry.x
                } else {
                    i32::MAX
                },
                if o.geometry.is_valid() {
                    o.geometry.y
                } else {
                    i32::MAX
                },
                i,
            )
        });
        indices
    }

    fn output_page_index(&self, slot: usize) -> Option<usize> {
        let absolute = self
            .pages
            .output_page
            .checked_mul(OUTPUT_PAGE_SLOTS)?
            .checked_add(slot)?;
        self.ordered_output_indices().get(absolute).copied()
    }

    /// Output row `slot` in visual left-to-right order; page/slot and the
    /// selection/drag input path MUST use this same mapping.
    fn output_page_slot(&self, slot: usize) -> Option<&flamewm_api::display::OutputSnapshot> {
        self.client
            .displays()?
            .outputs
            .get(self.output_page_index(slot)?)
    }

    fn execute(&mut self, action: SettingsAction) -> Result<(), String> {
        // A native key id (e.g. the Super release that ends a capture gesture)
        // must not disarm capture; the key-event path decides when it commits.
        if !matches!(
            action,
            SettingsAction::Hotkey(_, HotkeyOperation::Capture) | SettingsAction::NativeKeyEvent
        ) {
            self.client.cancel_hotkey_capture().map_err(control_error)?;
            self.hotkey_armed = false;
        }
        if let Some(picker) = self.color_picker.as_mut() {
            if !matches!(
                action,
                SettingsAction::ColorPickerFocus | SettingsAction::NativeKeyEvent
            ) {
                picker.focused = false;
            }
        }
        match action {
            SettingsAction::NativeKeyEvent => {}
            SettingsAction::ColorPickerOpen(key) => {
                self.color_picker = Some(ColorPicker {
                    key,
                    buffer: String::new(),
                    focused: true,
                    error: None,
                });
            }
            SettingsAction::ColorPickerClose => self.color_picker = None,
            SettingsAction::ColorPickerFocus => {
                if let Some(picker) = self.color_picker.as_mut() {
                    picker.focused = true;
                }
            }
            SettingsAction::ColorPickerPick(value) => {
                let key = self.open_color_picker()?.key;
                self.apply_text(key, value.clone())?;
                if let Some(picker) = self.color_picker.as_mut() {
                    picker.buffer = value;
                    picker.error = None;
                }
            }
            SettingsAction::ColorPickerApply => self.apply_picker_buffer()?,
            SettingsAction::Page(page) => {
                self.scale_slider_focused = false;
                self.scale_drag = None;
                self.client.set_page(page);
            }
            SettingsAction::AccentSwatch(accent) => {
                self.apply_text("accent", accent.to_owned())?;
            }
            SettingsAction::AppearanceTab(tab) => {
                self.appearance_tab = tab;
                self.client.set_page(SettingsPage::Appearance);
            }
            SettingsAction::StartIconChoose => self.start_file_chooser(ChooserTarget::StartIcon)?,
            SettingsAction::StartIconClear => {
                let previous = self.text_setting("customStartIcon")?;
                self.apply_text("customStartIcon", String::new())?;
                remove_managed_media(&previous)?;
            }
            SettingsAction::ThemePickerOpen(kind) => self.theme_picker = Some((kind, 0)),
            SettingsAction::ThemePickerClose => self.theme_picker = None,
            SettingsAction::ThemePickerPage(kind, step) => {
                let page = self.open_theme_page(kind)?;
                let pages = page_count(self.themes.items(kind).len(), theme_picker::PICKER_SLOTS);
                self.theme_picker = Some((kind, step_page(page, step, pages)));
            }
            SettingsAction::ThemePickerSelect(kind, slot) => {
                let index = self.open_theme_page(kind)? * theme_picker::PICKER_SLOTS + slot;
                let id = self
                    .themes
                    .items(kind)
                    .get(index)
                    .map(|item| item.id.clone())
                    .ok_or_else(|| format!("no theme at index {index}; rescan the themes"))?;
                self.apply_text(kind.setting_key(), id)?;
                self.theme_picker = None;
            }
            SettingsAction::ThemeInstallChoose { folder } => self.start_theme_install(folder)?,
            SettingsAction::WallpaperChoose => self.start_file_chooser(ChooserTarget::Wallpaper)?,
            SettingsAction::WallpaperDefault => {
                self.apply_text("wallpaperMode", "default".to_owned())?;
            }
            SettingsAction::WallpaperClear => {
                self.apply_text("wallpaperMode", "solid".to_owned())?;
            }
            SettingsAction::LockBackgroundChoose => {
                self.start_file_chooser(ChooserTarget::LockBackground)?;
            }
            SettingsAction::LockBackgroundClear => {
                self.apply_text("lockBackgroundMode", "solid".to_owned())?;
            }
            SettingsAction::LockBackgroundDefault => {
                self.apply_text("lockBackgroundMode", "default".to_owned())?;
            }
            SettingsAction::AvatarChoose => {
                self.start_file_chooser(ChooserTarget::Avatar)?;
            }
            SettingsAction::AvatarClear => {
                self.apply_text("avatarImage", String::new())?;
            }
            SettingsAction::LockPreview => start_session_preview(SessionAction::LockPreview)?,
            SettingsAction::ScreensaverPreview => {
                start_session_preview(SessionAction::ScreensaverPreview)?;
            }
            SettingsAction::IntegerSelect { key, value } => {
                self.client
                    .apply_setting(key, SettingValue::Integer(value))
                    .map_err(control_error)?;
            }
            SettingsAction::Toggle(key) => {
                let enabled = self
                    .setting_value(key)
                    .and_then(|value| match value {
                        SettingValue::Boolean(value) => Some(!value),
                        _ => None,
                    })
                    .ok_or_else(|| format!("toggle '{key}' has no authoritative boolean value"))?;
                self.client
                    .apply_setting(key, SettingValue::Boolean(enabled))
                    .map_err(control_error)?;
            }
            SettingsAction::SelectionOpacity(direction) => {
                self.step_percent("DesktopSelectionFillOpacity", direction, 60)?;
            }
            SettingsAction::SnapOpacity(direction) => {
                self.step_percent("WindowSnapPreviewFillOpacity", direction, 60)?;
            }
            SettingsAction::DesktopEntrySize(direction) => {
                self.step_integer(
                    "DesktopEntryScalePercent",
                    i64::from(direction) * 10,
                    50,
                    200,
                )?;
            }
            SettingsAction::FontSize(direction) => {
                self.step_integer("fontSizeOffset", i64::from(direction), -2, 4)?;
            }
            SettingsAction::ColorSwatch { key, value } => self.apply_text(key, value.to_owned())?,
            SettingsAction::PercentStep(key, direction) => {
                self.step_percent(key, direction, 100)?
            }
            SettingsAction::IntegerStep {
                key,
                delta,
                minimum,
                maximum,
            } => self.step_integer(key, delta, minimum, maximum)?,
            // Value is applied from the drag text in `handle_animation_speed_drag`.
            SettingsAction::AnimationSpeedSlider | SettingsAction::DisplayScaleSlider => {}
            SettingsAction::TextCycle { key, values } => {
                let current = self.text_setting(key)?;
                self.apply_text(key, cycle_text(values, &current).to_owned())?;
            }
            SettingsAction::TextSelect { key, value } => self.apply_text(key, value.to_owned())?,
            SettingsAction::TaskbarEdgeCycle => self.cycle_panel_edge()?,
            SettingsAction::TaskbarEdge(edge) => {
                let (output, _) = self.selected_panel()?;
                self.client
                    .set_panel_edge(output, edge)
                    .map_err(control_error)?;
            }
            // Size is the global `taskbarHeight` service setting, shared by all outputs.
            SettingsAction::TaskbarSizeCycle => self.set_taskbar_size(1)?,
            SettingsAction::FontFamilyCycle => self.cycle_font_family()?,
            SettingsAction::Hotkey(action, operation) => self.handle_hotkey(action, operation)?,
            SettingsAction::DisplayArrange(side) => self.arrange_display(side)?,
            SettingsAction::DisplaySelect(slot) => {
                let generation = self
                    .client
                    .displays()
                    .map_or(0, |snapshot| snapshot.generation);
                let index = page_slot_index(
                    self.pages.generation.unwrap_or(generation),
                    generation,
                    self.pages.output_page,
                    OUTPUT_PAGE_SLOTS,
                    slot,
                    self.output_count(),
                    "display output",
                )?;
                let output_id = self
                    .ordered_output_indices()
                    .get(index)
                    .and_then(|&physical| {
                        self.client
                            .displays()
                            .and_then(|snapshot| snapshot.outputs.get(physical))
                    })
                    .map(|output| output.id.clone())
                    .ok_or_else(|| format!("no display output at index {index}"))?;
                if !self.client.select_output(&output_id) {
                    return Err(format!(
                        "display output {index} is not an authoritative output"
                    ));
                }
                // Modes belong to the selected output, so its mode list starts on page 1.
                self.pages.mode_page = 0;
                self.scale_slider_focused = false;
                self.scale_drag = None;
            }
            SettingsAction::DisplaySlot(slot) => {
                let generation = self
                    .client
                    .displays()
                    .map_or(0, |snapshot| snapshot.generation);
                let index = page_slot_index(
                    self.pages.generation.unwrap_or(generation),
                    generation,
                    self.pages.mode_page,
                    DISPLAY_MODE_SLOTS,
                    slot,
                    self.client.display_mode_count(),
                    "display mode",
                )?;
                let mode = self.client.display_mode_for_index(index).ok_or_else(|| {
                    format!("display mode at index {index} has no authoritative mode")
                })?;
                self.begin_display_mode(mode)?;
            }
            SettingsAction::OutputPage(step) => {
                let pages = page_count(self.output_count(), OUTPUT_PAGE_SLOTS);
                self.pages.output_page = step_page(self.pages.output_page, step, pages);
            }
            SettingsAction::ModePage(step) => {
                let pages = page_count(self.client.display_mode_count(), DISPLAY_MODE_SLOTS);
                self.pages.mode_page = step_page(self.pages.mode_page, step, pages);
            }
            SettingsAction::DisplayBegin(mode) => self.begin_display_mode(mode)?,
            SettingsAction::DisplayScale(percent) => {
                self.client
                    .set_shell_scale(percent)
                    .map_err(control_error)?;
            }
            SettingsAction::DisplayKeep => self
                .client
                .keep_display_mode(monotonic_now_ms())
                .map_err(control_error)?,
            SettingsAction::DisplayRevert => {
                self.client.revert_display_mode().map_err(control_error)?;
            }
            SettingsAction::OpenUri(uri) => self.start_open_uri(uri)?,
        }
        Ok(())
    }

    fn start_file_chooser(&mut self, target: ChooserTarget) -> Result<(), String> {
        if self.wallpaper_chooser.is_some() {
            return Err("file chooser is already open".to_owned());
        }
        self.wallpaper_chooser = Some(file_chooser::WallpaperChooserWorker::start()?);
        self.chooser_target = target;
        Ok(())
    }

    /// Starts a catalog scan unless one is already in flight. Runs off the UI thread.
    pub fn start_theme_scan(&mut self) {
        if self.theme_scan.is_some() {
            return;
        }
        match theme_picker::start_scan(theme_picker::theme_data_dirs_from_env()) {
            Ok(receiver) => self.theme_scan = Some(receiver),
            Err(error) => self.theme_status = Some(error),
        }
    }

    /// Applies finished scan and install results. Returns true when the view must be projected.
    fn poll_theme_jobs(&mut self) -> bool {
        let mut changed = false;
        match self.theme_scan.as_ref().map(Receiver::try_recv) {
            Some(Ok(registry)) => {
                self.theme_scan = None;
                self.themes = registry;
                if let Some((kind, page)) = self.theme_picker {
                    let pages =
                        page_count(self.themes.items(kind).len(), theme_picker::PICKER_SLOTS);
                    self.theme_picker = Some((kind, page.min(pages.saturating_sub(1))));
                }
                changed = true;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.theme_scan = None;
                self.theme_status = Some("theme scan ended without a result".to_owned());
                changed = true;
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
        match self.theme_install.as_ref().map(Receiver::try_recv) {
            Some(Ok(outcome)) => {
                self.theme_install = None;
                self.theme_status = Some(match outcome {
                    Ok(Some(installed)) => {
                        // Rescan so the installed theme appears in the picker.
                        self.theme_scan = None;
                        self.start_theme_scan();
                        format!("Installed {}", installed.id)
                    }
                    Ok(None) => "Install cancelled".to_owned(),
                    Err(error) => error,
                });
                changed = true;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.theme_install = None;
                self.theme_status = Some("theme install ended without a result".to_owned());
                changed = true;
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
        changed
    }

    fn start_theme_install(&mut self, folder: bool) -> Result<(), String> {
        if self.theme_install.is_some() {
            return Err("a theme install is already running".to_owned());
        }
        let icons_dir = theme_picker::user_icons_dir_from_env()
            .ok_or_else(|| "no user data home: set XDG_DATA_HOME or HOME".to_owned())?;
        self.theme_install = Some(theme_picker::start_install(icons_dir, folder)?);
        self.theme_status = Some("Choose a theme archive or folder".to_owned());
        Ok(())
    }

    /// Current page of the open picker; an action for a closed picker is rejected.
    fn open_theme_page(&self, kind: theme_picker::PickerKind) -> Result<usize, String> {
        match self.theme_picker {
            Some((open, page)) if open == kind => Ok(page),
            _ => Err(format!("the {} theme picker is not open", kind.word())),
        }
    }

    fn sync_theme_pickers(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        for kind in [
            theme_picker::PickerKind::Icon,
            theme_picker::PickerKind::Cursor,
        ] {
            let open = self
                .theme_picker
                .filter(|(open, _)| *open == kind)
                .map(|(_, page)| page);
            let node = kind.node();
            document.visible(&format!("{node}-picker"), open.is_some())?;
            let items = self.themes.items(kind);
            let page = open.unwrap_or(0);
            project_pager(
                document,
                theme_pager(kind),
                page,
                page_count(items.len(), theme_picker::PICKER_SLOTS),
                page_label(page, theme_picker::PICKER_SLOTS, items.len()),
            )?;
            for slot in 0..theme_picker::PICKER_SLOTS {
                let row = format!("{node}-row-{slot}");
                let preview = format!("{node}-preview-{slot}");
                let Some(item) =
                    open.and_then(|page| items.get(page * theme_picker::PICKER_SLOTS + slot))
                else {
                    document.visible(&row, false)?;
                    continue;
                };
                document.visible(&row, true)?;
                document.text(&format!("{node}-name-{slot}"), item.name.clone())?;
                match &item.preview {
                    Some(image) => {
                        document.visible(&preview, true)?;
                        document.image_rgba8(&preview, image.clone())?;
                    }
                    None => document.visible(&preview, false)?,
                }
            }
        }
        Ok(())
    }

    /// Points `customStartIcon` at `managed` and deletes the previous managed copy when it differs.
    fn replace_start_icon(&mut self, managed: String) -> Result<(), String> {
        let previous = self.text_setting("customStartIcon")?;
        self.apply_text("customStartIcon", managed.clone())?;
        if previous != managed {
            remove_managed_media(&previous)?;
        }
        Ok(())
    }

    fn apply_text(&mut self, key: &str, value: String) -> Result<(), String> {
        self.client
            .apply_setting(key, SettingValue::Text(value))
            .map_err(control_error)
    }

    fn open_color_picker(&self) -> Result<&ColorPicker, String> {
        self.color_picker
            .as_ref()
            .ok_or_else(|| "colour picker is not open".to_owned())
    }

    /// Applies typed keys to the picker: Escape closes, Enter applies, Backspace
    /// edits and hex digits append only while the field is focused.
    fn color_picker_text(&mut self, text: &str) -> Result<(), String> {
        for ch in text.chars() {
            match ch {
                '\u{1b}' => {
                    self.color_picker = None;
                    return Ok(());
                }
                '\n' | '\r' => self.apply_picker_buffer()?,
                '\u{8}' => {
                    if let Some(picker) = self.color_picker.as_mut().filter(|picker| picker.focused)
                    {
                        picker.buffer.pop();
                        picker.error = None;
                    }
                }
                ch if ch == '#' || ch.is_ascii_hexdigit() => {
                    if let Some(picker) = self
                        .color_picker
                        .as_mut()
                        .filter(|picker| picker.focused && picker.buffer.len() < PICKER_BUFFER_MAX)
                    {
                        picker.buffer.push(ch);
                        picker.error = None;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Writes the buffer as the picker's setting. An invalid buffer is shown inline
    /// and writes nothing.
    fn apply_picker_buffer(&mut self) -> Result<(), String> {
        let picker = self.open_color_picker()?;
        let key = picker.key;
        match parse_hex_color(&picker.buffer) {
            Ok(value) => {
                self.apply_text(key, value.clone())?;
                if let Some(picker) = self.color_picker.as_mut() {
                    picker.buffer = value;
                    picker.error = None;
                }
            }
            Err(message) => {
                if let Some(picker) = self.color_picker.as_mut() {
                    picker.error = Some(message);
                }
            }
        }
        Ok(())
    }

    fn arrange_display(&mut self, side: display_layout::Side) -> Result<(), String> {
        let output = self
            .client
            .ui()
            .selected_output
            .as_ref()
            .ok_or_else(|| "select an active monitor before arranging".to_owned())?
            .clone();
        // Validate against a *fresh* WM snapshot instead of a stale Settings
        // projection. The external RandR adapter never saves optimistic state.
        self.client.refresh_displays().map_err(control_error)?;
        let snapshot = self
            .client
            .displays()
            .ok_or_else(|| "display state unavailable".to_owned())?;
        let Some(layout) = display_layout::plan(snapshot, &output, side)? else {
            return Ok(());
        };
        display_layout::apply(&layout)?;
        // Readback must confirm every output's geometry, mode, scale and primary.
        // No success or persistence solely from xrandr's exit status.
        self.client.refresh_displays().map_err(control_error)?;
        let observed = self
            .client
            .displays()
            .ok_or_else(|| "display readback unavailable".to_owned())?;
        if let Err(error) = display_layout::verify(&layout, observed) {
            let rollback = display_layout::rollback_if_safe(&layout, observed);
            // Best-effort recovery must never be misreported as an accepted
            // commit, even if the rollback command itself exited successfully.
            let _ = self.client.refresh_displays();
            return Err(match rollback {
                Ok(()) => format!("{error}; previous monitor positions requested"),
                Err(reason) => format!("{error}; rollback unavailable: {reason}"),
            });
        }
        self.pages.generation = Some(observed.generation);
        if let Err(error) = display_layout::persist_verified(&layout) {
            return Err(format!("monitor layout applied but not saved: {error}"));
        }
        Ok(())
    }

    fn begin_display_mode(&mut self, mode: ModeId) -> Result<(), String> {
        let now_ms = monotonic_now_ms();
        self.client
            .begin_display_mode(mode, now_ms)
            .map(|_| ())
            .map_err(control_error)
    }

    fn handle_hotkey(
        &mut self,
        action: &'static str,
        operation: HotkeyOperation,
    ) -> Result<(), String> {
        match operation {
            HotkeyOperation::Capture => {
                self.hotkey_armed = false;
                self.client
                    .begin_hotkey_capture(action)
                    .map_err(control_error)?;
                self.hotkey_armed = true;
                Ok(())
            }
            HotkeyOperation::Clear => {
                self.client
                    .begin_hotkey_capture(action)
                    .map_err(control_error)?;
                self.client.capture_hotkey("Escape");
                self.client.commit_hotkey_capture().map_err(control_error)
            }
            HotkeyOperation::Reset => self.client.reset_hotkey(action).map_err(control_error),
        }
    }

    /// Commits the armed hotkey capture on a non-modifier key release.
    /// Escape clears the row, Backspace restores its default, and a binding
    /// already used by another row is rejected while the capture stays armed.
    fn finish_hotkey_capture(&mut self, binding: &str) -> Result<(), String> {
        let Some(action) = self
            .client
            .ui()
            .hotkey_capture
            .as_ref()
            .map(|capture| capture.action.clone())
        else {
            self.hotkey_armed = false;
            return Ok(());
        };
        match binding {
            "Backspace" => {
                self.client.cancel_hotkey_capture().map_err(control_error)?;
                self.hotkey_armed = false;
                self.client.reset_hotkey(action).map_err(control_error)
            }
            "Escape" => {
                self.client.capture_hotkey("Escape");
                self.client.commit_hotkey_capture().map_err(control_error)?;
                self.hotkey_armed = false;
                Ok(())
            }
            _ => {
                if let Some(owner) = self.duplicate_owner(&action, binding) {
                    return Err(format!(
                        "{binding} is already assigned to {}; press another key, Escape clears, Backspace restores the default",
                        shortcut_label(&owner)
                    ));
                }
                self.client.capture_hotkey(binding);
                self.client.commit_hotkey_capture().map_err(control_error)?;
                self.hotkey_armed = false;
                Ok(())
            }
        }
    }

    /// Another row that already owns `binding`, if any.
    fn duplicate_owner(&self, action: &str, binding: &str) -> Option<String> {
        self.client
            .shortcuts()?
            .bindings
            .iter()
            .find(|(owner, value)| {
                owner.as_str() != action
                    && value
                        .0
                        .as_deref()
                        .is_some_and(|assigned| assigned.eq_ignore_ascii_case(binding))
            })
            .map(|(owner, _)| owner.clone())
    }

    fn start_open_uri(&mut self, uri: &'static str) -> Result<(), String> {
        // Do not replace the only receiver for an in-flight opener: its completion
        // carries the only authoritative success/failure result for the user.
        self.poll_open_uri()?;
        if self.pending_open.is_some() {
            return Err(
                "a link is still opening; wait for it to finish before opening another".to_owned(),
            );
        }
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("flamewm-settings-open-uri".to_owned())
            .spawn(move || {
                let outcome = match Command::new("xdg-open").arg(uri).status() {
                    Ok(status) if status.success() => Ok(()),
                    Ok(status) => Err(format!(
                        "could not open {uri}: xdg-open exited with {status}"
                    )),
                    Err(error) => Err(format!("could not open {uri}: {error}")),
                };
                file_chooser::publish_background_completion(&sender, outcome);
            })
            .map_err(|error| format!("could not start link opener: {error}"))?;
        self.pending_open = Some(receiver);
        Ok(())
    }

    fn poll_open_uri(&mut self) -> Result<(), String> {
        let Some(receiver) = self.pending_open.as_ref() else {
            return Ok(());
        };
        match receiver.try_recv() {
            Ok(outcome) => {
                self.pending_open = None;
                outcome
            }
            Err(TryRecvError::Empty) => Ok(()),
            Err(TryRecvError::Disconnected) => {
                self.pending_open = None;
                Err("link opener ended without a result".to_owned())
            }
        }
    }

    fn step_percent(&mut self, key: &str, direction: i8, maximum: u8) -> Result<(), String> {
        let current = self.percent_setting(key)?;
        let next = i16::from(current) + i16::from(direction) * 5;
        let next = next.clamp(0, i16::from(maximum)) as u8;
        self.client
            .apply_setting(key, SettingValue::Percent(next))
            .map_err(control_error)
    }

    fn step_integer(
        &mut self,
        key: &str,
        delta: i64,
        minimum: i64,
        maximum: i64,
    ) -> Result<(), String> {
        let current = self.integer_setting(key)?;
        let next = (current + delta).clamp(minimum, maximum);
        self.client
            .apply_setting(key, SettingValue::Integer(next))
            .map_err(control_error)
    }

    fn set_taskbar_size(&mut self, direction: i8) -> Result<(), String> {
        let height = self.integer_setting("taskbarHeight")?;
        let sizes = [34_i64, 44, 52, 64, 72];
        self.client
            .set_taskbar_height(cycle_value(&sizes, height, direction) as u16)
            .map_err(control_error)
    }

    fn cycle_font_family(&mut self) -> Result<(), String> {
        const FAMILIES: [&str; 3] = ["IBM Plex Sans", "Noto Sans", "DejaVu Sans"];
        let current = self.text_setting("fontFamily")?;
        let next = cycle_text(&FAMILIES, &current);
        self.apply_text("fontFamily", next.to_owned())
    }

    fn cycle_panel_edge(&mut self) -> Result<(), String> {
        let (output, edge) = self.selected_panel()?;
        self.client
            .set_panel_edge(output, next_edge(edge))
            .map_err(control_error)
    }

    fn selected_panel(&self) -> Result<(OutputId, PanelEdge), String> {
        let output = self
            .client
            .ui()
            .selected_output
            .clone()
            .ok_or_else(|| "no authoritative display output is selected".to_owned())?;
        let edge = self
            .client
            .panels()
            .and_then(|snapshot| snapshot.panels.iter().find(|panel| panel.output == output))
            .map(|panel| panel.edge)
            .ok_or_else(|| "selected output has no authoritative panel".to_owned())?;
        Ok((output, edge))
    }

    fn selected_display(&self) -> Option<&flamewm_api::display::OutputSnapshot> {
        let id = self.client.ui().selected_output.as_ref()?;
        self.client
            .displays()?
            .outputs
            .iter()
            .find(|output| &output.id == id)
    }

    fn setting_value(&self, key: &str) -> Option<&SettingValue> {
        self.client
            .settings()
            .and_then(|snapshot| snapshot.values.get(key))
    }

    fn percent_setting(&self, key: &str) -> Result<u8, String> {
        match self.setting_value(key) {
            Some(SettingValue::Percent(value)) => Ok(*value),
            _ => Err(format!(
                "setting '{key}' has no authoritative percent value"
            )),
        }
    }

    fn integer_setting(&self, key: &str) -> Result<i64, String> {
        match self.setting_value(key) {
            Some(SettingValue::Integer(value)) => Ok(*value),
            _ => Err(format!(
                "setting '{key}' has no authoritative integer value"
            )),
        }
    }

    /// The configured accent, falling back to `DEFAULT_ACCENT` when the setting is absent.
    fn accent(&self) -> Result<UiColor, String> {
        accent_color(
            &self
                .text_setting("accent")
                .unwrap_or(DEFAULT_ACCENT.to_owned()),
        )
    }

    fn text_setting(&self, key: &str) -> Result<String, String> {
        match self.setting_value(key) {
            Some(SettingValue::Text(value)) => Ok(value.clone()),
            _ => Err(format!("setting '{key}' has no authoritative text value")),
        }
    }

    /// Boolean setting value; an absent or non-boolean key reads as OFF.
    fn bool_setting(&self, key: &str) -> bool {
        matches!(self.setting_value(key), Some(SettingValue::Boolean(true)))
    }

    /// Writes every node; a failed node write is recorded and the rest still apply.
    fn sync(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let mut nodes = NodeFailures {
            inner: document,
            failures: Vec::new(),
        };
        let result = self.sync_nodes(&mut nodes);
        match (result, nodes.into_result()) {
            (Ok(()), recorded) => recorded,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(recorded)) => Err(format!("{error}; {recorded}")),
        }
    }

    fn sync_nodes(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        document.position("nav-selection", 8.0, nav_top(self.nav_index()))?;
        if let Some(SettingValue::Text(accent)) = self.setting_value("accent") {
            document.background("nav-selection", accent_color(accent)?)?;
            sync_accent_nodes(document, accent)?;
            for name in ["red", "blue", "purple", "teal"] {
                let palette = accent_for_swatch(name);
                document.visible(
                    &format!("accent-selected-{name}"),
                    palette.is_some_and(|value| value.eq_ignore_ascii_case(accent)),
                )?;
            }
        }
        for tab in AppearanceTab::ALL {
            document.visible(
                &format!("page-{}", tab.node()),
                self.client.ui().page == SettingsPage::Appearance && self.appearance_tab == tab,
            )?;
        }
        for (name, page) in [
            ("desktop", SettingsPage::Desktop),
            ("taskbar", SettingsPage::Taskbar),
            ("displays", SettingsPage::Displays),
            ("fonts", SettingsPage::Fonts),
            ("hotkeys", SettingsPage::Hotkeys),
            ("about", SettingsPage::About),
        ] {
            document.visible(&format!("page-{name}"), self.client.ui().page == page)?;
        }
        for slot in 0..OUTPUT_PAGE_SLOTS {
            let id = format!("display-output-{slot}");
            let Some(output) = self.output_page_slot(slot) else {
                document.visible(&id, false)?;
                continue;
            };
            document.visible(&id, true)?;
            let shown = output.connected && output.geometry.is_valid();
            document.text(
                &format!("display-output-name-{slot}"),
                format!(
                    "{} · {}",
                    self.pages.output_page * OUTPUT_PAGE_SLOTS + slot + 1,
                    output.connector
                ),
            )?;
            let resolution = output
                .modes
                .iter()
                .find(|mode| mode.id == output.current_mode)
                .map_or(output.geometry.size(), |mode| mode.resolution);
            document.text(
                &format!("display-output-size-{slot}"),
                if shown {
                    format!("{} × {}", resolution.width, resolution.height)
                } else {
                    "Disconnected".to_owned()
                },
            )?;
            let selected = self.client.ui().selected_output.as_ref() == Some(&output.id);
            document.visible(&format!("display-output-selected-{slot}"), selected)?;
            document.visible(&format!("display-output-unselected-{slot}"), !selected)?;
            document.border(
                &id,
                if selected {
                    self.accent()?
                } else {
                    UiColor {
                        r: 89,
                        g: 96,
                        b: 104,
                        a: 255,
                    }
                },
            )?;
        }
        project_pager(
            document,
            &OUTPUT_PAGER,
            self.pages.output_page,
            page_count(self.output_count(), OUTPUT_PAGE_SLOTS),
            page_label(
                self.pages.output_page,
                OUTPUT_PAGE_SLOTS,
                self.output_count(),
            ),
        )?;
        if let Some(output) = self.selected_display() {
            document.text(
                "display-selected-title",
                format!("Selected: {}", output.connector),
            )?;
            let resolution = output
                .modes
                .iter()
                .find(|mode| mode.id == output.current_mode)
                .map_or(output.geometry.size(), |mode| mode.resolution);
            document.text(
                "display-selected-resolution",
                format!("{} x {}", resolution.width, resolution.height),
            )?;
            self.sync_display_scale(
                document,
                self.scale_drag
                    .as_ref()
                    .and_then(|(id, generation, preview)| {
                        (self
                            .client
                            .displays()
                            .is_some_and(|snap| snap.generation == *generation)
                            && id == &output.id)
                            .then_some(*preview)
                    }),
            )?;
        } else {
            for id in ["display-selected-title", "display-selected-resolution"] {
                document.text(id, "\u{200b}".to_owned())?;
            }
            self.sync_display_scale(document, None)?;
        }
        self.sync_display_modes(document)?;
        for (key, on_id, off_id) in [
            (
                "stickyNotesEnabled",
                "sticky-toggle-on",
                "sticky-toggle-off",
            ),
            ("fontBold", "font-toggle-on", "font-toggle-off"),
            ("animationsEnabled", "anim-global-on", "anim-global-off"),
            ("startAnimationEnabled", "anim-start-on", "anim-start-off"),
            (
                "windowOpenAnimationEnabled",
                "anim-window-on",
                "anim-window-off",
            ),
            (
                "minimizeAnimationEnabled",
                "anim-minimize-on",
                "anim-minimize-off",
            ),
            (
                "maximizeAnimationEnabled",
                "anim-maximize-on",
                "anim-maximize-off",
            ),
            ("lockShowDate", "lock-date-on", "lock-date-off"),
            ("lockShowUsername", "lock-username-on", "lock-username-off"),
            ("lockOnSuspend", "lock-suspend-on", "lock-suspend-off"),
            (
                "taskbarShowSeconds",
                "taskbar-seconds-on",
                "taskbar-seconds-off",
            ),
            ("taskbarShowDate", "taskbar-date-on", "taskbar-date-off"),
            ("snapPreviewEnabled", "snap-preview-on", "snap-preview-off"),
            ("focusFollowsMouse", "focus-mouse-on", "focus-mouse-off"),
        ] {
            let enabled = self.setting_value(key).and_then(|value| match value {
                SettingValue::Boolean(value) => Some(*value),
                _ => None,
            });
            document.visible(on_id, enabled == Some(true))?;
            document.visible(off_id, enabled == Some(false))?;
        }
        for (key, on_id, off_id, default) in SETTING_TWINS {
            let enabled = match self.setting_value(key) {
                Some(SettingValue::Boolean(value)) => *value,
                _ => *default,
            };
            document.visible(on_id, enabled)?;
            document.visible(off_id, !enabled)?;
        }
        if let Some(SettingValue::Text(family)) = self.setting_value("fontFamily") {
            document.font_family(family.clone())?;
        }
        document.ui_font_defaults(
            self.integer_setting("fontSizeOffset").unwrap_or(0) as f32,
            self.bool_setting("fontBold"),
        )?;
        self.sync_value_text(document)?;
        self.sync_weather(document)?;
        self.sync_time_zone(document)?;
        self.sync_theme_pickers(document)?;
        if let Some(status) = &self.theme_status {
            optional_text(document, "theme-install-status", status.clone());
        }
        self.sync_appearance(document)?;
        self.sync_option_setting(
            document,
            "decoration",
            &[
                ("rounded", "decoration-rounded"),
                ("squared", "decoration-squared"),
            ],
        )?;
        self.sync_option_setting(
            document,
            "titlebarButtonStyle",
            &[
                ("circle", "titlebar-button-style-circle"),
                ("square", "titlebar-button-style-square"),
            ],
        )?;
        self.sync_animation_styles(document)?;
        self.sync_lock_screen(document)?;
        self.sync_segment_rows(document)?;
        self.sync_taskbar_outputs(document);
        self.sync_hotkeys(document);
        optional_text(
            document,
            ABOUT_VERSION_NODE,
            format!("Version {APP_VERSION}"),
        );
        self.sync_error(document);
        Ok(())
    }

    /// Nav row index of the visible page: the six Appearance tabs first, then the rest.
    fn nav_index(&self) -> usize {
        match self.client.ui().page {
            SettingsPage::Appearance => self.appearance_tab as usize,
            SettingsPage::Desktop => 6,
            SettingsPage::Taskbar => 7,
            SettingsPage::Displays => 8,
            SettingsPage::Fonts => 9,
            SettingsPage::Hotkeys => 10,
            SettingsPage::About => 11,
        }
    }

    fn sync_display_scale(
        &self,
        document: &mut impl UiDocumentAccess,
        preview: Option<u16>,
    ) -> Result<(), String> {
        let value = preview
            .or_else(|| {
                self.selected_display()
                    .map(|output| output.shell_scale_percent)
            })
            .unwrap_or(100);
        document.text("display-scale-value", format!("{value}%"))?;
        let ratio = f32::from(value.saturating_sub(100)) / 100.0;
        let thumb = ratio * ANIMATION_SPEED_TRACK_TRAVEL as f32;
        document.size(
            "display-scale-fill",
            thumb + ANIMATION_SPEED_THUMB as f32,
            12.0,
        )?;
        document.position("display-scale-thumb", thumb, 10.0)
    }

    fn sync_appearance(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        self.sync_color_rows(document)?;
        self.sync_color_picker(document)?;
        for (key, id) in [
            ("surfaceBlurPercent", "surface-blur-value"),
            ("accentOpacity", "accent-opacity-value"),
        ] {
            if let Ok(value) = self.percent_setting(key) {
                document.text(id, format!("{value}%"))?;
            }
        }
        if let Ok(speed) = self.integer_setting("animationSpeed") {
            document.text(
                "animation-speed-value",
                format!("{:.2}x", speed as f64 / 100.0),
            )?;
            let ratio = (speed - ANIMATION_SPEED_MIN) as f64
                / (ANIMATION_SPEED_MAX - ANIMATION_SPEED_MIN) as f64;
            let fill = ratio * ANIMATION_SPEED_TRACK_TRAVEL;
            let fill_width = (fill + ANIMATION_SPEED_THUMB).min(ANIMATION_SPEED_TRACK_WIDTH);
            document.size("animation-speed-fill", fill_width as f32, 12.0)?;
            document.position("animation-speed-thumb", fill as f32, 0.0)?;
        }
        Ok(())
    }

    /// Highlights the option matching the authoritative `key` setting; an unset key leaves the row alone.
    fn sync_option_setting(
        &self,
        document: &mut impl UiDocumentAccess,
        key: &str,
        options: &[(&str, &str)],
    ) -> Result<(), String> {
        let Ok(current) = self.text_setting(key) else {
            return Ok(());
        };
        let accent = self.accent()?;
        highlight_options(document, Some(&current), options, accent)
    }

    /// Highlights the preset that matches each surface setting, and shows each
    /// setting's current colour on its wheel circle.
    fn sync_color_rows(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        for (target, key) in COLOR_TARGETS {
            let Ok(value) = self.text_setting(key) else {
                continue;
            };
            let current = solid_color(&value)?;
            document.background(&format!("color-{target}-current"), current)?;
            if target == "accent" {
                continue;
            }
            for (index, preset) in SURFACE_PRESETS.iter().enumerate() {
                document.background(
                    &format!("color-{target}-preset-{index}"),
                    solid_color(preset)?,
                )?;
                document.visible(
                    &format!("color-{target}-selected-{index}"),
                    preset.eq_ignore_ascii_case(&value),
                )?;
            }
        }
        Ok(())
    }

    /// Projects the shared picker panel: open state, the hex buffer with its focus
    /// caret, the preview, the inline error and the palette cells.
    fn sync_color_picker(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        for hue in 0..PICKER_HUES {
            for shade in 0..PICKER_SHADES {
                let id = format!("color-picker-grid-{hue}-{shade}");
                document.background(&id, solid_color(&picker_grid_color(hue, shade))?)?;
            }
        }
        for step in 0..PICKER_GREYS {
            let id = format!("color-picker-grid-grey-{step}");
            document.background(&id, solid_color(&picker_grey(step))?)?;
        }
        let Some(picker) = self.color_picker.as_ref() else {
            document.visible("color-picker", false)?;
            return Ok(());
        };
        document.visible("color-picker", true)?;
        let caret = if picker.focused { "|" } else { "" };
        let hex = if picker.buffer.is_empty() {
            "#RRGGBB".to_owned()
        } else {
            format!("{}{caret}", picker.buffer)
        };
        document.text("color-picker-hex", hex)?;
        let shown = parse_hex_color(&picker.buffer)
            .ok()
            .or_else(|| self.text_setting(picker.key).ok())
            .unwrap_or_else(|| SURFACE_PRESETS[0].to_owned());
        document.background("color-picker-preview", solid_color(&shown)?)?;
        optional_text(
            document,
            "color-picker-error",
            picker
                .error
                .clone()
                .unwrap_or_else(|| "\u{200b}".to_owned()),
        );
        Ok(())
    }

    /// Highlights, in each animation style row, the segment whose value matches the stored style.
    fn sync_animation_styles(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let accent = self.accent()?;
        for (key, values) in ANIMATION_STYLE_ROWS {
            let Ok(current) = self.text_setting(key) else {
                continue;
            };
            for value in values {
                let id = animation_style_node(key, value);
                highlight_segment(document, &id, current == *value, accent)?;
            }
        }
        Ok(())
    }

    /// Projects the lock-screen background path (or `None`) and highlights the clock and timeout segments.
    fn sync_lock_screen(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        if let Ok(mode) = self.text_setting("lockBackgroundMode") {
            let label = match mode.as_str() {
                "solid" => "Solid colour".to_owned(),
                "custom" => self.text_setting("lockBackgroundImage").unwrap_or_default(),
                _ => "Built-in".to_owned(),
            };
            document.text("lock-background-image-value", label)?;
        }
        if let Ok(path) = self.text_setting("avatarImage") {
            document.text("avatar-value", avatar_label(&path))?;
        }
        let accent = self.accent()?;
        let clock = self.text_setting("lockClockFormat").ok();
        highlight_options(
            document,
            clock.as_deref(),
            &[("24h", "lock-clock-24h"), ("12h", "lock-clock-12h")],
            accent,
        )?;
        let timeout = self.integer_setting("lockTimeoutMinutes").ok();
        for minutes in LOCK_TIMEOUT_CHOICES {
            let id = format!("lock-timeout-{minutes}");
            highlight_segment(document, &id, timeout == Some(minutes), accent)?;
        }
        Ok(())
    }

    /// Highlights the segment rows (taskbar layout, clock, cursor size, screensaver mode) and shows
    /// the floating-only taskbar steppers only while `taskbarStyle` is `floating`.
    fn sync_segment_rows(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let accent = self.accent()?;
        let style = self.text_setting("taskbarStyle").ok();
        highlight_options(
            document,
            style.as_deref(),
            &[
                ("classic", "taskbar-style-classic"),
                ("floating", "taskbar-style-floating"),
            ],
            accent,
        )?;
        let floating = style.as_deref() == Some("floating");
        document.visible("taskbar-floating-margin-row", floating)?;
        document.visible("taskbar-floating-radius-row", floating)?;
        let clock = self.text_setting("taskbarClockFormat").ok();
        highlight_options(
            document,
            clock.as_deref(),
            &[("24h", "taskbar-clock-24h"), ("12h", "taskbar-clock-12h")],
            accent,
        )?;
        let appearance = self.text_setting("appearance").ok();
        highlight_options(
            document,
            appearance.as_deref(),
            &[
                ("dark", "appearance-apps-dark"),
                ("light", "appearance-apps-light"),
            ],
            accent,
        )?;
        let size = self.integer_setting("cursorSize").ok();
        for value in CURSOR_SIZE_CHOICES {
            highlight_segment(
                document,
                &format!("cursor-size-{value}"),
                size == Some(value),
                accent,
            )?;
        }
        let mode = self.text_setting("screensaverMode").ok();
        highlight_options(
            document,
            mode.as_deref(),
            &[
                ("ribbons", "screensaver-mode-ribbons"),
                ("none", "screensaver-mode-none"),
            ],
            accent,
        )?;
        Ok(())
    }

    fn sync_error(&self, document: &mut impl UiDocumentAccess) {
        optional_text(document, ERROR_NODE, self.error.clone().unwrap_or_default());
    }

    /// Names the output the Taskbar page configures and lists the outputs it can switch to.
    /// The selection is the shared `selected_output` authority.
    fn sync_taskbar_outputs(&self, document: &mut impl UiDocumentAccess) {
        let title = match self.selected_display() {
            Some(output) => format!("Configuring output: {}", output.connector),
            None => "No output selected".to_owned(),
        };
        optional_text(document, "taskbar-output-title", title);
        for slot in 0..OUTPUT_PAGE_SLOTS {
            let id = format!("taskbar-output-{slot}");
            match self.output_page_slot(slot) {
                Some(output) => {
                    let selected = self.client.ui().selected_output.as_ref() == Some(&output.id);
                    let label = if selected {
                        format!("{} (selected)", output.connector)
                    } else {
                        output.connector.clone()
                    };
                    optional_visible(document, &id, true);
                    optional_text(document, &id, label);
                }
                None => optional_visible(document, &id, false),
            }
        }
        let _ = project_pager(
            document,
            &TASKBAR_OUTPUT_PAGER,
            self.pages.output_page,
            page_count(self.output_count(), OUTPUT_PAGE_SLOTS),
            page_label(
                self.pages.output_page,
                OUTPUT_PAGE_SLOTS,
                self.output_count(),
            ),
        );
    }

    /// Projects each curated row's authoritative binding, or the active capture prompt.
    fn sync_hotkeys(&self, document: &mut impl UiDocumentAccess) {
        let capturing = if self.hotkey_armed {
            self.client
                .ui()
                .hotkey_capture
                .as_ref()
                .map(|capture| capture.action.clone())
        } else {
            None
        };
        for row in &HOTKEY_ROWS {
            if capturing.as_deref() == Some(row.action) {
                optional_text(document, row.node, "Press a key...".to_owned());
                continue;
            }
            let Some(snapshot) = self.client.shortcuts() else {
                continue;
            };
            let Some(flamewm_api::shortcuts::KeyBinding(binding)) =
                snapshot.bindings.get(row.action)
            else {
                continue;
            };
            let text = binding
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or("Not assigned")
                .to_owned();
            optional_text(document, row.node, text);
        }
    }

    /// Projects the selected output's mode slots, overflow count and the
    /// Keep/Revert confirmation row from the authoritative display snapshot.
    fn sync_display_modes(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let slots = self.client.display_mode_page(self.pages.mode_page);
        let current = self.selected_display().map(|output| output.current_mode);
        for slot in 0..DISPLAY_MODE_SLOTS {
            let id = format!("display-mode-slot-{slot}");
            match slots.get(slot) {
                Some(mode) => {
                    document.visible(&id, true)?;
                    document.text(&id, display_mode_label(mode, current == Some(mode.id)))?;
                }
                None => document.visible(&id, false)?,
            }
        }
        let total = self.client.display_mode_count();
        let pages = page_count(total, DISPLAY_MODE_SLOTS);
        project_pager(
            document,
            &MODE_PAGER,
            self.pages.mode_page,
            pages,
            page_label(self.pages.mode_page, DISPLAY_MODE_SLOTS, total),
        )?;
        let note = if total == 0 {
            "This output reports no selectable modes".to_owned()
        } else {
            "\u{200b}".to_owned()
        };
        document.visible("display-mode-overflow", total == 0)?;
        document.text("display-mode-overflow", note)?;
        let pending = self.client.ui().pending_display_transaction.is_some();
        document.visible("display-confirm", pending)?;
        Ok(())
    }

    /// Highlights the saved units and shows the saved weather city (or the unset hint).
    fn sync_weather(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let units = self
            .text_setting("weatherUnits")
            .unwrap_or_else(|_| "celsius".to_owned());
        let accent = self.accent()?;
        highlight_options(
            document,
            Some(&units),
            &[
                ("celsius", "weather-units-celsius"),
                ("fahrenheit", "weather-units-fahrenheit"),
            ],
            accent,
        )?;
        let location = self.text_setting("weatherLocation").unwrap_or_default();
        let automatic = self.bool_setting("autoLocationEnabled");
        document.text(
            "weather-location-value",
            weather_location_text(&location, automatic),
        )
    }

    fn sync_time_zone(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let automatic_zone = self.bool_setting("autoTimeZoneEnabled");
        let cached = self.text_setting("autoTimeZone").unwrap_or_default();
        let zone = time_zone_label(automatic_zone, &cached);
        let city = if self.bool_setting("autoLocationEnabled") {
            let location = self.text_setting("weatherLocation").unwrap_or_default();
            weather_location_label(&location)
        } else {
            "not detected".to_owned()
        };
        document.text(
            "time-zone-value",
            format!("Current time zone: {zone} - detected city: {city}"),
        )
    }

    /// Writes the authoritative value text into every value pill and select
    /// label. Missing snapshot values leave the authored placeholder untouched.
    fn sync_value_text(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let percent = |key: &str| match self.setting_value(key) {
            Some(SettingValue::Percent(value)) => Some(*value),
            _ => None,
        };
        let integer = |key: &str| match self.setting_value(key) {
            Some(SettingValue::Integer(value)) => Some(*value),
            _ => None,
        };
        let text = |key: &str| match self.setting_value(key) {
            Some(SettingValue::Text(value)) => Some(value.clone()),
            _ => None,
        };
        if let Some(value) = percent("DesktopSelectionFillOpacity") {
            document.text("selection-opacity-value", format!("{value}%"))?;
        }
        if let Some(value) = percent("WindowSnapPreviewFillOpacity") {
            document.text("snap-opacity-value", format!("{value}%"))?;
        }
        if let Some(value) = integer("DesktopEntryScalePercent") {
            document.text("desktop-entry-size-value", format!("{value}%"))?;
        }
        for kind in [
            theme_picker::PickerKind::Icon,
            theme_picker::PickerKind::Cursor,
        ] {
            if let Some(value) = text(kind.setting_key()) {
                document.text(
                    if kind == theme_picker::PickerKind::Icon {
                        "icon-theme-selected-name"
                    } else {
                        kind.select_box()
                    },
                    self.themes.label(kind, &value),
                )?;
                if kind == theme_picker::PickerKind::Icon {
                    let preview = self.themes.selected_preview(kind, &value);
                    document.visible("icon-theme-selected-preview", preview.is_some())?;
                    if let Some(image) = preview {
                        document.image_rgba8("icon-theme-selected-preview", image.clone())?;
                    }
                }
            }
        }
        if let Some(value) = percent("surfaceOpacity") {
            document.text("surface-opacity-value", format!("{value}%"))?;
        }
        if let Some(value) = text("customStartIcon") {
            document.text("start-icon-value", start_icon_label(&value))?;
        }
        for row in INTEGER_STEPPERS {
            if let Some(value) = integer(row.key) {
                let label = if row.key == "screensaverRibbonWidthTenths" {
                    format!("{:.1} px", value as f64 / 10.0)
                } else {
                    value.to_string()
                };
                document.text(&format!("{}-value", row.slug), label)?;
            }
        }
        if let Some(value) = text("wallpaperFit") {
            document.text("wallpaper-fit-value", wallpaper_fit_label(&value))?;
        }
        if let Some(value) = integer("taskbarHeight") {
            document.text("taskbar-size-value", format!("{value} px (all outputs)"))?;
        }
        if let Some(value) = text("fontFamily") {
            document.text("font-family-value", value)?;
        }
        if let Some(value) = integer("fontSizeOffset") {
            document.text("font-size-value", font_size_label(value))?;
        }
        let selected_edge = self
            .selected_display()
            .and_then(|output| {
                self.client.panels().and_then(|snapshot| {
                    snapshot
                        .panels
                        .iter()
                        .find(|panel| panel.output == output.id)
                })
            })
            .map(|panel| panel.edge);
        if let Some(edge) = selected_edge {
            document.text("taskbar-edge-value", panel_edge_label(edge).to_owned())?;
        }
        Ok(())
    }
}

impl<T: ControlTransport> Drop for SettingsApplication<T> {
    fn drop(&mut self) {
        // Best effort; loss of the owning D-Bus connection also restores backend grabs.
        let _ = self.client.cancel_hotkey_capture();
    }
}

/// Writes to a node the compiled markup may not declare yet. Best effort, so a
/// document without the optional node keeps projecting its required nodes.
fn optional_text(document: &mut impl UiDocumentAccess, id: &str, text: String) {
    let _ = document.text(id, text);
}

fn optional_visible(document: &mut impl UiDocumentAccess, id: &str, visible: bool) {
    let _ = document.visible(id, visible);
}

/// Bare Super commits as `Super` when its release carries no other modifier.
/// The release reports its own Super bit, so only that bit may be present.
fn bare_super_binding(event: &UiKeyEvent) -> Option<&'static str> {
    let is_super = matches!(
        event.key.to_ascii_lowercase().as_str(),
        "super" | "super_l" | "super_r"
    );
    let only_super = event
        .modifiers
        .iter()
        .all(|modifier| modifier.eq_ignore_ascii_case("super"));
    (is_super && only_super).then_some("Super")
}

fn is_modifier_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "ctrl"
            | "control"
            | "control_l"
            | "control_r"
            | "shift"
            | "shift_l"
            | "shift_r"
            | "alt"
            | "alt_l"
            | "alt_r"
            | "super"
            | "super_l"
            | "super_r"
            | "meta"
            | "meta_l"
            | "meta_r"
    )
}

const ANIMATION_SPEED_SLIDER: &str = "slider.animation-speed";
const DISPLAY_SCALE_SLIDER: &str = "slider.display-scale";

fn display_scale_from_slider(text: &str) -> Result<u16, String> {
    let fraction: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("invalid display scale slider value '{text}'"))?;
    if !fraction.is_finite() {
        return Err("display scale slider requires a finite number".to_owned());
    }
    let ticks = (fraction.clamp(0.0, 100.0) / 5.0).round() as u16;
    Ok(100 + ticks * 5)
}

const ANIMATION_SPEED_MIN: i64 = 25;
const ANIMATION_SPEED_MAX: i64 = 200;
const ANIMATION_SPEED_STEP: f64 = 5.0;
/// Must match the `.slider-track` width in ui/settings.css.
const ANIMATION_SPEED_TRACK_WIDTH: f64 = 180.0;
const ANIMATION_SPEED_THUMB: f64 = 12.0;
const ANIMATION_SPEED_TRACK_TRAVEL: f64 = ANIMATION_SPEED_TRACK_WIDTH - ANIMATION_SPEED_THUMB;

/// Maps a native slider value (track percent 0..=100) to `animationSpeed`
/// hundredths in 25..=200, snapped to 5.
fn animation_speed_from_slider(text: &str) -> Result<i64, String> {
    let percent: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("invalid animation speed value '{text}'"))?;
    if !percent.is_finite() {
        return Err(format!("invalid animation speed value '{text}'"));
    }
    let span = (ANIMATION_SPEED_MAX - ANIMATION_SPEED_MIN) as f64;
    let raw = ANIMATION_SPEED_MIN as f64 + percent.clamp(0.0, 100.0) / 100.0 * span;
    let steps = ((raw - ANIMATION_SPEED_MIN as f64) / ANIMATION_SPEED_STEP).round();
    let snapped = ANIMATION_SPEED_MIN + (steps * ANIMATION_SPEED_STEP) as i64;
    Ok(snapped.clamp(ANIMATION_SPEED_MIN, ANIMATION_SPEED_MAX))
}

fn step(value: &str) -> Option<i8> {
    match value {
        "down" => Some(-1),
        "up" => Some(1),
        _ => None,
    }
}

/// Color rows in the Appearance page: action target and the `#RRGGBB` setting.
const COLOR_TARGETS: [(&str, &str); 3] = [
    ("surface", "surfaceColor"),
    ("accent", "accent"),
    ("lockbackground", "lockBackgroundColor"),
];

/// Minutes offered by the lock-screen idle timeout segments; 0 means never.
const LOCK_TIMEOUT_CHOICES: [i64; 5] = [0, 5, 10, 15, 30];

/// Cursor sizes offered by the Appearance segments; the wire value is the pixel size.
const CURSOR_SIZE_CHOICES: [i64; 4] = [24, 32, 48, 64];

/// Wallpaper placements cycled by the Desktop selector, in wire order.
const WALLPAPER_FITS: [&str; 5] = ["fill", "fit", "stretch", "center", "tile"];

fn wallpaper_fit_label(value: &str) -> String {
    let label = match value {
        "fill" => "Fill",
        "fit" => "Fit",
        "stretch" => "Stretch",
        "center" => "Center",
        "tile" => "Tile",
        other => other,
    };
    label.to_owned()
}

/// A stepper row: its HTML slug (`settings.<slug>.up|down`), the integer key it writes, and the
/// step size and inclusive bounds.
struct IntegerStepper {
    slug: &'static str,
    key: &'static str,
    step: i64,
    minimum: i64,
    maximum: i64,
}

const INTEGER_STEPPERS: [IntegerStepper; 8] = [
    IntegerStepper {
        slug: "taskbar-floating-margin",
        key: "taskbarFloatingMargin",
        step: 1,
        minimum: 0,
        maximum: 32,
    },
    IntegerStepper {
        slug: "taskbar-floating-radius",
        key: "taskbarFloatingRadius",
        step: 1,
        minimum: 0,
        maximum: 24,
    },
    IntegerStepper {
        slug: "screensaver-after-lock",
        key: "screensaverAfterLockMinutes",
        step: 1,
        minimum: 0,
        maximum: 60,
    },
    IntegerStepper {
        slug: "screensaver-ribbons",
        key: "screensaverNumRibbons",
        step: 1,
        minimum: 1,
        maximum: 12,
    },
    IntegerStepper {
        slug: "screensaver-ribbon-width",
        key: "screensaverRibbonWidthTenths",
        step: 5,
        minimum: 20,
        maximum: 280,
    },
    IntegerStepper {
        slug: "screensaver-display-off",
        key: "screensaverDisplayOffMinutes",
        step: 5,
        minimum: 0,
        maximum: 240,
    },
    IntegerStepper {
        slug: "lock-bg-blur",
        key: "lockBackgroundBlurPercent",
        step: 5,
        minimum: 0,
        maximum: 100,
    },
    IntegerStepper {
        slug: "lock-bg-dim",
        key: "lockBackgroundDimPercent",
        step: 5,
        minimum: 0,
        maximum: 80,
    },
];

/// Preset circles offered beside each surface color row; `preset.<index>` selects one.
const SURFACE_PRESETS: [&str; 6] = [
    "#191B1D", "#336699", "#3DAEE9", "#1ABC9C", "#EF4048", "#9B59B6",
];

/// Colour picker palette: hue columns by shade rows, then one row of greys.
const PICKER_HUES: usize = 12;
const PICKER_SHADES: usize = 6;
const PICKER_GREYS: usize = 6;
/// Typed key text from the native key source, see `keyboard_text`.
const KEYBOARD_INPUT_ACTION: &str = "keyboard.input";
/// Longest buffer the picker accepts: `#` plus six hex digits.
const PICKER_BUFFER_MAX: usize = 7;

/// State of the one shared colour picker: the setting it writes and its hex buffer.
struct ColorPicker {
    key: &'static str,
    buffer: String,
    focused: bool,
    error: Option<String>,
}

/// Animation style rows in the Animations card: stable key and the values its segments write.
/// Each segment's node id is `animation_style_node(key, value)`.
const ANIMATION_STYLE_ROWS: [(&str, &[&str]); 5] = [
    ("startAnimation", &["none", "slide", "fade", "magic-lamp"]),
    ("minimizeAnimation", &["none", "fade", "magic-lamp"]),
    ("maximizeAnimation", &["none", "fade", "scale"]),
    ("windowOpenAnimation", &["none", "scale", "fade"]),
    ("windowCloseAnimation", &["none", "fade"]),
];

/// Node id of one animation style segment, e.g. `startAnimation` + `magic-lamp`.
fn animation_style_node(key: &str, value: &str) -> String {
    let row = match key {
        "startAnimation" => "start-style",
        "minimizeAnimation" => "minimize-style",
        "maximizeAnimation" => "maximize-style",
        "windowCloseAnimation" => "window-close-style",
        _ => "window-open-style",
    };
    format!("{row}-{value}")
}

/// Highlights each `(value, node id)` option that equals `current`; a missing value clears them all.
fn highlight_options(
    document: &mut impl UiDocumentAccess,
    current: Option<&str>,
    options: &[(&str, &str)],
    accent: UiColor,
) -> Result<(), String> {
    for (value, id) in options {
        highlight_segment(document, id, current == Some(*value), accent)?;
    }
    Ok(())
}

/// Paints one segment with the accent when selected, and clears its highlight otherwise.
fn highlight_segment(
    document: &mut impl UiDocumentAccess,
    id: &str,
    selected: bool,
    accent: UiColor,
) -> Result<(), String> {
    if selected {
        document.background(id, accent)
    } else {
        document.background_clear(id)
    }
}

/// Parses `<target>.preset.<0..5>`, the rest after `settings.color.`.
fn parse_color_preset(rest: &str) -> Result<SettingsAction, String> {
    let (target, rest) = rest
        .split_once(".preset.")
        .ok_or_else(|| format!("invalid color action '{rest}'"))?;
    let key = color_target_key(target)?;
    let index = rest
        .parse::<usize>()
        .ok()
        .and_then(|index| SURFACE_PRESETS.get(index))
        .ok_or_else(|| format!("unknown color preset '{rest}'"))?;
    Ok(SettingsAction::ColorSwatch { key, value: index })
}

/// Parses the picker actions, the rest after `settings.picker.`.
fn parse_color_picker(rest: &str) -> Result<SettingsAction, String> {
    match rest {
        "close" => return Ok(SettingsAction::ColorPickerClose),
        "apply" => return Ok(SettingsAction::ColorPickerApply),
        "hex.focus" => return Ok(SettingsAction::ColorPickerFocus),
        _ => {}
    }
    if let Some(target) = rest.strip_prefix("open.") {
        return Ok(SettingsAction::ColorPickerOpen(color_target_key(target)?));
    }
    let cell = rest
        .strip_prefix("pick.")
        .ok_or_else(|| format!("unknown picker action '{rest}'"))?;
    if let Some(step) = cell.strip_prefix("grey.") {
        let step = step
            .parse::<usize>()
            .ok()
            .filter(|step| *step < PICKER_GREYS)
            .ok_or_else(|| format!("unknown picker grey '{step}'"))?;
        return Ok(SettingsAction::ColorPickerPick(picker_grey(step)));
    }
    let (hue, shade) = cell
        .split_once('.')
        .and_then(|(hue, shade)| Some((hue.parse::<usize>().ok()?, shade.parse::<usize>().ok()?)))
        .filter(|(hue, shade)| *hue < PICKER_HUES && *shade < PICKER_SHADES)
        .ok_or_else(|| format!("unknown picker cell '{cell}'"))?;
    Ok(SettingsAction::ColorPickerPick(picker_grid_color(
        hue, shade,
    )))
}

fn color_target_key(target: &str) -> Result<&'static str, String> {
    COLOR_TARGETS
        .iter()
        .find(|(name, _)| *name == target)
        .map(|(_, key)| *key)
        .ok_or_else(|| format!("unknown color target '{target}'"))
}

/// Parses `#RRGGBB` (the leading `#` is optional) and returns it uppercase with `#`.
fn parse_hex_color(text: &str) -> Result<String, String> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(format!("#{}", digits.to_ascii_uppercase()))
    } else {
        Err(format!("'{text}' is not a #RRGGBB colour"))
    }
}

/// Palette cell at `hue` (0..PICKER_HUES) and `shade` (0..PICKER_SHADES): HSL with
/// fixed saturation and six lightness steps, lightest first.
fn picker_grid_color(hue: usize, shade: usize) -> String {
    const LIGHTNESS: [f64; PICKER_SHADES] = [0.88, 0.72, 0.56, 0.42, 0.28, 0.14];
    let lightness = LIGHTNESS[shade];
    let spread = (1.0 - (2.0 * lightness - 1.0).abs()) * 0.75;
    let sector = hue as f64 / 2.0;
    let x = spread * (1.0 - ((sector % 2.0) - 1.0).abs());
    let (r, g, b) = match sector.floor() as usize {
        0 => (spread, x, 0.0),
        1 => (x, spread, 0.0),
        2 => (0.0, spread, x),
        3 => (0.0, x, spread),
        4 => (x, 0.0, spread),
        _ => (spread, 0.0, x),
    };
    let offset = lightness - spread / 2.0;
    let channel = |value: f64| ((value + offset) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}", channel(r), channel(g), channel(b))
}

/// Neutral palette cell `step` (0..PICKER_GREYS), black to white.
fn picker_grey(step: usize) -> String {
    let value = (step * 255 / (PICKER_GREYS - 1)) as u8;
    format!("#{value:02X}{value:02X}{value:02X}")
}

/// Next value in `values`, wrapping at both ends. An unknown current value
/// starts from the first entry.
fn cycle_value(values: &[i64], current: i64, direction: i8) -> i64 {
    let len = values.len();
    if len == 0 {
        return current;
    }
    let index = values
        .iter()
        .position(|value| *value == current)
        .unwrap_or(0);
    let next = if direction < 0 {
        (index + len - 1) % len
    } else {
        (index + 1) % len
    };
    values[next]
}

fn cycle_text<'a>(values: &'a [&'a str], current: &str) -> &'a str {
    let index = values
        .iter()
        .position(|value| value.eq_ignore_ascii_case(current))
        .unwrap_or(0);
    values[(index + 1) % values.len()]
}

fn next_edge(edge: PanelEdge) -> PanelEdge {
    match edge {
        PanelEdge::Bottom => PanelEdge::Top,
        PanelEdge::Top => PanelEdge::Left,
        PanelEdge::Left => PanelEdge::Right,
        PanelEdge::Right => PanelEdge::Bottom,
    }
}

fn hotkey_row(slot: &str) -> Option<&'static HotkeyRow> {
    HOTKEY_ROWS.iter().find(|row| row.slot == slot)
}

/// Display label for a shortcut action owner, or the raw owner if it has no row.
fn shortcut_label(owner: &str) -> String {
    HOTKEY_ROWS
        .iter()
        .find(|row| row.action == owner)
        .map_or_else(|| owner.to_owned(), |row| row.label.to_owned())
}

fn nav_top(index: usize) -> f32 {
    72.0 + index as f32 * 32.0
}

/// Deletes the managed copy behind a cleared Start icon. Empty, unmanaged and
/// already-missing paths are left alone; the store never deletes foreign files.
fn remove_managed_media(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Ok(());
    }
    let Some(root) = MediaStore::default_root() else {
        return Ok(());
    };
    let path = Path::new(path);
    if !MediaStore::new(root).is_managed(path) {
        return Ok(());
    }
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("remove start icon copy: {error}"))
        }
        _ => Ok(()),
    }
}

/// Authoritative accent value behind each palette swatch in the Appearance page.
fn accent_for_swatch(name: &str) -> Option<&'static str> {
    match name {
        "red" => Some(DEFAULT_ACCENT),
        "blue" => Some("#3DAEE9"),
        "purple" => Some("#9B59B6"),
        "teal" => Some("#1ABC9C"),
        _ => None,
    }
}

/// An opaque swatch colour for a `#RRGGBB` value.
fn solid_color(value: &str) -> Result<UiColor, String> {
    Ok(UiColor {
        a: 255,
        ..accent_color(value)?
    })
}

/// Generates `UiDocumentAccess` methods that forward one write to `inner` and record its failure.
macro_rules! forward_writes {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(
            fn $name(&mut self, $($arg: $ty),*) -> Result<(), String> {
                if let Err(error) = self.inner.$name($($arg),*) {
                    self.failures.push(error);
                }
                Ok(())
            }
        )*
    };
}

/// Forwards every write to `inner`, but a rejected write (for example a missing node id)
/// is recorded instead of aborting the sync, so one bad node cannot blank the whole page.
/// `into_result` reports all recorded failures together.
struct NodeFailures<'a, D: UiDocumentAccess + ?Sized> {
    inner: &'a mut D,
    failures: Vec<String>,
}

impl<D: UiDocumentAccess + ?Sized> NodeFailures<'_, D> {
    fn into_result(self) -> Result<(), String> {
        if self.failures.is_empty() {
            Ok(())
        } else {
            Err(self.failures.join("; "))
        }
    }
}

impl<D: UiDocumentAccess + ?Sized> UiDocumentAccess for NodeFailures<'_, D> {
    forward_writes! {
        text(id: &str, text: String);
        image_rgb8(id: &str, image: flamewm_ui_x11::RuntimeImage);
        image_rgba8(id: &str, image: flamewm_ui_x11::RuntimeImage);
        image_reset(id: &str);
        visible(id: &str, visible: bool);
        toggle(track_id: &str, knob_id: &str, checked: bool);
        position(id: &str, left: f32, top: f32);
        size(id: &str, width: f32, height: f32);
        background(id: &str, color: UiColor);
        border(id: &str, color: UiColor);
        corner_radius(id: &str, radius_px: f32);
        node_backdrop(id: &str, blur_percent: Option<u8>);
        foreground(id: &str, color: UiColor);
        background_clear(id: &str);
        border_clear(id: &str);
        foreground_clear(id: &str);
        font_size(id: &str, size_px: f32);
        font_weight(id: &str, weight: u16);
        ui_font_defaults(size_offset: f32, bold: bool);
        font_family(family: String);
        color_variable(name: &str, color: UiColor);
        overflow(id: &str, x: flamewm_ui_x11::Overflow, y: flamewm_ui_x11::Overflow);
        layer(id: &str, layer: flamewm_ui_core::style::UiLayer);
    }
}

/// Paints every accent-coloured node (selected-sidebar line and ON toggles) with the live accent.
fn sync_accent_nodes(document: &mut impl UiDocumentAccess, accent: &str) -> Result<(), String> {
    let color = solid_color(accent)?;
    for id in ACCENT_NODE_IDS {
        document.background(id, color)?;
    }
    Ok(())
}

fn accent_color(value: &str) -> Result<UiColor, String> {
    let digits = value
        .strip_prefix('#')
        .filter(|digits| digits.len() == 6 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| format!("accent is not a #RRGGBB value: '{value}'"))?;
    let channel = |range: std::ops::Range<usize>, name: &str| {
        digits
            .get(range)
            .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            .ok_or_else(|| format!("invalid accent {name}"))
    };
    Ok(UiColor {
        r: channel(0..2, "red")?,
        g: channel(2..4, "green")?,
        b: channel(4..6, "blue")?,
        a: 56,
    })
}

fn page(value: &str) -> Option<SettingsPage> {
    match value {
        "appearance" => Some(SettingsPage::Appearance),
        "desktop" => Some(SettingsPage::Desktop),
        "taskbar" => Some(SettingsPage::Taskbar),
        "displays" => Some(SettingsPage::Displays),
        "fonts" => Some(SettingsPage::Fonts),
        "hotkeys" => Some(SettingsPage::Hotkeys),
        "about" => Some(SettingsPage::About),
        _ => None,
    }
}

fn control_error(error: ControlError) -> String {
    format!("{}: {}", error.name, error.message)
}

/// Asks the desktop to start a preview over the control bus.
/// The desktop owns the preview process; Settings only sends the typed request.
fn start_session_preview(action: SessionAction) -> Result<(), String> {
    let mut control = control::SettingsControl::connect_session()
        .map_err(|error| format!("could not reach the desktop: {error}"))?;
    control
        .call(ControlRequest::SessionAction(action))
        .map(|_| ())
        .map_err(control_error)
}

fn about_uri(action: &str) -> Option<&'static str> {
    match action {
        "about.donate" => Some(DONATE_URI),
        "about.source" => Some(SOURCE_URI),
        "about.website" => Some(WEBSITE_URI),
        _ => None,
    }
}

/// Parses the rest after `settings.theme.`: `close`, `install`, or `<icon|cursor>.<op>`
/// where `<op>` is `open`, `prev`, `next` or `select.<slot>`.
fn parse_theme_action(rest: &str) -> Result<SettingsAction, String> {
    match rest {
        "close" => return Ok(SettingsAction::ThemePickerClose),
        "install" => return Ok(SettingsAction::ThemeInstallChoose { folder: false }),
        "install-folder" => return Ok(SettingsAction::ThemeInstallChoose { folder: true }),
        _ => {}
    }
    let (target, operation) = rest
        .split_once('.')
        .ok_or_else(|| format!("unknown theme action '{rest}'"))?;
    let kind = theme_picker::PickerKind::parse(target)
        .ok_or_else(|| format!("unknown theme target '{target}'"))?;
    match operation {
        "open" => Ok(SettingsAction::ThemePickerOpen(kind)),
        "prev" => Ok(SettingsAction::ThemePickerPage(kind, PageStep::Prev)),
        "next" => Ok(SettingsAction::ThemePickerPage(kind, PageStep::Next)),
        _ => {
            let slot = operation
                .strip_prefix("select.")
                .and_then(|slot| slot.parse::<usize>().ok())
                .filter(|slot| *slot < theme_picker::PICKER_SLOTS)
                .ok_or_else(|| format!("unknown theme operation '{operation}'"))?;
            Ok(SettingsAction::ThemePickerSelect(kind, slot))
        }
    }
}

const WEATHER_UNSET_HINT: &str = "Not set - click the weather icon in the taskbar";

/// `(country, city)` from the shell's stored `lat\tlon\tcountry\tname` location.
fn decode_weather_location(encoded: &str) -> Option<(String, String)> {
    let mut parts = encoded.splitn(4, '\t');
    let coordinates_ok = [parts.next(), parts.next()]
        .into_iter()
        .all(|value| value.is_some_and(|value| value.parse::<f64>().is_ok()));
    let country = parts.next().unwrap_or_default();
    match parts.next() {
        Some(name) if coordinates_ok && !name.is_empty() => {
            Some((country.to_owned(), name.to_owned()))
        }
        _ => None,
    }
}

/// City name from the stored location, or the unset hint.
fn weather_location_label(encoded: &str) -> String {
    decode_weather_location(encoded).map_or_else(|| WEATHER_UNSET_HINT.to_owned(), |(_, city)| city)
}

/// Location row text; while automatic detection is ON the city is prefixed `Automatic:`.
fn weather_location_text(encoded: &str, automatic: bool) -> String {
    match decode_weather_location(encoded) {
        Some((country, city)) if automatic && country.is_empty() => format!("Automatic: {city}"),
        Some((country, city)) if automatic => format!("Automatic: {city}, {country}"),
        Some((_, city)) => city,
        None => WEATHER_UNSET_HINT.to_owned(),
    }
}

/// Current IANA zone: the IP-lookup cache only while automatic time zone is on, else the system zone.
fn time_zone_label(automatic: bool, cached: &str) -> String {
    pick_time_zone(automatic, cached, &system_time_zone())
}

fn pick_time_zone(automatic: bool, cached: &str, system: &str) -> String {
    if automatic && !cached.is_empty() {
        cached.to_owned()
    } else {
        system.to_owned()
    }
}

fn system_time_zone() -> String {
    std::fs::read_link("/etc/localtime")
        .ok()
        .and_then(|path| {
            path.to_str()
                .and_then(|path| path.strip_prefix("/usr/share/zoneinfo/"))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(test)]
mod time_zone_label_tests {
    use super::pick_time_zone;

    #[test]
    fn cached_auto_zone_is_ignored_when_automatic_switch_is_off() {
        assert_eq!(
            pick_time_zone(false, "Asia/Tokyo", "Europe/Paris"),
            "Europe/Paris"
        );
    }

    #[test]
    fn cached_auto_zone_is_shown_when_automatic_switch_is_on() {
        assert_eq!(
            pick_time_zone(true, "Asia/Tokyo", "Europe/Paris"),
            "Asia/Tokyo"
        );
        assert_eq!(pick_time_zone(true, "", "Europe/Paris"), "Europe/Paris");
    }
}

/// Parses a numeric action suffix that must be one of `choices`; `label` names the setting in errors.
fn integer_choice_action(
    key: &'static str,
    choices: &[i64],
    raw: &str,
    label: &str,
) -> Result<SettingsAction, String> {
    raw.parse::<i64>()
        .ok()
        .filter(|value| choices.contains(value))
        .map(|value| SettingsAction::IntegerSelect { key, value })
        .ok_or_else(|| format!("unknown {label} '{raw}'"))
}

/// Keys that `settings.toggle.<key>` may flip, in no particular order.
const TOGGLE_KEYS: [&str; 26] = [
    "stickyNotesEnabled",
    "fontBold",
    "animationsEnabled",
    "startAnimationEnabled",
    "windowOpenAnimationEnabled",
    "minimizeAnimationEnabled",
    "maximizeAnimationEnabled",
    "lockShowDate",
    "lockShowUsername",
    "lockOnSuspend",
    "taskbarShowSeconds",
    "taskbarShowDate",
    "taskbarShowWeather",
    "taskbarShowVolume",
    "taskbarShowNetwork",
    "taskbarShowBattery",
    "taskbarBatteryOnlyWhenRelevant",
    "taskbarShowClock",
    "taskbarShowWorkspaces",
    "weatherEnabled",
    "autoLocationEnabled",
    "autoTimeZoneEnabled",
    "snapPreviewEnabled",
    "focusFollowsMouse",
    "shareWallpaper",
    "cursorShadow",
];

/// Parses one `data-action` value. Unknown IDs are errors, never silent no-ops.
fn parse_action(action: &str) -> Result<SettingsAction, String> {
    if action.starts_with("keyboard.") {
        return Ok(SettingsAction::NativeKeyEvent);
    }
    if let Some(tab) = action
        .strip_prefix("settings.page.")
        .and_then(AppearanceTab::parse)
    {
        return Ok(SettingsAction::AppearanceTab(tab));
    }
    if let Some(page) = action.strip_prefix("settings.page.").and_then(page) {
        return Ok(SettingsAction::Page(page));
    }
    if let Some(name) = action.strip_prefix("settings.accent.") {
        return accent_for_swatch(name)
            .map(SettingsAction::AccentSwatch)
            .ok_or_else(|| format!("unknown accent '{name}'"));
    }
    if let Some(edge) = action.strip_prefix("settings.taskbar.position.") {
        return match edge {
            "bottom" => Ok(SettingsAction::TaskbarEdge(PanelEdge::Bottom)),
            "top" => Ok(SettingsAction::TaskbarEdge(PanelEdge::Top)),
            "left" => Ok(SettingsAction::TaskbarEdge(PanelEdge::Left)),
            "right" => Ok(SettingsAction::TaskbarEdge(PanelEdge::Right)),
            _ => Err(format!("unknown panel edge '{edge}'")),
        };
    }
    if let Some(key) = action.strip_prefix("settings.toggle.") {
        return TOGGLE_KEYS
            .into_iter()
            .find(|known| *known == key)
            .map(SettingsAction::Toggle)
            .ok_or_else(|| format!("unknown toggle '{key}'"));
    }
    if let Some(rest) = action.strip_prefix("settings.color.") {
        return parse_color_preset(rest);
    }
    if let Some(rest) = action.strip_prefix("settings.picker.") {
        return parse_color_picker(rest);
    }
    if let Some(direction) = action.strip_prefix("settings.surface-blur.").and_then(step) {
        return Ok(SettingsAction::PercentStep("surfaceBlurPercent", direction));
    }
    if let Some(direction) = action
        .strip_prefix("settings.surface-opacity.")
        .and_then(step)
    {
        return Ok(SettingsAction::PercentStep("surfaceOpacity", direction));
    }
    if let Some((slug, direction)) = action
        .strip_prefix("settings.")
        .and_then(|rest| rest.rsplit_once('.'))
        .filter(|(slug, _)| INTEGER_STEPPERS.iter().any(|row| row.slug == *slug))
    {
        let row = INTEGER_STEPPERS
            .iter()
            .find(|row| row.slug == slug)
            .ok_or_else(|| format!("unknown stepper '{slug}'"))?;
        let direction =
            step(direction).ok_or_else(|| format!("unknown step '{direction}' for {slug}"))?;
        return Ok(SettingsAction::IntegerStep {
            key: row.key,
            delta: i64::from(direction) * row.step,
            minimum: row.minimum,
            maximum: row.maximum,
        });
    }
    if let Some(direction) = action
        .strip_prefix("settings.accent-opacity.")
        .and_then(step)
    {
        return Ok(SettingsAction::PercentStep("accentOpacity", direction));
    }
    if action == ANIMATION_SPEED_SLIDER {
        return Ok(SettingsAction::AnimationSpeedSlider);
    }
    if action == DISPLAY_SCALE_SLIDER {
        return Ok(SettingsAction::DisplayScaleSlider);
    }
    if let Some(direction) = action
        .strip_prefix("settings.selection-opacity.")
        .and_then(step)
    {
        return Ok(SettingsAction::SelectionOpacity(direction));
    }
    if let Some(direction) = action.strip_prefix("settings.snap-opacity.").and_then(step) {
        return Ok(SettingsAction::SnapOpacity(direction));
    }
    if let Some(direction) = action
        .strip_prefix("settings.desktop-entry-size.")
        .and_then(step)
    {
        return Ok(SettingsAction::DesktopEntrySize(direction));
    }
    if let Some(direction) = action.strip_prefix("settings.font-size.").and_then(step) {
        return Ok(SettingsAction::FontSize(direction));
    }
    if action == "settings.display.arrange.left" {
        return Ok(SettingsAction::DisplayArrange(display_layout::Side::Left));
    }
    if action == "settings.display.arrange.right" {
        return Ok(SettingsAction::DisplayArrange(display_layout::Side::Right));
    }
    if let Some(slot) = action.strip_prefix("settings.display.select.") {
        return slot
            .parse::<usize>()
            .map(SettingsAction::DisplaySelect)
            .map_err(|_| format!("invalid display slot '{slot}'"));
    }
    if let Some(slot) = action.strip_prefix("settings.display.slot.") {
        return slot
            .parse::<usize>()
            .map(SettingsAction::DisplaySlot)
            .map_err(|_| format!("invalid display mode slot '{slot}'"));
    }
    if let Some(raw_mode) = action
        .strip_prefix("settings.display.begin.")
        .or_else(|| action.strip_prefix("settings.display.mode."))
    {
        return raw_mode
            .parse::<u64>()
            .map(|mode| SettingsAction::DisplayBegin(ModeId(mode)))
            .map_err(|_| format!("invalid display mode '{raw_mode}'"));
    }
    if let Some(percent) = action.strip_prefix("settings.display.scale.") {
        return percent
            .parse::<u16>()
            .map(SettingsAction::DisplayScale)
            .map_err(|_| format!("invalid display scale '{percent}'"));
    }
    if let Some(rest) = action.strip_prefix("settings.hotkey.") {
        let (slot, operation) = match rest.split_once('.') {
            Some((slot, "clear")) => (slot, HotkeyOperation::Clear),
            Some((slot, "reset")) => (slot, HotkeyOperation::Reset),
            Some(_) => return Err(format!("unknown hotkey operation '{rest}'")),
            None => (rest, HotkeyOperation::Capture),
        };
        let row = hotkey_row(slot).ok_or_else(|| format!("unknown hotkey row '{slot}'"))?;
        return Ok(SettingsAction::Hotkey(row.action, operation));
    }
    if let Some(uri) = about_uri(action) {
        return Ok(SettingsAction::OpenUri(uri));
    }
    if let Some(rest) = action.strip_prefix("settings.theme.") {
        return parse_theme_action(rest);
    }
    if let Some((key, value)) = action
        .strip_prefix("settings.")
        .and_then(|rest| rest.split_once('.'))
    {
        if let Some((row, values)) = ANIMATION_STYLE_ROWS.iter().find(|(row, _)| *row == key) {
            return values
                .iter()
                .find(|allowed| **allowed == value)
                .map(|allowed| SettingsAction::TextSelect {
                    key: row,
                    value: allowed,
                })
                .ok_or_else(|| format!("unknown animation style '{value}' for {key}"));
        }
    }
    if let Some(size) = action.strip_prefix("settings.cursor.size.") {
        return integer_choice_action("cursorSize", &CURSOR_SIZE_CHOICES, size, "cursor size");
    }
    if let Some(minutes) = action.strip_prefix("settings.lock.timeout.") {
        return integer_choice_action(
            "lockTimeoutMinutes",
            &LOCK_TIMEOUT_CHOICES,
            minutes,
            "lock timeout",
        );
    }
    match action {
        "settings.lock.clock.24h" => Ok(SettingsAction::TextSelect {
            key: "lockClockFormat",
            value: "24h",
        }),
        "settings.lock.clock.12h" => Ok(SettingsAction::TextSelect {
            key: "lockClockFormat",
            value: "12h",
        }),
        "settings.taskbar.style.classic" => Ok(SettingsAction::TextSelect {
            key: "taskbarStyle",
            value: "classic",
        }),
        "settings.taskbar.style.floating" => Ok(SettingsAction::TextSelect {
            key: "taskbarStyle",
            value: "floating",
        }),
        "settings.taskbar.clock.24h" => Ok(SettingsAction::TextSelect {
            key: "taskbarClockFormat",
            value: "24h",
        }),
        "settings.taskbar.clock.12h" => Ok(SettingsAction::TextSelect {
            key: "taskbarClockFormat",
            value: "12h",
        }),
        "settings.weather.units.celsius" => Ok(SettingsAction::TextSelect {
            key: "weatherUnits",
            value: "celsius",
        }),
        "settings.weather.units.fahrenheit" => Ok(SettingsAction::TextSelect {
            key: "weatherUnits",
            value: "fahrenheit",
        }),
        "settings.weather.location.clear" => Ok(SettingsAction::TextSelect {
            key: "weatherLocation",
            value: "",
        }),
        "settings.wallpaper.fit" => Ok(SettingsAction::TextCycle {
            key: "wallpaperFit",
            values: &WALLPAPER_FITS,
        }),
        "settings.screensaver.mode.ribbons" => Ok(SettingsAction::TextSelect {
            key: "screensaverMode",
            value: "ribbons",
        }),
        "settings.screensaver.mode.none" => Ok(SettingsAction::TextSelect {
            key: "screensaverMode",
            value: "none",
        }),
        "settings.lock.background.choose" => Ok(SettingsAction::LockBackgroundChoose),
        "settings.lock.background.clear" => Ok(SettingsAction::LockBackgroundClear),
        "settings.lock.background.default" => Ok(SettingsAction::LockBackgroundDefault),
        "settings.avatar.choose" => Ok(SettingsAction::AvatarChoose),
        "settings.avatar.clear" => Ok(SettingsAction::AvatarClear),
        "settings.lock.preview" => Ok(SettingsAction::LockPreview),
        "settings.screensaver.preview" => Ok(SettingsAction::ScreensaverPreview),
        "settings.wallpaper.choose" => Ok(SettingsAction::WallpaperChoose),
        "settings.wallpaper.default" => Ok(SettingsAction::WallpaperDefault),
        "settings.wallpaper.clear" => Ok(SettingsAction::WallpaperClear),
        "settings.taskbar.position" => Ok(SettingsAction::TaskbarEdgeCycle),
        "settings.taskbar.size" => Ok(SettingsAction::TaskbarSizeCycle),
        "settings.font.family" => Ok(SettingsAction::FontFamilyCycle),
        "settings.decoration.rounded" => Ok(SettingsAction::TextSelect {
            key: "decoration",
            value: "rounded",
        }),
        "settings.appearance.apps.dark" => Ok(SettingsAction::TextSelect {
            key: "appearance",
            value: "dark",
        }),
        "settings.appearance.apps.light" => Ok(SettingsAction::TextSelect {
            key: "appearance",
            value: "light",
        }),
        "settings.decoration.squared" => Ok(SettingsAction::TextSelect {
            key: "decoration",
            value: "squared",
        }),
        "settings.start-icon.choose" => Ok(SettingsAction::StartIconChoose),
        "settings.start-icon.clear" => Ok(SettingsAction::StartIconClear),
        "settings.titlebar.button-style.circle" => Ok(SettingsAction::TextSelect {
            key: "titlebarButtonStyle",
            value: "circle",
        }),
        "settings.titlebar.button-style.square" => Ok(SettingsAction::TextSelect {
            key: "titlebarButtonStyle",
            value: "square",
        }),
        "settings.display.keep" => Ok(SettingsAction::DisplayKeep),
        "settings.display.revert" => Ok(SettingsAction::DisplayRevert),
        // The Taskbar output pager shares the Displays output cursor.
        "settings.display.outputs.prev" => Ok(SettingsAction::OutputPage(PageStep::Prev)),
        "settings.display.outputs.next" => Ok(SettingsAction::OutputPage(PageStep::Next)),
        "settings.display.modes.prev" => Ok(SettingsAction::ModePage(PageStep::Prev)),
        "settings.display.modes.next" => Ok(SettingsAction::ModePage(PageStep::Next)),
        _ => Err(format!("unknown settings action '{action}'")),
    }
}

#[must_use]
pub fn panel_edge_label(edge: PanelEdge) -> &'static str {
    match edge {
        PanelEdge::Bottom => "Bottom",
        PanelEdge::Top => "Top",
        PanelEdge::Left => "Left",
        PanelEdge::Right => "Right",
    }
}

/// Label for one display-mode slot, e.g. `1920 x 1080 @ 60.00 Hz · current`.
#[must_use]
pub fn display_mode_label(mode: &flamewm_api::display::DisplayMode, current: bool) -> String {
    let tag = if current {
        " · current"
    } else if mode.preferred {
        " · preferred"
    } else {
        ""
    };
    format!(
        "{} x {} @ {:.2} Hz{tag}",
        mode.resolution.width,
        mode.resolution.height,
        f64::from(mode.refresh_millihz) / 1000.0
    )
}

#[must_use]
pub fn font_size_label(offset: i64) -> String {
    match offset {
        0 => "Default".to_owned(),
        value => format!("{value:+}"),
    }
}

/// Profile picture label: the managed file name, or `Default` for the built-in ring icon.
fn avatar_label(path: &str) -> String {
    if path.is_empty() {
        "Default".to_owned()
    } else {
        file_name_label(path)
    }
}

/// Start icon label: the managed file name, or `Built-in icon` for the default logo.
fn start_icon_label(path: &str) -> String {
    if path.is_empty() {
        "Built-in icon".to_owned()
    } else {
        file_name_label(path)
    }
}

fn file_name_label(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_owned()
}

#[cfg(test)]
mod cursor_reload_tests {
    use flamewm_session_core::cursor_theme_reload;

    #[test]
    fn cursor_reload_only_when_resolved_theme_changes() {
        let root =
            std::env::temp_dir().join(format!("flamewm-settings-cursor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Custom").join("cursors")).expect("theme");
        let search = root.display().to_string();
        let first = cursor_theme_reload(None, "Custom", &search);
        assert_eq!(first.as_deref(), Some("Custom"));
        assert_eq!(
            cursor_theme_reload(first.as_deref(), "Custom", &search),
            None
        );
        let fallback = flamewm_session_core::resolve_cursor_theme("Missing", &search);
        assert_eq!(
            cursor_theme_reload(Some(&fallback), "Missing", &search),
            None
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod test_transport {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use flamewm_control_core::{ControlError, ControlRequest, ControlResponse};
    use flamewm_settings_core::ControlTransport;

    #[derive(Default)]
    pub(super) struct ScriptedTransport {
        pub(super) responses: VecDeque<Result<ControlResponse, ControlError>>,
        pub(super) requests: Rc<RefCell<Vec<ControlRequest>>>,
    }

    impl ControlTransport for ScriptedTransport {
        fn call(&mut self, request: ControlRequest) -> Result<ControlResponse, ControlError> {
            self.requests.borrow_mut().push(request);
            self.responses
                .pop_front()
                .expect("scripted response was not queued")
        }
    }
}

#[cfg(test)]
mod value_text_tests {
    use super::*;

    macro_rules! assert_action {
        ($action:expr, $($pat:tt)+) => {
            assert!(matches!(parse_action($action), $($pat)+))
        };
    }

    #[test]
    fn avatar_chooser_targets_avatar_media_and_setting() {
        assert_eq!(ChooserTarget::Avatar.media_kind(), MediaKind::Avatar);
        assert_eq!(
            ChooserTarget::LockBackground.media_kind(),
            MediaKind::LockWallpaper
        );
        assert_eq!(
            ChooserTarget::Wallpaper.media_kind(),
            MediaKind::DesktopWallpaper
        );
        assert_eq!(ChooserTarget::StartIcon.media_kind(), MediaKind::StartIcon);
    }

    #[test]
    fn avatar_actions_parse_to_choose_and_clear() {
        assert!(matches!(
            parse_action("settings.avatar.choose"),
            Ok(SettingsAction::AvatarChoose)
        ));
        assert!(matches!(
            parse_action("settings.avatar.clear"),
            Ok(SettingsAction::AvatarClear)
        ));
        assert!(parse_action("settings.avatar.reset").is_err());
    }

    #[test]
    fn avatar_label_shows_file_name_or_default() {
        assert_eq!(avatar_label(""), "Default");
        assert_eq!(
            avatar_label("/home/u/.local/share/flamewm/wallpapers/avatar-0123456789abcdef.png"),
            "avatar-0123456789abcdef.png"
        );
    }

    #[test]
    fn theme_picker_actions_parse_to_typed_targets() {
        use theme_picker::PickerKind;
        assert_action!(
            "settings.theme.icon.open",
            Ok(SettingsAction::ThemePickerOpen(PickerKind::Icon))
        );
        assert_action!(
            "settings.theme.cursor.next",
            Ok(SettingsAction::ThemePickerPage(
                PickerKind::Cursor,
                PageStep::Next
            ))
        );
        assert_action!(
            "settings.theme.icon.select.5",
            Ok(SettingsAction::ThemePickerSelect(PickerKind::Icon, 5))
        );
        assert!(matches!(
            parse_action("settings.theme.close"),
            Ok(SettingsAction::ThemePickerClose)
        ));
        assert_action!(
            "settings.theme.install",
            Ok(SettingsAction::ThemeInstallChoose { folder: false })
        );
        assert_action!(
            "settings.theme.install-folder",
            Ok(SettingsAction::ThemeInstallChoose { folder: true })
        );
    }

    #[test]
    fn theme_picker_actions_reject_out_of_range_and_retired_cycle() {
        for action in [
            "settings.theme.icon.select.6",
            "settings.theme.icon.select.x",
            "settings.theme.font.open",
            "settings.theme.icon.sideways",
            "settings.icon-theme.next",
        ] {
            assert!(parse_action(action).is_err(), "{action} must be rejected");
        }
    }

    #[test]
    fn selecting_a_theme_writes_its_stable_id_and_closes_the_picker() {
        use flamewm_api::settings::{SettingValue, SettingsChange, SettingsSnapshot};
        use flamewm_control_core::{ControlRequest, ControlResponse};
        use test_transport::ScriptedTransport;
        use theme_picker::{PickerKind, ThemeItem, ThemeRegistry};

        let item = |n: usize| ThemeItem {
            id: format!("theme-{n}"),
            name: format!("Theme {n}"),
            preview: None,
        };
        let mut transport = ScriptedTransport::default();
        transport.responses = (0..8)
            .map(|_| Ok(ControlResponse::Settings(SettingsSnapshot::new(1))))
            .collect();
        let requests = transport.requests.clone();
        let mut app = SettingsApplication::new(transport);
        app.themes = ThemeRegistry {
            icon: (0..8).map(item).collect(),
            cursor: Vec::new(),
        };

        // Selecting without an open picker is rejected before any request.
        assert!(
            app.execute(SettingsAction::ThemePickerSelect(PickerKind::Icon, 0))
                .is_err()
        );
        assert!(requests.borrow().is_empty());

        app.execute(SettingsAction::ThemePickerOpen(PickerKind::Icon))
            .expect("open");
        app.execute(SettingsAction::ThemePickerPage(
            PickerKind::Icon,
            PageStep::Next,
        ))
        .expect("page");
        app.execute(SettingsAction::ThemePickerSelect(PickerKind::Icon, 1))
            .expect("select");

        let requests = requests.borrow();
        let transaction = match requests.last() {
            Some(ControlRequest::ApplySettings(transaction)) => transaction,
            other => panic!("expected one ApplySettings, got {other:?}"),
        };
        assert_eq!(
            transaction.changes,
            vec![SettingsChange {
                key: "iconTheme".to_owned(),
                value: Some(SettingValue::Text("theme-7".to_owned())),
            }]
        );
        assert!(app.theme_picker.is_none(), "selection closes the picker");
    }

    #[test]
    fn value_text_mapping_is_authoritative() {
        assert_eq!(panel_edge_label(PanelEdge::Right), "Right");
        assert_eq!(font_size_label(0), "Default");
        assert_eq!(font_size_label(2), "+2");
        assert_eq!(font_size_label(-1), "-1");
    }

    #[test]
    fn taskbar_size_cycle_wraps_in_both_directions() {
        let sizes = [34_i64, 44, 52, 64, 72];
        assert_eq!(cycle_value(&sizes, 72, 1), 34);
        assert_eq!(cycle_value(&sizes, 34, -1), 72);
        assert_eq!(cycle_value(&sizes, 44, 1), 52);
    }

    #[test]
    fn color_swatches_and_coarse_steps_parse_to_fixed_typed_targets() {
        assert_action!(
            "settings.color.surface.preset.0",
            Ok(SettingsAction::ColorSwatch {
                key: "surfaceColor",
                value: "#191B1D"
            })
        );
        assert!(
            parse_action("settings.color.surface.preset.6").is_err(),
            "only six surface presets exist"
        );
        assert!(parse_action("settings.color.taskbar.preset.0").is_err());
        assert_action!(
            "settings.color.lockbackground.preset.5",
            Ok(SettingsAction::ColorSwatch {
                key: "lockBackgroundColor",
                value: "#9B59B6"
            })
        );
        assert!(
            parse_action("settings.picker.open.titlebar").is_err(),
            "titlebar has no colour target"
        );
        assert!(parse_action("settings.color.titlebar.preset.0").is_err());
        assert_action!("settings.picker.pick.0.0", Ok(SettingsAction::ColorPickerPick(value)) if value == "#F7C9C9");
        assert_action!("settings.picker.pick.grey.5", Ok(SettingsAction::ColorPickerPick(value)) if value == "#FFFFFF");
        for action in [
            "settings.color.taskbar.preset.9",
            "settings.color.taskbar.preset.x",
            "settings.color.desktop.preset.0",
            "settings.color.accent.r.step.32",
            "settings.picker.open.desktop",
            "settings.picker.pick.12.0",
            "settings.picker.pick.grey.6",
        ] {
            assert!(parse_action(action).is_err(), "{action} must be rejected");
        }
        assert_eq!(
            animation_style_node("startAnimation", "magic-lamp"),
            "start-style-magic-lamp"
        );
        assert_eq!(
            animation_style_node("windowOpenAnimation", "scale"),
            "window-open-style-scale"
        );
        assert_eq!(
            animation_style_node("windowCloseAnimation", "fade"),
            "window-close-style-fade"
        );
    }

    #[test]
    fn appearance_actions_parse_to_their_exact_typed_targets() {
        assert!(parse_action("settings.start-menu-opacity.up").is_err());
        assert_action!(
            "settings.accent-opacity.down",
            Ok(SettingsAction::PercentStep("accentOpacity", -1))
        );
        assert_action!(
            "settings.toggle.maximizeAnimationEnabled",
            Ok(SettingsAction::Toggle("maximizeAnimationEnabled"))
        );
        assert_action!(
            "settings.titlebar.button-style.square",
            Ok(SettingsAction::TextSelect {
                key: "titlebarButtonStyle",
                value: "square"
            })
        );
        assert!(parse_action("settings.titlebar.minimize.style").is_err());
        assert!(parse_action("settings.titlebar.button-style.cycle").is_err());
        assert_action!(
            "settings.decoration.rounded",
            Ok(SettingsAction::TextSelect {
                key: "decoration",
                value: "rounded"
            })
        );
        assert_action!(
            "settings.decoration.squared",
            Ok(SettingsAction::TextSelect {
                key: "decoration",
                value: "squared"
            })
        );
        assert_action!(
            "settings.appearance.apps.dark",
            Ok(SettingsAction::TextSelect {
                key: "appearance",
                value: "dark"
            })
        );
        assert_action!(
            "settings.appearance.apps.light",
            Ok(SettingsAction::TextSelect {
                key: "appearance",
                value: "light"
            })
        );
        assert!(parse_action("settings.appearance.apps.system").is_err());
        assert_eq!(
            flamewm_api::settings::AppearanceMode::default().as_str(),
            "dark",
            "application appearance defaults to Dark"
        );
        assert!(parse_action("settings.decoration.ios").is_err());
        assert!(parse_action("settings.decoration.cycle").is_err());
    }

    #[test]
    fn screensaver_preview_action_is_distinct_from_lock_preview() {
        assert_action!(
            "settings.screensaver.preview",
            Ok(SettingsAction::ScreensaverPreview)
        );
        assert_action!("settings.lock.preview", Ok(SettingsAction::LockPreview));
    }

    #[test]
    fn accent_reset_and_start_icon_actions_parse_to_their_typed_targets() {
        assert!(parse_action("settings.accent.reset").is_err());
        assert_action!(
            "settings.start-icon.clear",
            Ok(SettingsAction::StartIconClear)
        );
        assert_action!(
            "settings.start-icon.choose",
            Ok(SettingsAction::StartIconChoose)
        );
        assert_action!(
            "settings.page.surfaces",
            Ok(SettingsAction::AppearanceTab(AppearanceTab::Surfaces))
        );
        assert_action!(
            "settings.page.desktop",
            Ok(SettingsAction::Page(SettingsPage::Desktop))
        );
    }

    #[test]
    fn start_icon_label_shows_file_name_or_built_in_default() {
        assert_eq!(start_icon_label(""), "Built-in icon");
        assert_eq!(
            start_icon_label("/home/u/.local/share/flamewm/wallpapers/start-0123456789abcdef.webp"),
            "start-0123456789abcdef.webp"
        );
    }

    #[test]
    fn remove_managed_media_ignores_empty_and_unmanaged_paths() {
        assert!(remove_managed_media("").is_ok());
        assert!(remove_managed_media("/etc/hostname").is_ok());
        let unmanaged = std::env::temp_dir().join("flamewm-unmanaged-start-icon.png");
        std::fs::write(&unmanaged, b"x").expect("write fixture");
        assert!(remove_managed_media(unmanaged.to_str().expect("utf-8")).is_ok());
        assert!(unmanaged.exists(), "unmanaged files are never deleted");
        std::fs::remove_file(&unmanaged).expect("cleanup fixture");
    }

    #[test]
    fn lock_background_steppers_parse_to_their_exact_typed_targets() {
        assert_action!(
            "settings.lock-bg-blur.up",
            Ok(SettingsAction::IntegerStep {
                key: "lockBackgroundBlurPercent",
                delta: 5,
                minimum: 0,
                maximum: 100
            })
        );
        assert_action!(
            "settings.lock-bg-blur.down",
            Ok(SettingsAction::IntegerStep {
                key: "lockBackgroundBlurPercent",
                delta: -5,
                minimum: 0,
                maximum: 100
            })
        );
        assert_action!(
            "settings.lock-bg-dim.up",
            Ok(SettingsAction::IntegerStep {
                key: "lockBackgroundDimPercent",
                delta: 5,
                minimum: 0,
                maximum: 80
            })
        );
        assert_action!(
            "settings.lock-bg-dim.down",
            Ok(SettingsAction::IntegerStep {
                key: "lockBackgroundDimPercent",
                delta: -5,
                minimum: 0,
                maximum: 80
            })
        );
        assert!(parse_action("settings.lock-bg-dim.sideways").is_err());
    }

    #[test]
    fn taskbar_and_screensaver_actions_parse_to_their_exact_typed_targets() {
        assert_action!(
            "settings.surface-blur.up",
            Ok(SettingsAction::PercentStep("surfaceBlurPercent", 1))
        );
        assert_action!(
            "settings.surface-opacity.down",
            Ok(SettingsAction::PercentStep("surfaceOpacity", -1))
        );
        assert!(parse_action("settings.taskbar-blur.up").is_err());
        assert!(parse_action("settings.taskbar-opacity.up").is_err());
        assert!(parse_action("settings.start-menu-blur.down").is_err());
        assert_action!(
            "settings.taskbar-floating-margin.up",
            Ok(SettingsAction::IntegerStep {
                key: "taskbarFloatingMargin",
                delta: 1,
                minimum: 0,
                maximum: 32
            })
        );
        assert_action!(
            "settings.screensaver-ribbon-width.down",
            Ok(SettingsAction::IntegerStep {
                key: "screensaverRibbonWidthTenths",
                delta: -5,
                minimum: 20,
                maximum: 280
            })
        );
        assert_action!(
            "settings.screensaver-display-off.up",
            Ok(SettingsAction::IntegerStep {
                key: "screensaverDisplayOffMinutes",
                delta: 5,
                minimum: 0,
                maximum: 240
            })
        );
        assert_action!(
            "settings.cursor.size.48",
            Ok(SettingsAction::IntegerSelect {
                key: "cursorSize",
                value: 48
            })
        );
        assert_action!(
            "settings.taskbar.style.floating",
            Ok(SettingsAction::TextSelect {
                key: "taskbarStyle",
                value: "floating"
            })
        );
        assert_action!(
            "settings.taskbar.clock.12h",
            Ok(SettingsAction::TextSelect {
                key: "taskbarClockFormat",
                value: "12h"
            })
        );
        assert_action!(
            "settings.screensaver.mode.none",
            Ok(SettingsAction::TextSelect {
                key: "screensaverMode",
                value: "none"
            })
        );
        assert_action!(
            "settings.wallpaper.fit",
            Ok(SettingsAction::TextCycle {
                key: "wallpaperFit",
                ..
            })
        );
        assert!(parse_action("settings.toggle.screensaverSpanMonitors").is_err());
    }

    #[test]
    fn appearance_actions_reject_unknown_targets_channels_and_steps() {
        for action in [
            "settings.color.desktop.r.step.1",
            "settings.color.accent.x.step.1",
            "settings.color.accent.r.step.2",
            "settings.color.accent.r.step.0",
            "settings.color.accent.r",
            "settings.animation-speed.sideways",
        ] {
            assert!(parse_action(action).is_err(), "{action} must be rejected");
        }
    }

    #[test]
    fn display_scale_slider_maps_all_five_percent_steps() {
        for percent in (100_u16..=200).step_by(5) {
            assert_eq!(
                display_scale_from_slider(&(percent - 100).to_string()),
                Ok(percent)
            );
        }
        assert_eq!(display_scale_from_slider("0"), Ok(100));
        assert_eq!(display_scale_from_slider("100"), Ok(200));
        assert_eq!(display_scale_from_slider("35.1"), Ok(135));
        assert_eq!(display_scale_from_slider("94.9"), Ok(195));
        assert!(display_scale_from_slider("NaN").is_err());
        assert!(display_scale_from_slider("abc").is_err());
    }

    #[test]
    fn animation_speed_slider_maps_pointer_percent_to_snapped_hundredths() {
        assert_eq!(animation_speed_from_slider("0"), Ok(25));
        assert_eq!(animation_speed_from_slider("100"), Ok(200));
        // 50% of 25..200 is 112.5, which snaps to 115 (not 100 or 112).
        assert_eq!(animation_speed_from_slider("50"), Ok(115));
        // Default 1.00x = 100 hundredths sits at 75/175 of the track, snapped to 100.
        assert_eq!(animation_speed_from_slider("42.85714285714286"), Ok(100));
        // 37.5% -> 90.625 -> snaps to 90, not the raw 90.625.
        assert_eq!(animation_speed_from_slider("37.5"), Ok(90));
        assert_eq!(animation_speed_from_slider(" 37.5 "), Ok(90));
    }

    #[test]
    fn animation_speed_slider_clamps_and_rejects_non_numeric_values() {
        assert_eq!(animation_speed_from_slider("-40"), Ok(25));
        assert_eq!(animation_speed_from_slider("140"), Ok(200));
        assert!(animation_speed_from_slider("").is_err());
        assert!(animation_speed_from_slider("abc").is_err());
        assert!(animation_speed_from_slider("NaN").is_err());
        assert!(animation_speed_from_slider("inf").is_err());
    }

    #[test]
    fn accent_color_rejects_malformed_values_without_panicking() {
        assert!(accent_color("#EF4048").is_ok());
        assert!(accent_color("#é1234").is_err());
        assert!(accent_color("EF4048").is_err());
        assert!(accent_color("#+F4048").is_err());
        assert!(accent_color("#EF+048").is_err());
    }
}

#[cfg(test)]
mod display_pager_fixture_tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot};
    use flamewm_api::{Rect, Size};
    use flamewm_control_core::{ControlRequest, ControlResponse};
    use flamewm_settings_core::DISPLAY_MODE_SLOTS;

    use super::test_transport::ScriptedTransport;
    use super::*;

    fn five_output_twelve_mode_snapshot() -> DisplaySnapshot {
        let outputs = (0..5_u64)
            .map(|output_index| {
                let output_id = format!("DP-{output_index}");
                let first_mode_id = output_index * 100 + 1;
                let modes = (0..12_u64)
                    .map(|mode_index| DisplayMode {
                        id: ModeId(first_mode_id + mode_index),
                        resolution: Size::new(1280 + mode_index as i32 * 10, 720),
                        refresh_millihz: 60_000 + mode_index as i32 * 1_000,
                        preferred: mode_index == 0,
                    })
                    .collect();
                OutputSnapshot {
                    id: OutputId::new(&output_id),
                    connector: output_id.clone(),
                    edid_identity: format!("fixture-{output_id}"),
                    connected: true,
                    primary: output_index == 0,
                    geometry: Rect::new(output_index as i32 * 1280, 0, 1280, 720),
                    current_mode: ModeId(first_mode_id + 10),
                    modes,
                    shell_scale_percent: 100,
                }
            })
            .collect();
        DisplaySnapshot {
            generation: 42,
            outputs,
            pending: None,
        }
    }

    #[test]
    fn five_outputs_and_twelve_modes_are_reached_through_pager_actions() {
        let mut transport = ScriptedTransport::default();
        let requests = Rc::new(RefCell::new(Vec::new()));
        transport.requests = requests.clone();
        transport.responses.push_back(Ok(ControlResponse::Displays(
            five_output_twelve_mode_snapshot(),
        )));
        transport
            .responses
            .push_back(Ok(ControlResponse::Transaction(
                flamewm_api::TransactionId(7),
            )));
        let mut app = SettingsApplication::new(transport);
        app.client.refresh_displays().expect("display snapshot");
        app.reconcile_pages();

        // Four output rows per page: advance once, then select slot zero (absolute output 4).
        assert_eq!(page_count(app.output_count(), OUTPUT_PAGE_SLOTS), 2);
        app.execute(SettingsAction::OutputPage(PageStep::Next))
            .expect("next output page");
        assert_eq!(app.pages.output_page, 1);
        app.execute(SettingsAction::DisplaySelect(0))
            .expect("select fifth output");
        assert_eq!(app.client.ui().selected_output, Some(OutputId::new("DP-4")));

        // Four vertical mode rows per page avoid the overflowing right column.
        assert_eq!(DISPLAY_MODE_SLOTS, 4);
        assert_eq!(app.client.display_mode_count(), 12);
        assert_eq!(
            page_count(app.client.display_mode_count(), DISPLAY_MODE_SLOTS),
            3
        );
        app.execute(SettingsAction::ModePage(PageStep::Next))
            .expect("next mode page");
        app.execute(SettingsAction::ModePage(PageStep::Next))
            .expect("last mode page");
        let fifth_output_current_mode = ModeId(411);
        let fifth_output_mode_11 = ModeId(412);
        let selected_output = app
            .client
            .displays()
            .and_then(|snapshot| snapshot.outputs.get(4))
            .expect("fifth output");
        assert_eq!(selected_output.current_mode, fifth_output_current_mode);
        assert_eq!(
            app.client.display_mode_page(app.pages.mode_page)[2].id,
            fifth_output_current_mode
        );
        assert!(
            display_mode_label(
                &app.client.display_mode_page(app.pages.mode_page)[2],
                selected_output.current_mode == fifth_output_current_mode,
            )
            .contains("current")
        );
        assert_eq!(
            app.client.display_mode_page(app.pages.mode_page)[3].id,
            fifth_output_mode_11
        );
        assert!(
            !display_mode_label(
                &app.client.display_mode_page(app.pages.mode_page)[3],
                selected_output.current_mode == fifth_output_mode_11,
            )
            .contains("current")
        );

        app.execute(SettingsAction::DisplaySlot(3))
            .expect("begin mode 11");
        assert!(matches!(
            requests.borrow().last(),
            Some(ControlRequest::BeginDisplayMode {
                output,
                mode,
                expected_generation: 42,
                ..
            }) if output == &OutputId::new("DP-4") && *mode == fifth_output_mode_11
        ));
        assert_eq!(
            app.client.ui().pending_display_transaction,
            Some(flamewm_api::TransactionId(7))
        );

        // End-of-list pager state is derived from the data and cannot advance past its last page.
        assert_eq!(
            step_page(app.pages.mode_page, PageStep::Next, 3),
            app.pages.mode_page
        );
        assert_eq!(
            page_label(app.pages.mode_page, DISPLAY_MODE_SLOTS, 12),
            "9-12 of 12"
        );

        // A cursor stamped against the prior generation rejects the same slot before any request.
        app.pages.generation = Some(41);
        let request_count = requests.borrow().len();
        let pending_before = app.client.ui().pending_display_transaction;
        assert!(
            app.execute(SettingsAction::DisplaySlot(3))
                .unwrap_err()
                .contains("stale")
        );
        assert_eq!(requests.borrow().len(), request_count);
        assert_eq!(app.client.ui().pending_display_transaction, pending_before);
    }
}

#[cfg(test)]
mod accent_node_tests {
    use std::collections::BTreeMap;

    use flamewm_api::settings::{SettingValue, SettingsSnapshot};
    use flamewm_control_core::ControlResponse;
    use flamewm_ui_x11::{RuntimeImage, UiColor, UiDocumentAccess};

    use super::test_transport::ScriptedTransport;
    use super::{ACCENT_NODE_IDS, SettingsApplication, solid_color};

    #[derive(Default)]
    struct ColorDocument {
        backgrounds: BTreeMap<String, UiColor>,
        reject_positions: bool,
    }

    impl UiDocumentAccess for ColorDocument {
        fn text(&mut self, _id: &str, _text: String) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)
        }

        fn image_rgb8(&mut self, _id: &str, _image: RuntimeImage) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)
        }

        fn image_rgba8(&mut self, _id: &str, _image: RuntimeImage) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)
        }

        fn visible(&mut self, _id: &str, _visible: bool) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)
        }

        fn toggle(
            &mut self,
            _track_id: &str,
            _knob_id: &str,
            _checked: bool,
        ) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_track_id)?;
            crate::ui_contract::require_markup_id(_knob_id)
        }

        fn position(&mut self, _id: &str, _left: f32, _top: f32) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)?;
            if self.reject_positions {
                return Err(format!("position rejected for {_id}"));
            }
            Ok(())
        }

        fn size(&mut self, _id: &str, _width: f32, _height: f32) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)
        }

        fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
            crate::ui_contract::require_markup_id(id)?;
            self.backgrounds.insert(id.to_owned(), color);
            Ok(())
        }

        fn border(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)
        }

        fn foreground(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)
        }

        fn background_clear(&mut self, _id: &str) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)
        }

        fn border_clear(&mut self, _id: &str) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)
        }

        fn foreground_clear(&mut self, _id: &str) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)
        }

        fn font_size(&mut self, _id: &str, _size_px: f32) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)
        }

        fn font_weight(&mut self, _id: &str, _weight: u16) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)
        }

        fn overflow(
            &mut self,
            _id: &str,
            _x: flamewm_ui_x11::Overflow,
            _y: flamewm_ui_x11::Overflow,
        ) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)
        }

        fn layer(
            &mut self,
            _id: &str,
            _layer: flamewm_ui_core::style::UiLayer,
        ) -> Result<(), String> {
            crate::ui_contract::require_markup_id(_id)
        }
    }

    fn application_with_accent(accent: &str) -> SettingsApplication<ScriptedTransport> {
        let mut snapshot = SettingsSnapshot::new(1);
        snapshot
            .values
            .insert("accent".to_owned(), SettingValue::Text(accent.to_owned()));
        snapshot
            .values
            .insert("animationsEnabled".to_owned(), SettingValue::Boolean(true));
        let mut transport = ScriptedTransport::default();
        transport
            .responses
            .push_back(Ok(ControlResponse::Settings(snapshot)));
        let mut application = SettingsApplication::new(transport);
        application
            .client_mut()
            .refresh_settings()
            .expect("initial snapshot");
        application
    }

    #[test]
    fn accent_nodes_carry_the_active_accent_not_the_compiled_red() {
        for accent in ["#3DAEE9", "#9B59B6", "#1ABC9C", "#123456"] {
            let application = application_with_accent(accent);
            let mut document = ColorDocument::default();
            application.sync(&mut document).expect("sync");
            let expected = solid_color(accent).expect("accent");
            for id in ACCENT_NODE_IDS {
                assert_eq!(
                    document.backgrounds.get(*id),
                    Some(&expected),
                    "{id} under {accent}"
                );
            }
            assert!(!document.backgrounds.contains_key("anim-global-off"));
        }
    }

    #[test]
    fn sync_reports_recorded_write_failures_alongside_a_later_error() {
        let application = application_with_accent("not-a-colour");
        let mut document = ColorDocument {
            reject_positions: true,
            ..ColorDocument::default()
        };
        let error = application.sync(&mut document).expect_err("sync must fail");
        assert!(
            error.contains("position rejected for nav-selection"),
            "{error}"
        );
        assert!(error.contains("accent is not a #RRGGBB value"), "{error}");
    }

    #[test]
    fn red_swatch_still_paints_red_accent_nodes() {
        let application = application_with_accent("#EF4048");
        let mut document = ColorDocument::default();
        application.sync(&mut document).expect("sync");
        for id in ACCENT_NODE_IDS {
            let color = document.backgrounds.get(*id).expect("accent node painted");
            assert_eq!(
                (color.r, color.g, color.b, color.a),
                (0xEF, 0x40, 0x48, 255),
                "{id}"
            );
        }
    }

    #[test]
    fn accent_registry_covers_every_toggle_on_twin_in_markup() {
        let html = include_str!("../../../ui/settings.html");
        let marker = "toggle toggle-on\" id=\"";
        let ids: Vec<&str> = html
            .split(marker)
            .skip(1)
            .map(|after| &after[..after.find('"').expect("closed toggle id")])
            .collect();
        assert!(!ids.is_empty(), "no toggle-on ids found in settings.html");
        let missing: Vec<&str> = ids
            .into_iter()
            .filter(|id| !ACCENT_NODE_IDS.contains(id))
            .collect();
        assert!(
            missing.is_empty(),
            "toggle-on ids missing from ACCENT_NODE_IDS: {missing:?}"
        );
    }
}

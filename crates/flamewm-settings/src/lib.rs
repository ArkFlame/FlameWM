use std::process::Command;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::sync::OnceLock;
use std::time::Instant;

pub mod control;
pub mod file_chooser;
pub mod runtime;
pub mod theme_picker;
pub mod view;

#[cfg(test)]
mod controller;
#[cfg(test)]
mod projection;

use flamewm_api::panels::PanelEdge;
use flamewm_api::settings::SettingValue;
use flamewm_api::{ModeId, OutputId};
use flamewm_control_core::ControlError;
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
    ORIGIN.get_or_init(Instant::now).elapsed().as_millis().min(u128::from(u64::MAX)) as u64
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
        return Err(format!("{what} slot {slot} is outside the page of {page_size}"));
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

static HOTKEY_ROWS: [HotkeyRow; 5] = [
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
];

#[derive(Debug, Clone, Copy)]
enum HotkeyOperation {
    Capture,
    Clear,
    Reset,
}

/// Every control action the Settings markup may emit. Parsing is pure, so the
/// markup coverage test can check each `data-action` without side effects.
enum SettingsAction {
    Page(SettingsPage),
    AccentSwatch(&'static str),
    AccentReset,
    /// Opens the icon or cursor theme picker on its first page.
    ThemePickerOpen(theme_picker::PickerKind),
    ThemePickerClose,
    ThemePickerPage(theme_picker::PickerKind, PageStep),
    /// Row `slot` on the open picker's current page.
    ThemePickerSelect(theme_picker::PickerKind, usize),
    /// Opens the portal chooser for one archive, or one folder when `folder` is set.
    ThemeInstallChoose { folder: bool },
    WallpaperChoose,
    WallpaperReset,
    Toggle(&'static str),
    SelectionOpacity(i8),
    SnapOpacity(i8),
    TaskbarOpacity(i8),
    FontSize(i8),
    TaskbarEdgeCycle,
    TaskbarEdge(PanelEdge),
    TaskbarColorCycle,
    TaskbarSizeCycle,
    FontFamilyCycle,
    Hotkey(&'static str, HotkeyOperation),
    /// Output row `slot` on the current output page.
    DisplaySelect(usize),
    /// Mode row `slot` on the current mode page.
    DisplaySlot(usize),
    OutputPage(PageStep),
    ModePage(PageStep),
    DisplayBegin(ModeId),
    DisplayScale(u16),
    DisplayKeep,
    DisplayRevert,
    OpenUri(&'static str),
    /// Adds `delta` to one channel (0 = R, 1 = G, 2 = B) of a `#RRGGBB` setting.
    ColorStep {
        key: &'static str,
        channel: usize,
        delta: i16,
    },
    /// Sets a `#RRGGBB` setting to one fixed palette value in one transaction.
    ColorSwatch {
        key: &'static str,
        value: &'static str,
    },
    /// Steps a percent setting by 5 within 0..=100.
    PercentStep(&'static str, i8),
    /// Drag-only slider; the value arrives as event text, see `handle_animation_speed_drag`.
    AnimationSpeedSlider,
    /// Advances a text setting through `values`, wrapping.
    TextCycle {
        key: &'static str,
        values: &'static [&'static str],
    },
    /// A native `keyboard.*` id from the X11 key source. It is not markup, so
    /// it never edits a row; hotkey capture is driven by the key-event path.
    NativeKeyEvent,
}

pub struct SettingsApplication<T: ControlTransport> {
    client: SettingsClient<T>,
    wallpaper_chooser: Option<file_chooser::WallpaperChooserWorker>,
    pending_open: Option<Receiver<Result<(), String>>>,
    /// True only between a hotkey row capture and its committing key release.
    /// Gates key events so a stale capture armed by the core cannot commit later.
    hotkey_armed: bool,
    error: Option<String>,
    pages: DisplayPages,
    themes: theme_picker::ThemeRegistry,
    /// The open theme picker and its current page.
    theme_picker: Option<(theme_picker::PickerKind, usize)>,
    theme_scan: Option<Receiver<theme_picker::ThemeRegistry>>,
    theme_install: Option<Receiver<theme_picker::InstallOutcome>>,
    theme_status: Option<String>,
}

impl<T: ControlTransport> SettingsApplication<T> {
    pub fn new(transport: T) -> Self {
        Self {
            client: SettingsClient::new(transport),
            wallpaper_chooser: None,
            pending_open: None,
            hotkey_armed: false,
            error: None,
            pages: DisplayPages::default(),
            themes: theme_picker::ThemeRegistry::default(),
            theme_picker: None,
            theme_scan: None,
            theme_install: None,
            theme_status: None,
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
        if event.action == ANIMATION_SPEED_SLIDER {
            return self.handle_animation_speed_drag(event, document);
        }
        if event.phase != UiActionPhase::Release || !event.inside {
            return Ok(());
        }
        let action = parse_action(event.action.as_str())?;
        self.execute(action)?;
        self.settle(document)
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
                Ok(Some(path)) => match self
                    .client
                    .set_wallpaper(path.to_string_lossy().into_owned())
                    .map_err(control_error)
                {
                    Ok(()) => wallpaper_changed = true,
                    Err(error) => completion_errors.push(error),
                },
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
    fn settle(&mut self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        self.error = None;
        self.reconcile_pages();
        self.sync(document)
    }

    /// Clamps both page cursors to the authoritative snapshots and stamps them with the
    /// current display generation. Runs after every successful refresh or action.
    fn reconcile_pages(&mut self) {
        let generation = self.client.displays().map_or(0, |snapshot| snapshot.generation);
        let output_pages = page_count(self.output_count(), OUTPUT_PAGE_SLOTS);
        let mode_pages = page_count(self.client.display_mode_count(), DISPLAY_MODE_SLOTS);
        self.pages = DisplayPages {
            generation: Some(generation),
            output_page: self.pages.output_page.min(output_pages.saturating_sub(1)),
            mode_page: self.pages.mode_page.min(mode_pages.saturating_sub(1)),
        };
    }

    fn output_count(&self) -> usize {
        self.client.displays().map_or(0, |snapshot| snapshot.outputs.len())
    }

    /// Output row `slot` of the current output page, i.e. absolute `page * 4 + slot`.
    fn output_page_slot(&self, slot: usize) -> Option<&flamewm_api::display::OutputSnapshot> {
        let index = self.pages.output_page * OUTPUT_PAGE_SLOTS + slot;
        self.client.displays()?.outputs.get(index)
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
        match action {
            SettingsAction::NativeKeyEvent => {}
            SettingsAction::Page(page) => self.client.set_page(page),
            SettingsAction::AccentSwatch(accent) => {
                self.apply_text("accent", accent.to_owned())?;
            }
            SettingsAction::AccentReset => {
                self.apply_text("accent", DEFAULT_ACCENT.to_owned())?;
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
            SettingsAction::WallpaperChoose => {
                if self.wallpaper_chooser.is_some() {
                    return Err("wallpaper chooser is already open".to_owned());
                }
                self.wallpaper_chooser = Some(file_chooser::WallpaperChooserWorker::start()?);
            }
            SettingsAction::WallpaperReset => {
                self.client.set_wallpaper("").map_err(control_error)?;
            }
            SettingsAction::Toggle(key) => {
                let enabled = self
                    .client
                    .settings()
                    .and_then(|snapshot| snapshot.values.get(key))
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
            SettingsAction::TaskbarOpacity(direction) => {
                self.set_taskbar_value("opacity", direction)?;
            }
            SettingsAction::FontSize(direction) => {
                self.step_integer("fontSizeOffset", i64::from(direction), -2, 4)?;
            }
            SettingsAction::ColorStep { key, channel, delta } => {
                let color = accent_color(&self.text_setting(key)?)?;
                let mut channels = [color.r, color.g, color.b];
                channels[channel] = (i16::from(channels[channel]) + delta).clamp(0, 255) as u8;
                let [r, g, b] = channels;
                self.apply_text(key, format!("#{r:02X}{g:02X}{b:02X}"))?;
            }
            SettingsAction::ColorSwatch { key, value } => self.apply_text(key, value.to_owned())?,
            SettingsAction::PercentStep(key, direction) => self.step_percent(key, direction, 100)?,
            // Value is applied from the drag text in `handle_animation_speed_drag`.
            SettingsAction::AnimationSpeedSlider => {}
            SettingsAction::TextCycle { key, values } => {
                let current = self.text_setting(key)?;
                self.apply_text(key, cycle_text(values, &current).to_owned())?;
            }
            SettingsAction::TaskbarEdgeCycle => self.cycle_panel_edge()?,
            SettingsAction::TaskbarEdge(edge) => {
                let (output, _) = self.selected_panel()?;
                self.client
                    .set_panel_edge(output, edge)
                    .map_err(control_error)?;
            }
            SettingsAction::TaskbarColorCycle => self.cycle_taskbar_color()?,
            // Size is the global `taskbarHeight` service setting, shared by all outputs.
            SettingsAction::TaskbarSizeCycle => self.set_taskbar_value("size", 1)?,
            SettingsAction::FontFamilyCycle => self.cycle_font_family()?,
            SettingsAction::Hotkey(action, operation) => self.handle_hotkey(action, operation)?,
            SettingsAction::DisplaySelect(slot) => {
                let generation = self.client.displays().map_or(0, |snapshot| snapshot.generation);
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
                    .client
                    .displays()
                    .and_then(|snapshot| snapshot.outputs.get(index))
                    .map(|output| output.id.clone())
                    .ok_or_else(|| format!("no display output at index {index}"))?;
                if !self.client.select_output(&output_id) {
                    return Err(format!("display output {index} is not an authoritative output"));
                }
                // Modes belong to the selected output, so its mode list starts on page 1.
                self.pages.mode_page = 0;
            }
            SettingsAction::DisplaySlot(slot) => {
                let generation = self.client.displays().map_or(0, |snapshot| snapshot.generation);
                let index = page_slot_index(
                    self.pages.generation.unwrap_or(generation),
                    generation,
                    self.pages.mode_page,
                    DISPLAY_MODE_SLOTS,
                    slot,
                    self.client.display_mode_count(),
                    "display mode",
                )?;
                let mode = self
                    .client
                    .display_mode_for_index(index)
                    .ok_or_else(|| format!("display mode at index {index} has no authoritative mode"))?;
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
            SettingsAction::DisplayKeep => self.client.keep_display_mode(monotonic_now_ms()).map_err(control_error)?,
            SettingsAction::DisplayRevert => {
                self.client.revert_display_mode().map_err(control_error)?;
            }
            SettingsAction::OpenUri(uri) => self.start_open_uri(uri)?,
        }
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
                    let pages = page_count(self.themes.items(kind).len(), theme_picker::PICKER_SLOTS);
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
        for kind in [theme_picker::PickerKind::Icon, theme_picker::PickerKind::Cursor] {
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

    fn apply_text(&mut self, key: &str, value: String) -> Result<(), String> {
        self.client
            .apply_setting(key, SettingValue::Text(value))
            .map_err(control_error)
    }

    fn begin_display_mode(&mut self, mode: ModeId) -> Result<(), String> {
        let now_ms = monotonic_now_ms();
        self.client
            .begin_display_mode(mode, now_ms)
            .map(|_| ())
            .map_err(control_error)
    }

    fn handle_hotkey(&mut self, action: &'static str, operation: HotkeyOperation) -> Result<(), String> {
        match operation {
            HotkeyOperation::Capture => {
                self.hotkey_armed = false;
                self.client.begin_hotkey_capture(action).map_err(control_error)?;
                self.hotkey_armed = true;
                Ok(())
            }
            HotkeyOperation::Clear => {
                self.client.begin_hotkey_capture(action).map_err(control_error)?;
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
                    Ok(status) => {
                        Err(format!("could not open {uri}: xdg-open exited with {status}"))
                    }
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

    fn set_taskbar_value(&mut self, kind: &str, direction: i8) -> Result<(), String> {
        let color = self.text_setting("taskbarColor")?;
        let opacity = self.percent_setting("taskbarOpacity")?;
        let height = self.integer_setting("taskbarHeight")?;
        let (opacity, height) = match kind {
            "opacity" => (
                (i16::from(opacity) + i16::from(direction) * 5).clamp(0, 100) as u8,
                height,
            ),
            "size" => {
                let sizes = [34_i64, 44, 52, 64, 72];
                (opacity, cycle_value(&sizes, height, direction))
            }
            _ => return Err(format!("unknown taskbar setting '{kind}'")),
        };
        self.client
            .set_taskbar_appearance(color, opacity, height as u16)
            .map_err(control_error)
    }

    fn cycle_taskbar_color(&mut self) -> Result<(), String> {
        const COLORS: [&str; 3] = ["#191B1D", "#25282B", "#3D4145"];
        let current = self.text_setting("taskbarColor")?;
        let next = cycle_text(&COLORS, &current);
        self.apply_text("taskbarColor", next.to_owned())
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

    fn percent_setting(&self, key: &str) -> Result<u8, String> {
        match self
            .client
            .settings()
            .and_then(|snapshot| snapshot.values.get(key))
        {
            Some(SettingValue::Percent(value)) => Ok(*value),
            _ => Err(format!(
                "setting '{key}' has no authoritative percent value"
            )),
        }
    }

    fn integer_setting(&self, key: &str) -> Result<i64, String> {
        match self
            .client
            .settings()
            .and_then(|snapshot| snapshot.values.get(key))
        {
            Some(SettingValue::Integer(value)) => Ok(*value),
            _ => Err(format!(
                "setting '{key}' has no authoritative integer value"
            )),
        }
    }

    fn text_setting(&self, key: &str) -> Result<String, String> {
        match self
            .client
            .settings()
            .and_then(|snapshot| snapshot.values.get(key))
        {
            Some(SettingValue::Text(value)) => Ok(value.clone()),
            _ => Err(format!("setting '{key}' has no authoritative text value")),
        }
    }

    fn sync(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        document.position("nav-selection", 8.0, nav_top(self.client.ui().page))?;
        if let Some(SettingValue::Text(accent)) = self
            .client
            .settings()
            .and_then(|snapshot| snapshot.values.get("accent"))
        {
            document.background("nav-selection", accent_color(accent)?)?;
            for name in ["red", "blue", "purple", "teal"] {
                let palette = accent_for_swatch(name);
                document.visible(
                    &format!("accent-selected-{name}"),
                    palette.is_some_and(|value| value.eq_ignore_ascii_case(accent)),
                )?;
            }
        }
        for (name, page) in [
            ("appearance", SettingsPage::Appearance),
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
            document.text(
                &format!("display-output-name-{slot}"),
                output.connector.clone(),
            )?;
            let resolution = output
                .modes
                .iter()
                .find(|mode| mode.id == output.current_mode)
                .map_or(output.geometry.size(), |mode| mode.resolution);
            document.text(
                &format!("display-output-size-{slot}"),
                format!(
                    "{} x {}  |  {}%",
                    resolution.width, resolution.height, output.shell_scale_percent
                ),
            )?;
            let selected = self.client.ui().selected_output.as_ref() == Some(&output.id);
            document.visible(&format!("display-output-selected-{slot}"), selected)?;
            document.visible(&format!("display-output-unselected-{slot}"), !selected)?;
        }
        project_pager(
            document,
            &OUTPUT_PAGER,
            self.pages.output_page,
            page_count(self.output_count(), OUTPUT_PAGE_SLOTS),
            page_label(self.pages.output_page, OUTPUT_PAGE_SLOTS, self.output_count()),
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
            document.text(
                "display-selected-scale",
                format!("{}%", output.shell_scale_percent),
            )?;
        } else {
            for id in [
                "display-selected-title",
                "display-selected-resolution",
                "display-selected-scale",
            ] {
                document.text(id, "\u{200b}".to_owned())?;
            }
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
            ("windowOpenAnimationEnabled", "anim-window-on", "anim-window-off"),
            ("minimizeAnimationEnabled", "anim-minimize-on", "anim-minimize-off"),
            ("maximizeAnimationEnabled", "anim-maximize-on", "anim-maximize-off"),
        ] {
            let enabled = self
                .client
                .settings()
                .and_then(|snapshot| snapshot.values.get(key))
                .and_then(|value| match value {
                    SettingValue::Boolean(value) => Some(*value),
                    _ => None,
                });
            document.visible(on_id, enabled == Some(true))?;
            document.visible(off_id, enabled == Some(false))?;
        }
        if let Some(SettingValue::Text(family)) = self
            .client
            .settings()
            .and_then(|snapshot| snapshot.values.get("fontFamily"))
        {
            document.font_family(family.clone())?;
        }
        self.sync_value_text(document)?;
        self.sync_theme_pickers(document)?;
        if let Some(status) = &self.theme_status {
            optional_text(document, "theme-install-status", status.clone());
        }
        self.sync_appearance(document)?;
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

    fn sync_appearance(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        for (target, key) in COLOR_TARGETS {
            if let Ok(color) = self.text_setting(key) {
                document.text(&format!("color-value-{target}"), color.to_ascii_uppercase())?;
            }
        }
        for (key, id) in [
            ("startMenuOpacity", "start-menu-opacity-value"),
            ("accentOpacity", "accent-opacity-value"),
        ] {
            if let Ok(value) = self.percent_setting(key) {
                document.text(id, format!("{value}%"))?;
            }
        }
        if let Ok(speed) = self.integer_setting("animationSpeed") {
            document.text("animation-speed-value", format!("{:.2}x", speed as f64 / 100.0))?;
            let ratio = (speed - ANIMATION_SPEED_MIN) as f64
                / (ANIMATION_SPEED_MAX - ANIMATION_SPEED_MIN) as f64;
            let fill = ratio * ANIMATION_SPEED_TRACK_TRAVEL;
            let fill_width = (fill + ANIMATION_SPEED_THUMB).min(ANIMATION_SPEED_TRACK_WIDTH);
            document.size("animation-speed-fill", fill_width as f32, 12.0)?;
            document.position("animation-speed-thumb", fill as f32, 0.0)?;
        }
        for (key, id) in [
            ("decoration", "decoration-value"),
            ("titlebarMinimizeStyle", "titlebar-minimize-style-value"),
            ("titlebarMaximizeStyle", "titlebar-maximize-style-value"),
            ("titlebarCloseStyle", "titlebar-close-style-value"),
            ("startAnimation", "start-animation-value"),
        ] {
            if let Ok(value) = self.text_setting(key) {
                let label = if key == "startAnimation" { start_animation_label(&value) } else { value };
                document.text(id, label)?;
            }
        }
        Ok(())
    }

    fn sync_error(&self, document: &mut impl UiDocumentAccess) {
        optional_text(
            document,
            ERROR_NODE,
            self.error.clone().unwrap_or_default(),
        );
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
            page_label(self.pages.output_page, OUTPUT_PAGE_SLOTS, self.output_count()),
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
            let Some(flamewm_api::shortcuts::KeyBinding(binding)) = snapshot.bindings.get(row.action)
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
        } else if pages > 1 {
            format!("{total} modes across {pages} pages")
        } else {
            "\u{200b}".to_owned()
        };
        document.text("display-mode-overflow", note)?;
        let pending = self.client.ui().pending_display_transaction.is_some();
        document.visible("display-confirm", pending)?;
        Ok(())
    }

    /// Writes the authoritative value text into every value pill and select
    /// label. Missing snapshot values leave the authored placeholder untouched.
    fn sync_value_text(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let values = self.client.settings().map(|snapshot| &snapshot.values);
        let percent = |key: &str| {
            values.and_then(|values| match values.get(key) {
                Some(SettingValue::Percent(value)) => Some(*value),
                _ => None,
            })
        };
        let integer = |key: &str| {
            values.and_then(|values| match values.get(key) {
                Some(SettingValue::Integer(value)) => Some(*value),
                _ => None,
            })
        };
        let text = |key: &str| {
            values.and_then(|values| match values.get(key) {
                Some(SettingValue::Text(value)) => Some(value.clone()),
                _ => None,
            })
        };
        if let Some(value) = percent("DesktopSelectionFillOpacity") {
            document.text("selection-opacity-value", format!("{value}%"))?;
        }
        if let Some(value) = percent("WindowSnapPreviewFillOpacity") {
            document.text("snap-opacity-value", format!("{value}%"))?;
        }
        for kind in [theme_picker::PickerKind::Icon, theme_picker::PickerKind::Cursor] {
            if let Some(value) = text(kind.setting_key()) {
                document.text(kind.select_box(), self.themes.label(kind, &value))?;
            }
        }
        if let Some(value) = percent("taskbarOpacity") {
            document.text("taskbar-opacity-value", format!("{value}%"))?;
        }
        if let Some(value) = integer("taskbarHeight") {
            document.text(
                "taskbar-size-value",
                format!("{value} px (all outputs)"),
            )?;
        }
        if let Some(value) = text("taskbarColor") {
            document.text("taskbar-color-value", taskbar_color_label(&value))?;
        }
        if let Some(value) = text("fontFamily") {
            document.text("font-family-value", value)?;
        }
        if let Some(value) = integer("fontSizeOffset") {
            document.text("font-size-value", font_size_label(value))?;
        }
        let selected_edge = self.selected_display().and_then(|output| {
            self.client
                .panels()
                .and_then(|snapshot| snapshot.panels.iter().find(|panel| panel.output == output.id))
        }).map(|panel| panel.edge);
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
const COLOR_TARGETS: [(&str, &str); 4] = [
    ("taskbar", "taskbarColor"),
    ("startmenu", "startMenuColor"),
    ("accent", "accent"),
    ("titlebar", "titlebarColor"),
];

/// Fixed swatch values offered beside each color row; `swatch.<index>` selects one.
const COLOR_PALETTE: [&str; 8] = [
    "#000000", "#FFFFFF", "#1E1E1E", "#2D2D2D", "#3DAEE9", "#9B59B6", "#1ABC9C", "#EF4048",
];

/// Display label for a stored text value, when it differs from the value.
const START_ANIMATION_LABELS: [(&str, &str); 1] = [("slide-up", "Slide Up")];

fn start_animation_label(value: &str) -> String {
    START_ANIMATION_LABELS
        .iter()
        .find(|(id, _)| *id == value)
        .map_or_else(|| value.to_owned(), |(_, label)| (*label).to_owned())
}

const TITLEBAR_STYLES: [&str; 4] = ["default", "flat", "circle", "square"];

/// Parses `<target>.<r|g|b>.step.<+-1|+-16|+-64>` or `<target>.swatch.<0..8>`,
/// the rest after `settings.color.`.
fn parse_color_step(rest: &str) -> Result<SettingsAction, String> {
    let (target, rest) = rest
        .split_once('.')
        .ok_or_else(|| format!("invalid color action '{rest}'"))?;
    if let Some(index) = rest.strip_prefix("swatch.") {
        let key = COLOR_TARGETS
            .iter()
            .find(|(name, _)| *name == target)
            .map(|(_, key)| *key)
            .ok_or_else(|| format!("unknown color target '{target}'"))?;
        let value = index
            .parse::<usize>()
            .ok()
            .and_then(|index| COLOR_PALETTE.get(index))
            .ok_or_else(|| format!("unknown color swatch '{index}'"))?;
        return Ok(SettingsAction::ColorSwatch { key, value });
    }
    let (channel, delta) = rest
        .split_once(".step.")
        .ok_or_else(|| format!("invalid color step '{rest}'"))?;
    let key = COLOR_TARGETS
        .iter()
        .find(|(name, _)| *name == target)
        .map(|(_, key)| *key)
        .ok_or_else(|| format!("unknown color target '{target}'"))?;
    let channel = match channel {
        "r" => 0,
        "g" => 1,
        "b" => 2,
        _ => return Err(format!("unknown color channel '{channel}'")),
    };
    let delta = match delta {
        "1" => 1,
        "-1" => -1,
        "16" => 16,
        "-16" => -16,
        "64" => 64,
        "-64" => -64,
        _ => return Err(format!("unknown color step '{delta}'")),
    };
    Ok(SettingsAction::ColorStep { key, channel, delta })
}

/// Next value in `values`, wrapping at both ends. An unknown current value
/// starts from the first entry.
fn cycle_value(values: &[i64], current: i64, direction: i8) -> i64 {
    if values.is_empty() {
        return current;
    }
    let index = values
        .iter()
        .position(|value| *value == current)
        .unwrap_or(0);
    let len = values.len();
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

fn nav_top(page: SettingsPage) -> f32 {
    let index = match page {
        SettingsPage::Appearance => 0,
        SettingsPage::Desktop => 1,
        SettingsPage::Taskbar => 2,
        SettingsPage::Displays => 3,
        SettingsPage::Fonts => 4,
        SettingsPage::Hotkeys => 5,
        SettingsPage::About => 6,
    };
    72.0 + index as f32 * 40.0
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

/// Parses one `data-action` value. Unknown IDs are errors, never silent no-ops.
fn parse_action(action: &str) -> Result<SettingsAction, String> {
    if action.starts_with("keyboard.") {
        return Ok(SettingsAction::NativeKeyEvent);
    }
    if let Some(page) = action.strip_prefix("settings.page.").and_then(page) {
        return Ok(SettingsAction::Page(page));
    }
    if let Some(name) = action.strip_prefix("settings.accent.") {
        return if name == "reset" {
            Ok(SettingsAction::AccentReset)
        } else {
            accent_for_swatch(name)
                .map(SettingsAction::AccentSwatch)
                .ok_or_else(|| format!("unknown accent '{name}'"))
        };
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
        return match key {
            "stickyNotesEnabled" => Ok(SettingsAction::Toggle("stickyNotesEnabled")),
            "fontBold" => Ok(SettingsAction::Toggle("fontBold")),
            "animationsEnabled" => Ok(SettingsAction::Toggle("animationsEnabled")),
            "startAnimationEnabled" => Ok(SettingsAction::Toggle("startAnimationEnabled")),
            "windowOpenAnimationEnabled" => Ok(SettingsAction::Toggle("windowOpenAnimationEnabled")),
            "minimizeAnimationEnabled" => Ok(SettingsAction::Toggle("minimizeAnimationEnabled")),
            "maximizeAnimationEnabled" => Ok(SettingsAction::Toggle("maximizeAnimationEnabled")),
            _ => Err(format!("unknown toggle '{key}'")),
        };
    }
    if let Some(rest) = action.strip_prefix("settings.color.") {
        return parse_color_step(rest);
    }
    if let Some(direction) = action.strip_prefix("settings.start-menu-opacity.").and_then(step) {
        return Ok(SettingsAction::PercentStep("startMenuOpacity", direction));
    }
    if let Some(direction) = action.strip_prefix("settings.accent-opacity.").and_then(step) {
        return Ok(SettingsAction::PercentStep("accentOpacity", direction));
    }
    if action == ANIMATION_SPEED_SLIDER {
        return Ok(SettingsAction::AnimationSpeedSlider);
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
        .strip_prefix("settings.taskbar-opacity.")
        .and_then(step)
    {
        return Ok(SettingsAction::TaskbarOpacity(direction));
    }
    if let Some(direction) = action.strip_prefix("settings.font-size.").and_then(step) {
        return Ok(SettingsAction::FontSize(direction));
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
    match action {
        "settings.wallpaper.choose" => Ok(SettingsAction::WallpaperChoose),
        "settings.wallpaper.reset" => Ok(SettingsAction::WallpaperReset),
        "settings.taskbar.position" => Ok(SettingsAction::TaskbarEdgeCycle),
        "settings.taskbar.color" => Ok(SettingsAction::TaskbarColorCycle),
        "settings.taskbar.size" => Ok(SettingsAction::TaskbarSizeCycle),
        "settings.font.family" => Ok(SettingsAction::FontFamilyCycle),
        "settings.decoration.cycle" => Ok(SettingsAction::TextCycle {
            key: "decoration",
            values: &["rounded", "ios"],
        }),
        "settings.titlebar.minimize.style" => Ok(SettingsAction::TextCycle {
            key: "titlebarMinimizeStyle",
            values: &TITLEBAR_STYLES,
        }),
        "settings.titlebar.maximize.style" => Ok(SettingsAction::TextCycle {
            key: "titlebarMaximizeStyle",
            values: &TITLEBAR_STYLES,
        }),
        "settings.titlebar.close.style" => Ok(SettingsAction::TextCycle {
            key: "titlebarCloseStyle",
            values: &TITLEBAR_STYLES,
        }),
        "settings.start-animation.cycle" => Ok(SettingsAction::TextCycle {
            key: "startAnimation",
            values: &["slide-up"],
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
pub fn taskbar_color_label(value: &str) -> String {
    if value.eq_ignore_ascii_case("#191B1D") {
        "Pitch black".to_owned()
    } else {
        value.to_owned()
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

#[cfg(test)]
mod value_text_tests {
    use super::*;

    #[test]
    fn theme_picker_actions_parse_to_typed_targets() {
        use theme_picker::PickerKind;
        assert!(matches!(
            parse_action("settings.theme.icon.open"),
            Ok(SettingsAction::ThemePickerOpen(PickerKind::Icon))
        ));
        assert!(matches!(
            parse_action("settings.theme.cursor.next"),
            Ok(SettingsAction::ThemePickerPage(PickerKind::Cursor, PageStep::Next))
        ));
        assert!(matches!(
            parse_action("settings.theme.icon.select.5"),
            Ok(SettingsAction::ThemePickerSelect(PickerKind::Icon, 5))
        ));
        assert!(matches!(parse_action("settings.theme.close"), Ok(SettingsAction::ThemePickerClose)));
        assert!(matches!(
            parse_action("settings.theme.install"),
            Ok(SettingsAction::ThemeInstallChoose { folder: false })
        ));
        assert!(matches!(
            parse_action("settings.theme.install-folder"),
            Ok(SettingsAction::ThemeInstallChoose { folder: true })
        ));
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
        use std::cell::RefCell;
        use std::collections::VecDeque;
        use std::rc::Rc;

        use flamewm_api::settings::{SettingValue, SettingsChange, SettingsSnapshot};
        use flamewm_control_core::{ControlError, ControlRequest, ControlResponse};
        use flamewm_settings_core::ControlTransport;
        use theme_picker::{PickerKind, ThemeItem, ThemeRegistry};

        struct Recorder {
            requests: Rc<RefCell<Vec<ControlRequest>>>,
            responses: VecDeque<ControlResponse>,
        }
        impl ControlTransport for Recorder {
            fn call(&mut self, request: ControlRequest) -> Result<ControlResponse, ControlError> {
                self.requests.borrow_mut().push(request);
                Ok(self.responses.pop_front().unwrap_or(ControlResponse::Settings(SettingsSnapshot::new(1))))
            }
        }

        let item = |n: usize| ThemeItem {
            id: format!("theme-{n}"),
            name: format!("Theme {n}"),
            preview: None,
        };
        let requests = Rc::new(RefCell::new(Vec::new()));
        let transport = Recorder {
            requests: requests.clone(),
            responses: VecDeque::new(),
        };
        let mut app = SettingsApplication::new(transport);
        app.themes = ThemeRegistry {
            icon: (0..8).map(item).collect(),
            cursor: Vec::new(),
        };

        // Selecting without an open picker is rejected before any request.
        assert!(app.execute(SettingsAction::ThemePickerSelect(PickerKind::Icon, 0)).is_err());
        assert!(requests.borrow().is_empty());

        app.execute(SettingsAction::ThemePickerOpen(PickerKind::Icon)).expect("open");
        app.execute(SettingsAction::ThemePickerPage(PickerKind::Icon, PageStep::Next))
            .expect("page");
        app.execute(SettingsAction::ThemePickerSelect(PickerKind::Icon, 1)).expect("select");

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
        assert_eq!(taskbar_color_label("#191b1d"), "Pitch black");
        assert_eq!(taskbar_color_label("#3D4145"), "#3D4145");
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
        assert!(matches!(
            parse_action("settings.color.taskbar.swatch.0"),
            Ok(SettingsAction::ColorSwatch { key: "taskbarColor", value: "#000000" })
        ));
        assert!(matches!(
            parse_action("settings.color.accent.swatch.7"),
            Ok(SettingsAction::ColorSwatch { key: "accent", value: "#EF4048" })
        ));
        assert!(matches!(
            parse_action("settings.color.titlebar.g.step.64"),
            Ok(SettingsAction::ColorStep { key: "titlebarColor", channel: 1, delta: 64 })
        ));
        assert!(matches!(
            parse_action("settings.color.startmenu.b.step.-64"),
            Ok(SettingsAction::ColorStep { key: "startMenuColor", channel: 2, delta: -64 })
        ));
        for action in [
            "settings.color.taskbar.swatch.8",
            "settings.color.taskbar.swatch.x",
            "settings.color.desktop.swatch.0",
            "settings.color.accent.r.step.32",
        ] {
            assert!(parse_action(action).is_err(), "{action} must be rejected");
        }
        assert_eq!(start_animation_label("slide-up"), "Slide Up");
        assert_eq!(start_animation_label("other"), "other");
    }

    #[test]
    fn appearance_actions_parse_to_their_exact_typed_targets() {
        assert!(matches!(
            parse_action("settings.color.startmenu.r.step.-16"),
            Ok(SettingsAction::ColorStep { key: "startMenuColor", channel: 0, delta: -16 })
        ));
        assert!(matches!(
            parse_action("settings.color.titlebar.b.step.1"),
            Ok(SettingsAction::ColorStep { key: "titlebarColor", channel: 2, delta: 1 })
        ));
        assert!(matches!(
            parse_action("settings.start-menu-opacity.up"),
            Ok(SettingsAction::PercentStep("startMenuOpacity", 1))
        ));
        assert!(matches!(
            parse_action("settings.accent-opacity.down"),
            Ok(SettingsAction::PercentStep("accentOpacity", -1))
        ));
        assert!(matches!(
            parse_action("settings.toggle.maximizeAnimationEnabled"),
            Ok(SettingsAction::Toggle("maximizeAnimationEnabled"))
        ));
        assert!(matches!(
            parse_action("settings.titlebar.close.style"),
            Ok(SettingsAction::TextCycle { key: "titlebarCloseStyle", .. })
        ));
        assert!(matches!(
            parse_action("settings.decoration.cycle"),
            Ok(SettingsAction::TextCycle { key: "decoration", .. })
        ));
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
    use std::collections::VecDeque;
    use std::rc::Rc;

    use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot};
    use flamewm_api::{Rect, Size};
    use flamewm_control_core::{ControlRequest, ControlResponse};
    use flamewm_settings_core::{ControlTransport, DISPLAY_MODE_SLOTS};

    use super::*;

    #[derive(Default)]
    struct FixtureTransport {
        responses: VecDeque<ControlResponse>,
        requests: Rc<RefCell<Vec<ControlRequest>>>,
    }

    impl ControlTransport for FixtureTransport {
        fn call(
            &mut self,
            request: ControlRequest,
        ) -> Result<ControlResponse, ControlError> {
            self.requests.borrow_mut().push(request);
            Ok(self.responses.pop_front().expect("fixture response"))
        }
    }

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
        let mut transport = FixtureTransport::default();
        let requests = Rc::new(RefCell::new(Vec::new()));
        transport.requests = requests.clone();
        transport
            .responses
            .push_back(ControlResponse::Displays(five_output_twelve_mode_snapshot()));
        transport
            .responses
            .push_back(ControlResponse::Transaction(flamewm_api::TransactionId(7)));
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

        // Eight mode rows per page: the second page's slot three is mode index 11.
        assert_eq!(DISPLAY_MODE_SLOTS, 8);
        assert_eq!(app.client.display_mode_count(), 12);
        assert_eq!(page_count(app.client.display_mode_count(), DISPLAY_MODE_SLOTS), 2);
        app.execute(SettingsAction::ModePage(PageStep::Next))
            .expect("next mode page");
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
        assert!(display_mode_label(
            &app.client.display_mode_page(app.pages.mode_page)[2],
            selected_output.current_mode == fifth_output_current_mode,
        )
        .contains("current"));
        assert_eq!(
            app.client.display_mode_page(app.pages.mode_page)[3].id,
            fifth_output_mode_11
        );
        assert!(!display_mode_label(
            &app.client.display_mode_page(app.pages.mode_page)[3],
            selected_output.current_mode == fifth_output_mode_11,
        )
        .contains("current"));

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
        assert_eq!(app.client.ui().pending_display_transaction, Some(flamewm_api::TransactionId(7)));

        // End-of-list pager state is derived from the data and cannot advance past its last page.
        assert_eq!(step_page(app.pages.mode_page, PageStep::Next, 2), app.pages.mode_page);
        assert_eq!(page_label(app.pages.mode_page, DISPLAY_MODE_SLOTS, 12), "9-12 of 12");

        // A cursor stamped against the prior generation rejects the same slot before any request.
        app.pages.generation = Some(41);
        let request_count = requests.borrow().len();
        let pending_before = app.client.ui().pending_display_transaction;
        assert!(app.execute(SettingsAction::DisplaySlot(3)).unwrap_err().contains("stale"));
        assert_eq!(requests.borrow().len(), request_count);
        assert_eq!(app.client.ui().pending_display_transaction, pending_before);
    }
}

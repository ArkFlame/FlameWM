use std::fmt::Write as _;

use chrono::{DateTime, Datelike, Local};
use flamewm_api::applications::DesktopApplication;
use flamewm_api::session::SessionCapabilities;
use flamewm_api::settings::SettingsSnapshot;
use flamewm_api::system::ServiceAvailability;
use flamewm_integrations_linux::icons::{IconResolver, IconSize, Rgb8Raster};
use flamewm_platform::settings::ClockFormat;
use flamewm_ui_x11::{RuntimeImage, UiColor, UiDocumentAccess};

use crate::start::{StartCategory, PRESENTATION_CATEGORIES};
use crate::taskbar::workspaces::{
    project as project_workspaces, slot_position, visible_page, WorkspaceView, PAGE_SIZE,
};
use crate::ShellSnapshot;
use flamewm_shell_core::start_menu::{bucket_for_application, StartBucket};
use flamewm_shell_core::{task_visual_states_on_output, StartModel};

mod audio;
mod calendar;
mod network;
mod weather;

pub use audio::{project_audio, project_audio_full};
pub use calendar::project_calendar;
pub use network::{project_network, project_network_full, project_network_secret};
pub use weather::{project_weather_popup, project_weather_tray};

const TASK_SLOT_COUNT: usize = 8;
const START_APP_SLOT_COUNT: usize = 8;
/// Bar height in CSS px used when the panel snapshot carries no logical size.
/// Matches the `.taskbar` height in ui/shell/flamewm.css.
const TASKBAR_DEFAULT_HEIGHT: f32 = 44.0;

/// Maps the panel's logical (settings) height to the CSS height of the bar.
/// A zero logical size falls back to the stylesheet default so 100% output
/// stays identical to the 44px layout.
#[must_use]
/// Accent used for the focused task underline (ARGB), fed by the settings projection so the
/// underline follows `accent`/`accentOpacity` instead of a fixed colour.
static TASK_ACCENT_ARGB: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0xFF7F_878E);

/// Records the accent colour the focused task underline uses from the next task projection on.
pub fn set_task_accent(color: UiColor) {
    let packed = (u32::from(color.a) << 24)
        | (u32::from(color.r) << 16)
        | (u32::from(color.g) << 8)
        | u32::from(color.b);
    TASK_ACCENT_ARGB.store(packed, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn task_accent() -> UiColor {
    let packed = TASK_ACCENT_ARGB.load(std::sync::atomic::Ordering::Relaxed);
    UiColor {
        a: (packed >> 24) as u8,
        r: (packed >> 16) as u8,
        g: (packed >> 8) as u8,
        b: packed as u8,
    }
}

pub fn taskbar_css_height(logical_size: u16) -> f32 {
    if logical_size == 0 {
        TASKBAR_DEFAULT_HEIGHT
    } else {
        f32::from(logical_size)
    }
}

/// Panel width in CSS px. `geometry.width` is device pixels; the document
/// lays out in CSS px at `scale_percent`, so dividing by the scale keeps the
/// bar exactly one window wide. Unsupported scales fall back to 100%, as the
/// panel surface does when it applies its UI scale.
pub fn taskbar_css_width(device_width: i32, scale_percent: u16) -> f32 {
    let percent = if matches!(scale_percent, 100 | 125 | 150 | 175 | 200) {
        scale_percent
    } else {
        100
    };
    device_width.max(0) as f32 * 100.0 / f32::from(percent)
}

/// Sizes the `taskbar` node to the panel's logical width and height so the
/// painted bar fills the panel window instead of leaving an undrawn band.
/// A device-pixel width here pushed the pager, tray and clock past the
/// layout viewport at 125% scale.
pub(crate) fn project_taskbar_height(
    document: &mut impl UiDocumentAccess,
    view: PanelView<'_>,
) -> Result<(), String> {
    let Some(panel) = view.panel else {
        return Ok(());
    };
    let scale_percent = view
        .output
        .map(|output| output.shell_scale_percent)
        .unwrap_or(100);
    document.size(
        "taskbar",
        taskbar_css_width(panel.geometry.width, scale_percent),
        taskbar_css_height(panel.logical_size),
    )
}

/// Borrowed single-panel view. `output` and `panel` are the record's own
/// entries (or the first panel for full-snapshot callers); everything else
/// is read from the shared snapshot without cloning it.
#[derive(Clone, Copy)]
pub struct PanelView<'a> {
    pub snapshot: &'a ShellSnapshot,
    pub output: Option<&'a flamewm_api::display::OutputSnapshot>,
    pub panel: Option<&'a flamewm_api::panels::PanelSnapshot>,
}

impl<'a> PanelView<'a> {
    /// First panel of the snapshot, with its connected output.
    #[must_use]
    pub fn first(snapshot: &'a ShellSnapshot) -> Self {
        let panel = snapshot.panels.panels.first();
        let output = panel.and_then(|panel| {
            snapshot
                .displays
                .outputs
                .iter()
                .find(|output| output.id == panel.output && output.connected)
        });
        Self {
            snapshot,
            output,
            panel,
        }
    }
}

/// Calls `project` once per panel record, in order, with that record's
/// borrowed view of the live snapshot. A record whose connected output or
/// panel is absent from the snapshot is skipped; sync removes its surface.
/// The first error stops the walk.
pub fn for_each_panel_record<K, E>(
    snapshot: &ShellSnapshot,
    records: impl IntoIterator<Item = (K, flamewm_api::OutputId)>,
    mut project: impl FnMut(K, PanelView<'_>) -> Result<(), E>,
) -> Result<(), E> {
    for (key, output_id) in records {
        let Some(output) = snapshot
            .displays
            .outputs
            .iter()
            .find(|output| output.id == output_id && output.connected)
        else {
            continue;
        };
        let Some(panel) = snapshot
            .panels
            .panels
            .iter()
            .find(|panel| panel.output == output_id)
        else {
            continue;
        };
        project(
            key,
            PanelView {
                snapshot,
                output: Some(output),
                panel: Some(panel),
            },
        )?;
    }
    Ok(())
}

pub fn project(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    icon_resolver: &mut IconResolver,
) -> Result<(), String> {
    project_panel(document, PanelView::first(snapshot), icon_resolver)
}

pub fn project_panel(
    document: &mut impl UiDocumentAccess,
    view: PanelView<'_>,
    icon_resolver: &mut IconResolver,
) -> Result<(), String> {
    let snapshot = view.snapshot;
    project_taskbar_height(document, view)?;
    let states = task_visual_states_on_output(
        &snapshot.panels,
        &snapshot.windows,
        &snapshot.applications,
        view.panel.map(|panel| &panel.output),
    );
    let status = crate::taskbar::status::project(&snapshot.system).views;
    let workspaces = project_workspaces(snapshot.workspaces.as_ref());
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
        let state = states.get(index);
        if let Some(state) = state {
            let label = snapshot
                .applications
                .iter()
                .find(|application| application.id == state.app_id)
                .map(|application| application.name.as_str())
                .filter(|name| !name.is_empty())
                .unwrap_or(state.app_id.as_str());
            let icon = resolve_icon(icon_resolver, &state.icon_name, IconSize::new(25, 25))?;
            document.image_rgb8(&icon_id, icon)?;
            document.text(&label_id, label.to_owned())?;
            document.visible(&line_id, state.running)?;
            if state.focused {
                document.background_clear(&slot_id)?;
                document.background(&line_id, task_accent())?;
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

    for index in 0..PAGE_SIZE {
        let button_id = format!("workspace-{}", index + 1);
        document.visible(&button_id, false)?;
    }
    let page: Vec<&WorkspaceView> = visible_page(&workspaces);
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

    project_clock(document)?;
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
    project_battery_tray(document, &snapshot.system.power)?;
    // Stop-gap: no main-process weather feed yet; the entry stays hidden.
    project_weather_tray(document, &flamewm_shell_core::weather::WeatherState::Disabled, flamewm_shell_core::weather::TemperatureUnit::Celsius)?;
    Ok(())
}

/// Tray battery: visibility and glyph come from the pure `battery` rules. Exactly
/// one glyph node is shown; the charging variant only while Charging.
pub fn project_battery_tray(
    document: &mut impl UiDocumentAccess,
    power: &flamewm_api::system::PowerSnapshot,
) -> Result<(), String> {
    use crate::taskbar::status::battery::{glyph_for_percent, tray_visible, BATTERY_GLYPHS};
    use flamewm_api::system::PowerState;
    let charging = power.state == PowerState::Charging;
    let glyph = glyph_for_percent(power.percent);
    document.visible("tray-battery", tray_visible(power))?;
    for candidate in BATTERY_GLYPHS {
        let shown = candidate == glyph;
        document.visible(
            &format!("tray-battery-{}", candidate.id_suffix()),
            shown && !charging,
        )?;
        document.visible(
            &format!("tray-battery-{}-charging", candidate.id_suffix()),
            shown && charging,
        )?;
    }
    Ok(())
}

/// Battery menu body: percent, state and CPU text, progress fill, and the
/// charging glyph variant. Uses the full 0..=100 range (menu is not hidden).
pub fn project_battery_menu(
    document: &mut impl UiDocumentAccess,
    power: &flamewm_api::system::PowerSnapshot,
    track_width: f32,
) -> Result<(), String> {
    use crate::taskbar::status::battery::{
        cpu_text, glyph_for_percent, progress_width, state_text, BATTERY_GLYPHS,
    };
    use flamewm_api::system::PowerState;
    let charging = power.state == PowerState::Charging;
    let glyph = glyph_for_percent(power.percent);
    document.text("battery-percent", format!("{}%", power.percent.min(100)))?;
    document.text("battery-state", state_text(power.state).to_owned())?;
    document.text("battery-cpu", cpu_text(power.cpu_percent))?;
    document.size(
        "battery-progress-fill",
        progress_width(power.percent, track_width),
        BATTERY_PROGRESS_HEIGHT,
    )?;
    for candidate in BATTERY_GLYPHS {
        let shown = candidate == glyph;
        document.visible(
            &format!("battery-icon-{}", candidate.id_suffix()),
            shown && !charging,
        )?;
        document.visible(
            &format!("battery-icon-{}-charging", candidate.id_suffix()),
            shown && charging,
        )?;
    }
    Ok(())
}

/// Height of the battery menu progress fill (matches ui/shell/battery.html).
pub const BATTERY_PROGRESS_HEIGHT: f32 = 8.0;

/// The single Start result owner: every projection and click path reads this.
/// A non-empty query searches ALL categories (the category rail is hidden);
/// an empty query shows the selected presentation category.
#[must_use]
pub fn start_visible_results<'a>(
    model: &'a StartModel,
    category: StartCategory,
    query: &str,
) -> Vec<&'a flamewm_api::applications::DesktopApplication> {
    if !query.trim().is_empty() {
        return model.query_view(query);
    }
    let base = model.results_view();
    match category {
        StartCategory::All => base,
        StartCategory::Power => Vec::new(),
        _ => base
            .into_iter()
            .filter(|application| application_matches_category(application, category))
            .collect(),
    }
}

pub fn project_start(
    document: &mut impl UiDocumentAccess,
    model: &StartModel,
    category: StartCategory,
    query: &str,
    session: &SessionCapabilities,
) -> Result<(), String> {
    // Search mode hides the whole category rail (incl. Power) and searches all.
    let searching = !query.trim().is_empty();
    document.visible("start-categories", !searching)?;
    // A search ignores the category, so one query serves every rail check and the count.
    let results = start_visible_results(model, category, query);
    for (name, value) in PRESENTATION_CATEGORIES {
        let visible = match value {
            StartCategory::Power => session_available(session),
            _ if value == category => true,
            _ if searching => !results.is_empty(),
            _ => !start_visible_results(model, value, query).is_empty(),
        };
        document.visible(&format!("start-category-{name}"), visible)?;
    }
    let count = results.len();
    document.text(
        "start-search-label",
        if query.trim().is_empty() {
            "Search applications".to_owned()
        } else {
            format!("{} ({})", query, count)
        },
    )?;
    Ok(())
}

/// Start error line. Markup must provide this node (hidden by default).
pub const START_STATUS_NODE: &str = "start-status-label";

/// Project the Start error line: shown with `message`, hidden when `None`.
pub fn project_start_error(
    document: &mut impl UiDocumentAccess,
    message: Option<&str>,
) -> Result<(), String> {
    match message {
        Some(message) => {
            document.text(START_STATUS_NODE, message.to_owned())?;
            document.visible(START_STATUS_NODE, true)
        }
        None => document.visible(START_STATUS_NODE, false),
    }
}

#[must_use]
pub fn start_application_for_slot(
    model: &StartModel,
    category: StartCategory,
    query: &str,
    slot: usize,
) -> Option<flamewm_api::DesktopAppId> {
    start_visible_results(model, category, query)
        .get(slot)
        .map(|application| application.id.clone())
}

/// User-visible name of the application in `slot`, for Start error text.
#[must_use]
pub fn start_application_name_for_slot(
    model: &StartModel,
    category: StartCategory,
    query: &str,
    slot: usize,
) -> Option<String> {
    start_visible_results(model, category, query)
        .get(slot)
        .map(|application| application.name.clone())
}

pub fn project_context_menu(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    settings: &SettingsSnapshot,
    state: Option<&crate::ContextMenuState>,
) -> Result<(), String> {
    // Shared menu surface (radius + fill) from the Surfaces settings, same owner as desktop menus.
    let surface = flamewm_ui_core::context_menu::menu_surface(settings);
    document.background("task-menu", crate::settings_projection::surface_background(settings))?;
    document.corner_radius("task-menu", surface.radius as f32)?;
    for id in [
        "context-task-activate",
        "context-task-close",
        "context-task-unpin",
        "context-workspace-activate",
        "context-workspace-insert-after",
        "context-workspace-remove",
        "context-taskbar-new-desktop",
        "context-taskbar-settings",
    ] {
        document.visible(id, false)?;
    }
    let Some(state) = state else {
        return Ok(());
    };
    match &state.kind {
        crate::ContextMenuKind::Task(id) => {
            for entry in crate::taskbar::context_menu::task_menu_for_entry(
                &snapshot.panels,
                &snapshot.windows,
                id,
            ) {
                if let flamewm_ui_core::MenuEntry::Action { id, .. } = entry {
                    document.visible(&format!("context-task-{id}"), true)?;
                }
            }
        }
        crate::ContextMenuKind::Workspace { index } => {
            if let Some(workspaces) = snapshot.workspaces.as_ref() {
                for (entry, _) in crate::taskbar::context_menu::workspace_menu(workspaces, *index) {
                    if let flamewm_ui_core::MenuEntry::Action { id, .. } = entry {
                        document.visible(&format!("context-workspace-{id}"), true)?;
                    }
                }
            }
        }
        crate::ContextMenuKind::Taskbar => {
            for row in crate::taskbar::context_menu::taskbar_menu() {
                document.visible(&format!("context-{}", row.id), true)?;
            }
        }
    }
    Ok(())
}

/// Char budget for the media popup title. The title box is 276px wide with
/// 10px padding per side (256px usable) at 13px / weight 600; ~7.8px per glyph
/// gives ~32 glyphs. The render compiler has no text-overflow, so the ellipsis
/// is applied here. Counted with `chars()`, so multibyte text never splits.
const MEDIA_TITLE_MAX_CHARS: usize = 32;

/// Returns `title` unchanged when it fits `MEDIA_TITLE_MAX_CHARS`; otherwise
/// keeps the leading chars, trims trailing whitespace and appends `…` so the
/// total stays within the budget.
#[must_use]
fn fit_media_title(title: &str) -> String {
    if title.chars().count() <= MEDIA_TITLE_MAX_CHARS {
        return title.to_owned();
    }
    let kept: String = title.chars().take(MEDIA_TITLE_MAX_CHARS - 1).collect();
    format!("{}…", kept.trim_end())
}

pub fn project_media(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    let view = crate::taskbar::status::media::project(&snapshot.system);
    document.text("media-title", fit_media_title(&snapshot.system.media.title))?;
    document.visible(
        "media-play-icon",
        matches!(
            view.play_pause_icon,
            Some(flamewm_ui_core::IconRole::MediaPlay)
        ),
    )?;
    document.visible(
        "media-pause-icon",
        matches!(
            view.play_pause_icon,
            Some(flamewm_ui_core::IconRole::MediaPause)
        ),
    )?;
    let (can_previous, can_play_pause, can_next) =
        flamewm_shell_core::StatusPopovers::from_snapshot(&snapshot.system)
            .media
            .map_or((false, false, false), |media| {
                (media.can_previous, media.can_play_pause, media.can_next)
            });
    for (enabled, disabled, active) in [
        ("media-previous", "media-previous-disabled", can_previous),
        ("media-playpause", "media-playpause-disabled", can_play_pause),
        ("media-next", "media-next-disabled", can_next),
    ] {
        document.visible(enabled, active)?;
        document.visible(disabled, !active)?;
    }
    Ok(())
}




/// Taskbar clock presentation from `taskbarClockFormat`, `taskbarShowSeconds`
/// and `taskbarShowDate`. Defaults match `ProductSettings::default()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockDisplay {
    pub format: ClockFormat,
    pub show_seconds: bool,
    pub show_date: bool,
}

impl Default for ClockDisplay {
    fn default() -> Self {
        Self {
            format: ClockFormat::TwentyFour,
            show_seconds: false,
            show_date: true,
        }
    }
}

/// Time text for the taskbar clock: `HH:MM` or `HH:MM:SS` in 24h, `H:MM AM/PM`
/// or `H:MM:SS AM/PM` in 12h.
#[must_use]
pub fn clock_time_text(display: ClockDisplay, now: DateTime<Local>) -> String {
    let pattern = match (display.format, display.show_seconds) {
        (ClockFormat::TwentyFour, false) => "%H:%M",
        (ClockFormat::TwentyFour, true) => "%H:%M:%S",
        (ClockFormat::Twelve, false) => "%-I:%M %p",
        (ClockFormat::Twelve, true) => "%-I:%M:%S %p",
    };
    now.format(pattern).to_string()
}

/// Panel rebuild paths carry no settings; the settings projection re-applies the
/// configured clock on the next settings pass and on every clock tick.
pub fn project_clock(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    project_clock_at(document, Local::now(), ClockDisplay::default())
}

pub fn project_clock_at(
    document: &mut impl UiDocumentAccess,
    now: DateTime<Local>,
    display: ClockDisplay,
) -> Result<(), String> {
    document.text("clock-time", clock_time_text(display, now))?;
    document.text("clock-date", now.format("%-d/%-m/%y").to_string())?;
    document.visible("clock-date", display.show_date)?;
    Ok(())
}

#[must_use]
pub fn local_date() -> (i32, u8, u8) {
    let now = Local::now();
    (now.year(), now.month() as u8, now.day() as u8)
}

fn month_name(month: u8) -> &'static str {
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ]
    .get(month.saturating_sub(1) as usize)
    .copied()
    .unwrap_or("Unknown")
}

#[must_use]
pub(crate) fn session_available(session: &SessionCapabilities) -> bool {
    session.lock || session.logout || session.suspend || session.reboot || session.shutdown
}

fn application_matches_category(application: &DesktopApplication, category: StartCategory) -> bool {
    let bucket = match category {
        StartCategory::All => return true,
        StartCategory::Power => return false,
        StartCategory::Development => StartBucket::Development,
        StartCategory::Games => StartBucket::Games,
        StartCategory::Graphics => StartBucket::Graphics,
        StartCategory::Internet => StartBucket::Internet,
        StartCategory::Multimedia => StartBucket::Multimedia,
        StartCategory::System => StartBucket::System,
        StartCategory::Utilities => StartBucket::Utilities,
    };
    bucket_for_application(application) == bucket
}

fn resolve_icon(
    resolver: &mut IconResolver,
    icon_name: &str,
    size: IconSize,
) -> Result<RuntimeImage, String> {
    let raster = resolver.prepare_name(icon_name, size).or_else(|error| {
        resolver.prepare_name("", size).map_err(|fallback| {
            format!("icon '{icon_name}' failed: {error}; generic fallback failed: {fallback}")
        })
    })?;
    Ok(runtime_image(raster))
}

fn runtime_image(raster: Rgb8Raster) -> RuntimeImage {
    RuntimeImage {
        source: raster.source,
        width: raster.width,
        height: raster.height,
        pixels: raster.pixels,
    }
}


#[cfg(test)]
mod panel_template_tests {
    use super::*;

    const PANEL_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-panel.rwr"));

    // CONTRACT-REGRESSION: the clock and workspace pager text targets and the
    // tray visibility targets written by the panel projection must exist in the
    // compiled panel template. A refused write aborts the scoped panel pass
    // after the Start button, which is the "only Start renders" failure.
    // TRIGGER: panel.html id/child edits or project_clock / project_workspaces changes.
    // OBSERVABLE: project_clock_at and each pager/tray target succeed on the real artifact.
    // MUTATION: drop the text child of #clock-time; project_clock_at must be refused.
    #[test]
    fn every_projected_panel_clock_and_pager_target_is_writable() {
        let mut document =
            flamewm_ui_x11::decode_document(PANEL_ARTIFACT).expect("panel artifact must decode");
        project_clock_at(&mut document, Local::now(), ClockDisplay::default())
            .unwrap_or_else(|error| panic!("panel clock refused: {error}"));
        for slot in 1..=PAGE_SIZE {
            let id = format!("workspace-{slot}");
            document
                .text(&id, String::new())
                .unwrap_or_else(|error| panic!("panel pager text '{id}' refused: {error}"));
            document
                .visible(&id, true)
                .unwrap_or_else(|error| panic!("panel pager visible '{id}' refused: {error}"));
        }
        for id in ["tray-media", "tray-volume", "tray-network"] {
            document
                .visible(id, true)
                .unwrap_or_else(|error| panic!("panel tray '{id}' refused: {error}"));
        }
    }

    // REGRESSION: the clock, workspace pager and tray must lay out with a
    // non-zero rect inside the panel viewport. The live panel window is
    // 1350x90 device px at 125%, i.e. a 1080x72 CSS viewport. A device-pixel
    // taskbar width (1350) pushed these nodes past the right edge, so they
    // were laid out but never painted.
    // OBSERVABLE: every listed node has width/height > 0 and lies inside 1080x72.
    // MUTATION: pass geometry.width unscaled to taskbar_css_width (or to size)
    // and the rightmost nodes fall outside and this test fails.
    #[test]
    fn panel_clock_pager_tray_lay_out_inside_scaled_window() {
        let mut document =
            flamewm_ui_x11::decode_document(PANEL_ARTIFACT).expect("panel artifact must decode");
        // Live panel: 1350x90 device px window, 125% output scale, logical 72.
        let output = flamewm_api::OutputId::new("HDMI-1");
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot
            .displays
            .outputs
            .push(flamewm_api::display::OutputSnapshot {
                id: output.clone(),
                connector: "HDMI-1".to_owned(),
                edid_identity: String::new(),
                connected: true,
                primary: true,
                geometry: flamewm_api::Rect::new(0, 0, 1350, 641),
                current_mode: flamewm_api::ModeId(1),
                modes: Vec::new(),
                shell_scale_percent: 125,
            });
        snapshot
            .panels
            .panels
            .push(flamewm_api::panels::PanelSnapshot {
                output,
                edge: flamewm_api::PanelEdge::Bottom,
                logical_size: 72,
                visible: true,
                geometry: flamewm_api::Rect::new(0, 551, 1350, 90),
            });
        project_taskbar_height(&mut document, PanelView::first(&snapshot))
            .expect("taskbar size projects from the panel snapshot");
        project_clock_at(&mut document, Local::now(), ClockDisplay::default())
            .expect("clock projects");
        for slot in 1..=PAGE_SIZE {
            let id = format!("workspace-{slot}");
            document
                .text(&id, slot.to_string())
                .expect("pager text projects");
            document.visible(&id, true).expect("pager visible projects");
        }
        for id in [
            "tray-media",
            "tray-play-icon",
            "tray-volume",
            "tray-volume-icon",
            "tray-network",
        ] {
            document.visible(id, true).expect("tray visible projects");
        }
        let mut checked = Vec::new();
        for id in [
            "clock-time",
            "clock-date",
            "clock-button",
            "workspace-1",
            "workspace-2",
            "workspace-3",
            "workspace-4",
            "workspace-pager",
            "tray-media",
            "tray-volume",
            "tray-network",
            "tray-wifi",
        ] {
            let (x, y, width, height) = document
                .layout_box_by_id(id, (1080.0, 72.0))
                .unwrap_or_else(|| panic!("panel node '{id}' missing from artifact"));
            assert!(
                width > 0.0 && height > 0.0,
                "panel node '{id}' is zero-size: ({x}, {y}, {width}, {height})"
            );
            assert!(
                x >= 0.0 && y >= 0.0 && x + width <= 1080.5 && y + height <= 72.5,
                "panel node '{id}' lies outside the 1080x72 viewport: ({x}, {y}, {width}, {height})"
            );
            checked.push(id);
        }
        assert_eq!(checked.len(), 12);
    }

    // REGRESSION: the battery popup shows exactly one glyph node, charging-aware,
    // on the compiled flamewm-battery.rwr artifact for each charge bucket.
    // MUTATION: keep a retired low node or project the wrong bucket in the menu.
    #[test]
    fn battery_menu_shows_exactly_one_glyph_on_real_artifact() {
        use crate::taskbar::status::battery::BATTERY_GLYPHS;
        use flamewm_api::system::{PowerSnapshot, PowerState, ServiceAvailability};
        const BATTERY_ARTIFACT: &[u8] =
            include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-battery.rwr"));
        let cases = [
            (5, PowerState::Discharging, "battery-icon-caution"),
            (15, PowerState::Discharging, "battery-icon-020"),
            (45, PowerState::Discharging, "battery-icon-050"),
            (100, PowerState::Discharging, "battery-icon-100"),
            (45, PowerState::Charging, "battery-icon-050-charging"),
        ];
        let ids: Vec<String> = BATTERY_GLYPHS
            .iter()
            .flat_map(|glyph| {
                [
                    format!("battery-icon-{}", glyph.id_suffix()),
                    format!("battery-icon-{}-charging", glyph.id_suffix()),
                ]
            })
            .collect();
        for (percent, state, expected) in cases {
            let power = PowerSnapshot {
                available: ServiceAvailability::Available,
                present: true,
                percent,
                state,
                cpu_percent: None,
            };
            let mut document =
                flamewm_ui_x11::decode_document(BATTERY_ARTIFACT).expect("battery artifact must decode");
            project_battery_menu(&mut document, &power, 268.0).expect("battery menu projects");
            let viewport = (300.0, 132.0);
            let lit: Vec<&str> = ids
                .iter()
                .map(String::as_str)
                .filter(|id| document.layout_box_by_id(id, viewport).map_or(0.0, |rect| rect.2) > 0.0)
                .collect();
            assert_eq!(lit, vec![expected], "{percent}% {state:?}");
        }
    }

    // REGRESSION: the tray battery button and its glyph must follow the power
    // bucket on the compiled panel through every panel projection path: the
    // sync `project` pass, the full async refresh, and a power-only SystemChanged
    // scope. Visible = laid out with a non-zero box.
    // OBSERVABLE: hidden only when the service is unavailable or no battery is
    // present; charging and full batteries show one glyph (Caution 0-10,
    // otherwise the level rounded up to 10%).
    // MUTATION: drop the battery projection from one path (default markup stays
    // hidden there, so that path's case fails) or skip the glyph bucket.
    #[test]
    fn tray_battery_button_follows_power_bucket_on_real_artifact() {
        use crate::taskbar::status::battery::BATTERY_GLYPHS;
        use flamewm_api::system::{PowerSnapshot, PowerState, ServiceAvailability};
        let glyphs: Vec<String> = BATTERY_GLYPHS
            .iter()
            .flat_map(|glyph| {
                [
                    format!("tray-battery-{}", glyph.id_suffix()),
                    format!("tray-battery-{}-charging", glyph.id_suffix()),
                ]
            })
            .collect();
        let power = |available, present, percent, state| PowerSnapshot {
            available,
            present,
            percent,
            state,
            cpu_percent: None,
        };
        let avail = ServiceAvailability::Available;
        let cases: [(PowerSnapshot, Option<&str>); 14] = [
            (power(avail, true, 87, PowerState::Unknown), None),
            (power(avail, true, 45, PowerState::Discharging), Some("tray-battery-050")),
            (power(avail, true, 45, PowerState::Unknown), None),
            (power(avail, true, 15, PowerState::Discharging), Some("tray-battery-020")),
            (power(avail, true, 5, PowerState::Discharging), Some("tray-battery-caution")),
            (power(avail, true, 30, PowerState::Charging), Some("tray-battery-030-charging")),
            (power(avail, true, 79, PowerState::Charging), Some("tray-battery-080-charging")),
            (power(avail, true, 80, PowerState::Charging), Some("tray-battery-080-charging")),
            (power(avail, true, 100, PowerState::Charging), Some("tray-battery-100-charging")),
            (power(ServiceAvailability::Unavailable, true, 40, PowerState::Discharging), None),
            (power(avail, true, 79, PowerState::Discharging), Some("tray-battery-080")),
            (power(avail, true, 80, PowerState::Discharging), Some("tray-battery-080")),
            (power(avail, true, 100, PowerState::Full), Some("tray-battery-100")),
            (power(avail, false, 45, PowerState::Discharging), None),
        ];
        for (power, expected) in cases {
            let mut snapshot = crate::ShellSnapshot::default();
            snapshot.system.power = power;
            let paths: [(&str, fn(&mut flamewm_ui_x11::UiDocument, &crate::ShellSnapshot)); 3] = [
                ("sync project", |document, snapshot| {
                    let mut resolver = flamewm_integrations_linux::icons::IconResolver::new(
                        std::path::PathBuf::new(),
                        Vec::new(),
                        Vec::new(),
                        0,
                    );
                    project(document, snapshot, &mut resolver).expect("sync panel projects");
                }),
                ("async full refresh", |document, snapshot| {
                    crate::async_projection::project_panel_async(
                        document,
                        PanelView::first(snapshot),
                        None,
                        &mut Default::default(),
                    )
                        .expect("async panel projects");
                }),
                ("async system scope", |document, snapshot| {
                    crate::async_projection::project_panel_for_changed(
                        document,
                        PanelView::first(snapshot),
                        None,
                        &mut Default::default(),
                        crate::runtime::ChangedDomains {
                            system: true,
                            ..crate::runtime::ChangedDomains::default()
                        },
                    )
                    .expect("system-scope panel projects");
                }),
            ];
            for (path, project_path) in paths {
                let mut document =
                    flamewm_ui_x11::decode_document(PANEL_ARTIFACT).expect("panel artifact must decode");
                project_path(&mut document, &snapshot);
                let viewport = (1080.0, 72.0);
                let width = |id: &str| {
                    document
                        .layout_box_by_id(id, viewport)
                        .map_or(0.0, |rect| rect.2)
                };
                let button = width("tray-battery") > 0.0;
                assert_eq!(button, expected.is_some(), "{path}: button for {:?}", snapshot.system.power);
                let lit: Vec<&str> = glyphs
                    .iter()
                    .map(String::as_str)
                    .filter(|id| width(id) > 0.0)
                    .collect();
                match expected {
                    Some(glyph) => assert_eq!(lit, vec![glyph], "{path}: glyph for {:?}", snapshot.system.power),
                    None => assert!(lit.is_empty(), "{path}: no glyph while hidden for {:?}", snapshot.system.power),
                }
            }
        }
    }

    // REGRESSION: the tray battery matrix over state x percent x presence. The
    // button shows iff present and state != Unknown; exactly one glyph is
    // lit, and only the Charging case may light a `-charging` node.
    // MUTATION: light the plain glyph while charging, or drop the charging
    // hide rule at 80%.
    #[test]
    fn battery_tray_matrix_shows_one_glyph_and_charging_variant_only_while_charging() {
        use crate::taskbar::status::battery::BATTERY_GLYPHS;
        use flamewm_api::system::{PowerSnapshot, PowerState, ServiceAvailability};
        let glyph_for = |percent: u8| match percent {
            15 => "020",
            45 => "050",
            79 | 80 => "080",
            100 => "100",
            other => panic!("matrix has no expected glyph for {other}%"),
        };
        let all_ids: Vec<String> = BATTERY_GLYPHS
            .iter()
            .flat_map(|glyph| {
                [
                    format!("tray-battery-{}", glyph.id_suffix()),
                    format!("tray-battery-{}-charging", glyph.id_suffix()),
                ]
            })
            .collect();
        for state in [
            PowerState::Charging,
            PowerState::Discharging,
            PowerState::Full,
            PowerState::Empty,
            PowerState::Unknown,
        ] {
            for percent in [15_u8, 45, 79, 80, 100] {
                for present in [true, false] {
                    let power = PowerSnapshot {
                        available: ServiceAvailability::Available,
                        present,
                        percent,
                        state,
                        cpu_percent: None,
                    };
                    let charging = state == PowerState::Charging;
                    let visible = present && state != PowerState::Unknown;
                    let mut document = flamewm_ui_x11::decode_document(PANEL_ARTIFACT)
                        .expect("panel artifact must decode");
                    project_battery_tray(&mut document, &power).expect("tray battery projects");
                    let viewport = (1080.0, 72.0);
                    let width = |id: &str| {
                        document
                            .layout_box_by_id(id, viewport)
                            .map_or(0.0, |rect| rect.2)
                    };
                    assert_eq!(width("tray-battery") > 0.0, visible, "{state:?} {percent}% present={present}");
                    let lit: Vec<&str> = all_ids
                        .iter()
                        .map(String::as_str)
                        .filter(|id| width(id) > 0.0)
                        .collect();
                    let expected = if visible {
                        let suffix = if charging { "-charging" } else { "" };
                        vec![format!("tray-battery-{}{suffix}", glyph_for(percent))]
                    } else {
                        Vec::new()
                    };
                    assert_eq!(lit, expected, "{state:?} {percent}% present={present}");
                }
            }
        }
    }

    // Full cold-start panel pass against the real artifact: a refused write in
    // any scope (task slots, geometry, pager, tray, clock) aborts the pass
    // before the later nodes are written.
    #[test]
    fn full_panel_projection_succeeds_on_real_artifact() {
        let mut document =
            flamewm_ui_x11::decode_document(PANEL_ARTIFACT).expect("panel artifact must decode");
        let snapshot = crate::ShellSnapshot::default();
        crate::async_projection::project_panel_async(
            &mut document,
            PanelView::first(&snapshot),
            None,
            &mut Default::default(),
        )
        .unwrap_or_else(|error| panic!("full panel projection refused: {error}"));
    }
}

#[cfg(test)]
mod panel_record_tests {
    use super::*;
    use flamewm_api::{OutputId, PanelEdge, Rect};

    fn output(id: &str, x: i32, scale: u16) -> flamewm_api::display::OutputSnapshot {
        flamewm_api::display::OutputSnapshot {
            id: OutputId::new(id),
            connector: id.to_owned(),
            edid_identity: String::new(),
            connected: true,
            primary: x == 0,
            geometry: Rect::new(x, 0, 1920, 1080),
            current_mode: flamewm_api::ModeId::default(),
            modes: Vec::new(),
            shell_scale_percent: scale,
        }
    }

    fn panel(id: &str, x: i32) -> flamewm_api::panels::PanelSnapshot {
        flamewm_api::panels::PanelSnapshot {
            output: OutputId::new(id),
            edge: PanelEdge::Bottom,
            logical_size: 44,
            visible: true,
            geometry: Rect::new(x, 1036, 1920, 44),
        }
    }

    #[test]
    fn panel_projection_visits_every_record_with_its_own_geometry_and_scale() {
        let mut snapshot = ShellSnapshot::default();
        snapshot.displays.outputs = vec![output("A", 0, 100), output("B", 1920, 150)];
        snapshot.panels.panels = vec![panel("A", 0), panel("B", 1920)];
        let mut seen: Vec<(&str, Rect, u16)> = Vec::new();
        let result = for_each_panel_record(
            &snapshot,
            vec![
                ("primary", OutputId::new("A")),
                ("secondary", OutputId::new("B")),
            ],
            |key: &str, view: PanelView<'_>| -> Result<(), String> {
                seen.push((
                    key,
                    view.panel.expect("panel").geometry,
                    view.output.expect("output").shell_scale_percent,
                ));
                Ok(())
            },
        );
        assert_eq!(result, Ok(()));
        assert_eq!(
            seen,
            vec![
                ("primary", Rect::new(0, 1036, 1920, 44), 100),
                ("secondary", Rect::new(1920, 1036, 1920, 44), 150),
            ]
        );
    }

    #[test]
    fn panel_projection_skips_disconnected_outputs_and_revisits_replugged_output() {
        let mut snapshot = ShellSnapshot::default();
        snapshot.displays.outputs = vec![output("A", 0, 100), output("B", 1920, 150)];
        snapshot.panels.panels = vec![panel("A", 0), panel("B", 1920)];
        let records = vec![
            ("primary", OutputId::new("A")),
            ("secondary", OutputId::new("B")),
        ];
        let mut seen = Vec::new();

        snapshot.displays.outputs[1].connected = false;
        for_each_panel_record(&snapshot, records.iter().cloned(), |key, _| {
            seen.push(key);
            Ok::<_, String>(())
        })
        .expect("partial output snapshot projects");
        assert_eq!(seen, vec!["primary"]);

        snapshot.displays.outputs[1].connected = true;
        snapshot.displays.outputs[1].shell_scale_percent = 175;
        for_each_panel_record(&snapshot, records, |key, view| {
            seen.push(key);
            if key == "secondary" {
                assert_eq!(
                    view.panel.expect("panel").geometry,
                    Rect::new(1920, 1036, 1920, 44)
                );
                assert_eq!(view.output.expect("output").shell_scale_percent, 175);
            }
            Ok::<_, String>(())
        })
        .expect("replugged output projects with fresh scale");
        assert_eq!(seen, vec!["primary", "primary", "secondary"]);
    }
}

#[cfg(test)]
mod clock_display_tests {
    use super::{clock_time_text, ClockDisplay, ClockFormat};
    use chrono::{Local, TimeZone};

    fn at(hour: u32, minute: u32, second: u32) -> chrono::DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 10, 12, hour, minute, second)
            .single()
            .expect("unambiguous local time")
    }

    fn display(format: ClockFormat, show_seconds: bool) -> ClockDisplay {
        ClockDisplay {
            format,
            show_seconds,
            show_date: true,
        }
    }

    #[test]
    fn twenty_four_hour_clock_omits_seconds_by_default() {
        let text = clock_time_text(display(ClockFormat::TwentyFour, false), at(14, 5, 9));
        assert_eq!(text, "14:05");
    }

    #[test]
    fn twenty_four_hour_clock_shows_seconds_when_enabled() {
        let text = clock_time_text(display(ClockFormat::TwentyFour, true), at(14, 5, 9));
        assert_eq!(text, "14:05:09");
    }

    #[test]
    fn twelve_hour_clock_uses_unpadded_hour_and_meridiem() {
        let text = clock_time_text(display(ClockFormat::Twelve, false), at(14, 5, 9));
        assert_eq!(text, "2:05 PM");
    }

    #[test]
    fn twelve_hour_midnight_and_noon_map_to_twelve() {
        assert_eq!(
            clock_time_text(display(ClockFormat::Twelve, false), at(0, 30, 0)),
            "12:30 AM"
        );
        assert_eq!(
            clock_time_text(display(ClockFormat::Twelve, false), at(12, 0, 0)),
            "12:00 PM"
        );
    }

    #[test]
    fn twelve_hour_clock_shows_seconds_when_enabled() {
        let text = clock_time_text(display(ClockFormat::Twelve, true), at(9, 7, 3));
        assert_eq!(text, "9:07:03 AM");
    }

    #[test]
    fn default_display_matches_product_defaults() {
        let defaults = ClockDisplay::default();
        assert_eq!(defaults.format, ClockFormat::TwentyFour);
        assert!(!defaults.show_seconds);
        assert!(defaults.show_date);
    }
}

#[cfg(test)]
mod media_projection_tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MediaDoc {
        texts: HashMap<String, String>,
        visible: HashMap<String, bool>,
    }

    impl UiDocumentAccess for MediaDoc {
        fn text(&mut self, id: &str, text: String) -> Result<(), String> {
            self.texts.insert(id.to_owned(), text);
            Ok(())
        }
        fn image_rgb8(&mut self, _: &str, _: RuntimeImage) -> Result<(), String> {
            Ok(())
        }
        fn image_rgba8(&mut self, _: &str, _: RuntimeImage) -> Result<(), String> {
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
        crate::noop_document_overflow!();
        fn font_size(&mut self, _: &str, _: f32) -> Result<(), String> {
            Ok(())
        }
        fn font_weight(&mut self, _: &str, _: u16) -> Result<(), String> {
            Ok(())
        }
    }

    // REGRESSION: the popup has one title row and no artist line, so project_media
    // must not write `media-artist` (the compiled artifact no longer has that node).
    #[test]
    fn media_projects_title_only_without_artist_node() {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.media.title = "Song Title".to_owned();
        snapshot.system.media.artist = "Some Artist".to_owned();
        let mut document = MediaDoc::default();

        project_media(&mut document, &snapshot).expect("media projection must succeed");

        assert_eq!(
            document.texts.get("media-title").map(String::as_str),
            Some("Song Title")
        );
        assert!(
            !document.texts.contains_key("media-artist"),
            "artist line was removed from media.html"
        );
    }

    fn media_title_text(title: &str) -> String {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.media.title = title.to_owned();
        let mut document = MediaDoc::default();
        project_media(&mut document, &snapshot).expect("media projection must succeed");
        document
            .texts
            .get("media-title")
            .cloned()
            .expect("media-title must be written")
    }

    #[test]
    fn media_title_short_is_unchanged() {
        assert_eq!(media_title_text("Song Title"), "Song Title");
        let exact = "a".repeat(MEDIA_TITLE_MAX_CHARS);
        assert_eq!(media_title_text(&exact), exact);
    }

    #[test]
    fn media_title_long_ascii_is_truncated_with_ellipsis_within_budget() {
        let long = format!("{} tail words that keep going", "Abcdefghij ".repeat(5));
        let fitted = media_title_text(&long);
        assert!(fitted.ends_with('…'), "{fitted:?}");
        assert!(fitted.chars().count() <= MEDIA_TITLE_MAX_CHARS, "{fitted:?}");
        assert!(fitted.starts_with("Abcdefghij Abcdefghij"), "{fitted:?}");
        assert!(!fitted.contains("  "), "{fitted:?}");
    }

    #[test]
    fn media_title_trims_whitespace_before_ellipsis() {
        // Budget keeps 31 chars: 30 'x' plus one space, which must be trimmed.
        let title = format!("{}     {}", "x".repeat(30), "y".repeat(10));
        let fitted = media_title_text(&title);
        assert_eq!(fitted, format!("{}…", "x".repeat(30)));
    }

    #[test]
    fn media_title_multibyte_and_emoji_do_not_panic_and_end_on_char_boundary() {
        let title = "🎵 Ünïcødé 日本語のタイトル 🎶 ".repeat(4);
        let fitted = media_title_text(&title);
        assert!(fitted.ends_with('…'), "{fitted:?}");
        assert!(fitted.chars().count() <= MEDIA_TITLE_MAX_CHARS, "{fitted:?}");
        assert!(std::str::from_utf8(fitted.as_bytes()).is_ok());
    }

    // REGRESSION: a disabled control must show only its action-less twin; the
    // enabled button (which carries data-action) must be hidden, and vice versa.
    #[test]
    fn media_controls_swap_to_disabled_twins_by_capability() {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.media.availability = flamewm_api::system::ServiceAvailability::Available;
        snapshot.system.media.can_next = true;
        snapshot.system.media.can_play = true;
        let mut document = MediaDoc::default();

        project_media(&mut document, &snapshot).expect("media projection must succeed");

        let visible = |id: &str| document.visible.get(id).copied();
        assert_eq!(visible("media-next"), Some(true));
        assert_eq!(visible("media-next-disabled"), Some(false));
        assert_eq!(visible("media-playpause"), Some(true));
        assert_eq!(visible("media-playpause-disabled"), Some(false));
        assert_eq!(visible("media-previous"), Some(false));
        assert_eq!(visible("media-previous-disabled"), Some(true));
    }

    #[test]
    fn media_controls_all_disabled_without_player() {
        let snapshot = crate::ShellSnapshot::default();
        let mut document = MediaDoc::default();

        project_media(&mut document, &snapshot).expect("media projection must succeed");

        for id in ["media-previous", "media-playpause", "media-next"] {
            assert_eq!(document.visible.get(id).copied(), Some(false), "{id}");
        }
        for id in [
            "media-previous-disabled",
            "media-playpause-disabled",
            "media-next-disabled",
        ] {
            assert_eq!(document.visible.get(id).copied(), Some(true), "{id}");
        }
    }
}

#[cfg(test)]
mod start_category_table_tests {
    use super::*;
    use flamewm_api::DesktopAppId;

    fn application(categories: &[&str]) -> DesktopApplication {
        DesktopApplication {
            id: DesktopAppId::new("org.example.app"),
            name: "App".to_owned(),
            generic_name: String::new(),
            comment: String::new(),
            startup_wm_class: String::new(),
            argv: Vec::new(),
            keywords: Vec::new(),
            icon_name: String::new(),
            categories: categories.iter().map(|value| (*value).to_owned()).collect(),
        }
    }

    /// Literal oracle: the presentation bucket each application set must land in.
    /// Unmatched or empty category sets fall to Utilities.
    const ORACLE: &[(&[&str], StartCategory)] = &[
        (&["Accessories"], StartCategory::Utilities),
        (&["utility"], StartCategory::Utilities),
        (&["Education"], StartCategory::Utilities),
        (&["Game"], StartCategory::Games),
        (&["GAMES"], StartCategory::Games),
        (&["Graphics"], StartCategory::Graphics),
        (&["Network"], StartCategory::Internet),
        (&["Internet"], StartCategory::Internet),
        (&["AudioVideo"], StartCategory::Multimedia),
        (&["Audio"], StartCategory::Multimedia),
        (&["Video"], StartCategory::Multimedia),
        (&["Multimedia"], StartCategory::Multimedia),
        (&["Development"], StartCategory::Development),
        (&["Office"], StartCategory::Utilities),
        (&["Science"], StartCategory::Utilities),
        (&["Settings"], StartCategory::System),
        (&["System"], StartCategory::System),
        (&["Unknown"], StartCategory::Utilities),
        (&[""], StartCategory::Utilities),
        (&[], StartCategory::Utilities),
        (&["Unknown", "Games"], StartCategory::Games),
        (&["Office", "Games"], StartCategory::Utilities),
        (&["Unknown", "Development"], StartCategory::Development),
    ];

    #[test]
    fn application_matches_category_is_the_oracle_table_for_every_selectable_category() {
        let rows = [
            (StartCategory::Development, StartCategory::Development),
            (StartCategory::Games, StartCategory::Games),
            (StartCategory::Graphics, StartCategory::Graphics),
            (StartCategory::Internet, StartCategory::Internet),
            (StartCategory::Multimedia, StartCategory::Multimedia),
            (StartCategory::System, StartCategory::System),
            (StartCategory::Utilities, StartCategory::Utilities),
        ];
        for (categories, bucket) in ORACLE {
            let app = application(categories);
            for (selected, expected_bucket) in rows {
                assert_eq!(
                    application_matches_category(&app, selected),
                    expected_bucket == *bucket,
                    "categories {categories:?} vs {selected:?}"
                );
            }
            assert!(application_matches_category(&app, StartCategory::All));
            assert!(!application_matches_category(&app, StartCategory::Power));
        }
    }
}

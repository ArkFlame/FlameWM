//! Flame Shell view models. No X11 widget/window owns product state here.

pub mod clock;
pub mod menu_policy;
pub mod popup;
pub mod quickswitch;
pub mod start_menu;
pub mod status;
pub mod weather;
pub mod workspaces;

use flamewm_api::applications::DesktopApplication;
use flamewm_api::display::DisplaySnapshot;
use flamewm_api::panels::{PanelsSnapshot, TaskEntry, TaskEntryKind};
use flamewm_api::system::{
    NetworkAccessPointSnapshot, NetworkKind, PlaybackState, ServiceAvailability, SystemSnapshot,
};
use flamewm_api::window::{WindowSnapshot, WindowState};
use flamewm_api::{DesktopAppId, OutputId, Rect, Size, TaskEntryId, WindowRef};
use flamewm_ui_core::IconRole;
use flamewm_ui_core::style::ShellMetrics;

pub use start_menu::{StartModel, StartSurfaceLayout, start_surface_layout};
pub use workspaces::workspace_labels;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkView {
    pub visible: bool,
    pub enabled: bool,
    pub icon: IconRole,
    pub label: String,
    pub strength_percent: u8,
    pub wifi_enabled: bool,
    pub networking_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioView {
    pub visible: bool,
    pub enabled: bool,
    pub icon: IconRole,
    pub label: String,
    pub volume_percent: u8,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaView {
    pub visible: bool,
    pub enabled: bool,
    pub play_pause_icon: Option<IconRole>,
    pub identity: String,
    pub title: String,
    pub artist: String,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_next: bool,
    pub can_previous: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusViews {
    pub network: NetworkView,
    pub audio: AudioView,
    pub media: MediaView,
}

impl StatusViews {
    #[must_use]
    pub fn from_snapshot(system: &SystemSnapshot) -> Self {
        let network_enabled = system.network.availability == ServiceAvailability::Available;
        let network_icon = match system.network.kind {
            NetworkKind::Wired => IconRole::NetworkWired,
            NetworkKind::Wireless => wifi_icon(system.network.strength_percent),
            NetworkKind::Disconnected | NetworkKind::Connecting | NetworkKind::Unavailable => {
                IconRole::NetworkOffline
            }
        };
        let audio_enabled = system.audio.availability == ServiceAvailability::Available;
        let audio_icon = if system.audio.muted {
            IconRole::AudioMuted
        } else {
            match system.audio.volume_percent {
                0..=32 => IconRole::AudioLow,
                33..=66 => IconRole::AudioMedium,
                _ => IconRole::AudioHigh,
            }
        };
        let media_enabled = system.media.availability == ServiceAvailability::Available
            && system.media.playback != PlaybackState::Unavailable;
        Self {
            network: NetworkView {
                visible: system.network.availability != ServiceAvailability::Unavailable,
                enabled: network_enabled,
                icon: network_icon,
                label: system.network.label.clone(),
                strength_percent: system.network.strength_percent.min(100),
                wifi_enabled: system.network.wifi_enabled,
                networking_enabled: system.network.networking_enabled,
            },
            audio: AudioView {
                visible: system.audio.availability != ServiceAvailability::Unavailable,
                enabled: audio_enabled,
                icon: audio_icon,
                label: system.audio.sink_name.clone(),
                volume_percent: system.audio.volume_percent.min(100),
                muted: system.audio.muted,
            },
            media: MediaView {
                visible: system.media.availability != ServiceAvailability::Unavailable,
                enabled: media_enabled,
                // Shows the action the control performs: Playing -> Pause glyph, Paused -> Play glyph.
                play_pause_icon: match system.media.playback {
                    PlaybackState::Playing => Some(IconRole::MediaPause),
                    PlaybackState::Paused => Some(IconRole::MediaPlay),
                    PlaybackState::Stopped | PlaybackState::Unavailable => None,
                },
                identity: system.media.identity.clone(),
                title: system.media.title.clone(),
                artist: system.media.artist.clone(),
                can_play: system.media.can_play,
                can_pause: system.media.can_pause,
                can_next: system.media.can_next,
                can_previous: system.media.can_previous,
            },
        }
    }
}

/// Audio popup: 459x460 logical px (matches ui/shell/audio.html root).
pub const AUDIO_POPOVER_SIZE: Size = Size::new(459, 460);
pub const MEDIA_POPOVER_SIZE: Size = Size::new(276, 80);
/// Network popup: 461x453 logical px (matches ui/shell/network.html root).
pub const NETWORK_POPOVER_SIZE: Size = Size::new(461, 453);
/// Battery popup: 300x132 logical px (matches ui/shell/battery.html root).
pub const BATTERY_POPOVER_SIZE: Size = Size::new(300, 132);
/// Weather popup: 300x320 logical px (matches ui/shell/weather.html root).
pub const WEATHER_POPOVER_SIZE: Size = Size::new(300, 320);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPopoverModel {
    pub size: Size,
    pub volume_percent: u8,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPopoverModel {
    pub size: Size,
    pub details: String,
    pub play_pause_icon: Option<IconRole>,
    pub can_play_pause: bool,
    pub can_previous: bool,
    pub can_next: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkAccessPointRow {
    pub path: String,
    pub label: String,
    pub strength_percent: u8,
    pub secured: bool,
    pub known: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkPopoverModel {
    pub size: Size,
    pub details: String,
    pub wifi_enabled: bool,
    pub can_disconnect: bool,
    pub can_scan: bool,
    pub access_points: Vec<NetworkAccessPointRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusPopovers {
    pub audio: Option<AudioPopoverModel>,
    pub media: Option<MediaPopoverModel>,
    pub network: Option<NetworkPopoverModel>,
}

impl StatusPopovers {
    #[must_use]
    pub fn from_snapshot(system: &SystemSnapshot) -> Self {
        let views = StatusViews::from_snapshot(system);
        let audio = views.audio.visible.then(|| AudioPopoverModel {
            size: AUDIO_POPOVER_SIZE,
            volume_percent: system.audio.volume_percent.min(100),
            muted: system.audio.muted,
        });
        let media = views.media.visible.then(|| {
            let details = [
                system.media.identity.as_str(),
                system.media.title.as_str(),
                system.media.artist.as_str(),
            ]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join(" - ");
            MediaPopoverModel {
                size: MEDIA_POPOVER_SIZE,
                details: if details.is_empty() {
                    "No media player".to_owned()
                } else {
                    details
                },
                play_pause_icon: views.media.play_pause_icon,
                can_play_pause: system.media.can_play || system.media.can_pause,
                can_previous: system.media.can_previous,
                can_next: system.media.can_next,
            }
        });
        let network = views.network.visible.then(|| NetworkPopoverModel {
            size: NETWORK_POPOVER_SIZE,
            details: if system.network.label.is_empty() {
                "Network unavailable".to_owned()
            } else {
                system.network.label.clone()
            },
            wifi_enabled: system.network.wifi_enabled,
            can_disconnect: views.network.enabled
                && system.network.kind != NetworkKind::Disconnected,
            can_scan: views.network.enabled,
            access_points: system
                .network
                .access_points
                .iter()
                .map(network_access_point_row)
                .collect(),
        });
        Self {
            audio,
            media,
            network,
        }
    }
}

fn network_access_point_row(access_point: &NetworkAccessPointSnapshot) -> NetworkAccessPointRow {
    NetworkAccessPointRow {
        path: access_point.path.clone(),
        label: if access_point.ssid.is_empty() {
            "Hidden network".to_owned()
        } else {
            access_point.ssid.clone()
        },
        strength_percent: access_point.strength_percent.min(100),
        secured: access_point.secured,
        known: access_point.known,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarMonth {
    Previous,
    Current,
    Next,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarCell {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub month_kind: CalendarMonth,
    pub today: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarGrid {
    pub year: i32,
    pub month: u8,
    pub cells: [CalendarCell; 42],
}

impl CalendarGrid {
    pub fn build(year: i32, month: u8) -> Option<Self> {
        let (today_year, today_month, today_day) = today_date();
        Self::build_for_date(year, month, today_year, today_month, today_day)
    }

    pub fn build_for_date(
        year: i32,
        month: u8,
        today_year: i32,
        today_month: u8,
        today_day: u8,
    ) -> Option<Self> {
        if !(1..=12).contains(&month) {
            return None;
        }
        let days = days_in_month(year, month);
        let first = monday_index(year, month, 1);
        let (previous_year, previous_month) = shift_month(year, month, -1);
        let (next_year, next_month) = shift_month(year, month, 1);
        let previous_days = days_in_month(previous_year, previous_month);
        let mut cells = Vec::with_capacity(42);
        for index in 0..42 {
            let relative_day = index as i32 - i32::from(first) + 1;
            let (cell_year, cell_month, day, month_kind) = if relative_day < 1 {
                (
                    previous_year,
                    previous_month,
                    (i32::from(previous_days) + relative_day) as u8,
                    CalendarMonth::Previous,
                )
            } else if relative_day > i32::from(days) {
                (
                    next_year,
                    next_month,
                    (relative_day - i32::from(days)) as u8,
                    CalendarMonth::Next,
                )
            } else {
                (year, month, relative_day as u8, CalendarMonth::Current)
            };
            cells.push(CalendarCell {
                year: cell_year,
                month: cell_month,
                day,
                month_kind,
                today: cell_year == today_year && cell_month == today_month && day == today_day,
            });
        }
        Some(Self {
            year,
            month,
            cells: cells.try_into().ok().expect("calendar has 42 cells"),
        })
    }
}

#[must_use]
fn wifi_icon(strength: u8) -> IconRole {
    match strength.min(100) {
        0..=10 => IconRole::Wifi0,
        11..=35 => IconRole::Wifi25,
        36..=60 => IconRole::Wifi50,
        61..=85 => IconRole::Wifi75,
        _ => IconRole::Wifi100,
    }
}

#[must_use]
fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

#[must_use]
fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Monday=0..Sunday=6, Gregorian calendar.
#[must_use]
fn monday_index(year: i32, month: u8, day: u8) -> u8 {
    let mut y = i64::from(year);
    let mut m = i64::from(month);
    if m < 3 {
        y -= 1;
        m += 12;
    }
    let k = y.rem_euclid(100);
    let j = y.div_euclid(100);
    let h = (i64::from(day) + (13 * (m + 1)) / 5 + k + k / 4 + j / 4 + 5 * j).rem_euclid(7);
    // Zeller: 0=Sat,1=Sun,2=Mon,... -> Monday=0.
    ((h + 5).rem_euclid(7)) as u8
}

/// Calendar month arithmetic: `delta` months from `year`/`month`, wrapping years.
#[must_use]
pub fn shift_month(year: i32, month: u8, delta: i32) -> (i32, u8) {
    let index = i64::from(year) * 12 + i64::from(month) - 1 + i64::from(delta);
    let year = index.div_euclid(12) as i32;
    let month = index.rem_euclid(12) as u8 + 1;
    (year, month)
}

#[must_use]
fn today_date() -> (i32, u8, u8) {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    civil_date_from_days((seconds / 86_400) as i64)
}

// Howard Hinnant's proleptic Gregorian conversion, with epoch days as input.
#[must_use]
fn civil_date_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    (year as i32 + (month <= 2) as i32, month as u8, day as u8)
}

pub const STATUS_SLOT_WIDTH: i32 = 30;
pub const STATUS_SLOT_COUNT: i32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusSlot {
    Audio = 0,
    Media = 1,
    Network = 2,
}

#[must_use]
pub fn status_slot_rects(panel_cross: i32) -> [Rect; STATUS_SLOT_COUNT as usize] {
    [StatusSlot::Audio, StatusSlot::Media, StatusSlot::Network].map(|slot| {
        Rect::new(
            slot as i32 * STATUS_SLOT_WIDTH,
            0,
            STATUS_SLOT_WIDTH,
            panel_cross.max(0),
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockFormat {
    pub time: &'static str,
    pub date: &'static str,
}

impl Default for ClockFormat {
    fn default() -> Self {
        Self {
            time: "%H:%M",
            date: "%d/%m/%y",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockTimerPlan {
    pub interval_ms: u64,
    pub initial_delay_ms: u64,
}

#[must_use]
pub fn clock_timer_plan(format: &str, now_epoch_ms: u64) -> ClockTimerPlan {
    let has_seconds = ["%S", "%T", "%X", "%r"]
        .iter()
        .any(|token| format.contains(token));
    let interval_ms = if has_seconds { 1_000 } else { 60_000 };
    let remainder = now_epoch_ms % interval_ms;
    let initial_delay_ms = interval_ms - remainder;
    ClockTimerPlan {
        interval_ms,
        initial_delay_ms,
    }
}

/// Clock calendar popup: 580x476 logical px (Plasma reference layout).
#[must_use]
pub fn calendar_popover_size() -> Size {
    Size::new(580, 476)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelSlot {
    Start = 0,
    Tasks = 1,
    Spacer = 2,
    Workspaces = 3,
    Status = 4,
    Tray = 5,
    Clock = 6,
}
pub const PANEL_SLOT_COUNT: usize = 7;

#[must_use]
pub fn default_panel_slot_footprints(
    metrics: ShellMetrics,
) -> ([i32; PANEL_SLOT_COUNT], [i32; PANEL_SLOT_COUNT]) {
    let preferred = [
        i32::from(metrics.start_button_width),
        0,
        12,
        i32::from(metrics.workspace_button_width) * 2,
        i32::from(metrics.tray_button_width) * STATUS_SLOT_COUNT,
        i32::from(metrics.tray_button_width),
        i32::from(metrics.clock_min_width),
    ];
    let minimum = [
        i32::from(metrics.start_button_width),
        i32::from(metrics.task_button_width),
        0,
        i32::from(metrics.workspace_button_width),
        i32::from(metrics.tray_button_width),
        i32::from(metrics.tray_button_width),
        i32::from(metrics.clock_min_width),
    ];
    (preferred, minimum)
}

/// Source-equivalent adaptive panel solver. It preserves disjoint slots even when
/// preferred/minimum footprints exceed a narrow portrait output. Extra space is
/// distributed only to Tasks and Spacer, matching the current native shell.
#[must_use]
pub fn solve_panel_slot_sizes(
    axis: i32,
    preferred: [i32; PANEL_SLOT_COUNT],
    minimum: [i32; PANEL_SLOT_COUNT],
) -> [i32; PANEL_SLOT_COUNT] {
    let axis = axis.max(0);
    let mut sizes = preferred.map(|value| value.max(0));
    let mut total: i32 = sizes.iter().sum();
    if total > axis {
        let mut excess = total - axis;
        while excess > 0 {
            let mut reduced = false;
            for index in 0..PANEL_SLOT_COUNT {
                let floor = minimum[index].max(0);
                if excess > 0 && sizes[index] > floor {
                    sizes[index] -= 1;
                    excess -= 1;
                    reduced = true;
                }
            }
            if !reduced {
                break;
            }
        }
        while excess > 0 {
            let mut reduced = false;
            for size in &mut sizes {
                if excess > 0 && *size > 0 {
                    *size -= 1;
                    excess -= 1;
                    reduced = true;
                }
            }
            if !reduced {
                break;
            }
        }
    } else if total < axis {
        let mut extra = axis - total;
        while extra > 0 {
            for index in [PanelSlot::Tasks as usize, PanelSlot::Spacer as usize] {
                if extra == 0 {
                    break;
                }
                sizes[index] += 1;
                extra -= 1;
            }
        }
    }
    total = sizes.iter().sum();
    debug_assert!(total <= axis || axis == 0);
    sizes
}

#[must_use]
pub fn panel_slot_rects(
    horizontal: bool,
    axis: i32,
    cross: i32,
    preferred: [i32; PANEL_SLOT_COUNT],
    minimum: [i32; PANEL_SLOT_COUNT],
) -> [Rect; PANEL_SLOT_COUNT] {
    let sizes = solve_panel_slot_sizes(axis, preferred, minimum);
    let mut result = [Rect::default(); PANEL_SLOT_COUNT];
    let mut cursor = 0;
    for (index, size) in sizes.into_iter().enumerate() {
        result[index] = if horizontal {
            Rect::new(cursor, 0, size, cross.max(0))
        } else {
            Rect::new(0, cursor, cross.max(0), size)
        };
        cursor += size;
    }
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrimaryPresentationRouter {
    primary: Option<OutputId>,
}
impl PrimaryPresentationRouter {
    #[must_use]
    pub fn from_displays(displays: &DisplaySnapshot) -> Self {
        let primary = displays
            .outputs
            .iter()
            .find(|o| o.connected && o.primary)
            .or_else(|| displays.outputs.iter().find(|o| o.connected))
            .map(|o| o.id.clone());
        Self { primary }
    }
    #[must_use]
    pub fn primary(&self) -> Option<&OutputId> {
        self.primary.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[deprecated(note = "canonical StartSurfaceLayout lives in start_menu; use that import")]
pub struct StartSurfaceLayoutCompat {
    pub button: Rect,
    pub anchor: Rect,
    pub popover: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskVisualState {
    pub id: TaskEntryId,
    pub app_id: DesktopAppId,
    pub window: Option<WindowRef>,
    pub icon_name: String,
    pub running: bool,
    pub focused: bool,
    pub minimized: bool,
}

fn entry_window(entry: &TaskEntry) -> Option<WindowRef> {
    match entry.kind {
        TaskEntryKind::PinnedSlot { window } => window,
        TaskEntryKind::Window { window } => Some(window),
    }
}

#[must_use]
pub fn task_visual_states(
    panels: &PanelsSnapshot,
    windows: &[WindowSnapshot],
    applications: &[DesktopApplication],
) -> Vec<TaskVisualState> {
    task_visual_states_on_output(panels, windows, applications, None)
}

/// Task states listed by the panel on `output`. A running entry is listed only
/// on the output its window is on; an entry with no live window (pinned, not
/// running) or no window snapshot is listed on every panel. `None` lists all.
#[must_use]
pub fn task_visual_states_on_output(
    panels: &PanelsSnapshot,
    windows: &[WindowSnapshot],
    applications: &[DesktopApplication],
    output: Option<&OutputId>,
) -> Vec<TaskVisualState> {
    panels
        .tasks
        .iter()
        .filter_map(|entry| {
            let window = entry_window(entry);
            let snapshot = window
                .and_then(|reference| windows.iter().find(|item| item.reference == reference));
            if let (Some(output), Some(snapshot)) = (output, snapshot) {
                if &snapshot.output != output {
                    return None;
                }
            }
            let icon_name = applications
                .iter()
                .find(|app| app.id == entry.app_id)
                .map(|app| app.icon_name.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| entry.app_id.as_str().to_owned());
            Some(TaskVisualState {
                id: entry.id.clone(),
                app_id: entry.app_id.clone(),
                window,
                icon_name,
                running: window.is_some(),
                focused: snapshot.is_some_and(|item| item.focused),
                minimized: snapshot.is_some_and(|item| item.state == WindowState::Minimized),
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskClickAction {
    Launch(DesktopAppId),
    RestoreAndActivate(WindowRef),
    Minimize(WindowRef),
    Activate(WindowRef),
}

#[must_use]
pub fn task_click_action(entry: &TaskEntry, windows: &[WindowSnapshot]) -> TaskClickAction {
    let Some(window) = entry_window(entry) else {
        return TaskClickAction::Launch(entry.app_id.clone());
    };
    let Some(snapshot) = windows.iter().find(|item| item.reference == window) else {
        return TaskClickAction::Activate(window);
    };
    if snapshot.state == WindowState::Minimized {
        TaskClickAction::RestoreAndActivate(window)
    } else if snapshot.focused {
        TaskClickAction::Minimize(window)
    } else {
        TaskClickAction::Activate(window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn september_2026_calendar_is_monday_first_and_marks_today() {
        let calendar = CalendarGrid::build_for_date(2026, 9, 2026, 9, 5).expect("valid month");
        assert_eq!(
            calendar.cells[0],
            CalendarCell {
                year: 2026,
                month: 8,
                day: 31,
                month_kind: CalendarMonth::Previous,
                today: false
            }
        );
        assert_eq!(calendar.cells[1].day, 1);
        assert_eq!(calendar.cells[5].day, 5);
        assert!(calendar.cells[5].today);
        assert!(calendar.cells[41].month_kind == CalendarMonth::Next);
    }

    #[test]
    fn calendar_always_contains_six_monday_first_rows() {
        let calendar = CalendarGrid::build_for_date(2026, 9, 2000, 1, 1).expect("valid month");
        assert_eq!(calendar.cells.len(), 42);
        assert_eq!(calendar.cells[0].month_kind, CalendarMonth::Previous);
        assert_eq!(calendar.cells[1].month_kind, CalendarMonth::Current);
        assert_eq!(calendar.cells[30].month_kind, CalendarMonth::Current);
        assert_eq!(calendar.cells[31].month_kind, CalendarMonth::Next);
    }

    #[test]
    fn unavailable_services_collapse_their_status_buttons() {
        let mut system = SystemSnapshot::default();
        system.network.availability = ServiceAvailability::Unavailable;
        system.audio.availability = ServiceAvailability::Unavailable;
        system.media.availability = ServiceAvailability::Unavailable;
        let views = StatusViews::from_snapshot(&system);
        assert!(!views.network.visible && !views.audio.visible && !views.media.visible);
    }

    #[test]
    fn panel_solver_keeps_slots_disjoint_on_narrow_axis() {
        let preferred = [43, 300, 12, 80, 90, 30, 70];
        let minimum = [43, 42, 0, 22, 30, 30, 70];
        let sizes = solve_panel_slot_sizes(220, preferred, minimum);
        assert_eq!(sizes.iter().sum::<i32>(), 220);
        assert!(sizes.iter().all(|size| *size >= 0));
    }

    #[test]
    fn default_panel_footprints_match_current_native_fallbacks() {
        let (preferred, minimum) = default_panel_slot_footprints(ShellMetrics::default());
        assert_eq!(preferred, [43, 0, 12, 44, 90, 30, 70]);
        assert_eq!(minimum, [43, 42, 0, 22, 30, 30, 70]);
    }

    #[test]
    fn primary_presentation_uses_display_primary_not_sorted_output_name() {
        use flamewm_api::display::{DisplaySnapshot, OutputSnapshot};
        use flamewm_api::{ModeId, Rect};
        let output = |name: &str, primary: bool| OutputSnapshot {
            id: OutputId::new(name),
            connector: name.to_owned(),
            edid_identity: String::new(),
            connected: true,
            primary,
            geometry: Rect::new(0, 0, 100, 100),
            current_mode: ModeId(1),
            modes: Vec::new(),
            shell_scale_percent: 100,
        };
        let displays = DisplaySnapshot {
            generation: 1,
            outputs: vec![output("AAA", false), output("ZZZ", true)],
            pending: None,
        };
        assert_eq!(
            PrimaryPresentationRouter::from_displays(&displays)
                .primary()
                .map(OutputId::as_str),
            Some("ZZZ")
        );
    }

    mod task_output {
        use super::super::*;
        use flamewm_api::window::WindowState;
        use flamewm_api::{DesktopAppId, TaskEntryId, WindowRef};

        fn window(id: u64, output: &str, state: WindowState) -> WindowSnapshot {
            WindowSnapshot {
                reference: WindowRef::new(id, 1),
                title: String::new(),
                app_id: DesktopAppId::new("term"),
                outer_geometry: Rect::new(-1920, 0, 100, 100),
                restore_geometry: Rect::new(-1920, 0, 100, 100),
                state,
                sticky: false,
                focused: false,
                workspace: flamewm_api::WorkspaceRef { index: 0, revision: 1 },
                output: OutputId::new(output),
                state_generation: 1,
            }
        }

        fn running(id: u64) -> TaskEntry {
            let window = WindowRef::new(id, 1);
            TaskEntry {
                id: TaskEntryId::window(window),
                app_id: DesktopAppId::new("term"),
                kind: TaskEntryKind::Window { window },
                order_index: id as usize,
            }
        }

        fn pinned(app: &str, window: Option<WindowRef>) -> TaskEntry {
            let app_id = DesktopAppId::new(app);
            TaskEntry {
                id: TaskEntryId::pinned(&app_id),
                app_id,
                kind: TaskEntryKind::PinnedSlot { window },
                order_index: 0,
            }
        }

        fn panels(tasks: Vec<TaskEntry>) -> PanelsSnapshot {
            PanelsSnapshot { revision: 1, panels: Vec::new(), tasks, pinned_apps: Vec::new() }
        }

        fn ids(states: &[TaskVisualState]) -> Vec<TaskEntryId> {
            states.iter().map(|state| state.id.clone()).collect()
        }

        #[test]
        fn window_moving_between_outputs_moves_its_entry() {
            let (a, b) = (OutputId::new("A"), OutputId::new("B"));
            let tasks = panels(vec![running(1)]);
            let on_a = [window(1, "A", WindowState::Normal)];
            assert_eq!(ids(&task_visual_states_on_output(&tasks, &on_a, &[], Some(&a))).len(), 1);
            assert!(task_visual_states_on_output(&tasks, &on_a, &[], Some(&b)).is_empty());
            let on_b = [window(1, "B", WindowState::Normal)];
            assert!(task_visual_states_on_output(&tasks, &on_b, &[], Some(&a)).is_empty());
            assert_eq!(ids(&task_visual_states_on_output(&tasks, &on_b, &[], Some(&b))).len(), 1);
        }

        #[test]
        fn minimized_window_stays_on_its_last_output() {
            let a = OutputId::new("A");
            let tasks = panels(vec![running(1)]);
            let windows = [window(1, "A", WindowState::Minimized)];
            let on_a = task_visual_states_on_output(&tasks, &windows, &[], Some(&a));
            assert_eq!(on_a.len(), 1);
            assert!(on_a[0].minimized);
            assert!(task_visual_states_on_output(&tasks, &windows, &[], Some(&OutputId::new("B")))
                .is_empty());
        }

        #[test]
        fn pinned_entry_without_window_lists_on_every_panel() {
            let tasks = panels(vec![pinned("term", None), pinned("ghost", Some(WindowRef::new(9, 1)))]);
            let windows = [window(1, "A", WindowState::Normal)];
            for output in ["A", "B"] {
                let output = OutputId::new(output);
                assert_eq!(
                    task_visual_states_on_output(&tasks, &windows, &[], Some(&output)).len(),
                    2
                );
            }
        }

        #[test]
        fn same_app_windows_on_two_outputs_split_between_panels() {
            let tasks = panels(vec![running(1), running(2)]);
            let windows = [window(1, "A", WindowState::Normal), window(2, "B", WindowState::Normal)];
            let a = task_visual_states_on_output(&tasks, &windows, &[], Some(&OutputId::new("A")));
            let b = task_visual_states_on_output(&tasks, &windows, &[], Some(&OutputId::new("B")));
            assert_eq!(ids(&a), vec![TaskEntryId::window(WindowRef::new(1, 1))]);
            assert_eq!(ids(&b), vec![TaskEntryId::window(WindowRef::new(2, 1))]);
        }

        #[test]
        fn single_output_filter_matches_unfiltered_states() {
            let a = OutputId::new("A");
            let tasks = panels(vec![pinned("term", None), running(1), running(2)]);
            let windows = [window(1, "A", WindowState::Normal), window(2, "A", WindowState::Minimized)];
            assert_eq!(
                task_visual_states(&tasks, &windows, &[]),
                task_visual_states_on_output(&tasks, &windows, &[], Some(&a)),
            );
            assert_eq!(
                task_visual_states(&tasks, &windows, &[]),
                task_visual_states_on_output(&tasks, &windows, &[], None),
            );
        }
    }

    #[test]
    fn bottom_start_surface_anchors_to_real_button() {
        let layout = start_surface_layout(
            OutputId::new("eDP-1"),
            Rect::new(0, 0, 1920, 1080),
            flamewm_api::PanelEdge::Bottom,
            ShellMetrics::default(),
        );
        assert_eq!(layout.anchor, Rect::new(0, 1036, 43, 44));
        assert!(layout.popover.bottom() <= layout.anchor.y);
    }

    #[test]
    fn workspace_buttons_ignore_names_and_remain_numeric() {
        assert_eq!(workspace_labels(3), vec!["1", "2", "3"]);
    }

    #[test]
    fn native_clock_date_format_is_day_month_short_year() {
        assert_eq!(ClockFormat::default().date, "%d/%m/%y");
    }

    #[test]
    fn status_slots_keep_native_audio_media_network_order() {
        assert_eq!(
            status_slot_rects(44),
            [
                Rect::new(0, 0, 30, 44),
                Rect::new(30, 0, 30, 44),
                Rect::new(60, 0, 30, 44)
            ]
        );
    }

    #[test]
    fn status_popovers_match_current_native_geometry() {
        let mut system = SystemSnapshot::default();
        system.audio.availability = ServiceAvailability::Available;
        system.media.availability = ServiceAvailability::Available;
        system.media.playback = PlaybackState::Paused;
        system.network.availability = ServiceAvailability::Available;
        let popovers = StatusPopovers::from_snapshot(&system);
        assert_eq!(
            popovers.network.expect("network").size,
            NETWORK_POPOVER_SIZE
        );
    }

    #[test]
    fn playing_media_shows_pause_action_glyph() {
        let mut system = SystemSnapshot::default();
        system.media.availability = ServiceAvailability::Available;
        system.media.playback = PlaybackState::Playing;
        assert_eq!(
            StatusViews::from_snapshot(&system).media.play_pause_icon,
            Some(IconRole::MediaPause)
        );
    }

    #[test]
    fn paused_media_shows_play_action_glyph() {
        let mut system = SystemSnapshot::default();
        system.media.availability = ServiceAvailability::Available;
        system.media.playback = PlaybackState::Paused;
        assert_eq!(
            StatusViews::from_snapshot(&system).media.play_pause_icon,
            Some(IconRole::MediaPlay)
        );
    }

    #[test]
    fn clock_timer_aligns_minute_format_to_next_boundary() {
        assert_eq!(
            clock_timer_plan("%H:%M", 61_234),
            ClockTimerPlan {
                interval_ms: 60_000,
                initial_delay_ms: 58_766
            }
        );
    }

    #[test]
    fn clock_timer_uses_one_second_for_second_formats() {
        assert_eq!(clock_timer_plan("%H:%M:%S", 1_250).interval_ms, 1_000);
    }

    #[test]
    fn audio_and_network_popovers_match_html_roots() {
        assert_eq!(AUDIO_POPOVER_SIZE, Size::new(459, 460));
        assert_eq!(NETWORK_POPOVER_SIZE, Size::new(461, 453));
    }

    #[test]
    fn battery_popover_matches_html_root() {
        assert_eq!(BATTERY_POPOVER_SIZE, Size::new(300, 132));
    }

    #[test]
    fn calendar_popover_is_580_by_476() {
        assert_eq!(calendar_popover_size(), Size::new(580, 476));
    }

    #[test]
    fn shift_month_wraps_years_both_directions() {
        assert_eq!(shift_month(2026, 12, 1), (2027, 1));
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2026, 10, -12), (2025, 10));
        assert_eq!(shift_month(2026, 10, 0), (2026, 10));
    }

    #[test]
    fn october_2026_is_monday_first_with_leading_days() {
        let calendar = CalendarGrid::build_for_date(2026, 10, 2026, 10, 13).expect("valid month");
        assert_eq!(
            calendar.cells[0],
            CalendarCell {
                year: 2026,
                month: 9,
                day: 28,
                month_kind: CalendarMonth::Previous,
                today: false,
            }
        );
        assert_eq!(calendar.cells[3].day, 1);
        assert_eq!(calendar.cells[3].month_kind, CalendarMonth::Current);
        assert_eq!(calendar.cells[6].day, 4);
        // Oct 13 2026 is a Tuesday in the third week (index 15).
        assert!(calendar.cells[15].today);
        assert_eq!(calendar.cells[15].day, 13);
        assert_eq!(calendar.cells[34].year, 2026);
        assert_eq!(calendar.cells[34].month, 11);
        assert_eq!(calendar.cells[34].day, 1);
    }

    #[test]
    fn leap_february_2028_has_29_days() {
        let calendar = CalendarGrid::build_for_date(2028, 2, 2000, 1, 1).expect("valid month");
        assert_eq!(calendar.cells[1].day, 1);
        assert_eq!(calendar.cells[29].day, 29);
        assert_eq!(calendar.cells[29].month_kind, CalendarMonth::Current);
        assert_eq!(calendar.cells[30].month, 3);
        assert_eq!(calendar.cells[30].day, 1);
    }

    #[test]
    fn january_2026_rolls_back_into_december_2025() {
        let calendar = CalendarGrid::build_for_date(2026, 1, 2000, 1, 1).expect("valid month");
        assert_eq!(calendar.cells[0].year, 2025);
        assert_eq!(calendar.cells[0].month, 12);
        assert_eq!(calendar.cells[0].day, 29);
        assert_eq!(calendar.cells[2].day, 31);
        assert_eq!(calendar.cells[3].month_kind, CalendarMonth::Current);
    }
}

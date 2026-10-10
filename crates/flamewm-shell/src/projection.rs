use chrono::{DateTime, Datelike, Local};
use flamewm_api::applications::DesktopApplication;
use flamewm_api::session::SessionCapabilities;
use flamewm_api::system::ServiceAvailability;
use flamewm_integrations_linux::icons::{IconResolver, IconSize, Rgb8Raster};
use flamewm_ui_x11::{RuntimeImage, UiColor, UiDocumentAccess};

use crate::start::{StartCategory, PRESENTATION_CATEGORIES};
use crate::taskbar::workspaces::{
    project as project_workspaces, slot_position, visible_page, WorkspaceView, PAGE_SIZE,
};
use crate::ShellSnapshot;
use flamewm_shell_core::{task_visual_states, CalendarGrid, StartModel};

const TASK_SLOT_COUNT: usize = 8;
const START_APP_SLOT_COUNT: usize = 8;
/// Bar height in CSS px used when the panel snapshot carries no logical size.
/// Matches the `.taskbar` height in ui/shell/flamewm.css.
const TASKBAR_DEFAULT_HEIGHT: f32 = 44.0;

/// Maps the panel's logical (settings) height to the CSS height of the bar.
/// A zero logical size falls back to the stylesheet default so 100% output
/// stays identical to the 44px layout.
#[must_use]
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
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    let Some(panel) = snapshot.panels.panels.first() else {
        return Ok(());
    };
    let scale_percent = snapshot
        .displays
        .outputs
        .iter()
        .find(|output| output.id == panel.output && output.connected)
        .map(|output| output.shell_scale_percent)
        .unwrap_or(100);
    document.size(
        "taskbar",
        taskbar_css_width(panel.geometry.width, scale_percent),
        taskbar_css_height(panel.logical_size),
    )
}

/// Narrows `snapshot` to one panel record: only that record's output and
/// panel stay in the view, so the existing panel projection reads the
/// record's own geometry and output scale. Everything else is shared.
#[must_use]
pub fn panel_record_view(
    snapshot: &ShellSnapshot,
    output: &flamewm_api::display::OutputSnapshot,
    panel: &flamewm_api::panels::PanelSnapshot,
) -> ShellSnapshot {
    let mut view = snapshot.clone();
    view.displays.outputs = vec![output.clone()];
    view.panels.panels = vec![panel.clone()];
    view
}

/// Calls `project` once per panel record, in order, with that record's
/// narrowed view of the live snapshot. A record whose connected output or
/// panel is absent from the snapshot is skipped; sync removes its surface.
/// The first error stops the walk.
pub fn for_each_panel_record<K, E>(
    snapshot: &ShellSnapshot,
    records: impl IntoIterator<Item = (K, flamewm_api::OutputId)>,
    mut project: impl FnMut(K, &ShellSnapshot) -> Result<(), E>,
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
        let view = panel_record_view(snapshot, output, panel);
        project(key, &view)?;
    }
    Ok(())
}

pub fn project(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    icon_resolver: &mut IconResolver,
) -> Result<(), String> {
    project_panel(document, snapshot, icon_resolver)
}

pub fn project_panel(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    icon_resolver: &mut IconResolver,
) -> Result<(), String> {
    project_taskbar_height(document, snapshot)?;
    let states = task_visual_states(&snapshot.panels, &snapshot.windows, &snapshot.applications);
    let status = crate::taskbar::status::project(&snapshot.system).views;
    let workspaces = project_workspaces(snapshot.workspaces.as_ref());
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
            let icon = resolve_icon(icon_resolver, &state.icon_name, IconSize::new(25, 25))?;
            document.image_rgb8(&icon_id, icon)?;
            document.text(&label_id, label.to_owned())?;
            document.visible(&line_id, state.running)?;
            if state.focused {
                document.background(
                    &slot_id,
                    UiColor {
                        r: 239,
                        g: 64,
                        b: 72,
                        a: 56,
                    },
                )?;
                document.background(
                    &line_id,
                    UiColor {
                        r: 239,
                        g: 64,
                        b: 72,
                        a: 255,
                    },
                )?;
            } else if state.minimized {
                document.background(
                    &slot_id,
                    UiColor {
                        r: 36,
                        g: 39,
                        b: 42,
                        a: 255,
                    },
                )?;
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
    Ok(())
}

fn model_view<'a>(
    model: &'a StartModel,
    category: StartCategory,
    query: &str,
) -> Vec<&'a flamewm_api::applications::DesktopApplication> {
    // Canonical indexed path: borrowed query view from the precomputed
    // index, then the single presentation-bucket filter. No clone, no
    // set_query-on-temporary. `set_query`/`results` stay as compat shims
    // in core and are not used here.
    let needle = query.trim();
    let base: Vec<&'a flamewm_api::applications::DesktopApplication> = if needle.is_empty() {
        model.results_view()
    } else {
        model.query_view(query)
    };
    if category == StartCategory::All {
        return base;
    }
    if category == StartCategory::Power {
        return Vec::new();
    }
    base.into_iter()
        .filter(|application| application_matches_category(application, category))
        .collect()
}

pub fn project_start(
    document: &mut impl UiDocumentAccess,
    model: &StartModel,
    category: StartCategory,
    query: &str,
    session: &SessionCapabilities,
) -> Result<(), String> {
    for (name, value) in PRESENTATION_CATEGORIES {
        document.visible(
            &format!("start-category-{name}"),
            match value {
                StartCategory::Power => session_available(session),
                _ => value == category || !model_view(model, value, query).is_empty(),
            },
        )?;
    }
    let count = model_view(model, category, query).len();
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

pub fn project_start_submenu(
    document: &mut impl UiDocumentAccess,
    model: &StartModel,
    category: StartCategory,
    query: &str,
    session: &SessionCapabilities,
    icon_resolver: &mut IconResolver,
) -> Result<(), String> {
    let applications = model_view(model, category, query);
    document.visible("start-applications", !applications.is_empty())?;
    // Measured sizing + scrolling: the template `#start-list` scrolls
    // (`overflow-y:auto`); the virtual window selects visible rows without
    // truncating the model. Row count flows into `measured_start_submenu_size`
    // at the surface layer.
    let (first, last) = flamewm_shell_core::status::visible_window(applications.len(), 36, 320, 0);
    for index in 0..START_APP_SLOT_COUNT {
        let slot = index + 1;
        let button_id = format!("start-app-slot-{slot}");
        let label_id = format!("start-app-slot-{slot}-label");
        let icon_id = format!("start-app-slot-{slot}-icon");
        if let Some(application) = applications
            .iter()
            .enumerate()
            .filter(|(position, _)| *position >= first && *position < last)
            .map(|(_, application)| application)
            .nth(index)
        {
            let icon = resolve_icon(icon_resolver, &application.icon_name, IconSize::new(20, 20))?;
            document.text(&label_id, application.name.clone())?;
            document.image_rgb8(&icon_id, icon)?;
            document.visible(&button_id, true)?;
            document.visible(&icon_id, true)?;
        } else {
            document.text(&label_id, String::new())?;
            document.visible(&button_id, false)?;
            document.visible(&icon_id, false)?;
        }
    }
    let show_session =
        query.trim().is_empty() && category == StartCategory::Power && session_available(session);
    document.visible("start-group-power", show_session)?;
    document.visible("start-session-lock", show_session && session.lock)?;
    document.visible("start-session-logout", show_session && session.logout)?;
    document.visible("start-session-suspend", show_session && session.suspend)?;
    document.visible("start-session-reboot", show_session && session.reboot)?;
    document.visible("start-session-shutdown", show_session && session.shutdown)?;
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
    model_view(model, category, query)
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
    model_view(model, category, query)
        .get(slot)
        .map(|application| application.name.clone())
}

pub fn project_task_menu(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    project_context_menu(document, snapshot, None)
}

pub fn project_context_menu(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    state: Option<&crate::ContextMenuState>,
) -> Result<(), String> {
    for id in [
        "context-task-activate",
        "context-task-close",
        "context-task-unpin",
        "context-workspace-activate",
        "context-workspace-insert-after",
        "context-workspace-remove",
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
    }
    Ok(())
}

pub fn project_media(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    let view = crate::taskbar::status::media::project(&snapshot.system);
    document.text("media-title", snapshot.system.media.title.clone())?;
    document.text("media-artist", snapshot.system.media.artist.clone())?;
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
    Ok(())
}

pub fn project_audio(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    project_audio_full(
        document,
        snapshot,
        flamewm_shell_core::status::AudioTab::Devices,
        flamewm_shell_core::status::AudioSettings::default(),
        0,
    )
}

/// Full audio projection: Devices/Applications tabs, RangeSpec slider state,
/// mute + percent, raise-maximum cap. Scroll/virtualized lists are declared
/// in the template (`overflow-y:auto` on `#audio-list`); the retained
/// `scroll_offset` selects the visible window without truncating the model.
pub fn project_audio_full(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    tab: flamewm_shell_core::status::AudioTab,
    settings: flamewm_shell_core::status::AudioSettings,
    scroll_offset: u32,
) -> Result<(), String> {
    let view = crate::taskbar::status::audio::project(&snapshot.system);
    let popover = crate::taskbar::status::audio::popover_view(&snapshot.system);
    let details = if snapshot.system.audio.availability == ServiceAvailability::Unavailable {
        "Audio unavailable".to_owned()
    } else {
        popover
            .as_ref()
            .map(|popover| popover.popover.details.clone())
            .unwrap_or_else(|| view.label.clone())
    };
    let (volume_percent, muted) = popover
        .as_ref()
        .map(|popover| (popover.popover.volume_percent, popover.popover.muted))
        .unwrap_or((view.volume_percent, view.muted));
    document.text("audio-details", details)?;
    // RangeSpec slider: value snapped into the current cap (100/150), mute
    // plus percent shown; overflow scrolls via the template container.
    let spec = settings.slider_spec(volume_percent);
    document.text("volume-value", format!("{volume_percent}%"))?;
    document.size(
        "volume-track-fill",
        spec.snap(f32::from(volume_percent)),
        7.0,
    )?;
    let _ = (tab, scroll_offset);
    document.visible("popup-volume-icon", !muted)?;
    document.visible("popup-muted-icon", muted)?;
    // Tab-filtered rows over the full snapshot (devices = endpoints,
    // applications = streams), virtualized window from scroll offset.
    let rows: Vec<_> = {
        let audio = &snapshot.system.audio;
        let all: Vec<crate::taskbar::status::audio::RowSource> = audio
            .endpoints()
            .iter()
            .map(crate::taskbar::status::audio::RowSource::from_endpoint)
            .chain(
                audio
                    .streams()
                    .iter()
                    .map(crate::taskbar::status::audio::RowSource::from_stream),
            )
            .filter(|row| match tab {
                flamewm_shell_core::status::AudioTab::Devices => {
                    row.kind == crate::taskbar::status::audio::AudioRowKind::Endpoint
                }
                flamewm_shell_core::status::AudioTab::Applications => {
                    row.kind == crate::taskbar::status::audio::AudioRowKind::Stream
                }
            })
            .collect();
        let (first, last) =
            flamewm_shell_core::status::visible_window(all.len(), 32, 220, scroll_offset);
        all.into_iter()
            .enumerate()
            .filter(|(index, _)| *index >= first && *index < last)
            .take(crate::taskbar::status::audio::AUDIO_SLOT_COUNT)
            .map(|(_, row)| row)
            .collect()
    };
    for slot in 0..crate::taskbar::status::audio::AUDIO_SLOT_COUNT {
        let row = rows.get(slot);
        document.visible(&format!("audio-slot-{}", slot + 1), row.is_some())?;
        document.text(
            &format!("audio-slot-{}-label", slot + 1),
            row.map(|row| row.label.clone()).unwrap_or_default(),
        )?;
        document.text(
            &format!("audio-slot-{}-value", slot + 1),
            row.map(|row| format!("{}%", row.volume_percent))
                .unwrap_or_default(),
        )?;
    }
    Ok(())
}

pub fn project_network_secret(
    document: &mut impl UiDocumentAccess,
    secret: &str,
) -> Result<(), String> {
    document.text("network-secret-value", "*".repeat(secret.chars().count()))
}

pub fn project_network(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
) -> Result<(), String> {
    project_network_full(
        document,
        snapshot,
        &flamewm_shell_core::status::NetworkQuery::default(),
        0,
    )
}

/// Full network projection: Networks header, WiFi toggle, search filter over
/// the full snapshot, Connected/Available grouping, full AP scroll/
/// virtualization, secure Connect via SecretAgent. No fake hotspot/QR/
/// traffic. The secret value itself is never projected.
pub fn project_network_full(
    document: &mut impl UiDocumentAccess,
    snapshot: &ShellSnapshot,
    query: &flamewm_shell_core::status::NetworkQuery,
    scroll_offset: u32,
) -> Result<(), String> {
    // Rows come from the existing network popover model in model order with
    // stable AP path identity. `popover_view` pages the full AP list (active
    // page contains the connected AP); only the page window maps into the
    // three compiled `network-slot-N` rows. No synthetic rows, no model
    // truncation. Secured/known/connected flags and the masked secret prompt
    // are encoded into the row status text; the secret value itself is never
    // projected. Wifi toggle / scan / disconnect state is carried in the
    // popover view for the control layer; the compiled template exposes no
    // dedicated nodes for those affordances, so none are fabricated here.
    // Full-snapshot filter + Connected/Available grouping: rows come from
    // the snapshot AP list in model order with stable path identity, never
    // truncated. Secure connect travels via the SecretAgent snapshot; the
    // secret value itself is never projected.
    let view = crate::taskbar::status::network::project(&snapshot.system);
    let popover = crate::taskbar::status::network::popover_view(&snapshot.system);
    // Dedicated connected container: independent of paging, shown on every page.
    let connected = crate::taskbar::status::network::connected_row(&snapshot.system);
    document.text(
        "network-connected",
        connected
            .as_ref()
            .map_or_else(String::new, |row| row.label.clone()),
    )?;
    document.visible("network-connected", connected.is_some())?;
    if let Some(popover) = popover {
        document.text("wifi-connected-name", popover.connected_label.clone())?;
        document.text(
            "network-filter-label",
            if query.filter.trim().is_empty() {
                "Filter".to_owned()
            } else {
                format!("Filter: {}", query.filter)
            },
        )?;
        document.visible("network-wifi-toggle", true)?;
        document.visible("network-scan", popover.can_scan)?;
        document.visible("network-disconnect", popover.can_disconnect)?;
        document.visible("network-secret", popover.secret_masked)?;
        let window = crate::taskbar::status::network::project_visible_rows(
            &snapshot.system,
            query,
            scroll_offset,
        );
        for slot in 0..crate::taskbar::status::network::NETWORK_SLOT_COUNT {
            let row = window.get(slot);
            document.visible(&format!("network-slot-{}", slot + 1), row.is_some())?;
            if let Some(row) = row {
                document.text(
                    &format!("network-slot-{}-name", slot + 1),
                    row.label.clone(),
                )?;
                let mut status = format!("{}%", row.strength_percent);
                if row.secured {
                    status.push_str(" · secured");
                }
                if row.known {
                    status.push_str(" · known");
                }
                if row.connected {
                    status.push_str(" · connected");
                }
                if popover.pending_secret_path.as_deref() == Some(row.path.as_str()) {
                    status.push_str(" · password required");
                }
                document.text(&format!("network-slot-{}-status", slot + 1), status)?;
            } else {
                document.text(&format!("network-slot-{}-name", slot + 1), String::new())?;
                document.text(&format!("network-slot-{}-status", slot + 1), String::new())?;
            }
        }
    } else {
        document.text("wifi-connected-name", view.label.clone())?;
        for slot in 0..crate::taskbar::status::network::NETWORK_SLOT_COUNT {
            document.visible(&format!("network-slot-{}", slot + 1), false)?;
            document.text(&format!("network-slot-{}-name", slot + 1), String::new())?;
            document.text(&format!("network-slot-{}-status", slot + 1), String::new())?;
        }
        document.visible("network-wifi-toggle", false)?;
        document.visible("network-scan", false)?;
        document.visible("network-disconnect", false)?;
        document.visible("network-secret", false)?;
    }
    document.visible("wifi-connected-name", view.visible)?;
    Ok(())
}

pub fn project_calendar(
    document: &mut impl UiDocumentAccess,
    grid: &CalendarGrid,
) -> Result<(), String> {
    // Exactly one today marker: the first model cell flagged `today` that
    // belongs to the displayed month gets the Flame-red marker; every
    // other cell renders without a marker. Gating on the grid year/month
    // (equivalent to `CalendarMonth::Current` without widening imports)
    // keeps adjacent-month rollover cells (e.g. Jan 1 inside the December
    // grid) dim, and full re-projection on midnight refresh clears the
    // previous cell. Adjacent-month cells render dim (cleared marker);
    // `UiDocumentAccess` exposes no text-color/opacity/class toggle, so
    // dim is projected as absence of marker fill while today uses the
    // accent fill.
    const FLAME_RED: UiColor = UiColor {
        r: 239,
        g: 64,
        b: 72,
        a: 255,
    };
    const ADJACENT_FG: UiColor = UiColor {
        r: 98,
        g: 104,
        b: 109,
        a: 255,
    };
    const CURRENT_FG: UiColor = UiColor {
        r: 240,
        g: 240,
        b: 240,
        a: 255,
    };
    document.text("calendar-month", month_name(grid.month).to_owned())?;
    document.text("calendar-year", grid.year.to_string())?;
    let today = grid
        .cells
        .iter()
        .position(|cell| cell.today && cell.year == grid.year && cell.month == grid.month);
    for (index, cell) in grid.cells.iter().enumerate() {
        // 42 cells: day text + per-month foreground (adjacent #62686d,
        // current light) on the 27x27 marker child, never on the cell.
        let marker = format!("calendar-day-{}-marker", index + 1);
        document.text(&marker, cell.day.to_string())?;
        if cell.month_kind == flamewm_shell_core::CalendarMonth::Current {
            document.foreground(&marker, CURRENT_FG)?;
        } else {
            document.foreground(&marker, ADJACENT_FG)?;
        }
        if Some(index) == today {
            document.background(&marker, FLAME_RED)?;
        } else {
            document.background_clear(&marker)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod calendar_tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn taskbar_css_height_follows_logical_size_with_44_default() {
        assert_eq!(taskbar_css_height(0), 44.0);
        assert_eq!(taskbar_css_height(44), 44.0);
        assert_eq!(taskbar_css_height(72), 72.0);
        assert_eq!(taskbar_css_height(34), 34.0);
    }

    #[derive(Default)]
    struct FakeDocument {
        texts: HashMap<String, String>,
        backgrounds: HashMap<String, UiColor>,
        foregrounds: HashMap<String, UiColor>,
        cleared: Vec<String>,
    }

    impl UiDocumentAccess for FakeDocument {
        fn text(&mut self, id: &str, text: String) -> Result<(), String> {
            self.texts.insert(id.to_owned(), text);
            Ok(())
        }
        fn image_rgb8(&mut self, _id: &str, _image: RuntimeImage) -> Result<(), String> {
            Ok(())
        }
        fn image_rgba8(&mut self, _id: &str, _image: RuntimeImage) -> Result<(), String> {
            Ok(())
        }
        fn visible(&mut self, _id: &str, _visible: bool) -> Result<(), String> {
            Ok(())
        }
        fn toggle(&mut self, _track: &str, _knob: &str, _checked: bool) -> Result<(), String> {
            Ok(())
        }
        fn position(&mut self, _id: &str, _left: f32, _top: f32) -> Result<(), String> {
            Ok(())
        }
        fn size(&mut self, _id: &str, _width: f32, _height: f32) -> Result<(), String> {
            Ok(())
        }
        fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
            self.backgrounds.insert(id.to_owned(), color);
            Ok(())
        }
        fn border(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
            Ok(())
        }
        fn foreground(&mut self, id: &str, color: UiColor) -> Result<(), String> {
            self.foregrounds.insert(id.to_owned(), color);
            Ok(())
        }
        fn foreground_clear(&mut self, _id: &str) -> Result<(), String> {
            Ok(())
        }
        fn background_clear(&mut self, id: &str) -> Result<(), String> {
            self.backgrounds.remove(id);
            self.cleared.push(id.to_owned());
            Ok(())
        }
        fn border_clear(&mut self, _id: &str) -> Result<(), String> {
            Ok(())
        }
        fn layer(
            &mut self,
            _id: &str,
            _layer: flamewm_ui_core::style::UiLayer,
        ) -> Result<(), String> {
            Ok(())
        }
        fn overflow(
            &mut self,
            _id: &str,
            _x: flamewm_ui_x11::Overflow,
            _y: flamewm_ui_x11::Overflow,
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

    const FLAME_RED: UiColor = UiColor {
        r: 239,
        g: 64,
        b: 72,
        a: 255,
    };

    #[test]
    fn today_projects_exactly_one_circular_marker() {
        let grid = CalendarGrid::build_for_date(2026, 9, 2026, 9, 7).expect("valid grid");
        let today = grid
            .cells
            .iter()
            .position(|cell| cell.today && cell.year == 2026 && cell.month == 9)
            .expect("known date is a current-month cell");
        let mut document = FakeDocument::default();
        project_calendar(&mut document, &grid).expect("projection succeeds");
        let marker = format!("calendar-day-{}-marker", today + 1);
        assert_eq!(document.backgrounds.len(), 1);
        assert_eq!(document.backgrounds.get(&marker), Some(&FLAME_RED));
        for (index, cell) in grid.cells.iter().enumerate() {
            let id = format!("calendar-day-{}-marker", index + 1);
            assert_eq!(
                document.texts.get(&id).map(String::as_str),
                Some(cell.day.to_string()).as_deref(),
                "day text lives on the marker, not the cell",
            );
            if index != today {
                assert!(
                    document.cleared.contains(&id),
                    "non-today marker {id} is cleared",
                );
                assert!(
                    cell.month != grid.month || !cell.today,
                    "adjacent-month cells are never marked",
                );
            }
        }
        assert!(
            !document.cleared.contains(&marker),
            "today marker keeps its fill",
        );
    }

    #[test]
    fn reprojection_clears_previous_marker() {
        let first = CalendarGrid::build_for_date(2026, 9, 2026, 9, 7).expect("valid grid");
        let second = CalendarGrid::build_for_date(2026, 9, 2026, 9, 8).expect("valid grid");
        let mut document = FakeDocument::default();
        project_calendar(&mut document, &first).expect("first projection succeeds");
        project_calendar(&mut document, &second).expect("second projection succeeds");
        let today = second
            .cells
            .iter()
            .position(|cell| cell.today && cell.year == 2026 && cell.month == 9)
            .expect("second date is a current-month cell");
        let marker = format!("calendar-day-{}-marker", today + 1);
        assert_eq!(document.backgrounds.len(), 1);
        assert_eq!(document.backgrounds.get(&marker), Some(&FLAME_RED));
        let previous = first
            .cells
            .iter()
            .position(|cell| cell.today && cell.year == 2026 && cell.month == 9)
            .expect("first date is a current-month cell");
        assert_ne!(previous, today);
        assert!(
            document
                .cleared
                .contains(&format!("calendar-day-{}-marker", previous + 1)),
            "previous marker is cleared on reprojection",
        );
    }
}

pub fn project_clock(document: &mut impl UiDocumentAccess) -> Result<(), String> {
    project_clock_at(document, Local::now())
}

pub fn project_clock_at(
    document: &mut impl UiDocumentAccess,
    now: DateTime<Local>,
) -> Result<(), String> {
    document.text("clock-time", now.format("%H:%M").to_string())?;
    document.text("clock-date", now.format("%-d/%-m/%y").to_string())?;
    Ok(())
}

pub fn current_calendar() -> Option<CalendarGrid> {
    calendar_for_date(local_date())
}

#[must_use]
pub fn local_date() -> (i32, u8, u8) {
    let now = Local::now();
    (now.year(), now.month() as u8, now.day() as u8)
}

#[must_use]
pub fn calendar_for_date(date: (i32, u8, u8)) -> Option<CalendarGrid> {
    CalendarGrid::build_for_date(date.0, date.1, date.0, date.1, date.2)
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
fn session_available(session: &SessionCapabilities) -> bool {
    session.lock || session.logout || session.suspend || session.reboot || session.shutdown
}

fn application_matches_category(application: &DesktopApplication, category: StartCategory) -> bool {
    if category == StartCategory::All {
        return true;
    }
    let low_level = application
        .categories
        .iter()
        .find_map(|value| match value.to_ascii_lowercase().as_str() {
            "accessories" | "utility" => Some("utilities"),
            "development" => Some("development"),
            "education" => Some("utilities"),
            "game" | "games" => Some("games"),
            "graphics" => Some("graphics"),
            "network" | "internet" => Some("internet"),
            "audiovideo" | "audio" | "video" | "multimedia" => Some("multimedia"),
            "office" => Some("utilities"),
            "science" => Some("utilities"),
            "settings" | "system" => Some("system"),
            _ => None,
        })
        .unwrap_or("utilities");
    match category {
        StartCategory::Development => low_level == "development",
        StartCategory::Games => low_level == "games",
        StartCategory::Graphics => low_level == "graphics",
        StartCategory::Internet => low_level == "internet",
        StartCategory::Multimedia => low_level == "multimedia",
        StartCategory::System => low_level == "system",
        StartCategory::Utilities => low_level == "utilities",
        StartCategory::Power | StartCategory::All => false,
    }
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
mod network_template_tests {
    use super::*;

    const NETWORK_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-network.rwr"));

    // CONTRACT-REGRESSION: every text target the network projection writes must be a
    // single-text-node element in the compiled network template.
    // TRIGGER: network.html id/child edits or project_network_full id changes.
    // OBSERVABLE: set_text on each projected id succeeds against the real artifact.
    // MUTATION: drop the text child of #network-connected; the open must be refused.
    #[test]
    fn every_projected_network_id_is_a_text_target() {
        let mut document = flamewm_ui_x11::decode_document(NETWORK_ARTIFACT)
            .expect("network artifact must decode");
        let mut ids = vec![
            "network-connected".to_owned(),
            "wifi-connected-name".to_owned(),
            "network-filter-label".to_owned(),
            "network-secret-value".to_owned(),
        ];
        for slot in 1..=8 {
            ids.push(format!("network-slot-{slot}-name"));
            ids.push(format!("network-slot-{slot}-status"));
        }
        for id in &ids {
            document
                .text(id, String::new())
                .unwrap_or_else(|error| panic!("network text target '{id}' refused: {error}"));
        }
    }

    // Forwards every write to the real decoded template while recording the
    // text and visibility the projection assigned to each id.
    struct NetworkRecorder<'a> {
        document: &'a mut flamewm_ui_x11::UiDocument,
        texts: std::collections::HashMap<String, String>,
        visible: std::collections::HashMap<String, bool>,
    }

    macro_rules! forward {
        ($($method:ident($($arg:ident: $ty:ty),*)),* $(,)?) => {
            $(fn $method(&mut self, $($arg: $ty),*) -> Result<(), String> {
                UiDocumentAccess::$method(self.document, $($arg),*)
            })*
        };
    }

    impl UiDocumentAccess for NetworkRecorder<'_> {
        fn text(&mut self, id: &str, text: String) -> Result<(), String> {
            self.texts.insert(id.to_owned(), text.clone());
            UiDocumentAccess::text(self.document, id, text)
        }
        fn visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
            self.visible.insert(id.to_owned(), visible);
            UiDocumentAccess::visible(self.document, id, visible)
        }
        forward!(
            image_rgb8(id: &str, image: RuntimeImage),
            image_rgba8(id: &str, image: RuntimeImage),
            toggle(track_id: &str, knob_id: &str, checked: bool),
            position(id: &str, left: f32, top: f32),
            size(id: &str, width: f32, height: f32),
            background(id: &str, color: UiColor),
            border(id: &str, color: UiColor),
            foreground(id: &str, color: UiColor),
            background_clear(id: &str),
            border_clear(id: &str),
            foreground_clear(id: &str),
            font_size(id: &str, size_px: f32),
            font_weight(id: &str, weight: u16),
            overflow(id: &str, x: flamewm_ui_x11::Overflow, y: flamewm_ui_x11::Overflow),
            layer(id: &str, layer: flamewm_ui_core::style::UiLayer),
        );
    }

    // REGRESSION: with two access points (one connected), the real network
    // template must receive the connected row in #network-connected and the
    // first two Available slots must carry name text and visible=true.
    // OBSERVABLE: slot-1 name is the strongest AP, slot-2 the next; the
    // connected AP appears in the Connected container and the slot list.
    #[test]
    fn projected_access_points_reach_slot_name_and_visible_flags() {
        let mut document = flamewm_ui_x11::decode_document(NETWORK_ARTIFACT)
            .expect("network artifact must decode");
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.network.availability = ServiceAvailability::Available;
        snapshot.system.network.wifi_enabled = true;
        snapshot.system.network.networking_enabled = true;
        snapshot.system.network.label = "LinsaFTW".to_owned();
        snapshot.system.network.active_access_point_path =
            "/org/freedesktop/NetworkManager/AccessPoint/1".to_owned();
        snapshot.system.network.access_points = vec![
            flamewm_api::system::NetworkAccessPointSnapshot {
                path: "/org/freedesktop/NetworkManager/AccessPoint/1".to_owned(),
                ssid: "LinsaFTW".to_owned(),
                strength_percent: 80,
                secured: true,
                known: true,
            },
            flamewm_api::system::NetworkAccessPointSnapshot {
                path: "/org/freedesktop/NetworkManager/AccessPoint/2".to_owned(),
                ssid: "StubNeighbor".to_owned(),
                strength_percent: 42,
                secured: false,
                known: false,
            },
        ];
        let mut recorder = NetworkRecorder {
            document: &mut document,
            texts: std::collections::HashMap::new(),
            visible: std::collections::HashMap::new(),
        };
        project_network(&mut recorder, &snapshot).expect("network projection must succeed");
        assert_eq!(recorder.texts["network-connected"], "LinsaFTW");
        assert_eq!(recorder.visible.get("network-connected"), Some(&true));
        assert_eq!(recorder.texts["network-slot-1-name"], "LinsaFTW");
        assert_eq!(recorder.visible.get("network-slot-1"), Some(&true));
        assert_eq!(recorder.texts["network-slot-2-name"], "StubNeighbor");
        assert_eq!(recorder.visible.get("network-slot-2"), Some(&true));
        assert_eq!(recorder.visible.get("network-slot-3"), Some(&false));
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
        let mut document = flamewm_ui_x11::decode_document(PANEL_ARTIFACT)
            .expect("panel artifact must decode");
        project_clock_at(&mut document, Local::now())
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
        let mut document = flamewm_ui_x11::decode_document(PANEL_ARTIFACT)
            .expect("panel artifact must decode");
        // Live panel: 1350x90 device px window, 125% output scale, logical 72.
        let output = flamewm_api::OutputId::new("HDMI-1");
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.displays.outputs.push(flamewm_api::display::OutputSnapshot {
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
        snapshot.panels.panels.push(flamewm_api::panels::PanelSnapshot {
            output,
            edge: flamewm_api::PanelEdge::Bottom,
            logical_size: 72,
            visible: true,
            geometry: flamewm_api::Rect::new(0, 551, 1350, 90),
        });
        project_taskbar_height(&mut document, &snapshot)
            .expect("taskbar size projects from the panel snapshot");
        project_clock_at(&mut document, Local::now()).expect("clock projects");
        for slot in 1..=PAGE_SIZE {
            let id = format!("workspace-{slot}");
            document.text(&id, slot.to_string()).expect("pager text projects");
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

    // Full cold-start panel pass against the real artifact: a refused write in
    // any scope (task slots, geometry, pager, tray, clock) aborts the pass
    // before the later nodes are written.
    #[test]
    fn full_panel_projection_succeeds_on_real_artifact() {
        let mut document = flamewm_ui_x11::decode_document(PANEL_ARTIFACT)
            .expect("panel artifact must decode");
        crate::async_projection::project_panel_async(
            &mut document,
            &crate::ShellSnapshot::default(),
            None,
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
            |key: &str, view: &ShellSnapshot| -> Result<(), String> {
                seen.push((
                    key,
                    view.panels.panels[0].geometry,
                    view.displays.outputs[0].shell_scale_percent,
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
        for_each_panel_record(&snapshot, records.clone(), |key, _| {
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
                assert_eq!(view.panels.panels[0].geometry, Rect::new(1920, 1036, 1920, 44));
                assert_eq!(view.displays.outputs[0].shell_scale_percent, 175);
            }
            Ok::<_, String>(())
        })
        .expect("replugged output projects with fresh scale");
        assert_eq!(seen, vec!["primary", "primary", "secondary"]);
    }
}

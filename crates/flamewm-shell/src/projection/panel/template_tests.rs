use super::*;

use flamewm_api::system::{PowerSnapshot, PowerState, ServiceAvailability};
use flamewm_api::workspace::WorkspaceSnapshot;

use crate::taskbar::workspaces::cell_index;

const PANEL_ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-panel.rwr"));

fn decode_panel() -> flamewm_ui_x11::UiDocument {
    flamewm_ui_x11::decode_document(PANEL_ARTIFACT).expect("panel artifact must decode")
}

// Every separated-mode capsule shares one top edge (the bar's 1 px border) and one
// border-box height; the start capsule is square; children fill the inner box.
// MUTATION: restore `height:100%` on `.capsule` or `.tray-group` (resolves against the
// 44 px border box, so the capsule grows to y=0, h=44 over the bar border).
#[test]
fn separate_mode_nodes_share_one_top_and_height() {
    let _scale = taskbar_scale::test_lock();
    taskbar_scale::set_factor(1.0);
    for logical in [34_u16, 44, 72] {
        let mut doc = decode_panel();
        let factor = taskbar_scale::scale_for_height(logical);
        taskbar_scale::set_factor(factor);
        let (bar_w, bar_h) = (1350.0_f32, taskbar_css_height(logical));
        doc.size("taskbar", bar_w, bar_h).expect("taskbar sizes");
        project_taskbar_metrics(&mut doc).expect("metrics project");
        project_task_slots_extent(&mut doc, 3).expect("slots extent");
        project_pager(&mut doc, Some(&pager_snapshot(4, 0))).expect("pager projects");
        doc.visible("workspace-pager", true).expect("pager visible");
        project_element_backgrounds(&mut doc, TaskbarItems::default(), 3, true, true)
            .expect("backgrounds project");
        let rect = |id: &str| {
            doc.layout_box_by_id(id, (bar_w, bar_h))
                .unwrap_or_else(|| panic!("h={logical}: node '{id}' has no layout box"))
        };
        let node = taskbar_scale::node_height(factor);
        let inner = taskbar_scale::inner_node_height(factor);
        let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
        for id in [
            "start-capsule",
            "task-slots",
            "workspace-pager",
            "tray-group",
            "clock-capsule",
        ] {
            let (_, y, _, h) = rect(id);
            assert!(
                close(y, 1.0) && close(h, node),
                "h={logical}: {id} y={y} h={h}, expected y=1 h={node}"
            );
        }
        let (_, _, start_w, start_h) = rect("start-capsule");
        assert!(
            close(start_w, start_h),
            "h={logical}: start capsule {start_w}x{start_h} not square"
        );
        let (_, start_button_y, start_button_w, start_button_h) = rect("task-start");
        assert!(
            close(start_button_w, inner) && close(start_button_h, inner),
            "h={logical}: start button is not the inner square"
        );
        assert!(
            close(start_button_y, 2.0),
            "h={logical}: start button off-centre"
        );
        let (_, _, _, slot_h) = rect("task-slot-1");
        let (_, clock_y, _, clock_h) = rect("clock-button");
        assert!(
            close(slot_h, inner) && close(clock_h, inner) && close(clock_y, 2.0),
            "h={logical}: slot {slot_h} / clock {clock_h}@{clock_y} must fill the inner box"
        );
    }
}

// REGRESSION: the bar must paint edge to edge at every output scale and
// `taskbarHeight`; a device-pixel width or a narrower bar leaves the classic
// right edge open. The panel lays out in CSS px at `device_w / scale`, so the
// bar sized with `taskbar_css_width` is exactly one viewport wide.
// MUTATION: size the bar from unscaled device px, or skip the `taskbar` size.
#[test]
fn taskbar_bar_spans_the_whole_viewport_at_every_scale_and_height() {
    let device_width = 1280_i32;
    for percent in [100_u16, 125, 150] {
        for logical in [34_u16, 44, 52, 72] {
            let mut doc = decode_panel();
            let width = taskbar_css_width(device_width, percent);
            let height = taskbar_css_height(logical);
            doc.size("taskbar", width, height).expect("taskbar sizes");
            let (x, y, w, h) = doc
                .layout_box_by_id("taskbar", (width, height))
                .unwrap_or_else(|| panic!("{percent}% h={logical}: bar has no layout box"));
            let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
            assert!(
                close(x, 0.0) && close(y, 0.0) && close(w, width) && close(h, height),
                "{percent}% h={logical}: bar ({x}, {y}, {w}, {h}) must fill ({width}, {height})"
            );
        }
    }
}

// REGRESSION: the classic top edge is one 1 px line across the whole panel viewport, not
// a contour around the bar's inset border box (whose side/bottom edges left the right end
// of the bar looking cut off). The outline starts at the viewport origin.
// MUTATION: move the outline inside `#taskbar`, or give it a border/inset.
#[test]
fn classic_outline_spans_the_whole_viewport_top_edge() {
    let viewport = (1280.0_f32, 52.0_f32);
    let doc = decode_panel();
    let (x, y, w, h) = doc
        .layout_box_by_id("taskbar-outline", viewport)
        .expect("panel artifact carries the classic top outline");
    assert_eq!((x, y, w, h), (0.0, 0.0, 1280.0, 1.0));
}

// REGRESSION: the running/focused line must stay centered under its icon at
// every `taskbarHeight` and output scale. The stylesheet offsets are authored
// for the 44 px bar, so only the scaled projection keeps both renderers equal.
// MUTATION: keep the authored 7 px/28 px offsets, or skip `project_task_line`.
#[test]
fn task_line_is_centered_under_its_icon_at_every_scale_and_height() {
    let _scale = taskbar_scale::test_lock();
    let device_width = 1280_i32;
    for percent in [100_u16, 125, 150] {
        for logical in [34_u16, 44, 52, 72] {
            let mut doc = decode_panel();
            taskbar_scale::set_factor(taskbar_scale::scale_for_height(logical));
            let width = taskbar_css_width(device_width, percent);
            let height = taskbar_css_height(logical);
            doc.size("taskbar", width, height).expect("taskbar sizes");
            project_taskbar_metrics(&mut doc).expect("metrics project");
            project_task_slots_extent(&mut doc, 1).expect("slots extent");
            doc.visible("task-slot-1", true).expect("slot visible");
            project_task_line(&mut doc, "task-slot-1-line", false).expect("line projects");
            let rect = |id: &str| {
                doc.layout_box_by_id(id, (width, height))
                    .unwrap_or_else(|| panic!("{percent}% h={logical}: '{id}' has no box"))
            };
            let (icon_x, _, icon_w, _) = rect("task-slot-1-icon");
            let (line_x, _, line_w, _) = rect("task-slot-1-line");
            let (icon_center, line_center) = (icon_x + icon_w / 2.0, line_x + line_w / 2.0);
            assert!(
                (icon_center - line_center).abs() < 0.01,
                "{percent}% h={logical}: line centre {line_center} != icon centre {icon_center}"
            );
            assert!(
                (line_w - scaled(TASK_LINE_WIDTH)).abs() < 1e-3,
                "{percent}% h={logical}: line width {line_w} != scaled {TASK_LINE_WIDTH}"
            );
        }
    }
    taskbar_scale::set_factor(1.0);
}

fn power_snapshot(
    available: ServiceAvailability,
    present: bool,
    percent: u8,
    state: PowerState,
) -> PowerSnapshot {
    PowerSnapshot {
        available,
        present,
        percent,
        state,
        cpu_percent: None,
        power_rate_mw: None,
    }
}

fn glyph_node_ids(prefix: &str) -> Vec<String> {
    use crate::taskbar::status::battery::BATTERY_GLYPHS;
    BATTERY_GLYPHS
        .iter()
        .flat_map(|glyph| {
            [
                format!("{prefix}{}", glyph.id_suffix()),
                format!("{prefix}{}-charging", glyph.id_suffix()),
            ]
        })
        .collect()
}

fn box_width(document: &flamewm_ui_x11::UiDocument, id: &str, viewport: (f32, f32)) -> f32 {
    document
        .layout_box_by_id(id, viewport)
        .map_or(0.0, |rect| rect.2)
}

fn lit_ids<'a>(
    document: &flamewm_ui_x11::UiDocument,
    ids: &'a [String],
    viewport: (f32, f32),
) -> Vec<&'a str> {
    ids.iter()
        .map(String::as_str)
        .filter(|id| box_width(document, id, viewport) > 0.0)
        .collect()
}

// CONTRACT-REGRESSION: the clock and workspace pager text targets and the
// tray visibility targets written by the panel projection must exist in the
// compiled panel template. A refused write aborts the scoped panel pass
// after the Start button, which is the "only Start renders" failure.
// TRIGGER: panel.html id/child edits or project_clock / project_workspaces changes.
// OBSERVABLE: project_clock_at and each pager/tray target succeed on the real artifact.
// MUTATION: drop the text child of #clock-time; project_clock_at must be refused.
#[test]
fn every_projected_panel_clock_and_pager_target_is_writable() {
    let mut document = decode_panel();
    project_clock_at(&mut document, Local::now(), ClockDisplay::default())
        .unwrap_or_else(|error| panic!("panel clock refused: {error}"));
    for slot in 1..=PAGER_CELLS {
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
    let _scale = taskbar_scale::test_lock();
    taskbar_scale::set_factor(1.0);
    let mut document = decode_panel();
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
    project_clock_at(&mut document, Local::now(), ClockDisplay::default()).expect("clock projects");
    for slot in 1..=PAGER_CELLS {
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

fn pager_snapshot(count: usize, active_index: usize) -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        revision: 1,
        count,
        active_index,
        last_index: None,
        names: Vec::new(),
    }
}

#[test]
fn single_workspace_has_no_pager_capsule_and_multiple_workspaces_restore_it() {
    let _scale = taskbar_scale::test_lock();
    taskbar_scale::set_factor(1.0);
    let items = TaskbarItems::default();
    let single = pager_snapshot(1, 0);
    let multiple = pager_snapshot(2, 0);
    assert!(!workspace_pager_visible(items, None));
    assert!(!workspace_pager_visible(items, Some(&single)));
    assert!(workspace_pager_visible(items, Some(&multiple)));
    assert!(!workspace_pager_visible(
        TaskbarItems {
            workspaces: false,
            ..items
        },
        Some(&multiple)
    ));

    // Reproject both transitions into the same document. The empty pager
    // must not leave a separate-mode glass outline or a layout gap.
    let mut document = decode_panel();
    for (snapshot, expected) in [(&single, false), (&multiple, true), (&single, false)] {
        project_pager(&mut document, Some(snapshot)).expect("pager projects");
        document
            .visible(
                "workspace-pager",
                workspace_pager_visible(items, Some(snapshot)),
            )
            .expect("pager visibility projects");
        let (_, _, width, height) = document
            .layout_box_by_id("workspace-pager", (1080.0, 72.0))
            .expect("pager node exists");
        assert_eq!(width > 0.0 && height > 0.0, expected);
    }
}

// REGRESSION: the pager must draw the same two-row topology the WM
// navigates (`1 2 / 3 4`, `1 2 3 / 4 5 6`), not a fixed 2x2 page of four
// slots, up to nine columns at the 18 workspace maximum. At four
// workspaces every cell keeps the device rect it had before the pager
// became count-dependent, and only then does the box grow.
// TRIGGER: panel.html pager markup, ui/shell/flamewm.css pager rules or
// project_pager changes.
// OBSERVABLE: `workspace-column-j` / `workspace-k` device rects on the
// compiled panel artifact.
// MUTATION: project cell ids by page slot, or drop `position:absolute` from
// `.workspace-column` (flow columns take width, so hidden columns would
// shift every later cell right).
#[test]
fn pager_cells_follow_the_two_row_workspace_topology() {
    let _scale = taskbar_scale::test_lock();
    taskbar_scale::set_factor(1.0);
    let viewport = (1080.0, 72.0);
    let mut document = decode_panel();
    document
        .visible("workspace-pager", true)
        .expect("pager node projects");
    let rect = |document: &flamewm_ui_x11::UiDocument, id: &str| {
        document
            .layout_box_by_id(id, viewport)
            .unwrap_or_else(|| panic!("panel node '{id}' missing from artifact"))
    };
    let column_rects = |document: &flamewm_ui_x11::UiDocument| {
        (1..=PAGER_COLUMNS)
            .map(|column| rect(document, &format!("workspace-column-{column}")))
            .collect::<Vec<_>>()
    };
    let hidden = (0.0, 0.0, 0.0, 0.0);

    // Four workspaces: `1 2 / 3 4`. Workspace index 0/1 fill the upper
    // cells of columns 1/2, index 2/3 the lower ones.
    project_pager(&mut document, Some(&pager_snapshot(4, 0))).expect("pager projects");
    let (pager_x, pager_y, pager_width_px, pager_height) = rect(&document, "workspace-pager");
    assert_eq!(
        (pager_width_px, pager_height),
        (51.0, crate::taskbar_scale::node_height(1.0)),
        "four workspaces keep today's two-column pager box at the shared node height"
    );
    for (index, cell, column, row) in [
        (0usize, 1usize, 0usize, 0usize),
        (1, 3, 1, 0),
        (2, 2, 0, 1),
        (3, 4, 1, 1),
    ] {
        assert_eq!(pager_cell(index, 4), Some(cell), "workspace {index} cell");
        assert_eq!(
            rect(&document, &format!("workspace-{cell}")),
            (
                pager_x + 3.0 + column as f32 * 23.0,
                pager_y + 4.0 + row as f32 * 17.0,
                22.0,
                16.0
            ),
            "workspace {index} device rect"
        );
    }
    for cell in 5..=PAGER_CELLS {
        assert_eq!(
            rect(&document, &format!("workspace-{cell}")),
            hidden,
            "cell {cell}"
        );
    }
    let columns = column_rects(&document);
    assert_eq!(columns[0], (pager_x + 3.0, pager_y + 4.0, 22.0, 33.0));
    assert_eq!(columns[1], (pager_x + 26.0, pager_y + 4.0, 22.0, 33.0));
    for (index, column) in columns.iter().enumerate().skip(2) {
        assert_eq!(*column, hidden, "column {}", index + 1);
    }

    // Six workspaces: `1 2 3 / 4 5 6` needs a third column and a 74px box.
    project_pager(&mut document, Some(&pager_snapshot(6, 5))).expect("pager projects");
    let (pager_x, pager_y, pager_width_px, _) = rect(&document, "workspace-pager");
    assert_eq!(pager_width_px, 74.0);
    for (index, cell, column, row) in [
        (0usize, 1usize, 0usize, 0usize),
        (1, 3, 1, 0),
        (2, 5, 2, 0),
        (3, 2, 0, 1),
        (4, 4, 1, 1),
        (5, 6, 2, 1),
    ] {
        assert_eq!(pager_cell(index, 6), Some(cell), "workspace {index} cell");
        assert_eq!(
            rect(&document, &format!("workspace-{cell}")),
            (
                pager_x + 3.0 + column as f32 * 23.0,
                pager_y + 4.0 + row as f32 * 17.0,
                22.0,
                16.0
            ),
            "workspace {index} device rect"
        );
    }
    let columns = column_rects(&document);
    assert_eq!(columns[2], (pager_x + 49.0, pager_y + 4.0, 22.0, 33.0));
    for (index, column) in columns.iter().enumerate().skip(3) {
        assert_eq!(*column, hidden, "column {}", index + 1);
    }

    // Eighteen workspaces: nine columns, a 212px box, every cell filled.
    project_pager(&mut document, Some(&pager_snapshot(18, 9))).expect("pager projects");
    let (pager_x, pager_y, pager_width_px, _) = rect(&document, "workspace-pager");
    assert_eq!(pager_width_px, 212.0);
    let columns = column_rects(&document);
    assert_eq!(columns.len(), PAGER_COLUMNS);
    for (index, column) in columns.iter().enumerate() {
        assert_eq!(
            *column,
            (
                pager_x + 3.0 + index as f32 * 23.0,
                pager_y + 4.0,
                22.0,
                33.0
            ),
            "column {}",
            index + 1
        );
    }
    for cell in 1..=PAGER_CELLS {
        let column = (cell - 1) / 2;
        let row = (cell - 1) % 2;
        assert_eq!(
            rect(&document, &format!("workspace-{cell}")),
            (
                pager_x + 3.0 + column as f32 * 23.0,
                pager_y + 4.0 + row as f32 * 17.0,
                22.0,
                16.0
            ),
            "cell {cell} device rect"
        );
    }
    // With eighteen workspaces every cell holds exactly one workspace, so
    // the cell -> workspace mapping is a bijection over 1..=18.
    for index in 0..18 {
        let cell = pager_cell(index, 18).expect("workspace cell");
        assert_eq!(cell_index(cell, 18), Some(index), "cell {cell} workspace");
    }
}

// REGRESSION: the battery popup shows exactly one glyph node, charging-aware,
// on the compiled flamewm-battery.rwr artifact for each charge bucket.
// MUTATION: keep a retired low node or project the wrong bucket in the menu.
#[test]
fn battery_menu_shows_exactly_one_glyph_on_real_artifact() {
    const BATTERY_ARTIFACT: &[u8] =
        include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-battery.rwr"));
    let cases = [
        (5, PowerState::Discharging, "battery-icon-caution"),
        (15, PowerState::Discharging, "battery-icon-020"),
        (45, PowerState::Discharging, "battery-icon-050"),
        (100, PowerState::Discharging, "battery-icon-100"),
        (45, PowerState::Charging, "battery-icon-050-charging"),
    ];
    let ids = glyph_node_ids("battery-icon-");
    for (percent, state, expected) in cases {
        let power = power_snapshot(ServiceAvailability::Available, true, percent, state);
        let mut document = flamewm_ui_x11::decode_document(BATTERY_ARTIFACT)
            .expect("battery artifact must decode");
        project_battery_menu(&mut document, &power, 268.0).expect("battery menu projects");
        assert_eq!(
            lit_ids(&document, &ids, (300.0, 132.0)),
            vec![expected],
            "{percent}% {state:?}"
        );
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
    type Projector = fn(&mut flamewm_ui_x11::UiDocument, &crate::ShellSnapshot);
    let _scale = taskbar_scale::test_lock();
    taskbar_scale::set_factor(1.0);
    let _guard = crate::async_projection::icon_test_lock().lock();
    let glyphs = glyph_node_ids("tray-battery-");
    let avail = ServiceAvailability::Available;
    let cases: [(PowerSnapshot, Option<&str>); 14] = [
        (
            power_snapshot(avail, true, 87, PowerState::Unknown),
            Some("tray-battery-090"),
        ),
        (
            power_snapshot(avail, true, 45, PowerState::Discharging),
            Some("tray-battery-050"),
        ),
        (
            power_snapshot(avail, true, 45, PowerState::Unknown),
            Some("tray-battery-050"),
        ),
        (
            power_snapshot(avail, true, 15, PowerState::Discharging),
            Some("tray-battery-020"),
        ),
        (
            power_snapshot(avail, true, 5, PowerState::Discharging),
            Some("tray-battery-caution"),
        ),
        (
            power_snapshot(avail, true, 30, PowerState::Charging),
            Some("tray-battery-030-charging"),
        ),
        (
            power_snapshot(avail, true, 79, PowerState::Charging),
            Some("tray-battery-080-charging"),
        ),
        (
            power_snapshot(avail, true, 80, PowerState::Charging),
            Some("tray-battery-080-charging"),
        ),
        (
            power_snapshot(avail, true, 100, PowerState::Charging),
            Some("tray-battery-100-charging"),
        ),
        (
            power_snapshot(
                ServiceAvailability::Unavailable,
                true,
                40,
                PowerState::Discharging,
            ),
            None,
        ),
        (
            power_snapshot(avail, true, 79, PowerState::Discharging),
            Some("tray-battery-080"),
        ),
        (
            power_snapshot(avail, true, 80, PowerState::Discharging),
            Some("tray-battery-080"),
        ),
        (
            power_snapshot(avail, true, 100, PowerState::Full),
            Some("tray-battery-100"),
        ),
        (
            power_snapshot(avail, false, 45, PowerState::Discharging),
            None,
        ),
    ];
    for (power, expected) in cases {
        let mut snapshot = crate::ShellSnapshot::default();
        snapshot.system.power = power;
        let paths: [(&str, Projector); 3] = [
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
            let mut document = decode_panel();
            project_path(&mut document, &snapshot);
            let viewport = (1080.0, 72.0);
            let button = box_width(&document, "tray-battery", viewport) > 0.0;
            assert_eq!(
                button,
                expected.is_some(),
                "{path}: button for {:?}",
                snapshot.system.power
            );
            let lit = lit_ids(&document, &glyphs, viewport);
            match expected {
                Some(glyph) => assert_eq!(
                    lit,
                    vec![glyph],
                    "{path}: glyph for {:?}",
                    snapshot.system.power
                ),
                None => assert!(
                    lit.is_empty(),
                    "{path}: no glyph while hidden for {:?}",
                    snapshot.system.power
                ),
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
    let glyph_for = |percent: u8| match percent {
        15 => "020",
        45 => "050",
        79 | 80 => "080",
        100 => "100",
        other => panic!("matrix has no expected glyph for {other}%"),
    };
    let all_ids = glyph_node_ids("tray-battery-");
    for state in [
        PowerState::Charging,
        PowerState::Discharging,
        PowerState::Full,
        PowerState::Empty,
        PowerState::Unknown,
    ] {
        for percent in [15_u8, 45, 79, 80, 100] {
            for present in [true, false] {
                let power = power_snapshot(ServiceAvailability::Available, present, percent, state);
                let charging = state == PowerState::Charging;
                let visible = present;
                let mut document = decode_panel();
                project_battery_tray(&mut document, &power, TaskbarItems::default())
                    .expect("tray battery projects");
                let viewport = (1080.0, 72.0);
                assert_eq!(
                    box_width(&document, "tray-battery", viewport) > 0.0,
                    visible,
                    "{state:?} {percent}% present={present}"
                );
                let lit = lit_ids(&document, &all_ids, viewport);
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
    let _scale = taskbar_scale::test_lock();
    taskbar_scale::set_factor(1.0);
    let _guard = crate::async_projection::icon_test_lock().lock();
    let mut document = decode_panel();
    let snapshot = crate::ShellSnapshot::default();
    crate::async_projection::project_panel_async(
        &mut document,
        PanelView::first(&snapshot),
        None,
        &mut Default::default(),
    )
    .unwrap_or_else(|error| panic!("full panel projection refused: {error}"));
}

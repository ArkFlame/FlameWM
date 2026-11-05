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

#[test]
fn task_icon_request_scales_with_output_shell_scale() {
    assert_eq!(task_icon_px(None), 25);
    assert_eq!(task_icon_px(Some(&output("a", 0, 100))), 25);
    assert_eq!(task_icon_px(Some(&output("a", 0, 125))), 31);
    assert_eq!(task_icon_px(Some(&output("a", 0, 150))), 38);
    assert_eq!(task_icon_px(Some(&output("a", 0, 200))), 50);
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

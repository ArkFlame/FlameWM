use super::*;

/// DP-1 at the origin and HDMI-1 to its right, both connected.
fn two_output_snapshot() -> DisplaySnapshot {
    let mut snapshot = display_snapshot();
    let mut right = snapshot.outputs[0].clone();
    right.id = OutputId::new("HDMI-1");
    right.connector = "HDMI-1".to_owned();
    right.edid_identity = "edid-hdmi".to_owned();
    right.primary = false;
    right.geometry = Rect::new(1920, 0, 1280, 720);
    snapshot.outputs[0].id = OutputId::new("DP-1");
    snapshot.outputs[0].connector = "DP-1".to_owned();
    snapshot.outputs[0].primary = true;
    snapshot.outputs.push(right);
    snapshot
}

fn pointer(action: &str, phase: UiActionPhase, x: f32, y: f32) -> UiActionEvent {
    UiActionEvent {
        action: action.to_owned(),
        phase,
        inside: true,
        button: 1,
        modifiers: 0,
        x,
        y,
        text: None,
    }
}

#[test]
fn canvas_renders_one_positioned_node_per_output_with_selection_and_labels() {
    let (_requests, mut transport) = recording_transport();
    transport
        .responses
        .push_back(Ok(ControlResponse::Displays(two_output_snapshot())));
    let application = displays_application(transport);
    let mut document = ProbeDocument::default();
    application
        .sync_display_canvas(&mut document)
        .expect("canvas projects");

    assert_eq!(document.visible.get("display-canvas"), Some(&true));
    assert_eq!(document.visible.get("display-canvas-node-0"), Some(&true));
    assert_eq!(document.visible.get("display-canvas-node-1"), Some(&true));
    assert_eq!(document.visible.get("display-canvas-node-2"), Some(&false));
    let (left0, top0) = document.positions["display-canvas-node-0"];
    let (left1, top1) = document.positions["display-canvas-node-1"];
    assert!(
        left1 > left0,
        "HDMI-1 sits right of DP-1 in the bounded canvas"
    );
    assert_eq!(top0, top1, "both outputs share a top edge");
    let (width0, _) = document.sizes["display-canvas-node-0"];
    assert!(
        left0 + width0 <= left1 + 0.5,
        "scaled nodes do not overlap on the canvas"
    );
    assert_eq!(document.texts["display-canvas-node-1"], "HDMI-1");
    assert_eq!(
        document.attributes[&("display-canvas-node-1".to_owned(), "aria-label".to_owned())],
        "HDMI-1, 1280 by 720, at 1920, 0"
    );
    assert_eq!(
        document.attributes[&(
            "display-canvas-node-0".to_owned(),
            "aria-pressed".to_owned()
        )],
        "true",
        "the primary output is selected by default"
    );
}

#[test]
fn pointer_drag_previews_snap_then_release_commits_one_keep_revert_arrangement() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Displays(two_output_snapshot())),
        Ok(ControlResponse::Displays(two_output_snapshot())),
        Ok(ControlResponse::Displays(two_output_snapshot())),
        Ok(ControlResponse::Transaction(flamewm_api::TransactionId(40))),
    ]);
    let mut application = displays_application(transport);
    let mut document = ProbeDocument::default();

    // Press HDMI-1 (node 1), then drag it down about 100 logical px at the canvas scale.
    application
        .handle_action(
            &pointer(
                "settings.display.canvas.1",
                UiActionPhase::Press,
                100.0,
                50.0,
            ),
            &mut document,
        )
        .expect("press");
    application
        .handle_action(
            &pointer(
                "settings.display.canvas.1",
                UiActionPhase::Motion,
                100.0,
                65.5,
            ),
            &mut document,
        )
        .expect("drag");
    let status = &document.texts["display-canvas-status"];
    assert!(status.starts_with("HDMI-1 at 1920, 100"), "{status}");
    assert!(status.ends_with("Snapped to an edge"), "{status}");
    assert!(
        document.highlighted.get("display-canvas-node-0") == Some(&true),
        "the snap target (DP-1) is tinted while attached"
    );
    assert!(
        !requests
            .borrow()
            .iter()
            .any(|r| matches!(r, ControlRequest::SetDisplayArrangement { .. })),
        "motion previews only; nothing reaches the WM before release"
    );

    application
        .handle_action(
            &pointer(
                "settings.display.canvas.1",
                UiActionPhase::Release,
                100.0,
                65.5,
            ),
            &mut document,
        )
        .expect("release commits");
    let placements = requests
        .borrow()
        .iter()
        .find_map(|r| match r {
            ControlRequest::SetDisplayArrangement { placements, .. } => Some(placements.clone()),
            _ => None,
        })
        .expect("release sends one arrangement");
    assert_eq!(placements.len(), 2);
    assert_eq!(
        (
            placements[1].output.0.as_str(),
            placements[1].x,
            placements[1].y
        ),
        ("HDMI-1", 1920, 100)
    );
    assert_eq!(
        application.client().ui().pending_display_transaction,
        Some(flamewm_api::TransactionId(40))
    );
    assert!(
        application.pending_arrangement.is_some(),
        "the kept order is persisted only after keep"
    );
    assert_eq!(document.texts["display-canvas-status"], "HDMI-1 at 1920, 0");
}

#[test]
fn press_without_movement_selects_the_output_and_sends_nothing() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Displays(two_output_snapshot())),
        Ok(ControlResponse::Displays(two_output_snapshot())),
    ]);
    let mut application = displays_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(
            &pointer(
                "settings.display.canvas.1",
                UiActionPhase::Press,
                10.0,
                10.0,
            ),
            &mut document,
        )
        .expect("press");
    application
        .handle_action(
            &pointer(
                "settings.display.canvas.1",
                UiActionPhase::Release,
                11.0,
                10.0,
            ),
            &mut document,
        )
        .expect("release");
    assert_eq!(
        application.client().ui().selected_output.as_ref(),
        Some(&OutputId::new("HDMI-1"))
    );
    assert!(
        !requests
            .borrow()
            .iter()
            .any(|r| matches!(r, ControlRequest::SetDisplayArrangement { .. }))
    );
}

#[test]
fn arrow_key_nudges_the_selected_output_into_a_pending_layout() {
    let (_requests, mut transport) = recording_transport();
    transport
        .responses
        .push_back(Ok(ControlResponse::Displays(two_output_snapshot())));
    transport
        .responses
        .push_back(Ok(ControlResponse::Displays(two_output_snapshot())));
    let mut application = displays_application(transport);
    let mut document = ProbeDocument::default();
    // DP-1 is selected by default; sliding it down stays attached to HDMI-1's west edge.
    application
        .display_canvas_key("Down", None)
        .expect("arrow slides the selected output");
    let canvas = application.canvas_source().expect("pending canvas");
    assert!(canvas.is_dirty());
    application
        .sync_display_canvas(&mut document)
        .expect("projects");
    assert!(document.texts["display-canvas-status"].contains("Enter keeps"));
}

#[test]
fn escape_during_pointer_drag_restores_the_node_and_sends_nothing() {
    let (requests, mut transport) = recording_transport();
    // One snapshot for the page load, one for the press that builds the canvas.
    transport.responses.extend([
        Ok(ControlResponse::Displays(two_output_snapshot())),
        Ok(ControlResponse::Displays(two_output_snapshot())),
    ]);
    let mut application = displays_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(
            &pointer(
                "settings.display.canvas.1",
                UiActionPhase::Press,
                100.0,
                50.0,
            ),
            &mut document,
        )
        .expect("press");
    application
        .handle_action(
            &pointer(
                "settings.display.canvas.1",
                UiActionPhase::Motion,
                100.0,
                65.5,
            ),
            &mut document,
        )
        .expect("drag");
    application
        .handle_key_event(
            &flamewm_ui_x11::UiKeyEvent {
                phase: flamewm_ui_x11::UiKeyPhase::Release,
                key: "Escape".to_owned(),
                modifiers: Vec::new(),
                text: None,
            },
            &mut document,
        )
        .expect("escape");
    assert!(application.canvas_drag.is_none(), "escape ends the drag");
    assert_eq!(document.texts["display-canvas-status"], "HDMI-1 at 1920, 0");
    application
        .handle_action(
            &pointer(
                "settings.display.canvas.1",
                UiActionPhase::Release,
                100.0,
                65.5,
            ),
            &mut document,
        )
        .expect("late release");
    assert!(
        !requests
            .borrow()
            .iter()
            .any(|r| matches!(r, ControlRequest::SetDisplayArrangement { .. })),
        "a cancelled drag never reaches the WM"
    );
}

#[test]
fn return_key_keeps_a_nudged_layout_as_one_arrangement_request() {
    let (requests, mut transport) = recording_transport();
    // Page load, canvas start, keep-time refresh, then the arrangement transaction.
    transport.responses.extend([
        Ok(ControlResponse::Displays(two_output_snapshot())),
        Ok(ControlResponse::Displays(two_output_snapshot())),
        Ok(ControlResponse::Displays(two_output_snapshot())),
        Ok(ControlResponse::Transaction(flamewm_api::TransactionId(41))),
    ]);
    let mut application = displays_application(transport);
    application
        .client_mut()
        .set_page(flamewm_settings_core::SettingsPage::Displays);
    let mut document = ProbeDocument::default();
    let release = |key: &str| flamewm_ui_x11::UiKeyEvent {
        phase: flamewm_ui_x11::UiKeyPhase::Release,
        key: key.to_owned(),
        modifiers: Vec::new(),
        text: None,
    };
    application
        .handle_key_event(&release("Down"), &mut document)
        .expect("nudge");
    application
        .handle_key_event(&release("Return"), &mut document)
        .expect("return commits");
    let arrangements = requests
        .borrow()
        .iter()
        .filter(|r| matches!(r, ControlRequest::SetDisplayArrangement { .. }))
        .count();
    assert_eq!(arrangements, 1, "Return keeps the nudged layout once");
}

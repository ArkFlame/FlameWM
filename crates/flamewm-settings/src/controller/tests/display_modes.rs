use super::*;

#[test]
fn display_mode_slots_show_one_label_per_mode_with_distinct_ids() {
    let mut transport = RecordingTransport::default();
    transport
        .responses
        .push_back(Ok(ControlResponse::Displays(two_mode_display_snapshot())));
    let mut application = displays_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.display.select.0"), &mut document)
        .expect("display select");

    assert_eq!(document.visible.get("display-mode-slot-0"), Some(&true));
    assert_eq!(document.visible.get("display-mode-slot-1"), Some(&true));
    assert_eq!(document.visible.get("display-mode-slot-2"), Some(&false));
    let first = document
        .texts
        .get("display-mode-slot-0")
        .expect("slot 0 label");
    let second = document
        .texts
        .get("display-mode-slot-1")
        .expect("slot 1 label");
    assert!(first.contains("1920 x 1080"), "slot 0 label: {first}");
    assert!(second.contains("1280 x 720"), "slot 1 label: {second}");
    assert_ne!(first, second);
}

#[test]
fn display_mode_slot_action_begins_the_mapped_mode() {
    let (requests, mut transport) = recording_transport();
    transport
        .responses
        .push_back(Ok(ControlResponse::Displays(two_mode_display_snapshot())));
    transport
        .responses
        .push_back(Ok(ControlResponse::Transaction(
            flamewm_api::TransactionId(11),
        )));
    let mut application = displays_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.display.select.0"), &mut document)
        .expect("display select");
    application
        .handle_action(&release("settings.display.slot.1"), &mut document)
        .expect("mode slot begin");

    let begun = requests
        .borrow()
        .iter()
        .filter_map(|request| match request {
            ControlRequest::BeginDisplayMode { mode, .. } => Some(*mode),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(begun, vec![ModeId(8)]);
}

#[test]
fn display_mode_slots_and_actions_follow_the_selected_output() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Displays(two_output_display_snapshot())),
        Ok(ControlResponse::Transaction(flamewm_api::TransactionId(12))),
    ]);
    let mut application = displays_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.display.select.1"), &mut document)
        .expect("select DP-1");
    assert!(document.texts["display-mode-slot-0"].contains("1920 x 1080"));
    application
        .handle_action(&release("settings.display.slot.1"), &mut document)
        .expect("begin DP-1 mode");
    assert!(
        matches!(requests.borrow().last(), Some(ControlRequest::BeginDisplayMode { output, mode: ModeId(8), .. }) if output == &OutputId::new("DP-1"))
    );
}

#[test]
fn display_confirmation_keep_revert_and_errors_track_pending_state() {
    for (action, response, expected_pending) in [
        ("settings.display.keep", Ok(ControlResponse::Unit), false),
        ("settings.display.revert", Ok(ControlResponse::Unit), false),
        (
            "settings.display.keep",
            Err(flamewm_control_core::ControlError {
                name: "com.arkflame.FlameWM1.Error.StaleRevision",
                code: flamewm_api::ErrorCode::StaleRevision,
                message: "stale transaction".to_owned(),
            }),
            true,
        ),
    ] {
        let (requests, mut transport) = recording_transport();
        let mut pending = two_mode_display_snapshot();
        pending.pending = Some(flamewm_api::display::PendingModeChange {
            transaction: flamewm_api::TransactionId(21),
            output: OutputId::new("HDMI-1"),
            mode: ModeId(8),
            deadline_ms: 5000,
        });
        transport
            .responses
            .push_back(Ok(ControlResponse::Displays(pending)));
        if expected_pending {
            transport.responses.push_back(response);
        } else {
            let mut settled = two_mode_display_snapshot();
            settled.outputs[0].current_mode = if action.ends_with("keep") {
                ModeId(8)
            } else {
                ModeId(7)
            };
            transport.responses.push_back(response);
            transport
                .responses
                .push_back(Ok(ControlResponse::Displays(settled)));
        }
        let mut application = displays_application(transport);
        let mut document = ProbeDocument::default();
        let before = crate::monotonic_now_ms();
        let result = application.handle_action(&release(action), &mut document);
        let after = crate::monotonic_now_ms();
        assert_eq!(result.is_err(), expected_pending);
        assert_eq!(
            application
                .client()
                .ui()
                .pending_display_transaction
                .is_some(),
            expected_pending
        );
        if !expected_pending {
            assert_eq!(document.visible.get("display-confirm"), Some(&false));
        }
        assert!(
            matches!(requests.borrow().get(1), Some(ControlRequest::KeepDisplayMode { transaction: flamewm_api::TransactionId(21), now_ms }) if action.ends_with("keep") && *now_ms >= before && *now_ms <= after)
                || matches!(requests.borrow().get(1), Some(ControlRequest::RevertDisplayMode { transaction: flamewm_api::TransactionId(21) }) if action.ends_with("revert"))
        );
    }
}

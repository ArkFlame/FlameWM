use super::*;

/// Five outputs OUT-0..OUT-4: page 0 holds 0..4, page 1 holds OUT-4.
fn five_output_display_snapshot() -> DisplaySnapshot {
    let mut snapshot = display_snapshot();
    let template = snapshot.outputs[0].clone();
    snapshot.outputs = (0..5)
        .map(|index| {
            let mut output = template.clone();
            output.id = OutputId::new(format!("OUT-{index}"));
            output.connector = format!("OUT-{index}");
            output.primary = index == 0;
            output
        })
        .collect();
    snapshot
}

#[test]
fn display_mode_page_two_slot_three_begins_absolute_mode_seven() {
    let (requests, mut transport) = recording_transport();
    transport.responses.extend([
        Ok(ControlResponse::Displays(twelve_mode_display_snapshot())),
        Ok(ControlResponse::Transaction(flamewm_api::TransactionId(31))),
    ]);
    let mut application = displays_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.display.modes.next"), &mut document)
        .expect("mode page 2");
    assert_eq!(document.texts["display-mode-page-label"], "5-8 of 12");
    application
        .handle_action(&release("settings.display.slot.3"), &mut document)
        .expect("page 2 slot 3 begins");
    assert_eq!(begun_modes(&requests), vec![ModeId(7)]);
}

#[test]
fn display_output_page_one_slot_zero_selects_absolute_output_four() {
    let mut transport = RecordingTransport::default();
    transport.responses.push_back(Ok(
        ControlResponse::Displays(five_output_display_snapshot()),
    ));
    let mut application = displays_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.display.outputs.next"), &mut document)
        .expect("output page 2");
    assert_eq!(document.texts["display-output-page-label"], "5-5 of 5");
    application
        .handle_action(&release("settings.display.select.0"), &mut document)
        .expect("page 1 slot 0 selects");
    assert_eq!(
        application.client().ui().selected_output.as_ref(),
        Some(&OutputId::new("OUT-4"))
    );
}

use super::*;

#[test]
fn stale_page_generation_is_rejected_without_applying() {
    // Cursor stamped at generation 3, authoritative snapshot now generation 4.
    let error = crate::page_slot_index(3, 4, 0, 4, 0, 5, "display output")
        .expect_err("stale generation must be rejected");
    assert!(error.contains("stale"), "legible stale error: {error}");
    assert!(crate::page_slot_index(4, 4, 0, 4, 0, 5, "display output").is_ok());
}

#[test]
fn out_of_range_page_slots_are_legible_errors_and_never_begin() {
    let (requests, mut transport) = recording_transport();
    transport.responses.push_back(Ok(
        ControlResponse::Displays(twelve_mode_display_snapshot()),
    ));
    let mut application = displays_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.display.modes.next"), &mut document)
        .expect("mode page 2");
    // The four-slot page never exposes a right-column/future slot.
    let past_end = application
        .handle_action(&release("settings.display.slot.4"), &mut document)
        .expect_err("slot 4 is outside the page");
    assert!(
        past_end.contains("outside the page"),
        "legible range error: {past_end}"
    );
    // Arbitrary higher slots stay rejected as well.
    let outside = application
        .handle_action(&release("settings.display.slot.9"), &mut document)
        .expect_err("slot 9 is outside the page");
    assert!(
        outside.contains("outside the page"),
        "legible slot error: {outside}"
    );
    assert!(
        begun_modes(&requests).is_empty(),
        "rejected slots must not begin a mode"
    );
}

#[test]
fn pager_steps_clamp_at_the_first_and_last_page() {
    let mut transport = RecordingTransport::default();
    transport.responses.push_back(Ok(
        ControlResponse::Displays(twelve_mode_display_snapshot()),
    ));
    let mut application = displays_application(transport);
    let mut document = ProbeDocument::default();
    application
        .handle_action(&release("settings.display.modes.prev"), &mut document)
        .expect("prev on first page is a no-op");
    assert_eq!(document.texts["display-mode-page-label"], "1-4 of 12");
    assert_eq!(document.visible.get("display-mode-prev-off"), Some(&true));
    for _ in 0..2 {
        application
            .handle_action(&release("settings.display.modes.next"), &mut document)
            .expect("next page");
    }
    assert_eq!(document.texts["display-mode-page-label"], "9-12 of 12");
    assert_eq!(document.visible.get("display-mode-next-off"), Some(&true));
    assert_eq!(document.visible.get("display-mode-next"), Some(&false));
}

use std::cell::RefCell;
use std::rc::Rc;

use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot};
use flamewm_api::{Rect, Size};
use flamewm_control_core::{ControlRequest, ControlResponse};
use flamewm_settings_core::DISPLAY_MODE_SLOTS;

use super::test_transport::ScriptedTransport;
use super::*;

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
    let mut transport = ScriptedTransport::default();
    let requests = Rc::new(RefCell::new(Vec::new()));
    transport.requests = requests.clone();
    transport.responses.push_back(Ok(ControlResponse::Displays(
        five_output_twelve_mode_snapshot(),
    )));
    transport
        .responses
        .push_back(Ok(ControlResponse::Transaction(
            flamewm_api::TransactionId(7),
        )));
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

    // Four vertical mode rows per page avoid the overflowing right column.
    assert_eq!(DISPLAY_MODE_SLOTS, 4);
    assert_eq!(app.client.display_mode_count(), 12);
    assert_eq!(
        page_count(app.client.display_mode_count(), DISPLAY_MODE_SLOTS),
        3
    );
    app.execute(SettingsAction::ModePage(PageStep::Next))
        .expect("next mode page");
    app.execute(SettingsAction::ModePage(PageStep::Next))
        .expect("last mode page");
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
    assert!(
        display_mode_label(
            &app.client.display_mode_page(app.pages.mode_page)[2],
            selected_output.current_mode == fifth_output_current_mode,
        )
        .contains("current")
    );
    assert_eq!(
        app.client.display_mode_page(app.pages.mode_page)[3].id,
        fifth_output_mode_11
    );
    assert!(
        !display_mode_label(
            &app.client.display_mode_page(app.pages.mode_page)[3],
            selected_output.current_mode == fifth_output_mode_11,
        )
        .contains("current")
    );

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
    assert_eq!(
        app.client.ui().pending_display_transaction,
        Some(flamewm_api::TransactionId(7))
    );

    // End-of-list pager state is derived from the data and cannot advance past its last page.
    assert_eq!(
        step_page(app.pages.mode_page, PageStep::Next, 3),
        app.pages.mode_page
    );
    assert_eq!(
        page_label(app.pages.mode_page, DISPLAY_MODE_SLOTS, 12),
        "9-12 of 12"
    );

    // A cursor stamped against the prior generation rejects the same slot before any request.
    app.pages.generation = Some(41);
    let request_count = requests.borrow().len();
    let pending_before = app.client.ui().pending_display_transaction;
    assert!(
        app.execute(SettingsAction::DisplaySlot(3))
            .unwrap_err()
            .contains("stale")
    );
    assert_eq!(requests.borrow().len(), request_count);
    assert_eq!(app.client.ui().pending_display_transaction, pending_before);
}

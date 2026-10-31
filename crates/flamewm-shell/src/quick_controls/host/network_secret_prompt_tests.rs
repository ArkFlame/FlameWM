use super::*;

/// The password field must have a real box: an unsized `<input>` in the
/// flex row lays out 0x0, so its `network.secret.focus` hit rect is empty
/// and clicks never reach the focus route. Source-scan over the compiled
/// `network.html` stylesheet, which the layout engine reads directly.
fn css_rule<'a>(html: &'a str, selector: &str) -> &'a str {
    html.split_once(&format!("{selector} {{"))
        .unwrap_or_else(|| panic!("{selector} must be styled with a rule"))
        .1
        .split_once('}')
        .expect("rule must close")
        .0
}

/// The password box must have a real hit area: the input is absolutely
/// covering a sized, positioned box (as the Filter field does), so a click
/// anywhere in the box reaches `network.secret.focus`.
#[test]
fn password_input_has_an_explicit_hit_box() {
    let html = include_str!("../../../../../ui/shell/network.html");
    let box_rule = css_rule(html, "#network-secret-box");
    assert!(
        box_rule.contains("position:relative") && box_rule.contains("height:"),
        "secret box needs position and height: {box_rule}"
    );
    let input_rule = css_rule(html, "#network-secret-input");
    assert!(
        input_rule.contains("position:absolute")
            && input_rule.contains("width:100%")
            && input_rule.contains("height:100%"),
        "password input must cover its box: {input_rule}"
    );
}

/// The masked value must render inside the same box as the input, not in a
/// separate bordered element beside it.
#[test]
fn password_value_renders_inside_the_input_box() {
    let html = include_str!("../../../../../ui/shell/network.html");
    let after_box = html
        .split_once("id=\"network-secret-box\"")
        .expect("secret box must exist")
        .1;
    let box_body = after_box
        .split_once("<button")
        .expect("buttons follow the box")
        .0;
    assert!(box_body.contains("id=\"network-secret-value\""));
    assert!(box_body.contains("id=\"network-secret-input\""));
    let value_rule = css_rule(html, "#network-secret-value");
    assert!(
        value_rule.contains("position:absolute"),
        "masked value must overlay the box, not sit beside it: {value_rule}"
    );
}

/// The error line must stay inside the popup width and must not push the
/// Connect/Cancel controls out of the row.
#[test]
fn secret_error_is_clipped_and_controls_stay_before_it() {
    let html = include_str!("../../../../../ui/shell/network.html");
    let error_rule = css_rule(html, "#network-secret-error");
    assert!(
        error_rule.contains("width:100%")
            && error_rule.contains("overflow:hidden")
            && error_rule.contains("height:"),
        "error needs a clipped fixed-width line: {error_rule}"
    );
    let row_body = html
        .split_once("class=\"network-secret-row\"")
        .expect("controls row must exist")
        .1
        .split_once("id=\"network-secret-error\"")
        .expect("error line must follow the controls row")
        .0;
    assert!(row_body.contains("data-action=\"network.secret.submit\""));
    assert!(row_body.contains("data-action=\"network.secret.cancel\""));
    let secret_rule = css_rule(html, "#network-secret");
    assert!(
        secret_rule.contains("flex-direction:column"),
        "error must stack below the row, not inline: {secret_rule}"
    );
}

/// Each secret edit and error transition must request reprojection so the
/// masked value and error line repaint without another input event.
#[test]
fn network_secret_edits_and_errors_request_reprojection() {
    use super::network_filter_reprojection_tests::{
        edit_block_requests_reprojection, keyboard_section,
    };
    let keyboard = keyboard_section();
    let source = include_str!("run.rs");
    assert!(
        edit_block_requests_reprojection(keyboard, "snapshot.network_secret.pop();"),
        "secret backspace must request reprojection"
    );
    assert!(
        edit_block_requests_reprojection(keyboard, "snapshot.network_secret.push_str(text);"),
        "secret typed edit must request reprojection"
    );
    let outcome = source
        .split_once("take_network_secret_outcomes();")
        .expect("outcome drain must exist")
        .1
        .split_once("Err(()) =>")
        .expect("failed outcome arm must exist")
        .1
        .split_once('}')
        .expect("failed outcome arm must close")
        .0;
    assert!(outcome.contains("network_secret_error = true;"));
    assert!(
        outcome.contains("system_reprojection_pending = true;"),
        "failed submit must request reprojection: {outcome}"
    );
}

#[test]
fn empty_password_builds_no_submit_request() {
    assert!(network_secret_submit_request((4, 9), "", 7).is_none());
}

#[test]
fn non_empty_password_builds_submit_for_its_identity() {
    let request =
        network_secret_submit_request((4, 9), "hunter2", 7).expect("non-empty secret submits");
    match request {
        ControlRequest::SystemAction {
            action:
                flamewm_api::system::SystemAction::SubmitNetworkSecret {
                    request_id,
                    generation,
                    secret,
                },
            expected_revision,
        } => {
            assert_eq!((request_id, generation, expected_revision), (4, 9, 7));
            assert_eq!(secret, "hunter2");
        }
        other => panic!("unexpected request: {other:?}"),
    }
}

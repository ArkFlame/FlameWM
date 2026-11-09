use super::*;

fn load(text: &str) -> FlameResult<ProductSettings> {
    ConfigStore::default().parse_snapshot(text)
}

// OBSERVABLE: the workspace switch popup is off by default and the default is not written.
// MUTATION: a default of true, or serializing the default, fails here.
#[test]
fn workspace_osd_defaults_off_and_is_not_written_at_default() {
    let defaults = ProductSettings::default();
    assert!(!defaults.workspace_osd);
    let text = ConfigStore::serialize(&defaults);
    assert!(
        !text.contains("workspaceOsd"),
        "default must not be written"
    );
}

// OBSERVABLE: an explicit on survives a round trip; an explicit off is accepted and dropped.
#[test]
fn workspace_osd_round_trips_on_and_drops_explicit_off() {
    let on = load("revision=2\nworkspaceOsd=true\n").expect("explicit on");
    assert!(on.workspace_osd);
    let text = ConfigStore::serialize(&on);
    assert!(text.contains("workspaceOsd=true\n"));
    assert_eq!(load(&text).expect("round trip"), on);

    let off = load("revision=2\nworkspaceOsd=false\n").expect("explicit off");
    assert!(!off.workspace_osd);
    assert!(!ConfigStore::serialize(&off).contains("workspaceOsd"));
}

// OBSERVABLE: values follow the shared bool convention; anything else is rejected, not coerced,
// and the key is a typed row rather than an unknown key.
#[test]
fn workspace_osd_rejects_invalid_values_and_is_a_typed_row() {
    for bad in ["maybe", "on", ""] {
        assert!(
            load(&format!("revision=2\nworkspaceOsd={bad}\n")).is_err(),
            "workspaceOsd={bad:?} must be rejected"
        );
    }
    let parsed = load("revision=2\nworkspaceOsd=true\n").expect("valid");
    assert!(
        parsed.unknown_keys.is_empty(),
        "must not fall into unknown_keys"
    );
    let stable = super::stable_setting("workspaceOsd").expect("typed workspace osd row");
    assert_eq!(
        (stable.get)(&ProductSettings::default()),
        flamewm_api::settings::SettingValue::Boolean(false)
    );
}

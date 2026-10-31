use super::*;

#[test]
fn begin_capture_request_and_lease_reply_round_trip_through_wire() {
    let request = ControlRequest::BeginHotkeyCapture {
        action: flamewm_api::shortcuts::ACTION_TOGGLE_START_MENU.to_owned(),
        expected_revision: 7,
    };
    let call = encode_call(&request).expect("encode request");
    assert_eq!(call.interface, IFACE_SHORTCUTS);
    assert_eq!(call.member, "BeginCapture");
    assert_eq!(decode_call(&call).expect("decode request"), request);

    let response = ControlResponse::CaptureLease(3);
    let reply = encode_reply(&call, &response).expect("encode reply");
    assert_eq!(reply.values, vec![WireValue::U64(3)]);
    assert_eq!(decode_reply(&call, &reply).expect("decode reply"), response);
}

#[test]
fn end_capture_request_and_unit_reply_round_trip_through_wire() {
    let request = ControlRequest::EndHotkeyCapture { lease: 3 };
    let call = encode_call(&request).expect("encode request");
    assert_eq!(call.interface, IFACE_SHORTCUTS);
    assert_eq!(call.member, "EndCapture");
    assert_eq!(call.args, vec![WireValue::U64(3)]);
    assert_eq!(decode_call(&call).expect("decode request"), request);

    let reply = encode_reply(&call, &ControlResponse::Unit).expect("encode reply");
    assert!(reply.values.is_empty());
    assert_eq!(
        decode_reply(&call, &reply).expect("decode reply"),
        ControlResponse::Unit
    );
}

#[test]
fn capture_lease_calls_reject_wrong_arity_and_types() {
    let begin_wrong_type = WireCall::new(
        IFACE_SHORTCUTS,
        "BeginCapture",
        vec![WireValue::U64(1), WireValue::U64(1)],
    );
    assert!(decode_call(&begin_wrong_type).is_err());

    let begin_short = WireCall::new(
        IFACE_SHORTCUTS,
        "BeginCapture",
        vec![WireValue::String("x".to_owned())],
    );
    assert!(decode_call(&begin_short).is_err());

    let end_wrong_type = WireCall::new(
        IFACE_SHORTCUTS,
        "EndCapture",
        vec![WireValue::String("3".to_owned())],
    );
    assert!(decode_call(&end_wrong_type).is_err());
}
#[test]
fn glass_keys_decode_to_their_persisted_wire_types() {
    for (key, value) in [
        ("glassOpacityPercent", WireValue::U32(42)),
        ("glassBlurPx", WireValue::U32(54)),
        ("taskbarOpacityPercent", WireValue::U32(75)),
    ] {
        assert!(
            matches!(
                super::setting_value_for_key(key, &value).expect("percent"),
                SettingValue::Percent(_)
            ),
            "{key}"
        );
    }
    assert_eq!(
        super::setting_value_for_key("reduceTransparency", &WireValue::Bool(false))
            .expect("switch"),
        SettingValue::Boolean(false)
    );
    assert_eq!(
        super::setting_value_for_key(
            "taskbarMaterialMode",
            &WireValue::String("custom-tint".to_owned())
        )
        .expect("mode"),
        SettingValue::Text("custom-tint".to_owned())
    );
    assert_eq!(
        super::setting_value_for_key("taskbarCustomTint", &WireValue::String("#3DAEE9".into()))
            .expect("tint"),
        SettingValue::Text("#3DAEE9".to_owned())
    );
}

#[test]
fn blur_percent_keys_decode_as_percent_like_the_opacities() {
    for key in ["surfaceOpacity", "surfaceBlurPercent"] {
        assert_eq!(
            super::setting_value_for_key(key, &WireValue::U32(80)).expect("percent"),
            SettingValue::Percent(80),
            "{key}"
        );
    }
}

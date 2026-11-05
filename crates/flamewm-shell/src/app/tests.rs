pub(super) fn production_source() -> String {
    [
        include_str!("../app.rs"),
        include_str!("control.rs"),
        include_str!("icons.rs"),
        include_str!("input.rs"),
        include_str!("ipc.rs"),
        include_str!("panels.rs"),
        include_str!("popups.rs"),
        include_str!("settings.rs"),
        include_str!("start_view.rs"),
        include_str!("timers.rs"),
        include_str!("weather.rs"),
    ]
    .iter()
    .map(|source| {
        source
            .split_once("\n#[cfg(test)]\nmod tests")
            .map_or(*source, |(production, _)| production)
    })
    .collect::<Vec<_>>()
    .join("\n")
}

#[test]
fn red_t06_settings_toggle_has_no_follow_up_get_settings_fetch() {
    // RED(T06): the current toggle submits ApplySettings, then performs a
    // blocking UI-thread GetSettings roundtrip instead of using its result.
    let source = production_source();
    assert!(
        !source.contains("self.control.call(&ControlRequest::GetSettings"),
        "T06 RED: settings mutation must not be followed by UI GetSettings"
    );
    assert!(
        !source.contains("refresh_settings_revision"),
        "T06 RED: remove the blocking settings refresh seam"
    );
}

#[test]
fn red_t08_shell_loop_has_no_steady_state_control_client_field() {
    // RED(T08): the current ShellLoop retains a blocking ControlClient
    // solely for steady-state reconciliation.
    let source = production_source();
    let shell_loop = source
        .split_once("pub(crate) struct ShellLoop {")
        .map(|(_, rest)| rest)
        .and_then(|rest| rest.split_once("\n}\n\nimpl ShellLoop"))
        .map(|(body, _)| body)
        .expect("ShellLoop definition must remain discoverable");
    assert!(
        !shell_loop.contains("ControlClient"),
        "T08 RED: ShellLoop steady state must not own a blocking ControlClient"
    );
}

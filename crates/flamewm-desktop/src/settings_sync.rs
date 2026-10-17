pub(super) fn source_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start = source
        .find(start)
        .unwrap_or_else(|| panic!("source is missing marker: {start}"));
    let body = &source[start..];
    let end = body
        .find(end)
        .unwrap_or_else(|| panic!("source is missing marker after {start}: {end}"));
    &body[..end]
}

pub(super) fn signal_callback_source() -> &'static str {
    let source = include_str!("main.rs");
    source_between(
        source,
        "let _ = client.on_ready(|signal| {",
        "FdAction::Continue",
    )
}

fn settings_projection_source() -> &'static str {
    source_between(
        include_str!("main.rs"),
        "fn refresh_settings_projection",
        "fn refresh_if_changed",
    )
}

#[test]
fn t10_settings_snapshot_changed_projects_payload_without_fetch_or_rescan() {
    // CONTRACT: SettingsSnapshotChanged is the complete live settings authority.
    // TRIGGER: deliver a settings snapshot through the existing control-signal watch.
    // OBSERVABLE: the callback consumes snapshot values and schedules visual projection.
    // CURRENT BAD MECHANISM: no settings snapshot arm exists, so settings changes remain
    // invisible until a later unrelated desktop turn or a blocking control fetch.
    // FAILURE MUTATION: remove payload consumption, add GetSettings, or route through the
    // filesystem refresh path; this regression must turn red.
    let callback = signal_callback_source();
    assert!(
        callback.contains("ControlSignal::SettingsSnapshotChanged"),
        "settings snapshot signal must be admitted by the desktop subscription"
    );
    assert!(
        callback.contains("snapshot.values"),
        "settings projection must consume the delivered snapshot payload"
    );
    assert!(
        callback.contains("settings_visual_dirty"),
        "settings delivery must schedule the isolated visual projection"
    );
    assert!(
        !callback.contains("ControlRequest::GetSettings")
            && !callback.contains("ControlClient::call"),
        "settings snapshot delivery must not perform a steady-state control fetch"
    );
    assert!(
        !callback.contains("rescan") && !callback.contains("refresh_launchers"),
        "settings delivery must not enter workspace filesystem synchronization"
    );
}

#[test]
fn t10_settings_projection_applies_sticky_selection_and_wallpaper_together() {
    // CONTRACT: one settings transition updates every desktop-owned visual affected by
    // settings: sticky visibility, selection appearance, and wallpaper presentation.
    // TRIGGER: project the pending settings snapshot on the next UI turn.
    // OBSERVABLE: the dedicated projection path names all three visual owners.
    // CURRENT BAD MECHANISM: the desktop has no settings projection owner, so the startup
    // wallpaper call is the only settings-related presentation path.
    // FAILURE MUTATION: omit any one visual owner or route through filesystem refresh;
    // this regression must turn red.
    assert!(
        include_str!("main.rs").contains("fn refresh_settings_projection"),
        "settings changes need a dedicated desktop projection seam"
    );
    let projection = settings_projection_source();
    for marker in [
        "sync_sticky",
        "sync_selection_visuals",
        "project_background",
        "wallpaper",
        "stickyNotesEnabled",
        "DesktopSelectionFillOpacity",
    ] {
        assert!(
            projection.contains(marker),
            "settings projection must apply {marker}"
        );
    }
    assert!(
        !projection.contains("refresh_if_changed"),
        "settings presentation must stay separate from filesystem refresh"
    );
}

#[test]
fn t10_settings_projection_retints_desktop_menus_from_surfaces() {
    // CONTRACT: live surfaceColor/surfaceOpacity changes reach every desktop menu root
    // through the settings projection, not only on the next menu open.
    // FAILURE MUTATION: drop the MENU_SURFACE_IDS loop or read the stale hardcoded CSS
    // fill; this regression must turn red.
    let projection = settings_projection_source();
    assert!(
        projection.contains("for id in projection::MENU_SURFACE_IDS")
            && projection
                .contains("projection::project_menu_surface(document, id, &state.settings)"),
        "settings projection must re-tint every desktop menu root from the snapshot"
    );
}

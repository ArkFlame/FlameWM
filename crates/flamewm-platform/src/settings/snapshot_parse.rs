use super::*;

impl ConfigStore {
    pub fn parse_snapshot(&self, text: &str) -> FlameResult<ProductSettings> {
        let mut snapshot = self.current.clone();
        let mut has_surface_key = false;
        let mut has_legacy_taskbar_surface_key = false;
        let mut has_wallpaper_mode = false;
        let mut has_lock_background_mode = false;
        let mut has_button_style = false;
        let mut has_render_backend = false;
        let mut seen_keys = std::collections::BTreeSet::new();
        // Legacy per-button styles: read only, each reduced to "is square".
        let mut legacy_square_flags = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return invalid_owned(format!("missing '=' in line: {line}"));
            };
            let key = key.trim();
            let value = value.trim();
            has_surface_key |= matches!(
                key,
                "surfaceColor" | "surfaceOpacity" | "surfaceBlurPercent"
            );
            has_legacy_taskbar_surface_key |= matches!(
                key,
                "taskbarColor" | "taskbarOpacity" | "taskbarBlurPercent"
            );
            has_wallpaper_mode |= key == "wallpaperMode";
            has_lock_background_mode |= key == "lockBackgroundMode";
            has_button_style |= key == "titlebarButtonStyle";
            has_render_backend |= key == "renderBackend";
            seen_keys.insert(key);
            if let Some(row) = stable_setting(key) {
                (row.parse)(&mut snapshot, value, key)?;
                continue;
            }
            match key {
                "revision" => snapshot.revision = parse(value, "revision")?,
                // Legacy titlebar button styles: read only, never kept as unknown keys.
                "titlebarMinimizeStyle" | "titlebarMaximizeStyle" | "titlebarCloseStyle" => {
                    legacy_square_flags.push(legacy_button_style_is_square(value)?);
                }
                // Legacy titlebar background colour: tolerated and ignored, never stored or written.
                "titlebarColor" => {}
                // Removed contrast-zone option: tolerated and dropped; contrast zones are always glass.
                "glassifyContrastZones" => {}
                // Renamed to surfaceOutline*: the old border values are dropped so the new white,
                // 50 %, 1 px default applies.
                "surfaceBorderColor" | "surfaceBorderOpacity" => {}
                // Removed Hermes URL key: tolerated and dropped, never stored or written.
                "assistant.hermesUrl" => {}
                // Legacy taskbar surface keys: read only, promoted to surface* below.
                "taskbarColor" => snapshot.taskbar_color = Slot::from_text(value, key)?,
                "taskbarOpacity" => snapshot.taskbar_opacity = Slot::from_text(value, key)?,
                "taskbarBlurPercent" => {
                    snapshot.taskbar_blur_percent = Slot::from_text(value, key)?;
                }
                _ if key.starts_with("scale.") => {
                    let output = key.trim_start_matches("scale.");
                    snapshot
                        .per_output_scale
                        .insert(output.to_owned(), parse(value, key)?);
                }
                _ if key.starts_with("panel.edge.") => {
                    let output = key.trim_start_matches("panel.edge.");
                    match parse_panel_edge(value) {
                        Some(edge) => snapshot.panel_edges.insert(output.to_owned(), edge),
                        None => return invalid("panel.edge must be bottom/top/left/right"),
                    };
                }
                _ if key.starts_with("hotkey.") => {
                    let action = key.trim_start_matches("hotkey.");
                    snapshot.hotkeys.insert(action.to_owned(), value.to_owned());
                }
                _ => {
                    snapshot
                        .unknown_keys
                        .insert(key.to_owned(), value.to_owned());
                }
            }
        }
        if has_legacy_taskbar_surface_key && !has_surface_key {
            snapshot.surface_color.clone_from(&snapshot.taskbar_color);
            snapshot.surface_opacity = snapshot.taskbar_opacity;
            snapshot.surface_blur_percent = snapshot.taskbar_blur_percent;
        }
        // Files written before the mode keys existed derive the mode from the saved values.
        if !has_wallpaper_mode {
            snapshot.wallpaper_mode = legacy_wallpaper_mode(&snapshot);
        }
        if !has_lock_background_mode {
            snapshot.lock_background_mode = legacy_lock_background_mode(&snapshot);
        }
        // One shared shape: square only when every present legacy button agrees; otherwise circle.
        if !has_button_style && !legacy_square_flags.is_empty() {
            let all_square = legacy_square_flags.iter().all(|square| *square);
            if all_square { "square" } else { "circle" }
                .clone_into(&mut snapshot.titlebar_button_style);
        }
        // Files without a group switch migrate: a group holding any non-default member is switched on.
        // Clean groups stay off. A file that names its switch keeps the stated value.
        for group in flamewm_api::settings_groups::SWITCH_GROUPS {
            if seen_keys.contains(group.switch) {
                continue;
            }
            let defaults = ProductSettings::default();
            let custom = group.members.iter().any(|(member, _)| {
                stable_setting(member)
                    .is_some_and(|row| (row.get)(&snapshot) != (row.get)(&defaults))
            });
            if let Some(row) = stable_setting(group.switch) {
                (row.set)(
                    &mut snapshot,
                    &defaults,
                    Some(&SettingValue::Boolean(custom)),
                    group.switch,
                )?;
            }
        }
        if snapshot.decoration == "ios" {
            "rounded".clone_into(&mut snapshot.decoration);
        }
        // Files clamp an out-of-range edge-snap distance instead of rejecting the whole file.
        snapshot.edge_snap_threshold_px = snapshot.edge_snap_threshold_px.clamp(4, 64);
        snapshot.validate()?;
        migrate_legacy_acceleration(&mut snapshot, has_render_backend);
        Ok(snapshot)
    }
}

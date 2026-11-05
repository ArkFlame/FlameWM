use super::*;

impl ConfigStore {
    #[must_use]
    pub fn serialize(snapshot: &ProductSettings) -> String {
        let defaults = ProductSettings::default();
        let mut lines = vec![format!("revision={}", snapshot.revision)];
        // Base rows are always written. Optional rows are written only when they differ from
        // defaults, so settings files that never set them keep their existing byte-for-byte shape.
        for row in STABLE_SETTINGS {
            if matches!(row.key, "wallpaperMode" | "lockBackgroundMode") {
                continue;
            }
            let value = (row.get)(snapshot);
            // A switch is written while its group holds a non-default stored value, so turning it
            // off never drops the saved members on reload.
            if row.base
                || value != (row.get)(&defaults)
                || switch_guards_custom_values(snapshot, row.key)
            {
                lines.push(format!("{}={}", row.key, wire_text(&value, row.base)));
            }
        }
        // A mode is written only when the legacy inference from the saved values would pick a
        // different one, so defaults stay silent and Default with a saved path still round-trips.
        if snapshot.wallpaper_mode != legacy_wallpaper_mode(snapshot) {
            lines.push(format!(
                "wallpaperMode={}",
                snapshot.wallpaper_mode.as_str()
            ));
        }
        if snapshot.lock_background_mode != legacy_lock_background_mode(snapshot) {
            lines.push(format!(
                "lockBackgroundMode={}",
                snapshot.lock_background_mode.as_str()
            ));
        }
        lines.extend(
            snapshot
                .per_output_scale
                .iter()
                .map(|(output, scale)| format!("scale.{output}={scale}")),
        );
        lines.extend(
            snapshot
                .panel_edges
                .iter()
                .map(|(output, edge)| format!("panel.edge.{output}={}", panel_edge_name(*edge))),
        );
        lines.extend(
            snapshot
                .hotkeys
                .iter()
                .map(|(action, binding)| format!("hotkey.{action}={binding}")),
        );
        lines.extend(
            snapshot
                .unknown_keys
                .iter()
                .map(|(key, value)| format!("{key}={value}")),
        );
        lines.push(String::new());
        lines.join("\n")
    }
}

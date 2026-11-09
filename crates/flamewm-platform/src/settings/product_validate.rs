use super::*;

impl ProductSettings {
    pub fn validate(&self) -> FlameResult<()> {
        validate_color(&self.taskbar_color, false, "taskbarColor")?;
        validate_color(&self.surface_color, false, "surfaceColor")?;
        validate_color(&self.surface_outline_color, false, "surfaceOutlineColor")?;
        validate_color(&self.window_outline_color, false, "windowOutlineColor")?;
        if self.window_outline_opacity > 100 {
            return invalid("windowOutlineOpacity must be 0..100");
        }
        if !(1..=4).contains(&self.window_outline_width) {
            return invalid("windowOutlineWidth must be 1..4");
        }
        if !(1..=4).contains(&self.surface_outline_width) {
            return invalid("surfaceOutlineWidth must be 1..4");
        }
        if self.surface_outline_opacity > 100 {
            return invalid("surfaceOutlineOpacity must be 0..100");
        }
        if self.surface_saturation_percent > 200 {
            return invalid("surfaceSaturationPercent must be 0..200");
        }
        if !(4..=100).contains(&self.glass_opacity_percent) {
            return invalid("glassOpacityPercent must be 4..100");
        }
        if self.glass_blur_px > 54 {
            return invalid("glassBlurPx must be 0..54");
        }
        if !matches!(
            self.taskbar_material_mode.as_str(),
            "blur-only" | "inherit" | "custom-tint"
        ) {
            return invalid("taskbarMaterialMode must be blur-only, inherit or custom-tint");
        }
        if self.taskbar_opacity_percent > 75 {
            return invalid("taskbarOpacityPercent must be 0..75");
        }
        validate_color(&self.taskbar_custom_tint, false, "taskbarCustomTint")?;
        validate_color(&self.accent, true, "accent")?;
        if self
            .wallpaper
            .chars()
            .any(|c| c == '\n' || c == '\r' || c == '\0')
        {
            return invalid("wallpaper path contains invalid control characters");
        }
        if self.desktop_selection_fill_opacity > 60 {
            return invalid("DesktopSelectionFillOpacity must be 0..60");
        }
        if !(50..=200).contains(&self.desktop_entry_scale_percent) {
            return invalid("DesktopEntryScalePercent must be 50..200");
        }
        if self.window_snap_preview_fill_opacity > 60 {
            return invalid("WindowSnapPreviewFillOpacity must be 0..60");
        }
        if !(-2..=4).contains(&self.font_size_offset) {
            return invalid("fontSizeOffset must be -2..4");
        }
        if self.taskbar_opacity > 100 {
            return invalid("taskbarOpacity must be 0..100");
        }
        validate_color(&self.dock_surface_color, false, "dock.surfaceColor")?;
        validate_color(
            &self.dock_surface_outline_color,
            false,
            "dock.surfaceOutlineColor",
        )?;
        if self.dock_surface_opacity > 100 || self.dock_surface_outline_opacity > 100 {
            return invalid("dock surface opacity values must be 0..100");
        }
        if self.dock_surface_blur_percent > 100 {
            return invalid("dock.surfaceBlurPercent must be 0..100");
        }
        if !(1..=4).contains(&self.dock_surface_outline_width) {
            return invalid("dock.surfaceOutlineWidth must be 1..4");
        }
        if !(24..=96).contains(&self.dock_icon_size) {
            return invalid("dock.iconSize must be 24..96");
        }
        if self.dock_pinned.len() > DOCK_PINNED_MAX
            || self.dock_pinned.iter().any(|id| !is_valid_dock_app_id(id))
        {
            return invalid("dock.pinned must hold at most 32 valid app ids");
        }
        if !(34..=72).contains(&self.taskbar_height) {
            return invalid("taskbarHeight must be 34..72");
        }
        if !(4..=64).contains(&self.edge_snap_threshold_px) {
            return invalid("edgeSnap.thresholdPx must be 4..64");
        }
        if self.window_margin > 32 {
            return invalid("windowMargin must be 0..32");
        }
        if self.taskbar_floating_margin > 32 {
            return invalid("taskbarFloatingMargin must be 0..32");
        }
        if self.taskbar_floating_radius > 24 {
            return invalid("taskbarFloatingRadius must be 0..24");
        }
        if self.taskbar_blur_percent > 100 {
            return invalid("taskbarBlurPercent must be 0..100");
        }
        if self.surface_opacity > 100 {
            return invalid("surfaceOpacity must be 0..100");
        }
        if self.surface_blur_percent > 100 {
            return invalid("surfaceBlurPercent must be 0..100");
        }
        if self.blue_light_filter > 100 {
            return invalid("blueLightFilter must be 0..100");
        }
        let minutes_per_day =
            u16::try_from(flamewm_api::blue_light::MINUTES_PER_DAY).unwrap_or(u16::MAX);
        if self.night_shift_start >= minutes_per_day {
            return invalid("nightShiftStart must be 0..1439");
        }
        if self.night_shift_end >= minutes_per_day {
            return invalid("nightShiftEnd must be 0..1439");
        }
        if !(100..=200).contains(&self.text_scale_percent) {
            return invalid("textScalePercent must be 100..200");
        }
        if !matches!(self.cursor_size, 24 | 32 | 48 | 64) {
            return invalid("cursorSize must be 24, 32, 48 or 64");
        }
        if !(flamewm_api::settings::POINTER_SPEED_MIN..=flamewm_api::settings::POINTER_SPEED_MAX)
            .contains(&self.pointer_speed)
        {
            return invalid("pointerSpeed must be -100..100");
        }
        if !flamewm_api::settings::KEYBOARD_LAYOUTS.contains(&self.keyboard_layout.as_str()) {
            return invalid("keyboardLayout must be a supported xkb layout");
        }
        if self.accent_opacity > 100 {
            return invalid("accentOpacity must be 0..100");
        }
        if !matches!(self.render_acceleration.as_str(), "cpu" | "gpu") {
            return invalid("renderAcceleration must be cpu or gpu");
        }
        if !matches!(
            self.render_backend.as_str(),
            "auto" | "cpu" | "opengl" | "vulkan"
        ) {
            return invalid("renderBackend must be auto, cpu, opengl or vulkan");
        }
        if !matches!(self.decoration.as_str(), "rounded" | "squared") {
            return invalid("decoration must be rounded or squared");
        }
        if !matches!(self.titlebar_button_style.as_str(), "circle" | "square") {
            return invalid("titlebarButtonStyle must be circle or square");
        }
        if self.cursor_theme.len() > 64
            || self
                .cursor_theme
                .chars()
                .any(|c| c == '/' || c == '\\' || c.is_control())
        {
            return invalid("cursorTheme must be at most 64 characters without path separators");
        }
        if !(25..=200).contains(&self.animation_speed) {
            return invalid("animationSpeed must be 25..200");
        }
        validate_color(&self.wallpaper_color, false, "wallpaperColor")?;
        validate_color(&self.lock_background_color, false, "lockBackgroundColor")?;
        validate_lock_image_path(&self.lock_background_image, "lockBackgroundImage")?;
        validate_lock_image_path(&self.avatar_image, "avatarImage")?;
        validate_terminal_program(&self.default_terminal)?;
        if let Some(message) =
            flamewm_api::settings::assistant_api_key_error(&self.assistant_api_key)
        {
            return invalid(message);
        }
        if !matches!(self.weather_units.as_str(), "celsius" | "fahrenheit") {
            return invalid("weatherUnits must be celsius or fahrenheit");
        }
        if !self.auto_time_zone.is_empty() && !is_valid_time_zone_name(&self.auto_time_zone) {
            return invalid("autoTimeZone must be an IANA time zone name");
        }
        if self.lock_background_blur_percent > 100 {
            return invalid("lockBackgroundBlurPercent must be 0..100");
        }
        if self.lock_background_dim_percent > 80 {
            return invalid("lockBackgroundDimPercent must be 0..80");
        }
        if self.lock_timeout_minutes > 240 {
            return invalid("lockTimeoutMinutes must be 0..240");
        }
        if self.screensaver_after_lock_minutes > 60 {
            return invalid("screensaverAfterLockMinutes must be 0..60");
        }
        if !(1..=12).contains(&self.screensaver_num_ribbons) {
            return invalid("screensaverNumRibbons must be 1..12");
        }
        if !(20..=280).contains(&self.screensaver_ribbon_width_tenths) {
            return invalid("screensaverRibbonWidthTenths must be 20..280");
        }
        if self.screensaver_display_off_minutes > 240 {
            return invalid("screensaverDisplayOffMinutes must be 0..240");
        }
        for output in self.panel_edges.keys() {
            if !is_valid_output_id(output) {
                return invalid("panel.edge output id is invalid");
            }
        }
        for (output, scale) in &self.per_output_scale {
            if !is_valid_output_id(output) {
                return invalid("perOutputScale output id is invalid");
            }
            if !crate::scale::is_supported_scale(*scale) {
                return invalid("perOutputScale supports 100..=200 in 5% steps");
            }
        }
        for action in self.hotkeys.keys() {
            if !KNOWN_HOTKEYS.contains(&action.as_str()) {
                return invalid("unknown hotkey action id");
            }
        }
        for key in self.unknown_keys.keys() {
            if is_stable_key(key) {
                return invalid("unknown key duplicates stable key");
            }
        }
        Ok(())
    }
}

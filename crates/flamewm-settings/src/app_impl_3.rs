//! Split from `lib.rs` (`app_impl_3`); see crate docs.

use super::*;

impl<T: ControlTransport> SettingsApplication<T> {
    /// Nav row index of the visible page: the six Appearance tabs first, then the rest.
    pub(crate) fn nav_index(&self) -> usize {
        match self.client.ui().page {
            SettingsPage::Appearance => self.appearance_tab as usize,
            SettingsPage::Desktop => 6,
            SettingsPage::Taskbar => 7,
            SettingsPage::Displays => 8,
            SettingsPage::Fonts => 9,
            SettingsPage::Hotkeys => 10,
            SettingsPage::Assistant => 11,
            SettingsPage::About => 12,
        }
    }

    pub(crate) fn sync_display_scale(
        &self,
        document: &mut impl UiDocumentAccess,
        preview: Option<u16>,
    ) -> Result<(), String> {
        let value = preview
            .or_else(|| {
                self.selected_display()
                    .map(|output| output.shell_scale_percent)
            })
            .unwrap_or(100);
        document.text("display-scale-value", format!("{value}%"))?;
        let ratio = f32::from(value.saturating_sub(100)) / 100.0;
        let thumb = ratio * ANIMATION_SPEED_TRACK_TRAVEL as f32;
        document.size(
            "display-scale-fill",
            thumb + (ANIMATION_SPEED_THUMB / 2.0) as f32,
            GLASS_SLIDER_TRACK_HEIGHT as f32,
        )?;
        document.position("display-scale-thumb", thumb, 8.5)
    }

    pub(crate) fn sync_appearance(
        &self,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        let gpu_selected = self
            .text_setting("renderAcceleration")
            .is_ok_and(|value| value == "gpu");
        optional_text(
            document,
            "render-acceleration-note",
            if gpu_selected {
                match flamewm_ui_x11::probe_gpu_renderer() {
                    Ok(renderer) => format!("GPU blur and backdrops run on {renderer}"),
                    Err(error) => format!("GPU unavailable, using CPU: {error}"),
                }
            } else {
                "CPU renders blur and backdrops. GPU moves that work to the graphics card and switches live"
                    .to_owned()
            },
        );
        self.sync_color_rows(document)?;
        self.sync_color_picker(document)?;
        self.sync_glass_rows(document)?;
        for (key, id) in [
            ("surfaceBlurPercent", "surface-blur-value"),
            ("surfaceSaturationPercent", "surface-saturation-value"),
            ("accentOpacity", "accent-opacity-value"),
        ] {
            if let Ok(value) = self.percent_setting(key) {
                document.text(id, format!("{value}%"))?;
            }
        }
        if let Ok(speed) = self.integer_setting("animationSpeed") {
            document.text(
                "animation-speed-value",
                format!("{:.2}x", speed as f64 / 100.0),
            )?;
            let ratio = (speed - ANIMATION_SPEED_MIN) as f64
                / (ANIMATION_SPEED_MAX - ANIMATION_SPEED_MIN) as f64;
            let fill = ratio * ANIMATION_SPEED_TRACK_TRAVEL;
            let fill_width = (fill + ANIMATION_SPEED_THUMB / 2.0).min(ANIMATION_SPEED_TRACK_WIDTH);
            document.size(
                "animation-speed-fill",
                fill_width as f32,
                GLASS_SLIDER_TRACK_HEIGHT as f32,
            )?;
            document.position(
                "animation-speed-thumb",
                fill as f32,
                GLASS_SLIDER_THUMB_TOP as f32,
            )?;
        }
        Ok(())
    }

    /// Projects the glass sliders, the taskbar material segments and the twins that show
    /// the unavailable state (inherit mode hides the opacity slider, blur-only and inherit hide the tint).
    pub(crate) fn sync_glass_rows(
        &self,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        let accent = self.accent()?;
        for slider in &GLASS_SLIDERS {
            let Some(value) = self.number_setting(slider.key) else {
                continue;
            };
            let ratio = (value - slider.minimum) as f64 / (slider.maximum - slider.minimum) as f64;
            let fill = ratio.clamp(0.0, 1.0) * GLASS_SLIDER_TRAVEL;
            let fill_width = (fill + GLASS_SLIDER_THUMB / 2.0).min(GLASS_SLIDER_TRACK_WIDTH);
            let label = if slider.key == "glassBlurPx" {
                format!("{value} px")
            } else {
                format!("{value}%")
            };
            document.text(&format!("{}-value", slider.slug), label)?;
            document.size(
                &format!("{}-fill", slider.slug),
                fill_width as f32,
                GLASS_SLIDER_TRACK_HEIGHT as f32,
            )?;
            document.position(
                &format!("{}-thumb", slider.slug),
                fill as f32,
                GLASS_SLIDER_THUMB_TOP as f32,
            )?;
        }
        let mode = Some(
            self.text_setting("taskbarMaterialMode")
                .unwrap_or_else(|_| "inherit".to_owned()),
        );
        highlight_options(
            document,
            mode.as_deref(),
            &[
                ("blur-only", "taskbar-material-blur-only"),
                ("inherit", "taskbar-material-inherit"),
                ("custom-tint", "taskbar-material-custom-tint"),
            ],
            accent,
        )?;
        let inherits = mode.as_deref() == Some("inherit");
        document.visible("taskbar-opacity-row", !inherits)?;
        document.visible("taskbar-opacity-live", !inherits)?;
        let custom = mode.as_deref() == Some("custom-tint");
        document.visible("taskbar-tint-row", custom)?;
        document.visible("taskbar-tint-live", custom)?;
        // Custom accent off hides the swatches and picker; the system accent applies.
        let custom_accent = matches!(
            self.setting_value("accentCustom"),
            Some(SettingValue::Boolean(true))
        );
        document.visible("accent-custom-row", custom_accent)?;
        Ok(())
    }

    /// Highlights the option matching the authoritative `key` setting; an unset key leaves the row alone.
    pub(crate) fn sync_option_setting(
        &self,
        document: &mut impl UiDocumentAccess,
        key: &str,
        options: &[(&str, &str)],
    ) -> Result<(), String> {
        let Ok(current) = self.text_setting(key) else {
            return Ok(());
        };
        let accent = self.accent()?;
        highlight_options(document, Some(&current), options, accent)
    }

    /// Highlights the preset that matches each surface setting, and shows each
    /// setting's current colour on its wheel circle.
    pub(crate) fn sync_color_rows(
        &self,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        for (target, key) in COLOR_TARGETS {
            let Ok(value) = self.text_setting(key) else {
                continue;
            };
            let current = solid_color(&value)?;
            document.background(&format!("color-{target}-current"), current)?;
            if target == "accent" {
                continue;
            }
            for (index, preset) in SURFACE_PRESETS.iter().enumerate() {
                document.background(
                    &format!("color-{target}-preset-{index}"),
                    solid_color(preset)?,
                )?;
                document.visible(
                    &format!("color-{target}-selected-{index}"),
                    preset.eq_ignore_ascii_case(&value),
                )?;
            }
        }
        Ok(())
    }

    /// Projects the shared picker panel: open state, the hex buffer with its focus
    /// caret, the preview, the inline error and the palette cells.
    pub(crate) fn sync_color_picker(
        &self,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        for hue in 0..PICKER_HUES {
            for shade in 0..PICKER_SHADES {
                let id = format!("color-picker-grid-{hue}-{shade}");
                document.background(&id, solid_color(&picker_grid_color(hue, shade))?)?;
            }
        }
        for step in 0..PICKER_GREYS {
            let id = format!("color-picker-grid-grey-{step}");
            document.background(&id, solid_color(&picker_grey(step))?)?;
        }
        let Some(picker) = self.color_picker.as_ref() else {
            document.visible("color-picker", false)?;
            return Ok(());
        };
        document.visible("color-picker", true)?;
        let caret = if picker.focused { "|" } else { "" };
        let hex = if picker.buffer.is_empty() {
            "#RRGGBB".to_owned()
        } else {
            format!("{}{caret}", picker.buffer)
        };
        document.text("color-picker-hex", hex)?;
        let shown = parse_hex_color(&picker.buffer)
            .ok()
            .or_else(|| self.text_setting(picker.key).ok())
            .unwrap_or_else(|| SURFACE_PRESETS[0].to_owned());
        document.background("color-picker-preview", solid_color(&shown)?)?;
        optional_text(
            document,
            "color-picker-error",
            picker
                .error
                .clone()
                .unwrap_or_else(|| "\u{200b}".to_owned()),
        );
        Ok(())
    }

    /// Highlights, in each animation style row, the segment whose value matches the stored style.
    pub(crate) fn sync_animation_styles(
        &self,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        let accent = self.accent()?;
        for (key, values) in ANIMATION_STYLE_ROWS {
            let Ok(current) = self.text_setting(key) else {
                continue;
            };
            for value in values {
                let id = animation_style_node(key, value);
                highlight_segment(document, &id, current == *value, accent)?;
            }
        }
        Ok(())
    }

    /// Projects the lock-screen background path (or `None`) and highlights the clock and timeout segments.
    pub(crate) fn sync_lock_screen(
        &self,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if let Ok(mode) = self.text_setting("lockBackgroundMode") {
            let label = match mode.as_str() {
                "solid" => "Solid colour".to_owned(),
                "custom" => self.text_setting("lockBackgroundImage").unwrap_or_default(),
                _ => "Built-in".to_owned(),
            };
            document.text("lock-background-image-value", label)?;
        }
        if let Ok(path) = self.text_setting("avatarImage") {
            document.text("avatar-value", avatar_label(&path))?;
        }
        let accent = self.accent()?;
        let clock = self.text_setting("lockClockFormat").ok();
        highlight_options(
            document,
            clock.as_deref(),
            &[("24h", "lock-clock-24h"), ("12h", "lock-clock-12h")],
            accent,
        )?;
        let timeout = self.integer_setting("lockTimeoutMinutes").ok();
        for minutes in LOCK_TIMEOUT_CHOICES {
            let id = format!("lock-timeout-{minutes}");
            highlight_segment(document, &id, timeout == Some(minutes), accent)?;
        }
        Ok(())
    }

    /// Highlights the segment rows (taskbar layout, clock, cursor size, screensaver mode) and shows
    /// the floating-only taskbar steppers only while `taskbarStyle` is `floating`.
    pub(crate) fn sync_segment_rows(
        &self,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        let accent = self.accent()?;
        let style = self.text_setting("taskbarStyle").ok();
        highlight_options(
            document,
            style.as_deref(),
            &[
                ("classic", "taskbar-style-classic"),
                ("floating", "taskbar-style-floating"),
            ],
            accent,
        )?;
        let floating = style.as_deref() == Some("floating");
        document.visible("taskbar-floating-margin-row", floating)?;
        document.visible("taskbar-floating-radius-row", floating)?;
        let clock = self.text_setting("taskbarClockFormat").ok();
        highlight_options(
            document,
            clock.as_deref(),
            &[("24h", "taskbar-clock-24h"), ("12h", "taskbar-clock-12h")],
            accent,
        )?;
        let appearance = self.text_setting("appearance").ok();
        highlight_options(
            document,
            appearance.as_deref(),
            &[
                ("dark", "appearance-apps-dark"),
                ("light", "appearance-apps-light"),
            ],
            accent,
        )?;
        let size = self.integer_setting("cursorSize").ok();
        for value in CURSOR_SIZE_CHOICES {
            highlight_segment(
                document,
                &format!("cursor-size-{value}"),
                size == Some(value),
                accent,
            )?;
        }
        let mode = self.text_setting("screensaverMode").ok();
        highlight_options(
            document,
            mode.as_deref(),
            &[
                ("ribbons", "screensaver-mode-ribbons"),
                ("none", "screensaver-mode-none"),
            ],
            accent,
        )?;
        Ok(())
    }

    pub(crate) fn sync_error(&self, document: &mut impl UiDocumentAccess) {
        optional_text(document, ERROR_NODE, self.error.clone().unwrap_or_default());
    }

    /// Names the output the Taskbar page configures and lists the outputs it can switch to.
    /// The selection is the shared `selected_output` authority.
    pub(crate) fn sync_taskbar_outputs(&self, document: &mut impl UiDocumentAccess) {
        let title = match self.selected_display() {
            Some(output) => format!("Configuring output: {}", output.connector),
            None => "No output selected".to_owned(),
        };
        optional_text(document, "taskbar-output-title", title);
        for slot in 0..OUTPUT_PAGE_SLOTS {
            let id = format!("taskbar-output-{slot}");
            match self.output_page_slot(slot) {
                Some(output) => {
                    let selected = self.client.ui().selected_output.as_ref() == Some(&output.id);
                    let label = if selected {
                        format!("{} (selected)", output.connector)
                    } else {
                        output.connector.clone()
                    };
                    optional_visible(document, &id, true);
                    optional_text(document, &id, label);
                }
                None => optional_visible(document, &id, false),
            }
        }
        let _ = project_pager(
            document,
            &TASKBAR_OUTPUT_PAGER,
            self.pages.output_page,
            page_count(self.output_count(), OUTPUT_PAGE_SLOTS),
            page_label(
                self.pages.output_page,
                OUTPUT_PAGE_SLOTS,
                self.output_count(),
            ),
        );
    }

    /// Projects each curated row's authoritative binding, or the active capture prompt.
    pub(crate) fn sync_hotkeys(&self, document: &mut impl UiDocumentAccess) {
        let capturing = if self.hotkey_armed {
            self.client
                .ui()
                .hotkey_capture
                .as_ref()
                .map(|capture| capture.action.clone())
        } else {
            None
        };
        for row in &HOTKEY_ROWS {
            if capturing.as_deref() == Some(row.action) {
                optional_text(document, row.node, "Press a key...".to_owned());
                continue;
            }
            let Some(snapshot) = self.client.shortcuts() else {
                continue;
            };
            let Some(flamewm_api::shortcuts::KeyBinding(binding)) =
                snapshot.bindings.get(row.action)
            else {
                continue;
            };
            let text = binding
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or("Not assigned")
                .to_owned();
            optional_text(document, row.node, text);
        }
    }

    /// Projects the selected output's mode slots, overflow count and the
    /// Keep/Revert confirmation row from the authoritative display snapshot.
    pub(crate) fn sync_display_modes(
        &self,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        let slots = self.client.display_mode_page(self.pages.mode_page);
        let current = self.selected_display().map(|output| output.current_mode);
        for slot in 0..DISPLAY_MODE_SLOTS {
            let id = format!("display-mode-slot-{slot}");
            match slots.get(slot) {
                Some(mode) => {
                    document.visible(&id, true)?;
                    document.text(&id, display_mode_label(mode, current == Some(mode.id)))?;
                }
                None => document.visible(&id, false)?,
            }
        }
        let total = self.client.display_mode_count();
        let pages = page_count(total, DISPLAY_MODE_SLOTS);
        project_pager(
            document,
            &MODE_PAGER,
            self.pages.mode_page,
            pages,
            page_label(self.pages.mode_page, DISPLAY_MODE_SLOTS, total),
        )?;
        let note = if total == 0 {
            "This output reports no selectable modes".to_owned()
        } else {
            "\u{200b}".to_owned()
        };
        document.visible("display-mode-overflow", total == 0)?;
        document.text("display-mode-overflow", note)?;
        let pending = self.client.ui().pending_display_transaction.is_some();
        document.visible("display-confirm", pending)?;
        Ok(())
    }

    /// Highlights the saved units and shows the saved weather city (or the unset hint).
    pub(crate) fn sync_weather(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let units = self
            .text_setting("weatherUnits")
            .unwrap_or_else(|_| "celsius".to_owned());
        let accent = self.accent()?;
        highlight_options(
            document,
            Some(&units),
            &[
                ("celsius", "weather-units-celsius"),
                ("fahrenheit", "weather-units-fahrenheit"),
            ],
            accent,
        )?;
        let location = self.text_setting("weatherLocation").unwrap_or_default();
        let automatic = self.bool_setting("autoLocationEnabled");
        document.text(
            "weather-location-value",
            weather_location_text(&location, automatic),
        )
    }

    pub(crate) fn sync_time_zone(
        &self,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        let automatic_zone = self.bool_setting("autoTimeZoneEnabled");
        let cached = self.text_setting("autoTimeZone").unwrap_or_default();
        let zone = time_zone_label(automatic_zone, &cached);
        let city = if self.bool_setting("autoLocationEnabled") {
            let location = self.text_setting("weatherLocation").unwrap_or_default();
            weather_location_label(&location)
        } else {
            "not detected".to_owned()
        };
        document.text(
            "time-zone-value",
            format!("Current time zone: {zone} - detected city: {city}"),
        )
    }

    /// Writes the authoritative value text into every value pill and select
    /// label. Missing snapshot values leave the authored placeholder untouched.
    pub(crate) fn sync_value_text(
        &self,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        let percent = |key: &str| match self.setting_value(key) {
            Some(SettingValue::Percent(value)) => Some(*value),
            _ => None,
        };
        let integer = |key: &str| match self.setting_value(key) {
            Some(SettingValue::Integer(value)) => Some(*value),
            _ => None,
        };
        let text = |key: &str| match self.setting_value(key) {
            Some(SettingValue::Text(value)) => Some(value.clone()),
            _ => None,
        };
        if let Some(value) = percent("DesktopSelectionFillOpacity") {
            document.text("selection-opacity-value", format!("{value}%"))?;
        }
        if let Some(value) = percent("WindowSnapPreviewFillOpacity") {
            document.text("snap-opacity-value", format!("{value}%"))?;
        }
        if let Some(value) = integer("DesktopEntryScalePercent") {
            document.text("desktop-entry-size-value", format!("{value}%"))?;
        }
        for kind in [
            theme_picker::PickerKind::Icon,
            theme_picker::PickerKind::Cursor,
        ] {
            if let Some(value) = text(kind.setting_key()) {
                document.text(
                    if kind == theme_picker::PickerKind::Icon {
                        "icon-theme-selected-name"
                    } else {
                        kind.select_box()
                    },
                    self.themes.label(kind, &value),
                )?;
                if kind == theme_picker::PickerKind::Icon {
                    let preview = self.themes.selected_preview(kind, &value);
                    document.visible("icon-theme-selected-preview", preview.is_some())?;
                    if let Some(image) = preview {
                        document.image_rgba8_shared(
                            "icon-theme-selected-preview",
                            image.source.clone(),
                            image.width,
                            image.height,
                            Arc::clone(&image.pixels),
                        )?;
                    }
                }
            }
        }
        document.text("file-manager-value", self.file_manager_label.clone())?;
        if let Some(value) = percent("surfaceOpacity") {
            document.text("surface-opacity-value", format!("{value}%"))?;
        }
        if let Some(value) = percent("surfaceBorderOpacity") {
            document.text("surface-border-opacity-value", format!("{value}%"))?;
        }
        if let Some(value) = text("customStartIcon") {
            document.text("start-icon-value", start_icon_label(&value))?;
        }
        for row in INTEGER_STEPPERS {
            if let Some(value) = integer(row.key) {
                let label = if row.key == "screensaverRibbonWidthTenths" {
                    format!("{:.1} px", value as f64 / 10.0)
                } else if row.key == "windowMargin" {
                    format!("{value} px")
                } else {
                    value.to_string()
                };
                document.text(&format!("{}-value", row.slug), label)?;
            }
        }
        if let Some(value) = text("wallpaperFit") {
            document.text("wallpaper-fit-value", wallpaper_fit_label(&value))?;
        }
        if let Some(value) = integer("taskbarHeight") {
            document.text("taskbar-size-value", format!("{value} px (all outputs)"))?;
        }
        if let Some(value) = text("fontFamily") {
            document.text("font-family-value", value)?;
        }
        if let Some(value) = integer("fontSizeOffset") {
            document.text("font-size-value", font_size_label(value))?;
        }
        let selected_edge = self
            .selected_display()
            .and_then(|output| {
                self.client.panels().and_then(|snapshot| {
                    snapshot
                        .panels
                        .iter()
                        .find(|panel| panel.output == output.id)
                })
            })
            .map(|panel| panel.edge);
        if let Some(edge) = selected_edge {
            document.text("taskbar-edge-value", panel_edge_label(edge).to_owned())?;
        }
        Ok(())
    }
}

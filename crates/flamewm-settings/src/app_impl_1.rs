//! Split from `lib.rs` (`app_impl_1`); see crate docs.

use super::*;

impl<T: ControlTransport> SettingsApplication<T> {
    pub fn new(transport: T) -> Self {
        Self {
            client: SettingsClient::new(transport),
            wallpaper_chooser: None,
            chooser_target: ChooserTarget::Wallpaper,
            pending_open: None,
            hotkey_armed: false,
            error: None,
            pages: DisplayPages::default(),
            themes: theme_picker::ThemeRegistry::default(),
            theme_picker: None,
            color_picker: None,
            theme_scan: None,
            theme_install: None,
            theme_status: None,
            appearance_tab: AppearanceTab::Theme,
            applied_cursor_theme: None,
            monitor_drag: None,
            scale_drag: None,
            scale_slider_focused: false,
            nav_query: String::new(),
            nav_search_focused: false,
            api_key_draft: None,
            file_manager_label: "FlameWM Explorer".into(),
        }
    }

    pub fn client(&self) -> &SettingsClient<T> {
        &self.client
    }
    pub fn client_mut(&mut self) -> &mut SettingsClient<T> {
        &mut self.client
    }

    /// Message from the last failed request, shown inline until the next success.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn refresh(&mut self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        self.client.refresh_all().map_err(control_error)?;
        self.file_manager_label = file_manager::current();
        self.sync_cursor_theme();
        self.reconcile_pages();
        self.sync(document)
    }

    /// Shows a failed request inline. The authoritative snapshots are left as
    /// they were, so the previous values stay on screen.
    pub fn report_error(&mut self, document: &mut impl UiDocumentAccess, message: String) {
        self.error = Some(message);
        self.sync_error(document);
    }

    pub fn handle_action(
        &mut self,
        event: &UiActionEvent,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if event.action == DISPLAY_SCALE_SLIDER {
            return self.handle_display_scale_drag(event, document);
        }
        if event.action == ANIMATION_SPEED_SLIDER {
            return self.handle_animation_speed_drag(event, document);
        }
        if let Some(slider) = glass_slider(&event.action) {
            return self.handle_glass_slider_drag(slider, event, document);
        }
        if let Some(raw_slot) = event.action.strip_prefix("settings.display.select.") {
            if let Ok(slot) = raw_slot.parse::<usize>() {
                if event.phase == UiActionPhase::Press {
                    self.monitor_drag = self.output_page_slot(slot).and_then(|output| {
                        self.client
                            .displays()
                            .map(|snapshot| (slot, event.x, snapshot.generation, output.id.clone()))
                    });
                    return Ok(());
                }
                if event.phase == UiActionPhase::Motion {
                    if let Some((pressed_slot, origin_x, _, _)) = &self.monitor_drag {
                        if *pressed_slot == slot && (event.x - origin_x).abs() >= 40.0 {
                            // Only visual feedback while dragging, never native RandR state.
                            let accent = self.accent()?;
                            document.border(&format!("display-output-{slot}"), accent)?;
                        }
                    }
                    return Ok(());
                }
                if event.phase == UiActionPhase::Release {
                    let origin = self.monitor_drag.take();
                    if let Some((_pressed_slot, initial_x, generation, pressed_id)) =
                        origin.filter(|(pressed_slot, _, _, _)| *pressed_slot == slot)
                    {
                        let distance = event.x - initial_x;
                        if distance.abs() >= 40.0 {
                            let current_generation =
                                self.client.displays().map_or(0, |s| s.generation);
                            if generation != current_generation
                                || self
                                    .output_page_slot(slot)
                                    .is_none_or(|o| o.id != pressed_id)
                            {
                                return Err("display topology changed during monitor drag".into());
                            }
                            self.client.select_output(&pressed_id);
                            let side = if distance < 0.0 {
                                display_layout::Side::Left
                            } else {
                                display_layout::Side::Right
                            };
                            self.arrange_display(side)?;
                            return self.settle(document);
                        }
                    }
                }
            }
        }
        if event.phase == UiActionPhase::Press {
            self.scale_slider_focused = false;
        }
        if event.phase != UiActionPhase::Release || !event.inside {
            return Ok(());
        }
        if event.action == KEYBOARD_INPUT_ACTION && self.nav_search_focused {
            self.nav_search_text(event.text.as_deref().unwrap_or_default());
            return self.settle(document);
        }
        if event.action == KEYBOARD_INPUT_ACTION && self.api_key_draft.is_some() {
            self.api_key_text(event.text.as_deref().unwrap_or_default());
            return self.settle(document);
        }
        if event.action == KEYBOARD_INPUT_ACTION && self.color_picker.is_some() {
            self.color_picker_text(event.text.as_deref().unwrap_or_default())?;
            return self.settle(document);
        }
        let action = parse_action(event.action.as_str())?;
        self.execute(action)?;
        self.settle(document)
    }

    /// Drag preview is display-local and never persists until the pointer releases,
    /// including release outside the original slider bounds.
    pub(crate) fn handle_display_scale_drag(
        &mut self,
        event: &UiActionEvent,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if event.button != 1 {
            return Ok(());
        }
        match event.phase {
            UiActionPhase::Press => {
                let (id, generation, value) = self.scale_selection()?;
                self.scale_drag = Some((id, generation, value));
                self.scale_slider_focused = true;
            }
            UiActionPhase::Motion | UiActionPhase::Release => {
                let Some((id, generation, _)) = self.scale_drag.clone() else {
                    return Ok(());
                };
                let Some(text) = event.text.as_deref() else {
                    if event.phase == UiActionPhase::Release {
                        self.scale_drag = None;
                    }
                    return Ok(());
                };
                let percent = display_scale_from_slider(text)?;
                if event.phase == UiActionPhase::Release {
                    self.scale_drag = None;
                    let (current_id, current_generation, current_scale) = self.scale_selection()?;
                    if current_id != id || current_generation != generation {
                        return Err("display topology changed during scale adjustment".into());
                    }
                    if percent != current_scale {
                        self.client
                            .set_shell_scale(percent)
                            .map_err(control_error)?;
                    }
                    return self.settle(document);
                }
                self.scale_drag = Some((id, generation, percent));
                self.sync_display_scale(document, Some(percent))?;
            }
            UiActionPhase::Hover => {}
        }
        Ok(())
    }

    pub(crate) fn scale_selection(&self) -> Result<(OutputId, u64, u16), String> {
        let snapshot = self
            .client
            .displays()
            .ok_or("display snapshot unavailable")?;
        let output = self.selected_display().ok_or("no selected display")?;
        Ok((
            output.id.clone(),
            snapshot.generation,
            output.shell_scale_percent,
        ))
    }

    /// The native range drag reports the pointer as `text` (0..=100) on every
    /// Motion and on the final Release, even when the pointer left the track.
    pub(crate) fn handle_animation_speed_drag(
        &mut self,
        event: &UiActionEvent,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if !matches!(event.phase, UiActionPhase::Motion | UiActionPhase::Release) {
            return Ok(());
        }
        let Some(text) = event.text.as_deref() else {
            return Ok(());
        };
        let speed = animation_speed_from_slider(text)?;
        if self.integer_setting("animationSpeed").ok() != Some(speed) {
            self.client
                .apply_setting("animationSpeed", SettingValue::Integer(speed))
                .map_err(control_error)?;
        }
        self.settle(document)
    }

    /// Live drag for the glass sliders: the native track reports 0..=100 on Motion and Release.
    /// The taskbar opacity slider is inert while the taskbar inherits its material.
    pub(crate) fn handle_glass_slider_drag(
        &mut self,
        slider: &GlassSlider,
        event: &UiActionEvent,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        if !matches!(event.phase, UiActionPhase::Motion | UiActionPhase::Release) {
            return Ok(());
        }
        let Some(text) = event.text.as_deref() else {
            return Ok(());
        };
        if slider.key == "taskbarOpacityPercent" && self.taskbar_inherits() {
            return Ok(());
        }
        let value = glass_slider_value(text, slider)?;
        if self.number_setting(slider.key) != Some(value) {
            let value = u8::try_from(value).map_err(|_| "glass slider value out of range")?;
            self.client
                .apply_setting(slider.key, SettingValue::Percent(value))
                .map_err(control_error)?;
        }
        self.settle(document)
    }

    /// `taskbarMaterialMode` is `inherit`, which makes the taskbar opacity unavailable.
    pub(crate) fn taskbar_inherits(&self) -> bool {
        self.text_setting("taskbarMaterialMode")
            .map_or(true, |mode| mode == "inherit")
    }

    pub fn handle_key_event(
        &mut self,
        event: &UiKeyEvent,
        document: &mut impl UiDocumentAccess,
    ) -> Result<(), String> {
        // Escape cancels an in-progress monitor gesture without ever applying RandR.
        if event.phase == UiKeyPhase::Release
            && event.key == "Escape"
            && self.monitor_drag.take().is_some()
        {
            return self.settle(document);
        }
        if event.phase == UiKeyPhase::Release && !self.hotkey_armed && self.scale_slider_focused {
            if event.key == "Escape" {
                self.scale_slider_focused = false;
                self.scale_drag = None;
                return self.settle(document);
            }
            let delta = match event.key.as_str() {
                "Left" | "Down" => -5,
                "Right" | "Up" => 5,
                "Home" => -100,
                "End" => 100,
                _ => 0,
            };
            if delta != 0 && event.modifiers.is_empty() {
                let (_, _, current) = self.scale_selection()?;
                let target = (i32::from(current) + delta).clamp(100, 200) as u16;
                if current != target {
                    self.client.set_shell_scale(target).map_err(control_error)?;
                }
                return self.settle(document);
            }
        }
        if event.phase == UiKeyPhase::Release && self.hotkey_armed {
            if let Some(binding) = bare_super_binding(event) {
                self.finish_hotkey_capture(binding)?;
            } else if !is_modifier_key(&event.key) {
                let binding = if event.modifiers.is_empty() {
                    event.key.clone()
                } else {
                    format!("{}+{}", event.modifiers.join("+"), event.key)
                };
                self.finish_hotkey_capture(&binding)?;
            }
        }
        self.settle(document)
    }

    pub fn poll_background(&mut self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let mut completion_errors: Vec<String> = self.poll_open_uri().err().into_iter().collect();
        if self.poll_theme_jobs() {
            if let Err(error) = self.sync(document) {
                completion_errors.push(error);
            }
        }
        let result = self
            .wallpaper_chooser
            .as_mut()
            .and_then(file_chooser::WallpaperChooserWorker::poll);
        if let Some(result) = result {
            self.wallpaper_chooser = None;
            let mut wallpaper_changed = false;
            match result {
                Ok(Some(path)) => {
                    let applied = match import_for_setting(self.chooser_target.media_kind(), &path)
                    {
                        Ok(managed) => match self.chooser_target {
                            ChooserTarget::Wallpaper => self
                                .client
                                .set_wallpaper(managed)
                                .map_err(control_error)
                                .and_then(|()| {
                                    self.apply_text("wallpaperMode", "custom".to_owned())
                                }),
                            ChooserTarget::LockBackground => self
                                .apply_text("lockBackgroundImage", managed)
                                .and_then(|()| {
                                    self.apply_text("lockBackgroundMode", "custom".to_owned())
                                }),
                            ChooserTarget::Avatar => self.apply_text("avatarImage", managed),
                            ChooserTarget::StartIcon => self.replace_start_icon(managed),
                        },
                        Err(error) => Err(error),
                    };
                    match applied {
                        Ok(()) => wallpaper_changed = true,
                        Err(error) => completion_errors.push(error),
                    }
                }
                Ok(None) => {}
                Err(error) => completion_errors.push(error),
            }
            if wallpaper_changed {
                self.settle(document)?;
            }
        }
        if completion_errors.is_empty() {
            Ok(())
        } else {
            Err(completion_errors.join("; "))
        }
    }

    /// Clears the inline error and projects the authoritative snapshots. Called
    /// only after a request succeeded.
    /// Reloads this window's cursor owners when the resolved `cursorTheme` changed. A failed
    /// reload keeps the previous theme (rollback) and is retried at the next settings snapshot.
    pub(crate) fn sync_cursor_theme(&mut self) {
        let shadow_enabled = !matches!(
            self.setting_value("cursorShadow"),
            Some(SettingValue::Boolean(false))
        );
        for error in flamewm_ui_x11::apply_cursor_shadow(shadow_enabled) {
            eprintln!("settings: cursor shadow reload failed: {error}");
        }
        let Ok(requested) = self.text_setting("cursorTheme") else {
            return;
        };
        let search_path = std::env::var("XCURSOR_PATH").unwrap_or_default();
        let Some(theme) = flamewm_session_core::cursor_theme_reload(
            self.applied_cursor_theme.as_deref(),
            &requested,
            &search_path,
        ) else {
            return;
        };
        let errors = flamewm_ui_x11::apply_cursor_theme(&theme);
        if errors.is_empty() {
            self.applied_cursor_theme = Some(theme);
        }
        for error in errors {
            eprintln!("settings: cursor theme reload failed: {error}");
        }
    }

    pub(crate) fn settle(&mut self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        self.error = None;
        self.sync_cursor_theme();
        self.reconcile_pages();
        self.sync(document)
    }

    /// Clamps both page cursors to the authoritative snapshots and stamps them with the
    /// current display generation. Runs after every successful refresh or action.
    pub(crate) fn reconcile_pages(&mut self) {
        let generation = self
            .client
            .displays()
            .map_or(0, |snapshot| snapshot.generation);
        let output_pages = page_count(self.output_count(), OUTPUT_PAGE_SLOTS);
        let mode_pages = page_count(self.client.display_mode_count(), DISPLAY_MODE_SLOTS);
        self.pages = DisplayPages {
            generation: Some(generation),
            output_page: self.pages.output_page.min(output_pages.saturating_sub(1)),
            mode_page: self.pages.mode_page.min(mode_pages.saturating_sub(1)),
        };
    }

    pub(crate) fn output_count(&self) -> usize {
        self.client
            .displays()
            .map_or(0, |snapshot| snapshot.outputs.len())
    }

    /// Stable visual left-to-right ordering derived from actual `RandR` geometry,
    /// not X server enumeration order. Output IDs remain the native authority.
    pub(crate) fn ordered_output_indices(&self) -> Vec<usize> {
        let Some(snapshot) = self.client.displays() else {
            return vec![];
        };
        let mut indices: Vec<_> = (0..snapshot.outputs.len()).collect();
        indices.sort_by_key(|&i| {
            let o = &snapshot.outputs[i];
            (
                !o.geometry.is_valid(),
                if o.geometry.is_valid() {
                    o.geometry.x
                } else {
                    i32::MAX
                },
                if o.geometry.is_valid() {
                    o.geometry.y
                } else {
                    i32::MAX
                },
                i,
            )
        });
        indices
    }

    pub(crate) fn output_page_index(&self, slot: usize) -> Option<usize> {
        let absolute = self
            .pages
            .output_page
            .checked_mul(OUTPUT_PAGE_SLOTS)?
            .checked_add(slot)?;
        self.ordered_output_indices().get(absolute).copied()
    }

    /// Output row `slot` in visual left-to-right order; page/slot and the
    /// selection/drag input path MUST use this same mapping.
    pub(crate) fn output_page_slot(&self, slot: usize) -> Option<&flamewm_api::display::OutputSnapshot> {
        self.client
            .displays()?
            .outputs
            .get(self.output_page_index(slot)?)
    }

    pub(crate) fn execute(&mut self, action: SettingsAction) -> Result<(), String> {
        // A native key id (e.g. the Super release that ends a capture gesture)
        // must not disarm capture; the key-event path decides when it commits.
        if !matches!(
            action,
            SettingsAction::Hotkey(_, HotkeyOperation::Capture) | SettingsAction::NativeKeyEvent
        ) {
            self.client.cancel_hotkey_capture().map_err(control_error)?;
            self.hotkey_armed = false;
        }
        if let Some(picker) = self.color_picker.as_mut() {
            if !matches!(
                action,
                SettingsAction::ColorPickerFocus | SettingsAction::NativeKeyEvent
            ) {
                picker.focused = false;
            }
        }
        // Any other control leaves the API key field without saving its draft.
        if !matches!(
            action,
            SettingsAction::ApiKeyFocus
                | SettingsAction::ApiKeyReset
                | SettingsAction::NativeKeyEvent
        ) {
            self.api_key_draft = None;
        }
        match action {
            SettingsAction::FileManagerCycle => {
                self.file_manager_label = file_manager::toggle()?;
            }
            SettingsAction::NativeKeyEvent => {}
            SettingsAction::ApiKeyFocus => {
                if self.api_key_draft.is_none() {
                    let buffer = self.text_setting("assistant.apiKey").unwrap_or_default();
                    self.api_key_draft = Some(ApiKeyDraft {
                        buffer,
                        error: None,
                    });
                }
            }
            SettingsAction::ApiKeyReset => {
                self.api_key_draft = None;
                self.apply_text("assistant.apiKey", String::new())?;
            }
            SettingsAction::AssistantOpen => {
                std::process::Command::new("flamewm-assistant")
                    .spawn()
                    .map_err(|error| format!("Could not open the assistant: {error}"))?;
            }
            SettingsAction::ColorPickerOpen(key) => {
                self.color_picker = Some(ColorPicker {
                    key,
                    buffer: String::new(),
                    focused: true,
                    error: None,
                });
            }
            SettingsAction::ColorPickerClose => self.color_picker = None,
            SettingsAction::ColorPickerFocus => {
                if let Some(picker) = self.color_picker.as_mut() {
                    picker.focused = true;
                }
            }
            SettingsAction::ColorPickerPick(value) => {
                let key = self.open_color_picker()?.key;
                self.apply_text(key, value.clone())?;
                if let Some(picker) = self.color_picker.as_mut() {
                    picker.buffer = value;
                    picker.error = None;
                }
            }
            SettingsAction::ColorPickerApply => self.apply_picker_buffer()?,
            SettingsAction::Page(page) => {
                self.scale_slider_focused = false;
                self.scale_drag = None;
                self.client.set_page(page);
            }
            SettingsAction::AccentSwatch(accent) => {
                self.apply_text("accent", accent.to_owned())?;
            }
            SettingsAction::AppearanceTab(tab) => {
                self.appearance_tab = tab;
                self.client.set_page(SettingsPage::Appearance);
            }
            SettingsAction::StartIconChoose => self.start_file_chooser(ChooserTarget::StartIcon)?,
            SettingsAction::StartIconClear => {
                let previous = self.text_setting("customStartIcon")?;
                self.apply_text("customStartIcon", String::new())?;
                remove_managed_media(&previous)?;
            }
            SettingsAction::ThemePickerOpen(kind) => self.theme_picker = Some((kind, 0)),
            SettingsAction::ThemePickerClose => self.theme_picker = None,
            SettingsAction::ThemePickerPage(kind, step) => {
                let page = self.open_theme_page(kind)?;
                let pages = page_count(self.themes.items(kind).len(), theme_picker::PICKER_SLOTS);
                self.theme_picker = Some((kind, step_page(page, step, pages)));
            }
            SettingsAction::ThemePickerSelect(kind, slot) => {
                let index = self.open_theme_page(kind)? * theme_picker::PICKER_SLOTS + slot;
                let id = self
                    .themes
                    .items(kind)
                    .get(index)
                    .map(|item| item.id.clone())
                    .ok_or_else(|| format!("no theme at index {index}; rescan the themes"))?;
                self.apply_text(kind.setting_key(), id)?;
                self.theme_picker = None;
            }
            SettingsAction::ThemeInstallChoose { folder } => self.start_theme_install(folder)?,
            SettingsAction::WallpaperChoose => self.start_file_chooser(ChooserTarget::Wallpaper)?,
            SettingsAction::WallpaperDefault => {
                self.apply_text("wallpaperMode", "default".to_owned())?;
            }
            SettingsAction::WallpaperClear => {
                self.apply_text("wallpaperMode", "solid".to_owned())?;
            }
            SettingsAction::LockBackgroundChoose => {
                self.start_file_chooser(ChooserTarget::LockBackground)?;
            }
            SettingsAction::LockBackgroundClear => {
                self.apply_text("lockBackgroundMode", "solid".to_owned())?;
            }
            SettingsAction::LockBackgroundDefault => {
                self.apply_text("lockBackgroundMode", "default".to_owned())?;
            }
            SettingsAction::AvatarChoose => {
                self.start_file_chooser(ChooserTarget::Avatar)?;
            }
            SettingsAction::AvatarClear => {
                self.apply_text("avatarImage", String::new())?;
            }
            SettingsAction::LockPreview => start_session_preview(SessionAction::LockPreview)?,
            SettingsAction::ScreensaverPreview => {
                start_session_preview(SessionAction::ScreensaverPreview)?;
            }
            SettingsAction::IntegerSelect { key, value } => {
                self.client
                    .apply_setting(key, SettingValue::Integer(value))
                    .map_err(control_error)?;
            }
            SettingsAction::Toggle(key) => {
                let enabled = self
                    .setting_value(key)
                    .and_then(|value| match value {
                        SettingValue::Boolean(value) => Some(!value),
                        _ => None,
                    })
                    .ok_or_else(|| format!("toggle '{key}' has no authoritative boolean value"))?;
                self.client
                    .apply_setting(key, SettingValue::Boolean(enabled))
                    .map_err(control_error)?;
                if enabled && SPEECH_TOGGLES.contains(&key) {
                    start_voice_setup()?;
                }
                // Custom accent off means the system accent everywhere: reset the stored value.
                if key == "accentCustom" && !enabled {
                    self.apply_text("accent", DEFAULT_ACCENT.to_owned())?;
                }
            }
            SettingsAction::SelectionOpacity(direction) => {
                self.step_percent("DesktopSelectionFillOpacity", direction, 60)?;
            }
            SettingsAction::SnapOpacity(direction) => {
                self.step_percent("WindowSnapPreviewFillOpacity", direction, 60)?;
            }
            SettingsAction::DesktopEntrySize(direction) => {
                self.step_integer(
                    "DesktopEntryScalePercent",
                    i64::from(direction) * 10,
                    50,
                    200,
                )?;
            }
            SettingsAction::FontSize(direction) => {
                self.step_integer("fontSizeOffset", i64::from(direction), -2, 4)?;
            }
            SettingsAction::ColorSwatch { key, value } => self.apply_text(key, value.to_owned())?,
            SettingsAction::PercentStep(key, direction) => {
                let maximum = if key == "surfaceSaturationPercent" { 200 } else { 100 };
                self.step_percent(key, direction, maximum)?;
            }
            SettingsAction::GlassPreset(preset) => {
                let changes = preset
                    .settings_values()
                    .into_iter()
                    .map(|(key, value)| flamewm_api::settings::SettingsChange {
                        key: key.to_owned(),
                        value: Some(value),
                    })
                    .collect();
                self.client.apply_settings(changes).map_err(control_error)?;
            }
            SettingsAction::IntegerStep {
                key,
                delta,
                minimum,
                maximum,
            } => self.step_integer(key, delta, minimum, maximum)?,
            // Value is applied from the drag text in `handle_animation_speed_drag`.
            SettingsAction::AnimationSpeedSlider
            | SettingsAction::DisplayScaleSlider
            | SettingsAction::GlassSlider => {}
            SettingsAction::SearchFocus => self.nav_search_focused = true,
            SettingsAction::ResetGlass => {
                self.client.reset_section("glass").map_err(control_error)?;
            }
            SettingsAction::TextCycle { key, values } => {
                let current = self.text_setting(key)?;
                self.apply_text(key, cycle_text(values, &current).to_owned())?;
            }
            SettingsAction::TextSelect { key, value } => self.apply_text(key, value.to_owned())?,
            SettingsAction::TaskbarEdgeCycle => self.cycle_panel_edge()?,
            SettingsAction::TaskbarEdge(edge) => {
                let (output, _) = self.selected_panel()?;
                self.client
                    .set_panel_edge(output, edge)
                    .map_err(control_error)?;
            }
            // Size is the global `taskbarHeight` service setting, shared by all outputs.
            SettingsAction::TaskbarSizeCycle => self.set_taskbar_size(1)?,
            SettingsAction::FontFamilyCycle => self.cycle_font_family()?,
            SettingsAction::Hotkey(action, operation) => self.handle_hotkey(action, operation)?,
            SettingsAction::DisplayArrange(side) => self.arrange_display(side)?,
            SettingsAction::DisplaySelect(slot) => {
                let generation = self
                    .client
                    .displays()
                    .map_or(0, |snapshot| snapshot.generation);
                let index = page_slot_index(
                    self.pages.generation.unwrap_or(generation),
                    generation,
                    self.pages.output_page,
                    OUTPUT_PAGE_SLOTS,
                    slot,
                    self.output_count(),
                    "display output",
                )?;
                let output_id = self
                    .ordered_output_indices()
                    .get(index)
                    .and_then(|&physical| {
                        self.client
                            .displays()
                            .and_then(|snapshot| snapshot.outputs.get(physical))
                    })
                    .map(|output| output.id.clone())
                    .ok_or_else(|| format!("no display output at index {index}"))?;
                if !self.client.select_output(&output_id) {
                    return Err(format!(
                        "display output {index} is not an authoritative output"
                    ));
                }
                // Modes belong to the selected output, so its mode list starts on page 1.
                self.pages.mode_page = 0;
                self.scale_slider_focused = false;
                self.scale_drag = None;
            }
            SettingsAction::DisplaySlot(slot) => {
                let generation = self
                    .client
                    .displays()
                    .map_or(0, |snapshot| snapshot.generation);
                let index = page_slot_index(
                    self.pages.generation.unwrap_or(generation),
                    generation,
                    self.pages.mode_page,
                    DISPLAY_MODE_SLOTS,
                    slot,
                    self.client.display_mode_count(),
                    "display mode",
                )?;
                let mode = self.client.display_mode_for_index(index).ok_or_else(|| {
                    format!("display mode at index {index} has no authoritative mode")
                })?;
                self.begin_display_mode(mode)?;
            }
            SettingsAction::OutputPage(step) => {
                let pages = page_count(self.output_count(), OUTPUT_PAGE_SLOTS);
                self.pages.output_page = step_page(self.pages.output_page, step, pages);
            }
            SettingsAction::ModePage(step) => {
                let pages = page_count(self.client.display_mode_count(), DISPLAY_MODE_SLOTS);
                self.pages.mode_page = step_page(self.pages.mode_page, step, pages);
            }
            SettingsAction::DisplayBegin(mode) => self.begin_display_mode(mode)?,
            SettingsAction::DisplayScale(percent) => {
                self.client
                    .set_shell_scale(percent)
                    .map_err(control_error)?;
            }
            SettingsAction::DisplayKeep => self
                .client
                .keep_display_mode(monotonic_now_ms())
                .map_err(control_error)?,
            SettingsAction::DisplayRevert => {
                self.client.revert_display_mode().map_err(control_error)?;
            }
            SettingsAction::OpenUri(uri) => self.start_open_uri(uri)?,
        }
        Ok(())
    }
}

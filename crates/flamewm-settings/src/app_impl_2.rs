//! Split from `lib.rs` (`app_impl_2`); see crate docs.

use super::*;

impl<T: ControlTransport> SettingsApplication<T> {
    pub(crate) fn start_file_chooser(&mut self, target: ChooserTarget) -> Result<(), String> {
        if self.wallpaper_chooser.is_some() {
            return Err("file chooser is already open".to_owned());
        }
        self.wallpaper_chooser = Some(file_chooser::WallpaperChooserWorker::start()?);
        self.chooser_target = target;
        Ok(())
    }

    /// Starts a catalog scan unless one is already in flight. Runs off the UI thread.
    pub fn start_theme_scan(&mut self) {
        if self.theme_scan.is_some() {
            return;
        }
        match theme_picker::start_scan(theme_picker::theme_data_dirs_from_env()) {
            Ok(receiver) => self.theme_scan = Some(receiver),
            Err(error) => self.theme_status = Some(error),
        }
    }

    /// Applies finished scan and install results. Returns true when the view must be projected.
    pub(crate) fn poll_theme_jobs(&mut self) -> bool {
        let mut changed = false;
        match self.theme_scan.as_ref().map(Receiver::try_recv) {
            Some(Ok(registry)) => {
                self.theme_scan = None;
                self.themes = registry;
                // The scan thread's decode temporaries stay resident in its malloc arena after it
                // exits; the registry keeps only the small previews, so trim once here.
                flamewm_ui_x11::release_free_heap();
                if let Some((kind, page)) = self.theme_picker {
                    let pages =
                        page_count(self.themes.items(kind).len(), theme_picker::PICKER_SLOTS);
                    self.theme_picker = Some((kind, page.min(pages.saturating_sub(1))));
                }
                changed = true;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.theme_scan = None;
                self.theme_status = Some("theme scan ended without a result".to_owned());
                changed = true;
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
        match self.theme_install.as_ref().map(Receiver::try_recv) {
            Some(Ok(outcome)) => {
                self.theme_install = None;
                self.theme_status = Some(match outcome {
                    Ok(Some(installed)) => {
                        // Rescan so the installed theme appears in the picker.
                        self.theme_scan = None;
                        self.start_theme_scan();
                        format!("Installed {}", installed.id)
                    }
                    Ok(None) => "Install cancelled".to_owned(),
                    Err(error) => error,
                });
                changed = true;
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.theme_install = None;
                self.theme_status = Some("theme install ended without a result".to_owned());
                changed = true;
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
        changed
    }

    pub(crate) fn start_theme_install(&mut self, folder: bool) -> Result<(), String> {
        if self.theme_install.is_some() {
            return Err("a theme install is already running".to_owned());
        }
        let icons_dir = theme_picker::user_icons_dir_from_env()
            .ok_or_else(|| "no user data home: set XDG_DATA_HOME or HOME".to_owned())?;
        self.theme_install = Some(theme_picker::start_install(icons_dir, folder)?);
        self.theme_status = Some("Choose a theme archive or folder".to_owned());
        Ok(())
    }

    /// Current page of the open picker; an action for a closed picker is rejected.
    pub(crate) fn open_theme_page(&self, kind: theme_picker::PickerKind) -> Result<usize, String> {
        match self.theme_picker {
            Some((open, page)) if open == kind => Ok(page),
            _ => Err(format!("the {} theme picker is not open", kind.word())),
        }
    }

    pub(crate) fn sync_theme_pickers(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        for kind in [
            theme_picker::PickerKind::Icon,
            theme_picker::PickerKind::Cursor,
        ] {
            let open = self
                .theme_picker
                .filter(|(open, _)| *open == kind)
                .map(|(_, page)| page);
            let node = kind.node();
            document.visible(&format!("{node}-picker"), open.is_some())?;
            let items = self.themes.items(kind);
            let page = open.unwrap_or(0);
            project_pager(
                document,
                theme_pager(kind),
                page,
                page_count(items.len(), theme_picker::PICKER_SLOTS),
                page_label(page, theme_picker::PICKER_SLOTS, items.len()),
            )?;
            for slot in 0..theme_picker::PICKER_SLOTS {
                let row = format!("{node}-row-{slot}");
                let preview = format!("{node}-preview-{slot}");
                let Some(item) =
                    open.and_then(|page| items.get(page * theme_picker::PICKER_SLOTS + slot))
                else {
                    document.visible(&row, false)?;
                    continue;
                };
                document.visible(&row, true)?;
                document.text(&format!("{node}-name-{slot}"), item.name.clone())?;
                match &item.preview {
                    Some(image) => {
                        document.visible(&preview, true)?;
                        document.image_rgba8_shared(
                            &preview,
                            image.source.clone(),
                            image.width,
                            image.height,
                            Arc::clone(&image.pixels),
                        )?;
                    }
                    None => document.visible(&preview, false)?,
                }
            }
        }
        Ok(())
    }

    /// Points `customStartIcon` at `managed` and deletes the previous managed copy when it differs.
    pub(crate) fn replace_start_icon(&mut self, managed: String) -> Result<(), String> {
        let previous = self.text_setting("customStartIcon")?;
        self.apply_text("customStartIcon", managed.clone())?;
        if previous != managed {
            remove_managed_media(&previous)?;
        }
        Ok(())
    }

    pub(crate) fn apply_text(&mut self, key: &str, value: String) -> Result<(), String> {
        self.client
            .apply_setting(key, SettingValue::Text(value))
            .map_err(control_error)
    }

    pub(crate) fn open_color_picker(&self) -> Result<&ColorPicker, String> {
        self.color_picker
            .as_ref()
            .ok_or_else(|| "colour picker is not open".to_owned())
    }

    /// Edits the API key draft from typed text: printable characters append, Backspace
    /// deletes, Escape discards the draft, Enter saves a valid value (an invalid one stays
    /// in the field with its inline message and writes nothing).
    pub(crate) fn api_key_text(&mut self, text: &str) {
        for ch in text.chars() {
            match ch {
                '\u{1b}' => {
                    self.api_key_draft = None;
                    return;
                }
                '\n' | '\r' => {
                    if let Err(error) = self.apply_api_key_draft() {
                        self.error = Some(error);
                    }
                    if self.api_key_draft.is_none() {
                        return;
                    }
                }
                '\u{8}' => {
                    if let Some(draft) = self.api_key_draft.as_mut() {
                        draft.buffer.pop();
                        draft.error = None;
                    }
                }
                ch if !ch.is_control() => {
                    if let Some(draft) = self.api_key_draft.as_mut() {
                        if draft.buffer.chars().count()
                            < flamewm_api::settings::ASSISTANT_API_KEY_MAX_CHARS
                        {
                            draft.buffer.push(ch);
                            draft.error = None;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Saves the trimmed draft when it is a valid `assistant.apiKey`; an empty draft clears the key
    /// to the built-in default. Otherwise shows the message inline and writes nothing.
    pub(crate) fn apply_api_key_draft(&mut self) -> Result<(), String> {
        let Some(draft) = self.api_key_draft.as_mut() else {
            return Ok(());
        };
        let value = draft.buffer.trim().to_owned();
        match flamewm_api::settings::assistant_api_key_error(&value) {
            Some(message) => draft.error = Some(message.to_owned()),
            None => {
                self.api_key_draft = None;
                self.apply_text("assistant.apiKey", value)?;
            }
        }
        Ok(())
    }

    /// Projects the API key field: the draft with a caret while editing, else the saved key masked
    /// to its first 8 characters (the built-in label when unset), plus the inline message.
    pub(crate) fn sync_api_key(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let (text, error) = match &self.api_key_draft {
            Some(draft) => (format!("{}|", draft.buffer), draft.error.clone()),
            None => {
                let saved = self.text_setting("assistant.apiKey").unwrap_or_default();
                (masked_api_key(&saved), None)
            }
        };
        document.text("assistant-api-key", text)?;
        optional_text(
            document,
            "assistant-api-key-error",
            error.unwrap_or_else(|| "\u{200b}".to_owned()),
        );
        Ok(())
    }

    /// Applies typed keys to the picker: Escape closes, Enter applies, Backspace
    /// edits and hex digits append only while the field is focused.
    /// Edits the sidebar search query from typed text: printable characters append, backspace
    /// deletes, Escape clears and leaves the field, Enter leaves it with the filter kept.
    pub(crate) fn nav_search_text(&mut self, text: &str) {
        for ch in text.chars() {
            match ch {
                '\u{1b}' => {
                    self.nav_query.clear();
                    self.nav_search_focused = false;
                }
                '\n' | '\r' => self.nav_search_focused = false,
                '\u{8}' => {
                    self.nav_query.pop();
                }
                ch if !ch.is_control() => self.nav_query.push(ch),
                _ => {}
            }
        }
    }

    pub(crate) fn color_picker_text(&mut self, text: &str) -> Result<(), String> {
        for ch in text.chars() {
            match ch {
                '\u{1b}' => {
                    self.color_picker = None;
                    return Ok(());
                }
                '\n' | '\r' => self.apply_picker_buffer()?,
                '\u{8}' => {
                    if let Some(picker) = self.color_picker.as_mut().filter(|picker| picker.focused)
                    {
                        picker.buffer.pop();
                        picker.error = None;
                    }
                }
                ch if ch == '#' || ch.is_ascii_hexdigit() => {
                    if let Some(picker) = self
                        .color_picker
                        .as_mut()
                        .filter(|picker| picker.focused && picker.buffer.len() < PICKER_BUFFER_MAX)
                    {
                        picker.buffer.push(ch);
                        picker.error = None;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Writes the buffer as the picker's setting. An invalid buffer is shown inline
    /// and writes nothing.
    pub(crate) fn apply_picker_buffer(&mut self) -> Result<(), String> {
        let picker = self.open_color_picker()?;
        let key = picker.key;
        match parse_hex_color(&picker.buffer) {
            Ok(value) => {
                self.apply_text(key, value.clone())?;
                if let Some(picker) = self.color_picker.as_mut() {
                    picker.buffer = value;
                    picker.error = None;
                }
            }
            Err(message) => {
                if let Some(picker) = self.color_picker.as_mut() {
                    picker.error = Some(message);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn arrange_display(&mut self, side: display_layout::Side) -> Result<(), String> {
        let output = self
            .client
            .ui()
            .selected_output
            .as_ref()
            .ok_or_else(|| "select an active monitor before arranging".to_owned())?
            .clone();
        // Validate against a *fresh* WM snapshot instead of a stale Settings
        // projection. The external RandR adapter never saves optimistic state.
        self.client.refresh_displays().map_err(control_error)?;
        let snapshot = self
            .client
            .displays()
            .ok_or_else(|| "display state unavailable".to_owned())?;
        let Some(layout) = display_layout::plan(snapshot, &output, side)? else {
            return Ok(());
        };
        display_layout::apply(&layout)?;
        // Readback must confirm every output's geometry, mode, scale and primary.
        // No success or persistence solely from xrandr's exit status.
        self.client.refresh_displays().map_err(control_error)?;
        let observed = self
            .client
            .displays()
            .ok_or_else(|| "display readback unavailable".to_owned())?;
        if let Err(error) = display_layout::verify(&layout, observed) {
            let rollback = display_layout::rollback_if_safe(&layout, observed);
            // Best-effort recovery must never be misreported as an accepted
            // commit, even if the rollback command itself exited successfully.
            let _ = self.client.refresh_displays();
            return Err(match rollback {
                Ok(()) => format!("{error}; previous monitor positions requested"),
                Err(reason) => format!("{error}; rollback unavailable: {reason}"),
            });
        }
        self.pages.generation = Some(observed.generation);
        if let Err(error) = display_layout::persist_verified(&layout) {
            return Err(format!("monitor layout applied but not saved: {error}"));
        }
        Ok(())
    }

    pub(crate) fn begin_display_mode(&mut self, mode: ModeId) -> Result<(), String> {
        let now_ms = monotonic_now_ms();
        self.client
            .begin_display_mode(mode, now_ms)
            .map(|_| ())
            .map_err(control_error)
    }

    pub(crate) fn handle_hotkey(
        &mut self,
        action: &'static str,
        operation: HotkeyOperation,
    ) -> Result<(), String> {
        match operation {
            HotkeyOperation::Capture => {
                self.hotkey_armed = false;
                self.client
                    .begin_hotkey_capture(action)
                    .map_err(control_error)?;
                self.hotkey_armed = true;
                Ok(())
            }
            HotkeyOperation::Clear => {
                self.client
                    .begin_hotkey_capture(action)
                    .map_err(control_error)?;
                self.client.capture_hotkey("Escape");
                self.client.commit_hotkey_capture().map_err(control_error)
            }
            HotkeyOperation::Reset => self.client.reset_hotkey(action).map_err(control_error),
        }
    }

    /// Commits the armed hotkey capture on a non-modifier key release.
    /// Escape clears the row, Backspace restores its default, and a binding
    /// already used by another row is rejected while the capture stays armed.
    pub(crate) fn finish_hotkey_capture(&mut self, binding: &str) -> Result<(), String> {
        let Some(action) = self
            .client
            .ui()
            .hotkey_capture
            .as_ref()
            .map(|capture| capture.action.clone())
        else {
            self.hotkey_armed = false;
            return Ok(());
        };
        match binding {
            "Backspace" => {
                self.client.cancel_hotkey_capture().map_err(control_error)?;
                self.hotkey_armed = false;
                self.client.reset_hotkey(action).map_err(control_error)
            }
            "Escape" => {
                self.client.capture_hotkey("Escape");
                self.client.commit_hotkey_capture().map_err(control_error)?;
                self.hotkey_armed = false;
                Ok(())
            }
            _ => {
                if let Some(owner) = self.duplicate_owner(&action, binding) {
                    return Err(format!(
                        "{binding} is already assigned to {}; press another key, Escape clears, Backspace restores the default",
                        shortcut_label(&owner)
                    ));
                }
                self.client.capture_hotkey(binding);
                self.client.commit_hotkey_capture().map_err(control_error)?;
                self.hotkey_armed = false;
                Ok(())
            }
        }
    }

    /// Another row that already owns `binding`, if any.
    pub(crate) fn duplicate_owner(&self, action: &str, binding: &str) -> Option<String> {
        self.client
            .shortcuts()?
            .bindings
            .iter()
            .find(|(owner, value)| {
                owner.as_str() != action
                    && value
                        .0
                        .as_deref()
                        .is_some_and(|assigned| assigned.eq_ignore_ascii_case(binding))
            })
            .map(|(owner, _)| owner.clone())
    }

    pub(crate) fn start_open_uri(&mut self, uri: &'static str) -> Result<(), String> {
        // Do not replace the only receiver for an in-flight opener: its completion
        // carries the only authoritative success/failure result for the user.
        self.poll_open_uri()?;
        if self.pending_open.is_some() {
            return Err(
                "a link is still opening; wait for it to finish before opening another".to_owned(),
            );
        }
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("flamewm-settings-open-uri".to_owned())
            .spawn(move || {
                let outcome = match Command::new("xdg-open").arg(uri).status() {
                    Ok(status) if status.success() => Ok(()),
                    Ok(status) => Err(format!(
                        "could not open {uri}: xdg-open exited with {status}"
                    )),
                    Err(error) => Err(format!("could not open {uri}: {error}")),
                };
                file_chooser::publish_background_completion(&sender, outcome);
            })
            .map_err(|error| format!("could not start link opener: {error}"))?;
        self.pending_open = Some(receiver);
        Ok(())
    }

    pub(crate) fn poll_open_uri(&mut self) -> Result<(), String> {
        let Some(receiver) = self.pending_open.as_ref() else {
            return Ok(());
        };
        match receiver.try_recv() {
            Ok(outcome) => {
                self.pending_open = None;
                outcome
            }
            Err(TryRecvError::Empty) => Ok(()),
            Err(TryRecvError::Disconnected) => {
                self.pending_open = None;
                Err("link opener ended without a result".to_owned())
            }
        }
    }

    pub(crate) fn step_percent(&mut self, key: &str, direction: i8, maximum: u8) -> Result<(), String> {
        let current = self.percent_setting(key)?;
        let next = i16::from(current) + i16::from(direction) * 5;
        let next = next.clamp(0, i16::from(maximum)) as u8;
        self.client
            .apply_setting(key, SettingValue::Percent(next))
            .map_err(control_error)
    }

    pub(crate) fn step_integer(
        &mut self,
        key: &str,
        delta: i64,
        minimum: i64,
        maximum: i64,
    ) -> Result<(), String> {
        let current = self.integer_setting(key)?;
        let next = (current + delta).clamp(minimum, maximum);
        self.client
            .apply_setting(key, SettingValue::Integer(next))
            .map_err(control_error)
    }

    pub(crate) fn set_taskbar_size(&mut self, direction: i8) -> Result<(), String> {
        let height = self.integer_setting("taskbarHeight")?;
        let sizes = [34_i64, 44, 52, 64, 72];
        self.client
            .set_taskbar_height(cycle_value(&sizes, height, direction) as u16)
            .map_err(control_error)
    }

    pub(crate) fn cycle_font_family(&mut self) -> Result<(), String> {
        const FAMILIES: [&str; 3] = ["IBM Plex Sans", "Noto Sans", "DejaVu Sans"];
        let current = self.text_setting("fontFamily")?;
        let next = cycle_text(&FAMILIES, &current);
        self.apply_text("fontFamily", next.to_owned())
    }

    pub(crate) fn cycle_panel_edge(&mut self) -> Result<(), String> {
        let (output, edge) = self.selected_panel()?;
        self.client
            .set_panel_edge(output, next_edge(edge))
            .map_err(control_error)
    }

    pub(crate) fn selected_panel(&self) -> Result<(OutputId, PanelEdge), String> {
        let output = self
            .client
            .ui()
            .selected_output
            .clone()
            .ok_or_else(|| "no authoritative display output is selected".to_owned())?;
        let edge = self
            .client
            .panels()
            .and_then(|snapshot| snapshot.panels.iter().find(|panel| panel.output == output))
            .map(|panel| panel.edge)
            .ok_or_else(|| "selected output has no authoritative panel".to_owned())?;
        Ok((output, edge))
    }

    pub(crate) fn selected_display(&self) -> Option<&flamewm_api::display::OutputSnapshot> {
        let id = self.client.ui().selected_output.as_ref()?;
        self.client
            .displays()?
            .outputs
            .iter()
            .find(|output| &output.id == id)
    }

    pub(crate) fn setting_value(&self, key: &str) -> Option<&SettingValue> {
        self.client
            .settings()
            .and_then(|snapshot| snapshot.values.get(key))
    }

    pub(crate) fn percent_setting(&self, key: &str) -> Result<u8, String> {
        match self.setting_value(key) {
            Some(SettingValue::Percent(value)) => Ok(*value),
            _ => Err(format!(
                "setting '{key}' has no authoritative percent value"
            )),
        }
    }

    /// A percent or integer value as an integer, for keys that the wire types either way.
    pub(crate) fn number_setting(&self, key: &str) -> Option<i64> {
        match self.setting_value(key)? {
            SettingValue::Percent(value) => Some(i64::from(*value)),
            SettingValue::Integer(value) => Some(*value),
            _ => None,
        }
    }

    pub(crate) fn integer_setting(&self, key: &str) -> Result<i64, String> {
        match self.setting_value(key) {
            Some(SettingValue::Integer(value)) => Ok(*value),
            _ => Err(format!(
                "setting '{key}' has no authoritative integer value"
            )),
        }
    }

    /// The configured accent, falling back to `DEFAULT_ACCENT` when the setting is absent.
    pub(crate) fn accent(&self) -> Result<UiColor, String> {
        accent_color(
            &self
                .text_setting("accent")
                .unwrap_or(DEFAULT_ACCENT.to_owned()),
        )
    }

    pub(crate) fn text_setting(&self, key: &str) -> Result<String, String> {
        match self.setting_value(key) {
            Some(SettingValue::Text(value)) => Ok(value.clone()),
            _ => Err(format!("setting '{key}' has no authoritative text value")),
        }
    }

    /// Boolean setting value; an absent or non-boolean key reads as OFF.
    pub(crate) fn bool_setting(&self, key: &str) -> bool {
        matches!(self.setting_value(key), Some(SettingValue::Boolean(true)))
    }

    /// Writes every node; a failed node write is recorded and the rest still apply.
    pub(crate) fn sync(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        let mut nodes = NodeFailures {
            inner: document,
            failures: Vec::new(),
        };
        let result = self.sync_nodes(&mut nodes);
        match (result, nodes.into_result()) {
            (Ok(()), recorded) => recorded,
            (Err(error), Ok(())) => Err(error),
            (Err(error), Err(recorded)) => Err(format!("{error}; {recorded}")),
        }
    }

    pub(crate) fn sync_nodes(&self, document: &mut impl UiDocumentAccess) -> Result<(), String> {
        if let Some(snapshot) = self.client.settings() {
            flamewm_ui_x11::surface_style::project_surface_border(
                document,
                "settings-root",
                snapshot,
            )?;
            project_glass_roles(document, snapshot)?;
        }
        document.position("nav-selection", 8.0, nav_top(self.nav_index()))?;
        let filtering = !self.nav_query.is_empty();
        for (slug, label) in NAV_PAGES {
            document.visible(&format!("nav-{slug}"), nav_matches(label, &self.nav_query))?;
        }
        document.visible("nav-selection", !filtering)?;
        let search_text = match (self.nav_search_focused, self.nav_query.is_empty()) {
            (false, true) => "Search".to_owned(),
            (true, _) => format!("{}|", self.nav_query),
            (false, false) => self.nav_query.clone(),
        };
        document.text("nav-search-text", search_text)?;
        if let Some(SettingValue::Text(accent)) = self.setting_value("accent") {
            document.background("nav-selection", solid_color(accent)?)?;
            sync_accent_nodes(document, accent)?;
            for name in ["red", "blue", "purple", "teal"] {
                let palette = accent_for_swatch(name);
                document.visible(
                    &format!("accent-selected-{name}"),
                    palette.is_some_and(|value| value.eq_ignore_ascii_case(accent)),
                )?;
            }
        }
        for tab in AppearanceTab::ALL {
            document.visible(
                &format!("page-{}", tab.node()),
                self.client.ui().page == SettingsPage::Appearance && self.appearance_tab == tab,
            )?;
        }
        for (name, page) in [
            ("desktop", SettingsPage::Desktop),
            ("taskbar", SettingsPage::Taskbar),
            ("displays", SettingsPage::Displays),
            ("fonts", SettingsPage::Fonts),
            ("hotkeys", SettingsPage::Hotkeys),
            ("assistant", SettingsPage::Assistant),
            ("about", SettingsPage::About),
        ] {
            document.visible(&format!("page-{name}"), self.client.ui().page == page)?;
        }
        for slot in 0..OUTPUT_PAGE_SLOTS {
            let id = format!("display-output-{slot}");
            let Some(output) = self.output_page_slot(slot) else {
                document.visible(&id, false)?;
                continue;
            };
            document.visible(&id, true)?;
            let shown = output.connected && output.geometry.is_valid();
            document.text(
                &format!("display-output-name-{slot}"),
                format!(
                    "{} · {}",
                    self.pages.output_page * OUTPUT_PAGE_SLOTS + slot + 1,
                    output.connector
                ),
            )?;
            let resolution = output
                .modes
                .iter()
                .find(|mode| mode.id == output.current_mode)
                .map_or(output.geometry.size(), |mode| mode.resolution);
            document.text(
                &format!("display-output-size-{slot}"),
                if shown {
                    format!("{} × {}", resolution.width, resolution.height)
                } else {
                    "Disconnected".to_owned()
                },
            )?;
            let selected = self.client.ui().selected_output.as_ref() == Some(&output.id);
            document.visible(&format!("display-output-selected-{slot}"), selected)?;
            document.visible(&format!("display-output-unselected-{slot}"), !selected)?;
            document.border(
                &id,
                if selected {
                    self.accent()?
                } else {
                    UiColor {
                        r: 255,
                        g: 255,
                        b: 255,
                        a: 0x1a,
                    }
                },
            )?;
        }
        project_pager(
            document,
            &OUTPUT_PAGER,
            self.pages.output_page,
            page_count(self.output_count(), OUTPUT_PAGE_SLOTS),
            page_label(
                self.pages.output_page,
                OUTPUT_PAGE_SLOTS,
                self.output_count(),
            ),
        )?;
        if let Some(output) = self.selected_display() {
            document.text(
                "display-selected-title",
                format!("Selected: {}", output.connector),
            )?;
            let resolution = output
                .modes
                .iter()
                .find(|mode| mode.id == output.current_mode)
                .map_or(output.geometry.size(), |mode| mode.resolution);
            document.text(
                "display-selected-resolution",
                format!("{} x {}", resolution.width, resolution.height),
            )?;
            self.sync_display_scale(
                document,
                self.scale_drag
                    .as_ref()
                    .and_then(|(id, generation, preview)| {
                        (self
                            .client
                            .displays()
                            .is_some_and(|snap| snap.generation == *generation)
                            && id == &output.id)
                            .then_some(*preview)
                    }),
            )?;
        } else {
            for id in ["display-selected-title", "display-selected-resolution"] {
                document.text(id, "\u{200b}".to_owned())?;
            }
            self.sync_display_scale(document, None)?;
        }
        self.sync_display_modes(document)?;
        for (key, on_id, off_id) in [
            (
                "stickyNotesEnabled",
                "sticky-toggle-on",
                "sticky-toggle-off",
            ),
            ("fontBold", "font-toggle-on", "font-toggle-off"),
            ("animationsEnabled", "anim-global-on", "anim-global-off"),
            ("startAnimationEnabled", "anim-start-on", "anim-start-off"),
            (
                "windowOpenAnimationEnabled",
                "anim-window-on",
                "anim-window-off",
            ),
            (
                "minimizeAnimationEnabled",
                "anim-minimize-on",
                "anim-minimize-off",
            ),
            (
                "maximizeAnimationEnabled",
                "anim-maximize-on",
                "anim-maximize-off",
            ),
            ("lockShowDate", "lock-date-on", "lock-date-off"),
            ("lockShowUsername", "lock-username-on", "lock-username-off"),
            ("lockOnSuspend", "lock-suspend-on", "lock-suspend-off"),
            (
                "organizeDownloads",
                "downloads-organize-on",
                "downloads-organize-off",
            ),
            (
                "taskbarShowSeconds",
                "taskbar-seconds-on",
                "taskbar-seconds-off",
            ),
            ("taskbarShowDate", "taskbar-date-on", "taskbar-date-off"),
            ("snapPreviewEnabled", "snap-preview-on", "snap-preview-off"),
            ("reduceTransparency", "reduce-transparency-on", "reduce-transparency-off"),
            (
                "showContentWhileDragging",
                "show-content-on",
                "show-content-off",
            ),
            ("focusFollowsMouse", "focus-mouse-on", "focus-mouse-off"),
        ] {
            let enabled = self.setting_value(key).and_then(|value| match value {
                SettingValue::Boolean(value) => Some(*value),
                _ => None,
            });
            document.visible(on_id, enabled == Some(true))?;
            document.visible(off_id, enabled == Some(false))?;
        }
        for (key, on_id, off_id, default) in SETTING_TWINS {
            let enabled = match self.setting_value(key) {
                Some(SettingValue::Boolean(value)) => *value,
                _ => *default,
            };
            document.visible(on_id, enabled)?;
            document.visible(off_id, !enabled)?;
        }
        if let Some(SettingValue::Text(family)) = self.setting_value("fontFamily") {
            document.font_family(family.clone())?;
        }
        document.ui_font_defaults(
            self.integer_setting("fontSizeOffset").unwrap_or(0) as f32,
            self.bool_setting("fontBold"),
        )?;
        self.sync_value_text(document)?;
        self.sync_weather(document)?;
        self.sync_time_zone(document)?;
        self.sync_theme_pickers(document)?;
        if let Some(status) = &self.theme_status {
            optional_text(document, "theme-install-status", status.clone());
        }
        self.sync_appearance(document)?;
        self.sync_option_setting(
            document,
            "decoration",
            &[
                ("rounded", "decoration-rounded"),
                ("squared", "decoration-squared"),
            ],
        )?;
        self.sync_option_setting(
            document,
            "renderAcceleration",
            &[
                ("cpu", "render-acceleration-cpu"),
                ("gpu", "render-acceleration-gpu"),
            ],
        )?;
        self.sync_option_setting(
            document,
            "titlebarButtonStyle",
            &[
                ("circle", "titlebar-button-style-circle"),
                ("square", "titlebar-button-style-square"),
            ],
        )?;
        self.sync_animation_styles(document)?;
        self.sync_lock_screen(document)?;
        self.sync_segment_rows(document)?;
        self.sync_taskbar_outputs(document);
        self.sync_hotkeys(document);
        self.sync_api_key(document)?;
        optional_text(
            document,
            ABOUT_VERSION_NODE,
            format!("Version {APP_VERSION}"),
        );
        self.sync_error(document);
        Ok(())
    }
}

//! `FlameWM` Settings application state and control-client contracts.
//!
//! The GUI process is intentionally separate from the WM process. It owns only transient UI state
//! and cached immutable snapshots; all authoritative mutations cross Flame Control.

use flamewm_api::display::{DisplayMode, DisplaySnapshot};
use flamewm_api::panels::PanelsSnapshot;
use flamewm_api::settings::{SettingValue, SettingsChange, SettingsSnapshot, SettingsTransaction};
use flamewm_api::shortcuts::ShortcutSnapshot;
use flamewm_api::{ModeId, OutputId, PanelEdge, TransactionId};
use flamewm_control_core::{ControlError, ControlRequest, ControlResponse};
use flamewm_ui_core::IconRole;

mod night_light;
pub mod xdg_applications;
pub use night_light::{NightLightMinute, NightLightPreview, NightLightSchedule, NightLightTemperature};

pub trait ControlTransport {
    fn call(&mut self, request: ControlRequest) -> Result<ControlResponse, ControlError>;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingsPage {
    #[default]
    Appearance,
    Desktop,
    Taskbar,
    Displays,
    Fonts,
    Accessibility,
    Hotkeys,
    Startup,
    Assistant,
    About,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSpec {
    pub page: SettingsPage,
    pub name: &'static str,
    pub description: &'static str,
    pub icon: IconRole,
}

#[must_use]
pub const fn page_specs() -> &'static [PageSpec; 7] {
    &[
        PageSpec {
            page: SettingsPage::Appearance,
            name: "Appearance",
            description: "Accent, icon theme and live preview",
            icon: IconRole::Appearance,
        },
        PageSpec {
            page: SettingsPage::Desktop,
            name: "Desktop",
            description: "Wallpaper, selection and snap preview",
            icon: IconRole::Desktop,
        },
        PageSpec {
            page: SettingsPage::Taskbar,
            name: "Taskbar",
            description: "Color, opacity, height and Start branding",
            icon: IconRole::Taskbar,
        },
        PageSpec {
            page: SettingsPage::Displays,
            name: "Displays",
            description: "Output mode, scale and reversible confirmation",
            icon: IconRole::Displays,
        },
        PageSpec {
            page: SettingsPage::Fonts,
            name: "Fonts",
            description: "Family, bold and size offset",
            icon: IconRole::Fonts,
        },
        PageSpec {
            page: SettingsPage::Hotkeys,
            name: "Hotkeys",
            description: "Capture, clear and reset shortcut rows",
            icon: IconRole::Hotkeys,
        },
        PageSpec {
            page: SettingsPage::About,
            name: "About",
            description: "A lightweight desktop by ArkFlame Studios",
            icon: IconRole::About,
        },
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyCapture {
    pub action: String,
    pub captured: Option<String>,
}

impl HotkeyCapture {
    #[must_use]
    pub fn new(action: impl Into<String>) -> Self {
        Self {
            action: action.into(),
            captured: None,
        }
    }

    /// Escape means "Not assigned" in the V5 product contract.
    pub fn capture(&mut self, normalized_key: &str) {
        self.captured = if normalized_key.eq_ignore_ascii_case("Escape") {
            Some(String::new())
        } else {
            Some(normalized_key.to_owned())
        };
    }
}

/// Number of display-mode slots the Displays page exposes.
pub const DISPLAY_MODE_SLOTS: usize = 4;
/// Display scale is limited to 1.0x–2.0x in 0.25x steps.
pub const DISPLAY_SCALE_MIN: u16 = 100;
pub const DISPLAY_SCALE_MAX: u16 = 200;
pub const DISPLAY_SCALE_STEP: u16 = 25;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsUiState {
    pub page: SettingsPage,
    pub selected_output: Option<OutputId>,
    pub hotkey_capture: Option<HotkeyCapture>,
    /// Server-owned grab suspension; shortcut writes are forbidden until it is ended.
    pub hotkey_capture_lease: Option<u64>,
    pub pending_display_transaction: Option<TransactionId>,
}

pub struct SettingsClient<T: ControlTransport> {
    transport: T,
    ui: SettingsUiState,
    settings: Option<SettingsSnapshot>,
    displays: Option<DisplaySnapshot>,
    shortcuts: Option<ShortcutSnapshot>,
    panels: Option<PanelsSnapshot>,
}

impl<T: ControlTransport> SettingsClient<T> {
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            ui: SettingsUiState::default(),
            settings: None,
            displays: None,
            shortcuts: None,
            panels: None,
        }
    }

    #[must_use]
    pub fn ui(&self) -> &SettingsUiState {
        &self.ui
    }

    #[must_use]
    pub fn settings(&self) -> Option<&SettingsSnapshot> {
        self.settings.as_ref()
    }

    #[must_use]
    pub fn displays(&self) -> Option<&DisplaySnapshot> {
        self.displays.as_ref()
    }

    #[must_use]
    pub fn shortcuts(&self) -> Option<&ShortcutSnapshot> {
        self.shortcuts.as_ref()
    }

    #[must_use]
    pub fn panels(&self) -> Option<&PanelsSnapshot> {
        self.panels.as_ref()
    }

    pub fn set_page(&mut self, page: SettingsPage) {
        self.ui.page = page;
    }

    pub fn refresh_all(&mut self) -> Result<(), ControlError> {
        self.refresh_settings()?;
        self.refresh_displays()?;
        self.refresh_shortcuts()?;
        self.refresh_panels()?;
        Ok(())
    }

    pub fn refresh_settings(&mut self) -> Result<(), ControlError> {
        let response = self.transport.call(ControlRequest::GetSettings)?;
        let ControlResponse::Settings(snapshot) = response else {
            return Err(unexpected_response("settings snapshot"));
        };
        self.settings = Some(snapshot);
        Ok(())
    }

    pub fn refresh_displays(&mut self) -> Result<(), ControlError> {
        let response = self.transport.call(ControlRequest::GetDisplays)?;
        let ControlResponse::Displays(snapshot) = response else {
            return Err(unexpected_response("display snapshot"));
        };
        self.reconcile_selected_output(&snapshot);
        self.ui.pending_display_transaction =
            snapshot.pending.as_ref().map(|pending| pending.transaction);
        self.displays = Some(snapshot);
        Ok(())
    }

    /// First-page modes offered as selectable slots for the selected output, in
    /// snapshot order. Slot `k` is `modes[k]`; later pages are available through
    /// [`Self::display_mode_page`]. Modes beyond [`DISPLAY_MODE_SLOTS`] are
    /// reported by [`Self::display_mode_overflow`].
    #[must_use]
    pub fn display_mode_slots(&self) -> &[DisplayMode] {
        self.selected_output_modes()
            .map_or(&[] as &[DisplayMode], |modes| {
                &modes[..modes.len().min(DISPLAY_MODE_SLOTS)]
            })
    }

    /// Number of selected-output modes beyond the first page.
    #[must_use]
    pub fn display_mode_overflow(&self) -> usize {
        self.selected_output_modes()
            .map_or(0, |modes| modes.len().saturating_sub(DISPLAY_MODE_SLOTS))
    }

    /// `ModeId` behind slot `slot` of the selected output, if that slot exists.
    #[must_use]
    pub fn display_mode_for_slot(&self, slot: usize) -> Option<ModeId> {
        self.display_mode_slots().get(slot).map(|mode| mode.id)
    }

    /// `ModeId` at `index` across every mode of the selected output, including modes beyond
    /// [`DISPLAY_MODE_SLOTS`] (indices `0..DISPLAY_MODE_SLOTS` are the slots, the rest overflow).
    #[must_use]
    pub fn display_mode_for_index(&self, index: usize) -> Option<ModeId> {
        self.selected_output_modes()
            .and_then(|modes| modes.get(index))
            .map(|mode| mode.id)
    }

    /// Modes on one page of the selected output: `modes[page*8 .. min(len, (page+1)*8)]`.
    /// Empty past the last page. Read-only: never changes selection or the snapshot cache.
    #[must_use]
    pub fn display_mode_page(&self, page: usize) -> &[DisplayMode] {
        self.selected_output_modes()
            .map_or(&[] as &[DisplayMode], |modes| {
                let start = page.saturating_mul(DISPLAY_MODE_SLOTS).min(modes.len());
                let end = start.saturating_add(DISPLAY_MODE_SLOTS).min(modes.len());
                &modes[start..end]
            })
    }

    /// Number of modes the selected output reports across all pages (0 when none).
    #[must_use]
    pub fn display_mode_count(&self) -> usize {
        self.selected_output_modes().map_or(0, <[DisplayMode]>::len)
    }

    fn selected_output_modes(&self) -> Option<&[DisplayMode]> {
        let output = self.ui.selected_output.as_ref()?;
        self.displays
            .as_ref()?
            .outputs
            .iter()
            .find(|candidate| &candidate.id == output)
            .map(|output| output.modes.as_slice())
    }

    pub fn refresh_shortcuts(&mut self) -> Result<(), ControlError> {
        let response = self.transport.call(ControlRequest::GetShortcuts)?;
        let ControlResponse::Shortcuts(snapshot) = response else {
            return Err(unexpected_response("shortcut snapshot"));
        };
        self.shortcuts = Some(snapshot);
        Ok(())
    }

    pub fn refresh_panels(&mut self) -> Result<(), ControlError> {
        let response = self.transport.call(ControlRequest::GetPanels)?;
        let ControlResponse::Panels(snapshot) = response else {
            return Err(unexpected_response("panels snapshot"));
        };
        self.panels = Some(snapshot);
        Ok(())
    }

    pub fn select_output(&mut self, output: &OutputId) -> bool {
        let exists = self.displays.as_ref().is_some_and(|snapshot| {
            snapshot
                .outputs
                .iter()
                .any(|candidate| &candidate.id == output)
        });
        if exists {
            self.ui.selected_output = Some(output.clone());
        }
        exists
    }

    pub fn apply_setting(
        &mut self,
        key: impl Into<String>,
        value: SettingValue,
    ) -> Result<(), ControlError> {
        self.apply_changes(vec![SettingsChange {
            key: key.into(),
            value: Some(value),
        }])
    }

    /// Applies several keys in one authoritative transaction (for example a glass preset).
    pub fn apply_settings(&mut self, changes: Vec<SettingsChange>) -> Result<(), ControlError> {
        self.apply_changes(changes)
    }

    /// Apply one native Settings-page transaction and replace the cached snapshot only after the
    /// authoritative WM process accepts it. This is the common immediate-apply path.
    fn apply_changes(&mut self, changes: Vec<SettingsChange>) -> Result<(), ControlError> {
        let expected_revision = self
            .settings
            .as_ref()
            .map_or(0, |snapshot| snapshot.revision);
        self.apply_transaction(SettingsTransaction {
            expected_revision,
            changes,
            reset_section: None,
        })
    }

    pub fn reset_section(&mut self, section: impl Into<String>) -> Result<(), ControlError> {
        let expected_revision = self
            .settings
            .as_ref()
            .map_or(0, |snapshot| snapshot.revision);
        self.apply_transaction(SettingsTransaction {
            expected_revision,
            changes: Vec::new(),
            reset_section: Some(section.into()),
        })
    }

    fn apply_transaction(&mut self, transaction: SettingsTransaction) -> Result<(), ControlError> {
        let response = self
            .transport
            .call(ControlRequest::ApplySettings(transaction))?;
        let ControlResponse::Settings(snapshot) = response else {
            return Err(unexpected_response("updated settings snapshot"));
        };
        self.settings = Some(snapshot);
        Ok(())
    }

    pub fn set_wallpaper(&mut self, path: impl Into<String>) -> Result<(), ControlError> {
        self.apply_setting("wallpaper", SettingValue::Text(path.into()))
    }

    pub fn set_taskbar_height(&mut self, height: u16) -> Result<(), ControlError> {
        self.apply_setting("taskbarHeight", SettingValue::Integer(i64::from(height)))
    }

    pub fn set_fonts(
        &mut self,
        family: impl Into<String>,
        bold: bool,
        size_offset: i8,
    ) -> Result<(), ControlError> {
        self.apply_changes(vec![
            SettingsChange {
                key: "fontFamily".to_owned(),
                value: Some(SettingValue::Text(family.into())),
            },
            SettingsChange {
                key: "fontBold".to_owned(),
                value: Some(SettingValue::Boolean(bold)),
            },
            SettingsChange {
                key: "fontSizeOffset".to_owned(),
                value: Some(SettingValue::Integer(i64::from(size_offset))),
            },
        ])
    }

    pub fn begin_hotkey_capture(&mut self, action: impl Into<String>) -> Result<(), ControlError> {
        self.cancel_hotkey_capture()?;
        let action = action.into();
        let expected_revision = self
            .shortcuts
            .as_ref()
            .map(|snapshot| snapshot.revision)
            .ok_or_else(|| missing_snapshot("shortcut"))?;
        let response = self.transport.call(ControlRequest::BeginHotkeyCapture {
            action: action.clone(),
            expected_revision,
        })?;
        let ControlResponse::CaptureLease(lease) = response else {
            return Err(unexpected_response("hotkey capture lease"));
        };
        self.ui.hotkey_capture_lease = Some(lease);
        self.ui.hotkey_capture = Some(HotkeyCapture::new(action));
        Ok(())
    }

    /// Ends the server grab lease and discards any locally captured key.
    pub fn cancel_hotkey_capture(&mut self) -> Result<(), ControlError> {
        if let Some(lease) = self.ui.hotkey_capture_lease {
            expect_unit(
                self.transport
                    .call(ControlRequest::EndHotkeyCapture { lease })?,
            )?;
        }
        self.ui.hotkey_capture_lease = None;
        self.ui.hotkey_capture = None;
        Ok(())
    }

    pub fn capture_hotkey(&mut self, normalized_key: &str) -> bool {
        let Some(capture) = self.ui.hotkey_capture.as_mut() else {
            return false;
        };
        capture.capture(normalized_key);
        true
    }

    pub fn commit_hotkey_capture(&mut self) -> Result<(), ControlError> {
        let Some(capture) = self.ui.hotkey_capture.clone() else {
            return Ok(());
        };
        let lease = self
            .ui
            .hotkey_capture_lease
            .ok_or_else(|| unexpected_response("hotkey capture lease"))?;
        // Restore WM grabs before the shortcut mutation. If End fails, retain the lease
        // and capture so the UI can retry or cancel instead of mutating under suspension.
        expect_unit(
            self.transport
                .call(ControlRequest::EndHotkeyCapture { lease })?,
        )?;
        self.ui.hotkey_capture_lease = None;
        self.ui.hotkey_capture = None;
        let expected_revision = self
            .shortcuts
            .as_ref()
            .map_or(0, |snapshot| snapshot.revision);
        let binding = capture.captured.unwrap_or_default();
        let unchanged = self.shortcuts.as_ref().is_some_and(|snapshot| {
            let assigned = snapshot
                .bindings
                .get(&capture.action)
                .and_then(|flamewm_api::shortcuts::KeyBinding(value)| value.as_deref())
                .filter(|value| !value.is_empty());
            assigned == (!binding.is_empty()).then_some(binding.as_str())
        });
        if unchanged {
            return Ok(());
        }
        let request = if binding.is_empty() {
            ControlRequest::ClearShortcut {
                action: capture.action,
                expected_revision,
            }
        } else {
            ControlRequest::SetShortcut {
                action: capture.action,
                binding,
                expected_revision,
            }
        };
        expect_unit(self.transport.call(request)?)?;
        self.refresh_shortcuts()?;
        self.refresh_settings()
    }

    pub fn reset_hotkey(&mut self, action: impl Into<String>) -> Result<(), ControlError> {
        if self.ui.hotkey_capture_lease.is_some() {
            return Err(ControlError {
                name: "com.arkflame.FlameWM1.Error.Busy",
                code: flamewm_api::ErrorCode::Busy,
                message: "end hotkey capture before changing shortcuts".to_owned(),
            });
        }
        let expected_revision = self
            .shortcuts
            .as_ref()
            .map_or(0, |snapshot| snapshot.revision);
        expect_unit(self.transport.call(ControlRequest::ResetShortcut {
            action: action.into(),
            expected_revision,
        })?)?;
        self.refresh_shortcuts()?;
        self.refresh_settings()
    }

    pub fn begin_display_mode(
        &mut self,
        mode: ModeId,
        now_ms: u64,
    ) -> Result<TransactionId, ControlError> {
        let snapshot = self
            .displays
            .as_ref()
            .ok_or_else(|| missing_snapshot("display"))?;
        let output = self
            .ui
            .selected_output
            .clone()
            .ok_or_else(|| missing_snapshot("selected output"))?;
        let response = self.transport.call(ControlRequest::BeginDisplayMode {
            output,
            mode,
            expected_generation: snapshot.generation,
            now_ms,
        })?;
        let ControlResponse::Transaction(transaction) = response else {
            return Err(unexpected_response("display transaction"));
        };
        self.ui.pending_display_transaction = Some(transaction);
        Ok(transaction)
    }

    pub fn keep_display_mode(&mut self, now_ms: u64) -> Result<(), ControlError> {
        let Some(transaction) = self.ui.pending_display_transaction else {
            return Ok(());
        };
        expect_unit(self.transport.call(ControlRequest::KeepDisplayMode {
            transaction,
            now_ms,
        })?)?;
        self.ui.pending_display_transaction = None;
        self.refresh_displays()
    }

    pub fn revert_display_mode(&mut self) -> Result<(), ControlError> {
        let Some(transaction) = self.ui.pending_display_transaction else {
            return Ok(());
        };
        expect_unit(
            self.transport
                .call(ControlRequest::RevertDisplayMode { transaction })?,
        )?;
        self.ui.pending_display_transaction = None;
        self.refresh_displays()
    }

    pub fn set_shell_scale(&mut self, percent: u16) -> Result<(), ControlError> {
        if !(DISPLAY_SCALE_MIN..=DISPLAY_SCALE_MAX).contains(&percent)
            || (percent - DISPLAY_SCALE_MIN) % DISPLAY_SCALE_STEP != 0
        {
            return Err(ControlError {
                name: "com.arkflame.FlameWM1.Error.InvalidArgument",
                code: flamewm_api::ErrorCode::InvalidArgument,
                message: format!(
                    "display scale must be {DISPLAY_SCALE_MIN}..={DISPLAY_SCALE_MAX} in {DISPLAY_SCALE_STEP}% steps"
                ),
            });
        }
        let snapshot = self
            .displays
            .as_ref()
            .ok_or_else(|| missing_snapshot("display"))?;
        let output = self
            .ui
            .selected_output
            .clone()
            .ok_or_else(|| missing_snapshot("selected output"))?;
        expect_unit(self.transport.call(ControlRequest::SetShellScale {
            output,
            percent,
            expected_revision: snapshot.generation,
        })?)?;
        self.refresh_displays()
    }

    pub fn set_panel_edge(
        &mut self,
        output: OutputId,
        edge: PanelEdge,
    ) -> Result<(), ControlError> {
        let expected_revision = self.panels.as_ref().map_or(0, |snapshot| snapshot.revision);
        self.apply_panel_mutation(ControlRequest::SetPanelEdge {
            output,
            edge,
            expected_revision,
        })
    }

    pub fn set_panel_size(
        &mut self,
        output: OutputId,
        logical_size: u16,
    ) -> Result<(), ControlError> {
        let expected_revision = self.panels.as_ref().map_or(0, |snapshot| snapshot.revision);
        self.apply_panel_mutation(ControlRequest::SetPanelSize {
            output,
            logical_size,
            expected_revision,
        })
    }

    fn apply_panel_mutation(&mut self, request: ControlRequest) -> Result<(), ControlError> {
        match self.transport.call(request.clone()) {
            Ok(response) => {
                expect_unit(response)?;
                self.refresh_panels()
            }
            Err(error) if error.code == flamewm_api::ErrorCode::StaleRevision => {
                self.refresh_panels()?;
                let expected_revision =
                    self.panels.as_ref().map_or(0, |snapshot| snapshot.revision);
                let retry_request = match request {
                    ControlRequest::SetPanelEdge { output, edge, .. } => {
                        ControlRequest::SetPanelEdge {
                            output,
                            edge,
                            expected_revision,
                        }
                    }
                    ControlRequest::SetPanelSize {
                        output,
                        logical_size,
                        ..
                    } => ControlRequest::SetPanelSize {
                        output,
                        logical_size,
                        expected_revision,
                    },
                    _ => return Err(unexpected_response("panel mutation request")),
                };
                expect_unit(self.transport.call(retry_request)?)?;
                self.refresh_panels()
            }
            Err(error) => Err(error),
        }
    }

    fn reconcile_selected_output(&mut self, snapshot: &DisplaySnapshot) {
        let selected_still_exists =
            self.ui.selected_output.as_ref().is_some_and(|selected| {
                snapshot.outputs.iter().any(|output| &output.id == selected)
            });
        if selected_still_exists {
            return;
        }
        self.ui.selected_output = snapshot
            .outputs
            .iter()
            .find(|output| output.primary && output.connected)
            .or_else(|| snapshot.outputs.iter().find(|output| output.connected))
            .map(|output| output.id.clone());
    }
}

fn expect_unit(response: ControlResponse) -> Result<(), ControlError> {
    if response == ControlResponse::Unit {
        Ok(())
    } else {
        Err(unexpected_response("unit response"))
    }
}

fn unexpected_response(expected: &str) -> ControlError {
    ControlError {
        name: "com.arkflame.FlameWM1.Error.InternalFailure",
        code: flamewm_api::ErrorCode::InternalFailure,
        message: format!("control transport returned unexpected response; expected {expected}"),
    }
}

fn missing_snapshot(name: &str) -> ControlError {
    ControlError {
        name: "com.arkflame.FlameWM1.Error.Unavailable",
        code: flamewm_api::ErrorCode::Unavailable,
        message: format!("{name} snapshot is not loaded"),
    }
}

#[cfg(test)]
mod tests;

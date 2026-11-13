//! Clipboard history popup: Super+Shift+V toggle, list/select/clear over the control bus, and
//! the keyboard walk. The ring and the paste selection live in the compositor; this module only
//! lists them and sends the three control calls.

use super::*;
use flamewm_control_core::ControlResponse;
use flamewm_control_dbus::ControlClient;
use flamewm_dbus_reactor::BusKind;
use flamewm_shell::focus_keys::list_focus_after;
use flamewm_shell::launcher::parse_typed_key;
use flamewm_shell_core::popup_session::PopupKind;

/// One control call over the session bus. Failures are logged and the popup keeps its state.
fn call_clipboard(request: &ControlRequest) -> Result<ControlResponse, String> {
    let client = ControlClient::connect(BusKind::Session).map_err(|error| format!("{error:?}"))?;
    client.call(request).map_err(|error| format!("{error:?}"))
}

impl ShellLoop {
    /// Super+Shift+V toggle. A second request while open closes it.
    pub(super) fn toggle_clipboard(
        &mut self,
        runtime: &mut SurfaceRuntime,
        requested: Option<flamewm_api::OutputId>,
        interaction: Option<&flamewm_shell::runtime::InteractionContext>,
    ) -> Result<(), UiBackendError> {
        if self.surfaces.is_clipboard_open() {
            return self.close_clipboard(runtime);
        }
        let Some(target) = self.select_control_target(requested, interaction) else {
            return Ok(());
        };
        self.close_transients(runtime)?;
        self.close_quick_control_nonfatal();
        let Ok(transition) = self
            .popup_session
            .open(PopupKind::Clipboard, target.clone())
        else {
            return Ok(());
        };
        match call_clipboard(&ControlRequest::ListClipboardHistory) {
            Ok(ControlResponse::ClipboardHistory(entries)) => self.clipboard.replace(entries),
            Ok(_) => eprintln!("flamewm-shell: clipboard list reply had the wrong shape"),
            Err(error) => eprintln!("flamewm-shell: clipboard history unavailable: {error}"),
        }
        self.project_clipboard(runtime)?;
        match self.surfaces.open_clipboard_on(runtime, &target.output) {
            Ok(_) => {
                self.popup_session.commit(&transition);
            }
            Err(error) => eprintln!("flamewm-shell: clipboard history open refused: {error}"),
        }
        Ok(())
    }

    /// Hides the clipboard popup and releases its lease. A no-op when it is closed.
    pub(super) fn close_clipboard(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        self.surfaces
            .close_clipboard(runtime)
            .map_err(UiBackendError::Renderer)?;
        self.release_popup_lease(PopupKind::Clipboard);
        Ok(())
    }

    /// Typed keys while the popup is open. Up/Down walk the list (`focus_keys`), Return pastes
    /// the selected entry and closes, Delete clears the history, Escape closes. Returns false
    /// when the popup is closed so the next key route can handle the event.
    pub(crate) fn route_clipboard_key(
        &mut self,
        runtime: &mut SurfaceRuntime,
        payload: &str,
    ) -> Result<bool, UiBackendError> {
        if !self.surfaces.is_clipboard_open() {
            return Ok(false);
        }
        let Some((_, key)) = parse_typed_key(payload) else {
            return Ok(true);
        };
        match key {
            "Return" | "KP_Enter" => {
                if let Some(id) = self.clipboard.selected_id() {
                    if let Err(error) =
                        call_clipboard(&ControlRequest::SelectClipboardHistory { id })
                    {
                        eprintln!("flamewm-shell: clipboard select failed: {error}");
                    }
                    self.close_clipboard(runtime)?;
                }
            }
            // The control wire has no per-entry delete, so Delete clears the whole history.
            // shortcut: Delete clears all entries; revisit when a per-entry remove control call lands.
            "Delete" | "KP_Delete" => {
                if let Err(error) = call_clipboard(&ControlRequest::ClearClipboardHistory) {
                    eprintln!("flamewm-shell: clipboard clear failed: {error}");
                }
                self.clipboard.replace(Vec::new());
                self.project_clipboard(runtime)?;
            }
            "Escape" => self.close_clipboard(runtime)?,
            _ => {
                if let Some(next) =
                    list_focus_after(payload, self.clipboard.focus(), self.clipboard.count())
                {
                    self.clipboard.select(next);
                    self.project_clipboard(runtime)?;
                }
            }
        }
        Ok(true)
    }

    /// Re-projects the rows, selection and empty state into the clipboard document.
    fn project_clipboard(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), UiBackendError> {
        let list = &self.clipboard;
        runtime.with_document(self.surfaces.clipboard, |document| list.project(document))
    }
}

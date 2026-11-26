//! Clipboard history popup: Super+Shift+V toggle, list/select/clear over the control bus, and
//! the keyboard walk. The ring and the paste selection live in the compositor; this module only
//! lists them and enqueues the three control calls on the nonblocking mutation dispatcher.

use super::*;
use flamewm_shell::focus_keys::list_focus_after;
use flamewm_shell::launcher::parse_typed_key;
use flamewm_shell_core::popup_session::PopupKind;

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
        // Nonblocking: the history arrives through the mutation result drain and is applied on a
        // later tick, so opening Super+Shift+V never waits on a control bus round trip.
        let _ = flamewm_shell::control_actions::enqueue_clipboard_list(self.dispatcher.as_ref());
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
                    flamewm_shell::control_actions::enqueue_action(
                        self.dispatcher.as_ref(),
                        ControlRequest::SelectClipboardHistory { id },
                    );
                    self.close_clipboard(runtime)?;
                }
            }
            // The control wire has no per-entry delete, so Delete clears the whole history.
            // shortcut: Delete clears all entries; revisit when a per-entry remove control call lands.
            "Delete" | "KP_Delete" => {
                flamewm_shell::control_actions::enqueue_action(
                    self.dispatcher.as_ref(),
                    ControlRequest::ClearClipboardHistory,
                );
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

    /// Applies clipboard history replies drained from the control mutation queue. A reply that
    /// arrives while the popup is open re-projects and redraws it; one that arrives after it
    /// closed only updates the model, which the next open re-requests anyway.
    pub(super) fn apply_clipboard_outcomes(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        let outcomes = flamewm_shell::control_actions::take_clipboard_list_outcomes();
        if outcomes.is_empty() {
            return Ok(());
        }
        let mut changed = false;
        for outcome in outcomes {
            match outcome {
                Ok(entries) => {
                    self.clipboard.replace(entries);
                    changed = true;
                }
                Err(error) => eprintln!("flamewm-shell: clipboard history unavailable: {error}"),
            }
        }
        if changed && self.surfaces.is_clipboard_open() {
            self.project_clipboard(runtime)?;
            runtime.redraw(self.surfaces.clipboard)?;
        }
        Ok(())
    }
}

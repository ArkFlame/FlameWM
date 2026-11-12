//! Polkit prompt card: opens and closes it with the dialog, routes keys and buttons into the
//! dialog, and hands a submitted password to the answer path. The password leaves the dialog only
//! as a `Submission`, which the answer path wipes after the helper write.

use std::rc::Rc;

use flamewm_integrations_linux::polkit_agent::AuthOutcome;
use flamewm_shell::polkit_answer;
use flamewm_shell::polkit_dialog::DialogModel;
use flamewm_shell::projection;
use flamewm_ui_x11::{SurfaceRuntime, UiBackendError};

use super::ShellLoop;

impl ShellLoop {
    /// Opens the card while a prompt is active, closes it when none is, and projects the prompt.
    pub(crate) fn sync_polkit(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        let shared = Rc::clone(&self.polkit_dialog);
        let dialog = shared.borrow();
        let Some(model) = dialog.active() else {
            return self
                .surfaces
                .close_polkit(runtime)
                .map_err(UiBackendError::Renderer);
        };
        self.surfaces
            .open_polkit(runtime)
            .map_err(UiBackendError::Renderer)?;
        runtime.with_document(self.surfaces.polkit, |document| {
            projection::project_polkit(document, model)
        })
    }

    /// Keyboard text while the card is open: Escape cancels, Enter submits, others edit the field.
    pub(crate) fn route_polkit_text(
        &mut self,
        runtime: &mut SurfaceRuntime,
        input: &str,
    ) -> Result<(), UiBackendError> {
        match input {
            "\u{1b}" => self.cancel_polkit(runtime),
            "\n" => self.submit_polkit(runtime),
            "\u{8}" => {
                self.edit_polkit(DialogModel::backspace);
                self.sync_polkit(runtime)
            }
            text => {
                self.edit_polkit(|model| {
                    model.input(text);
                });
                self.sync_polkit(runtime)
            }
        }
    }

    /// Card controls. `text` carries the select or input payload.
    pub(crate) fn route_polkit_action(
        &mut self,
        runtime: &mut SurfaceRuntime,
        action: &str,
        text: Option<&str>,
    ) -> Result<(), UiBackendError> {
        match action {
            "polkit.submit" => self.submit_polkit(runtime),
            "polkit.cancel" => self.cancel_polkit(runtime),
            "polkit.identity" => {
                if let Some(index) = text.and_then(|value| value.parse::<usize>().ok()) {
                    self.edit_polkit(|model| {
                        model.select(index);
                    });
                }
                self.sync_polkit(runtime)
            }
            "polkit.input" => self.route_polkit_text(runtime, text.unwrap_or_default()),
            _ => Ok(()),
        }
    }

    fn edit_polkit(&self, edit: impl FnOnce(&mut DialogModel)) {
        if let Some(model) = self.polkit_dialog.borrow_mut().active_mut() {
            edit(model);
        }
    }

    /// Cancel: the active prompt's pending call gets `Cancelled`, the next prompt then shows.
    fn cancel_polkit(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), UiBackendError> {
        if let Some(agent) = self.polkit_agent.clone() {
            polkit_answer::cancel_active(&agent, &self.polkit_dialog);
        }
        self.sync_polkit(runtime)
    }

    /// Submit: the helper gets the password. A start failure replies `Failed` at once.
    fn submit_polkit(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), UiBackendError> {
        let (Some(agent), Some(reactor)) = (self.polkit_agent.clone(), self.polkit_reactor.clone())
        else {
            return Ok(());
        };
        let dialog = Rc::clone(&self.polkit_dialog);
        let submission = dialog.borrow_mut().submit();
        if let Some(submission) = submission {
            let cookie = submission.cookie.clone();
            if let Err(error) =
                polkit_answer::start(&reactor, Rc::clone(&agent), Rc::clone(&dialog), submission)
            {
                eprintln!("flamewm-shell: polkit answer failed: {error}");
                polkit_answer::finish_and_dismiss(&agent, &dialog, &cookie, AuthOutcome::Failed);
            }
        }
        self.sync_polkit(runtime)
    }
}

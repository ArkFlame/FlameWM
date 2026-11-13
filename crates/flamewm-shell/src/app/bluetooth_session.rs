//! Bluetooth pairing card: opens and closes it with the agent's queue, shows the oldest prompt or
//! the display notice, and routes keys and buttons into the card model. Escape rejects a prompt
//! (or dismisses a notice), Enter accepts, and an accepted PIN or passkey goes to the agent through
//! `BluetoothAgent::decide`.

use std::rc::Rc;

use flamewm_integrations_linux::bluetooth_agent::{Decision, PromptKind};
use flamewm_shell::bluetooth_dialog::{button_activation, Shown};
use flamewm_shell::projection;
use flamewm_ui_x11::{SurfaceRuntime, UiBackendError};

use super::ShellLoop;

impl ShellLoop {
    /// Opens the card while the agent has a prompt or notice, closes it when none, and projects it.
    pub(crate) fn sync_bluetooth(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        let Some(agent) = self.bluetooth_agent.clone() else {
            return Ok(());
        };
        // The card shows the device's BlueZ name (address until it arrives); see `device_label`.
        let shown = {
            let mut agent = agent.borrow_mut();
            if let Err(error) = agent.expire_due() {
                eprintln!("flamewm-shell: bluetooth expiry failed: {error}");
            }
            let label =
                |kind: &PromptKind| kind.clone().with_device(agent.device_label(kind.device()));
            let prompt = agent.prompts().next().map(|(id, kind)| Shown::Prompt {
                id,
                kind: label(kind),
            });
            prompt.or_else(|| agent.notice().map(|kind| Shown::Notice(label(kind))))
        };
        let shared = Rc::clone(&self.bluetooth_dialog);
        let mut dialog = shared.borrow_mut();
        let Some(shown) = shown else {
            dialog.follow(None);
            return self
                .surfaces
                .close_bluetooth(runtime)
                .map_err(UiBackendError::Renderer);
        };
        dialog.follow(match &shown {
            Shown::Prompt { id, .. } => Some(*id),
            Shown::Notice(_) => None,
        });
        self.surfaces
            .open_bluetooth(runtime)
            .map_err(UiBackendError::Renderer)?;
        runtime.with_document(self.surfaces.bluetooth, |document| {
            projection::project_bluetooth(document, &shown, &dialog)
        })
    }

    /// Keyboard text while the card is open: Escape rejects, Enter accepts, others edit the value.
    pub(crate) fn route_bluetooth_text(
        &mut self,
        runtime: &mut SurfaceRuntime,
        input: &str,
    ) -> Result<(), UiBackendError> {
        let focus = self.bluetooth_dialog.borrow().focus();
        if let Some(action) = button_activation(Some(input), focus) {
            return self.route_bluetooth_action(runtime, action);
        }
        match input {
            "\u{1b}" => self.bluetooth_reject(),
            "\n" => self.bluetooth_accept(),
            "\u{8}" => self.bluetooth_dialog.borrow_mut().backspace(),
            text => self.bluetooth_dialog.borrow_mut().input(text),
        }
        self.sync_bluetooth(runtime)
    }

    /// Card buttons: `bluetooth.accept` or `bluetooth.reject`.
    pub(crate) fn route_bluetooth_action(
        &mut self,
        runtime: &mut SurfaceRuntime,
        action: &str,
    ) -> Result<(), UiBackendError> {
        match action {
            "bluetooth.accept" => self.bluetooth_accept(),
            "bluetooth.reject" => self.bluetooth_reject(),
            _ => {}
        }
        self.sync_bluetooth(runtime)
    }

    /// The oldest prompt gets `Reject`; with no prompt the notice is dismissed.
    fn bluetooth_reject(&self) {
        let Some(shared) = self.bluetooth_agent.clone() else {
            return;
        };
        let mut agent = shared.borrow_mut();
        let id = agent.prompts().next().map(|(id, _)| id);
        match id {
            Some(id) => report(agent.decide(id, Decision::Reject)),
            None => agent.dismiss_notice(),
        }
    }

    /// The oldest prompt gets its accept answer, with the typed value when it takes one. An invalid
    /// value keeps the prompt open with its reason. With no prompt the notice is dismissed.
    fn bluetooth_accept(&self) {
        let Some(shared) = self.bluetooth_agent.clone() else {
            return;
        };
        let mut agent = shared.borrow_mut();
        let prompt = agent.prompts().next().map(|(id, kind)| (id, kind.clone()));
        let Some((id, kind)) = prompt else {
            agent.dismiss_notice();
            return;
        };
        let decision = self.bluetooth_dialog.borrow_mut().accept(&kind);
        if let Some(decision) = decision {
            report(agent.decide(id, decision));
        }
    }
}

fn report(result: Result<bool, flamewm_api::FlameError>) {
    if let Err(error) = result {
        eprintln!("flamewm-shell: bluetooth answer failed: {error}");
    }
}

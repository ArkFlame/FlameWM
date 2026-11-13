//! Launcher toggle, keyboard and pointer routing, and launch for the top-center
//! Launcher. Filtering and selection live in `flamewm_shell::launcher`; placement
//! and map/grab live in `runtime::launcher_surface`.

use super::*;
use flamewm_shell::launcher::{launcher_move, parse_typed_key, LauncherMove};
use flamewm_shell_core::popup_session::PopupKind;

impl ShellLoop {
    /// Super+Space toggle. A second request while open closes it.
    pub(super) fn toggle_launcher(
        &mut self,
        runtime: &mut SurfaceRuntime,
        requested: Option<flamewm_api::OutputId>,
        interaction: Option<&flamewm_shell::runtime::InteractionContext>,
    ) -> Result<(), UiBackendError> {
        if self.surfaces.is_launcher_open() {
            return self.close_launcher(runtime);
        }
        let Some(target) = self.select_control_target(requested, interaction) else {
            return Ok(());
        };
        // The launcher owns the screen: Start, task menu, media and quick helpers close first.
        self.close_transients(runtime)?;
        self.close_quick_control_nonfatal();
        let Ok(transition) = self.popup_session.open(PopupKind::Launcher, target.clone()) else {
            return Ok(());
        };
        self.refresh_launcher_applications();
        self.launcher.open(&self.start_model);
        // Projection runs before the measured open so the placed size matches the rows shown.
        self.project_launcher(runtime)?;
        match self.surfaces.open_launcher_on(runtime, &target.output) {
            Ok(_) => {
                self.popup_session.commit(&transition);
                self.apply_launcher_backdrop(runtime);
            }
            Err(error) => eprintln!("flamewm-shell: launcher open refused: {error}"),
        }
        Ok(())
    }

    /// Launcher open: lists apps installed since the last read. Gated on XDG application dir
    /// mtimes (KDE kbuildsycoca precedent), so an unchanged set costs stat calls only. No watcher.
    fn refresh_launcher_applications(&mut self) {
        match flamewm_applications::refresh_installed_applications_if_changed() {
            Ok(delta) if !delta.is_empty() => {
                let applications = flamewm_applications::shared_application_catalog()
                    .current()
                    .all();
                self.start_model.replace_applications(applications);
            }
            Ok(_) => {}
            Err(error) => eprintln!("flamewm-shell: launcher app rescan failed: {error:?}"),
        }
    }

    /// Hides the launcher and releases its popup lease. A no-op when it is closed.
    pub(super) fn close_launcher(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        self.surfaces
            .close_launcher(runtime)
            .map_err(UiBackendError::Renderer)?;
        self.release_popup_lease(PopupKind::Launcher);
        Ok(())
    }

    /// Ends the popup lease of `kind` when it is the active one; a no-op otherwise.
    pub(super) fn release_popup_lease(&mut self, kind: PopupKind) {
        if let Some(lease) = self
            .popup_session
            .active()
            .filter(|lease| lease.kind == kind)
            .cloned()
        {
            let transition = self.popup_session.close(lease.sequence);
            self.popup_session.commit(&transition);
        }
    }

    /// Text input while the launcher is open. Returns false when the launcher is closed,
    /// so the Start search path can handle the same event.
    pub(crate) fn route_launcher_text(
        &mut self,
        runtime: &mut SurfaceRuntime,
        input: &str,
    ) -> Result<bool, UiBackendError> {
        if !self.surfaces.is_launcher_open() {
            return Ok(false);
        }
        match input {
            "\u{8}" => {
                if self.launcher.backspace(&self.start_model) {
                    self.project_launcher(runtime)?;
                }
            }
            "\n" => self.launch_launcher_slot(runtime, self.launcher.selected())?,
            value if !value.is_empty() && !value.chars().any(char::is_control) => {
                if self.launcher.type_text(&self.start_model, value) {
                    self.project_launcher(runtime)?;
                }
            }
            _ => {}
        }
        Ok(true)
    }

    /// Typed keys while the launcher is open. Enter launches; Up/Down, Ctrl+J/K, Tab/Shift+Tab,
    /// Home/End and PageUp/PageDown move the selection; every other key is consumed without effect.
    pub(crate) fn route_launcher_key(
        &mut self,
        runtime: &mut SurfaceRuntime,
        payload: &str,
    ) -> Result<bool, UiBackendError> {
        if !self.surfaces.is_launcher_open() {
            return Ok(false);
        }
        let Some((modifiers, key)) = parse_typed_key(payload) else {
            return Ok(true);
        };
        if key == "Return" || key == "KP_Enter" {
            self.launch_launcher_slot(runtime, self.launcher.selected())?;
        } else if let Some(movement) = launcher_move(modifiers, key) {
            match movement {
                LauncherMove::By(delta) => self.launcher.move_by(delta),
                LauncherMove::To(index) => self.launcher.move_to(index),
            }
            self.project_launcher(runtime)?;
        }
        Ok(true)
    }

    /// Pointer hover over visible row `slot`: moves the one selection highlight. Only the
    /// highlight node is rewritten, and only when the hovered row actually changed.
    pub(crate) fn route_launcher_hover(
        &mut self,
        runtime: &mut SurfaceRuntime,
        slot: usize,
    ) -> Result<(), UiBackendError> {
        if !self.surfaces.is_launcher_open() || !self.launcher.select_slot(slot) {
            return Ok(());
        }
        let launcher = &self.launcher;
        runtime.with_document(self.surfaces.launcher, |document| {
            launcher.project_highlight(document)
        })
    }

    /// Pointer left the launcher rows: drop the highlight (no-op when nothing is highlighted).
    pub(crate) fn route_launcher_leave(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        if !self.surfaces.is_launcher_open() || !self.launcher.leave() {
            return Ok(());
        }
        let launcher = &self.launcher;
        runtime.with_document(self.surfaces.launcher, |document| {
            launcher.project_highlight(document)
        })
    }

    /// Launches the result at `index` (absolute index in the result list) and closes.
    pub(super) fn launch_launcher_slot(
        &mut self,
        runtime: &mut SurfaceRuntime,
        index: usize,
    ) -> Result<(), UiBackendError> {
        let Some(application) = self.launcher.app_at(&self.start_model, index) else {
            return Ok(());
        };
        // One launch per open: a repeated activation (key repeat, Return plus a second route) is a no-op.
        if !self.launcher.claim_activation() {
            return Ok(());
        }
        let (id, name) = (application.id.clone(), application.name.clone());
        self.launcher.note_launch(&id);
        // Launch feedback and the mutation go through the same path as Start.
        flamewm_shell::control_actions::enqueue_start_launch(
            self.dispatcher.as_ref(),
            name,
            ControlRequest::LaunchApplication {
                app: id,
                options: flamewm_api::applications::ApplicationLaunchOptions::default(),
            },
        );
        self.close_launcher(runtime)
    }

    /// Pointer activation of a visible row (`launcher.app.N`, 1-based slot).
    pub(crate) fn click_launcher_row(
        &mut self,
        runtime: &mut SurfaceRuntime,
        slot: usize,
    ) -> Result<(), UiBackendError> {
        let index = self.launcher.window().start + slot;
        self.launch_launcher_slot(runtime, index)
    }

    /// Re-projects the rows, icons, highlight and query line into the launcher document.
    pub(super) fn project_launcher(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        self.ensure_loader();
        let loader = self.icon_loader.as_mut();
        let launcher = &self.launcher;
        let model = &self.start_model;
        runtime.with_document(self.surfaces.launcher, |document| {
            launcher.project(document, model, loader)
        })?;
        Ok(())
    }

    /// Installs the Launcher backdrop percents from settings, as the Start popup does.
    fn apply_launcher_backdrop(&self, runtime: &mut SurfaceRuntime) {
        let Some(settings) = self.settings_cache.as_ref() else {
            return;
        };
        let percents = crate::entry::settings_projection::start_backdrop_percents(settings);
        if let Err(error) = runtime.set_popup_backdrop(self.surfaces.launcher, Some(percents)) {
            eprintln!("flamewm-shell: launcher backdrop failed (nonfatal): {error:?}");
        }
    }
}

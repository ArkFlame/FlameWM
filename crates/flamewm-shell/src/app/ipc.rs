//! IPC and bus integration: control signals, job-view provider, status and network secret routing.

use super::*;

impl ShellLoop {
    /// J07: start the helper asynchronously after panel show. Spawn failure
    /// is nonfatal: the shell keeps running with in-process media only.
    pub(crate) fn start_helper_after_panel(&mut self) {
        if let Err(error) = self.quick_control.start_after_panel() {
            eprintln!("flamewm-shell: quick-control helper spawn failed (nonfatal): {error}");
        }
    }

    pub(crate) fn start_job_view_provider(&mut self) -> Option<flamewm_dbus_reactor::BusWatch> {
        use flamewm_integrations_linux::job_view::KioJobViewProvider;
        let mut provider = match KioJobViewProvider::connect() {
            Ok(provider) => provider,
            Err(error) => {
                eprintln!("flamewm-shell: KIO job progress unavailable: {error}");
                return None;
            }
        };
        match provider.start() {
            Ok(true) => {
                let watch = provider.watch();
                self.job_view_provider = Some(provider);
                Some(watch)
            }
            Ok(false) => {
                eprintln!("flamewm-shell: KIO job progress owned by another desktop");
                None
            }
            Err(error) => {
                eprintln!("flamewm-shell: KIO job progress start failed: {error}");
                None
            }
        }
    }

    pub(crate) fn job_view_ready(&mut self) {
        let Some(provider) = self.job_view_provider.as_mut() else {
            return;
        };
        if let Err(error) = provider.on_ready() {
            eprintln!("flamewm-shell: KIO job progress event failed: {error}");
            self.job_view_provider = None;
            self.task_progress.clear();
            self.task_progress_pending = true;
            return;
        }
        if provider.take_changed() {
            self.refresh_task_progress();
        }
    }

    pub(super) fn refresh_task_progress(&mut self) {
        self.task_progress = self
            .job_view_provider
            .as_ref()
            .map(|provider| provider.task_progress(&self.shell.snapshot().panels))
            .unwrap_or_default();
        self.task_progress_pending = true;
    }

    pub(crate) fn note_signal(&mut self, signal: &flamewm_control_wire::ControlSignal) {
        if let flamewm_control_wire::ControlSignal::SettingsSnapshotChanged { snapshot } = signal {
            self.apply_settings_snapshot(snapshot.clone());
        }
        self.shell.mark_signal(signal);
    }

    /// J07 nonblocking submit: enqueue onto the mutation queue and return
    /// immediately. Full/closed queue is a nonfatal diagnostic.
    pub(crate) fn submit(&self, request: ControlRequest) {
        flamewm_shell::control_actions::enqueue_action(self.dispatcher.as_ref(), request);
    }

    pub(crate) fn network_secret_request(&self, submit: bool) -> Option<ControlRequest> {
        let snapshot = self.shell.snapshot();
        let secret = snapshot.system.network.pending_secret.as_ref()?;
        let action = if submit {
            flamewm_api::system::SystemAction::SubmitNetworkSecret {
                request_id: secret.request_id,
                generation: snapshot.system.network.generation,
                secret: self.network_secret.clone(),
            }
        } else {
            flamewm_api::system::SystemAction::CancelNetworkSecret {
                request_id: secret.request_id,
                generation: snapshot.system.network.generation,
            }
        };
        Some(ControlRequest::SystemAction {
            action,
            expected_revision: snapshot.system.revision,
        })
    }

    pub(crate) fn project_network_secret(
        &self,
        _runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        // J07: the secret editor lives in the helper process; the parent
        // keeps the masked-length-free no-op so keyboard paths stay total.
        Ok(())
    }

    pub(crate) fn clear_network_secret(
        &mut self,
        _runtime: &mut SurfaceRuntime,
    ) -> Result<(), UiBackendError> {
        self.network_secret.clear();
        Ok(())
    }

    pub(crate) fn project_status_content(
        &mut self,
        runtime: &mut SurfaceRuntime,
        name: &str,
    ) -> Result<(), UiBackendError> {
        // Project-first: the measured open reads intrinsic size from the
        // already-projected document, so content must land before measure.
        // J07: only media projects in-process; helper kinds are unreachable
        // here (routed to the supervisor before this call).
        let snapshot = self.shell.snapshot();
        if name == "media" {
            runtime.with_document(self.surfaces.media, |document| {
                projection::project_media(document, snapshot)
            })?;
        }
        Ok(())
    }
}

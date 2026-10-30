//! Catalog icon job submission, completion, and deferred-icon bookkeeping.
use super::*;

pub(super) fn icon_completion_matches(
    current_token: u64,
    expected_key: Option<&flamewm_integrations_linux::icons::IconKey>,
    result_token: u64,
    result_key: &flamewm_integrations_linux::icons::IconKey,
) -> bool {
    current_token == result_token && expected_key == Some(result_key)
}

pub(super) fn retain_deferred_icon_job(
    deferred: &mut HashMap<Window, (u64, flamewm_integrations_linux::icon_service::IconJob)>,
    client: Window,
    token: u64,
    job: flamewm_integrations_linux::icon_service::IconJob,
) {
    deferred.insert(client, (token, job));
}

impl<'a, C: Connection> Wm<'a, C> {
    /// Submits a HIGH catalog-icon job; parks it on queue pressure. True when the service has stopped.
    pub(super) fn submit_catalog_icon(
        &mut self,
        client_id: Window,
        token: u64,
        job: flamewm_integrations_linux::icon_service::IconJob,
    ) -> bool {
        use flamewm_integrations_linux::icon_service::{IconSubmit, IconSubmitError};
        match self.icon_service.borrow().submit(job) {
            Ok(IconSubmit::Enqueued | IconSubmit::Coalesced) => false,
            Ok(IconSubmit::DroppedLow) => unreachable!("catalog icons are HIGH priority"),
            Err(IconSubmitError::HighQueueFull(job)) => {
                retain_deferred_icon_job(&mut self.deferred_icon_jobs, client_id, token, job);
                false
            }
            Err(IconSubmitError::Stopped(_)) => true,
        }
    }

    /// Consume worker completions on the WM owner thread and retry retained
    /// HIGH jobs only after a wake, so queue pressure never causes polling.
    pub(super) fn drain_catalog_icon_results(&mut self) {
        let ready = self.icon_service.borrow_mut().drain_ready();
        for result in ready {
            let requesters = self.icon_requests.remove(&result.key).unwrap_or_default();
            let Ok(raster) = result.result else { continue };
            for (client_id, token) in requesters {
                let Some(client) = self.clients.get_mut(&client_id) else {
                    continue;
                };
                if !icon_completion_matches(
                    client.icon_request_token,
                    client.catalog_icon_key.as_ref(),
                    token,
                    &result.key,
                ) {
                    continue;
                }
                client.icon = crate::decoration::cache::catalog_icon_image(&raster);
                client.icon_fallback = client
                    .icon
                    .is_none()
                    .then(|| "catalog-raster-invalid".to_owned());
                self.mark_windows();
                let _ = self.paint_chrome(client_id);
            }
        }

        let deferred = std::mem::take(&mut self.deferred_icon_jobs);
        for (client_id, (token, job)) in deferred {
            let current = self.clients.get(&client_id).is_some_and(|client| {
                client.icon_request_token == token
                    && client.catalog_icon_key.as_ref() == Some(&job.key)
            });
            if !current {
                continue;
            }
            self.submit_catalog_icon(client_id, token, job);
        }
    }

    pub(super) fn request_catalog_icon(&mut self, client_id: Window) {
        use flamewm_integrations_linux::icon_service::{IconJob, IconPriority};
        use flamewm_integrations_linux::icons::{
            IconKey, IconLookupPurpose, IconRequest, IconSize,
        };

        let Some((instance, class, token)) = self.clients.get(&client_id).map(|client| {
            (
                client.wm_instance.clone(),
                client.wm_class.clone(),
                client.icon_request_token,
            )
        }) else {
            return;
        };
        let identity = flamewm_applications::WindowApplicationIdentity::new(
            &*instance,
            &*class,
            String::new(),
        );
        let Some(icon_name) = self
            .catalog
            .find_by_window_identity(&identity)
            .map(|app| app.icon().to_owned())
        else {
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.icon_fallback = Some("catalog-icon-not-found".to_owned());
            }
            return;
        };
        if icon_name.trim().is_empty() {
            return;
        }
        let edge = u32::from(crate::chrome::icon_slot_for(self.config.titlebar_height));
        let service = self.icon_service.borrow();
        let key = IconKey::new(
            IconLookupPurpose::Application,
            IconRequest::name(icon_name),
            IconSize::new(edge, edge),
            service.theme_generation(),
            0,
        );
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.catalog_icon_key = Some(key.clone());
            client.icon_fallback = Some("catalog-icon-pending".to_owned());
        }
        let job = IconJob {
            key: key.clone(),
            priority: IconPriority::High,
        };
        if let Some(Ok(raster)) = service.cached(&key) {
            if let Some(client) = self.clients.get_mut(&client_id).filter(|client| {
                client.icon_request_token == token && client.catalog_icon_key.as_ref() == Some(&key)
            }) {
                client.icon = crate::decoration::cache::catalog_icon_image(&raster);
                client.icon_fallback = None;
            }
            return;
        }
        drop(service);
        self.icon_requests
            .entry(key)
            .or_default()
            .push((client_id, token));
        if self.submit_catalog_icon(client_id, token, job) {
            if let Some(client) = self.clients.get_mut(&client_id) {
                client.icon_fallback = Some("catalog-icon-service-stopped".to_owned());
            }
        }
    }
}

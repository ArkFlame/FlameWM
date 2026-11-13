use flamewm_api::applications::{ApplicationLaunchOptions, DesktopApplication};
use flamewm_api::ports::ApplicationPort;
use flamewm_api::{DesktopAppId, ErrorCode, FlameError, FlameResult};

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ApplicationService {
    applications: Vec<DesktopApplication>,
    revision: u64,
}

impl ApplicationService {
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn all(&self) -> &[DesktopApplication] {
        &self.applications
    }

    pub fn replace_catalog(&mut self, mut applications: Vec<DesktopApplication>) -> bool {
        applications.sort_by(|a, b| a.id.cmp(&b.id));
        applications.dedup_by(|a, b| a.id == b.id);
        if applications == self.applications {
            return false;
        }
        self.applications = applications;
        self.revision = self.revision.saturating_add(1);
        true
    }

    pub fn find_by_id(&self, id: &DesktopAppId) -> FlameResult<&DesktopApplication> {
        self.applications
            .iter()
            .find(|app| &app.id == id)
            .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "application id not found"))
    }

    #[must_use]
    pub fn search(&self, query: &str) -> Vec<DesktopApplication> {
        // An empty needle matches every field via `str::contains("")`, so no early return is needed.
        let needle = query.trim().to_lowercase();
        self.applications
            .iter()
            .filter(|app| {
                contains(app.id.as_str(), &needle)
                    || contains(&app.name, &needle)
                    || contains(&app.generic_name, &needle)
                    || contains(&app.comment, &needle)
                    || app.keywords.iter().any(|k| contains(k, &needle))
                    || app.categories.iter().any(|c| contains(c, &needle))
            })
            .cloned()
            .collect()
    }

    pub fn launch<P: ApplicationPort>(
        &self,
        port: &mut P,
        id: &DesktopAppId,
        options: &ApplicationLaunchOptions,
    ) -> FlameResult<()> {
        // The port resolves `id` against the live catalog with refresh-on-miss. This snapshot can
        // lag an app installed since the last push, so it must not reject the launch before the port.
        port.launch(id, options)
    }
}

fn contains(text: &str, needle: &str) -> bool {
    text.to_lowercase().contains(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct RecordingPort {
        launched: Vec<String>,
    }

    impl ApplicationPort for RecordingPort {
        fn launch(&mut self, app: &DesktopAppId, _: &ApplicationLaunchOptions) -> FlameResult<()> {
            self.launched.push(app.as_str().to_owned());
            Ok(())
        }
        fn launch_uri(&mut self, _: &str) -> FlameResult<()> {
            Ok(())
        }
    }

    #[test]
    fn launch_reaches_port_for_app_missing_from_snapshot() {
        // An app installed after the snapshot was pushed must still reach the port, which refreshes.
        let service = ApplicationService::default();
        let mut port = RecordingPort::default();
        let id = DesktopAppId::new("zorblax-probe.desktop");
        service
            .launch(&mut port, &id, &ApplicationLaunchOptions::default())
            .expect("port decides resolution");
        assert_eq!(port.launched, ["zorblax-probe.desktop"]);
    }
}

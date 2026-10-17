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
        self.find_by_id(id)?;
        port.launch(id, options)
    }

}

fn contains(text: &str, needle: &str) -> bool {
    text.to_lowercase().contains(needle)
}

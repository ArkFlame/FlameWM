use flamewm_api::applications::DesktopApplication;
use flamewm_shell_core::StartModel;

pub type StartState = StartModel;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartCategory {
    All,
    Development,
    Games,
    Graphics,
    Internet,
    Multimedia,
    System,
    Utilities,
    Power,
}

pub const PRESENTATION_CATEGORIES: [(&str, StartCategory); 8] = [
    ("development", StartCategory::Development),
    ("games", StartCategory::Games),
    ("graphics", StartCategory::Graphics),
    ("internet", StartCategory::Internet),
    ("multimedia", StartCategory::Multimedia),
    ("system", StartCategory::System),
    ("utilities", StartCategory::Utilities),
    ("power", StartCategory::Power),
];

#[must_use]
pub fn category_for_action(action: &str) -> Option<StartCategory> {
    PRESENTATION_CATEGORIES
        .iter()
        .find(|(name, _)| action == format!("start.category.{name}"))
        .map(|(_, category)| *category)
}

#[must_use]
pub fn state(applications: Vec<DesktopApplication>) -> StartState {
    StartModel::new(applications)
}

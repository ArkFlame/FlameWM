use flamewm_api::applications::DesktopApplication;
use flamewm_shell_core::StartModel;

pub type StartState = StartModel;

/// Start list view event actions (`start.view.*`), routed by the event router.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartViewAction {
    Show(flamewm_shell_core::start_menu::StartView),
    Toggle,
}

#[must_use]
pub fn view_for_action(action: &str) -> Option<StartViewAction> {
    use flamewm_shell_core::start_menu::StartView;
    match action {
        "start.view.all" => Some(StartViewAction::Show(StartView::AllPrograms)),
        "start.view.categories" => Some(StartViewAction::Show(StartView::Categories)),
        "start.view.toggle" => Some(StartViewAction::Toggle),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartCategory {
    All,
    Favorites,
    Development,
    Games,
    Graphics,
    Internet,
    Multimedia,
    System,
    Utilities,
    Power,
}

pub const PRESENTATION_CATEGORIES: [(&str, StartCategory); 9] = [
    ("favorites", StartCategory::Favorites),
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
    let name = action.strip_prefix("start.category.")?;
    PRESENTATION_CATEGORIES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, category)| *category)
}

/// Category to show after the Favorites list changed: with no favorites the Favorites entry is
/// hidden, so that view falls back to All.
#[must_use]
pub fn category_after_favorites(category: StartCategory, favorites_empty: bool) -> StartCategory {
    if category == StartCategory::Favorites && favorites_empty {
        StartCategory::All
    } else {
        category
    }
}

#[must_use]
pub fn state(applications: Vec<DesktopApplication>) -> StartState {
    StartModel::new(applications)
}

#[cfg(test)]
mod category_fallback_tests {
    use super::{category_after_favorites, StartCategory};

    #[test]
    fn empty_favorites_view_falls_back_to_all() {
        assert_eq!(
            category_after_favorites(StartCategory::Favorites, true),
            StartCategory::All
        );
        assert_eq!(
            category_after_favorites(StartCategory::Favorites, false),
            StartCategory::Favorites
        );
        assert_eq!(
            category_after_favorites(StartCategory::Games, true),
            StartCategory::Games
        );
    }
}

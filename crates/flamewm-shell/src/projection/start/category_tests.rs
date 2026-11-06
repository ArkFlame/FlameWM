use super::*;
use flamewm_api::DesktopAppId;

fn application(categories: &[&str]) -> DesktopApplication {
    DesktopApplication {
        id: DesktopAppId::new("org.example.app"),
        name: "App".to_owned(),
        generic_name: String::new(),
        comment: String::new(),
        startup_wm_class: String::new(),
        argv: Vec::new(),
        keywords: Vec::new(),
        icon_name: String::new(),
        categories: categories.iter().map(|value| (*value).to_owned()).collect(),
    }
}

/// Literal oracle: the presentation bucket each application set must land in.
/// Unmatched or empty category sets fall to Utilities.
const ORACLE: &[(&[&str], StartCategory)] = &[
    (&["Accessories"], StartCategory::Utilities),
    (&["utility"], StartCategory::Utilities),
    (&["Education"], StartCategory::Utilities),
    (&["Game"], StartCategory::Games),
    (&["GAMES"], StartCategory::Games),
    (&["Graphics"], StartCategory::Graphics),
    (&["Network"], StartCategory::Internet),
    (&["Internet"], StartCategory::Internet),
    (&["AudioVideo"], StartCategory::Multimedia),
    (&["Audio"], StartCategory::Multimedia),
    (&["Video"], StartCategory::Multimedia),
    (&["Multimedia"], StartCategory::Multimedia),
    (&["Development"], StartCategory::Development),
    (&["Office"], StartCategory::Utilities),
    (&["Science"], StartCategory::Utilities),
    (&["Settings"], StartCategory::System),
    (&["System"], StartCategory::System),
    (&["Unknown"], StartCategory::Utilities),
    (&[""], StartCategory::Utilities),
    (&[], StartCategory::Utilities),
    (&["Unknown", "Games"], StartCategory::Games),
    (&["Office", "Games"], StartCategory::Utilities),
    (&["Unknown", "Development"], StartCategory::Development),
];

#[test]
fn application_matches_category_is_the_oracle_table_for_every_selectable_category() {
    let rows = [
        (StartCategory::Development, StartCategory::Development),
        (StartCategory::Games, StartCategory::Games),
        (StartCategory::Graphics, StartCategory::Graphics),
        (StartCategory::Internet, StartCategory::Internet),
        (StartCategory::Multimedia, StartCategory::Multimedia),
        (StartCategory::System, StartCategory::System),
        (StartCategory::Utilities, StartCategory::Utilities),
    ];
    for (categories, bucket) in ORACLE {
        let app = application(categories);
        for (selected, expected_bucket) in rows {
            assert_eq!(
                application_matches_category(&app, selected),
                expected_bucket == *bucket,
                "categories {categories:?} vs {selected:?}"
            );
        }
        assert!(application_matches_category(&app, StartCategory::All));
        assert!(!application_matches_category(&app, StartCategory::Power));
    }
}

fn named(id: &str, name: &str) -> DesktopApplication {
    let mut app = application(&["Utility"]);
    app.id = DesktopAppId::new(id);
    app.name = name.to_owned();
    app
}

#[test]
fn favorites_entry_is_empty_without_favorites_and_ignores_unknown_ids() {
    let mut model = StartModel::new(vec![named("calc.desktop", "Calculator")]);
    assert!(start_visible_results(&model, StartCategory::Favorites, "").is_empty());
    model.set_favorites(vec!["ghost.desktop".to_owned()]);
    assert!(start_visible_results(&model, StartCategory::Favorites, "").is_empty());
    model.set_favorites(vec!["ghost.desktop".to_owned(), "calc.desktop".to_owned()]);
    let names: Vec<&str> = start_visible_results(&model, StartCategory::Favorites, "")
        .iter()
        .map(|app| app.name.as_str())
        .collect();
    assert_eq!(names, ["Calculator"]);
}

#[test]
fn favorites_round_trip_through_settings_snapshot() {
    use flamewm_api::settings::{SettingValue, SettingsSnapshot};
    let ids = vec!["gimp.desktop".to_owned(), "calc.desktop".to_owned()];
    let mut snapshot = SettingsSnapshot::new(7);
    snapshot.values.insert(
        flamewm_platform::settings::START_FAVORITES_KEY.to_owned(),
        SettingValue::Text(flamewm_shell_core::start_menu::encode_favorites(&ids)),
    );
    let read = crate::settings_projection::start_favorites(Some(&snapshot));
    assert_eq!(read, ids);
    assert!(crate::settings_projection::start_favorites(None).is_empty());
    assert!(
        crate::settings_projection::start_favorites(Some(&SettingsSnapshot::new(1))).is_empty()
    );
    let mut model = StartModel::new(vec![named("calc.desktop", "Calculator")]);
    model.set_favorites(read);
    assert_eq!(model.favorites(), ["calc.desktop"]);
}

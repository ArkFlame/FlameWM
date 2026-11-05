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

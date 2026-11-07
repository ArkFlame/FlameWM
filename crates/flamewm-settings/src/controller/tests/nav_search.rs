use super::*;
use crate::nav::NAV_PAGES;

#[test]
fn search_without_a_match_shows_the_empty_state_and_keeps_the_sidebar() {
    let (_requests, transport) = recording_transport();
    let mut application = SettingsApplication::new(transport);
    let mut document = ProbeDocument::default();
    application.nav_query = "zzz".to_owned();
    application
        .sync(&mut document)
        .expect("sync no-match search");
    assert!(
        document.visible["nav-empty"],
        "empty state shows for no match"
    );
    assert!(!document.visible["nav-selection"]);
    for (slug, _) in NAV_PAGES {
        assert!(
            !document.visible[&format!("nav-{slug}")],
            "{slug} row hidden"
        );
    }
    assert_eq!(document.texts["nav-search-text"], "zzz");
}

#[test]
fn search_with_a_match_and_idle_search_hide_the_empty_state() {
    let (_requests, transport) = recording_transport();
    let mut application = SettingsApplication::new(transport);
    let mut document = ProbeDocument::default();
    application.nav_query = "disp".to_owned();
    application
        .sync(&mut document)
        .expect("sync matching search");
    assert!(
        !document.visible["nav-empty"],
        "a matching page hides the empty state"
    );
    assert!(document.visible["nav-displays"]);
    application.nav_query.clear();
    application.sync(&mut document).expect("sync idle search");
    assert!(
        !document.visible["nav-empty"],
        "an idle search hides the empty state"
    );
    assert!(document.visible["nav-theme"]);
}

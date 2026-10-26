use super::*;

fn tab_with(names: &[&str]) -> Tab {
    let mut tab = Tab::new(PathBuf::from("/tmp"));
    tab.set_entries(
        names
            .iter()
            .map(|n| Entry::new(PathBuf::from(n), (*n).to_string(), false, false, 0, None))
            .collect(),
    );
    tab
}

#[test]
fn cached_count_follows_query_and_listing_changes() {
    let mut tab = tab_with(&["Alpha.txt", "beta.rs", "ALPHABET"]);
    assert_eq!(tab.filtered_count(), 3);
    tab.search = "alpha".into();
    assert_eq!(tab.filtered_count(), 2);
    assert_eq!(
        tab.filtered_count(),
        2,
        "second call is served from the cache"
    );
    tab.set_entries(vec![Entry::new(
        PathBuf::from("x/alpha"),
        "alpha".into(),
        false,
        false,
        0,
        None,
    )]);
    assert_eq!(
        tab.filtered_count(),
        1,
        "a listing replacement invalidates the cache"
    );
    tab.extend_entries(vec![Entry::new(
        PathBuf::from("x/alphamo"),
        "alphamo".into(),
        false,
        false,
        0,
        None,
    )]);
    assert_eq!(tab.filtered_count(), 2, "an append invalidates the cache");
    tab.clear_entries();
    assert_eq!(tab.filtered_count(), 0);
}

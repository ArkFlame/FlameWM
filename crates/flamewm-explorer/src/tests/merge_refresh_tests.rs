use super::*;

fn entry(name: &str) -> Entry {
    Entry::new(PathBuf::from(name), name.into(), false, false, 0, None)
}

fn names(rows: &[Entry]) -> Vec<&str> {
    rows.iter().map(|row| row.name.as_str()).collect()
}

fn merged(old: &[&str], new: &[&str]) -> (Vec<String>, bool) {
    let mut rows: Vec<Entry> = old.iter().map(|name| entry(name)).collect();
    let items: Vec<Entry> = new.iter().map(|name| entry(name)).collect();
    let changed = merge_refresh(&mut rows, items, view::ColumnSort::default());
    (
        names(&rows).into_iter().map(String::from).collect(),
        changed,
    )
}

#[test]
fn identical_listing_is_unchanged() {
    assert_eq!(
        merged(&["a", "b", "c"], &["a", "b", "c"]),
        (vec!["a".into(), "b".into(), "c".into()], false)
    );
}

#[test]
fn added_removed_and_trailing_rows_follow_the_new_listing() {
    assert_eq!(
        merged(&["a", "c"], &["a", "b", "c"]),
        (vec!["a".into(), "b".into(), "c".into()], true)
    );
    assert_eq!(
        merged(&["a", "b", "c"], &["a", "c"]),
        (vec!["a".into(), "c".into()], true)
    );
    // Rows left over on either side after the other ran out.
    assert_eq!(
        merged(&["a"], &["a", "b", "c"]),
        (vec!["a".into(), "b".into(), "c".into()], true)
    );
    assert_eq!(merged(&["a", "b", "c"], &["a"]), (vec!["a".into()], true));
    assert_eq!(merged(&[], &["a"]), (vec!["a".into()], true));
    assert_eq!(merged(&["a"], &[]), (Vec::<String>::new(), true));
    assert_eq!(merged(&[], &[]), (Vec::<String>::new(), false));
}

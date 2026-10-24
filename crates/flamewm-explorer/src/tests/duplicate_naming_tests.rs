use super::*;

#[test]
fn duplicate_names_are_fresh_copies() {
    let dir = std::env::temp_dir().join(format!("flamewm-dup-names-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let src = dir.join("note.txt");
    fs::write(&src, "x").unwrap();
    assert_eq!(duplicate_destination(&src, &dir), dir.join("note (copy).txt"));
    fs::write(dir.join("note (copy).txt"), "y").unwrap();
    assert_eq!(duplicate_destination(&src, &dir), dir.join("note (copy 2).txt"));
    let _ = fs::remove_dir_all(&dir);
}

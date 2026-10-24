use super::*;

#[test]
fn typing_replaces_a_selection_and_keeps_the_rest() {
    let mut field = TextField::default();
    field.set_selected("report.pdf".into(), rename_selection("report.pdf", false));
    field.insert("final");
    assert_eq!(field.text, "final.pdf");
    assert_eq!(field.caret, 5);
    assert!(!field.has_selection());
}

#[test]
fn select_all_then_type_replaces_everything() {
    let mut field = TextField::default();
    field.set("/home/u/docs".into());
    field.select_all();
    assert!(field.has_selection());
    field.insert("/tmp");
    assert_eq!(field.text, "/tmp");
    assert_eq!(field.display(), "/tmp|");
}

#[test]
fn backspace_and_delete_work_on_chars_not_bytes() {
    let mut field = TextField::default();
    field.set("aé日b".into());
    field.move_caret(2);
    field.backspace();
    assert_eq!(field.text, "a日b");
    assert_eq!(field.caret, 1);
    field.delete();
    assert_eq!(field.text, "ab");
    field.backspace();
    field.backspace();
    assert_eq!(field.text, "b");
    assert_eq!(field.caret, 0);
}

#[test]
fn caret_moves_clamp_and_display_marks_position() {
    let mut field = TextField::default();
    field.set("abc".into());
    field.move_caret(1);
    field.insert("X");
    assert_eq!(field.display(), "aX|bc");
    field.move_caret(usize::MAX);
    assert_eq!(field.caret, 4);
}

#[test]
fn rename_selection_picks_stem_for_files_only() {
    assert_eq!(rename_selection("archive.tar.gz", false), (0, 11));
    assert_eq!(rename_selection("notes", false), (0, 5));
    assert_eq!(rename_selection(".bashrc", false), (0, 7));
    assert_eq!(rename_selection("photos.2024", true), (0, 11));
    assert_eq!(rename_selection("é.txt", false), (0, 1));
}

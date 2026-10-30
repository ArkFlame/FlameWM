use super::*;
use crate::parser::Parser;

fn feed(screen: &mut Screen, bytes: &[u8]) {
    let mut parser = Parser::new();
    parser.advance(bytes, screen);
}

fn text(screen: &Screen, row: usize) -> String {
    screen
        .line(row)
        .iter()
        .filter(|c| c.attrs & attr::SPACER == 0)
        .map(|c| c.ch)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

#[test]
fn cursor_movement_is_clamped_and_absolute() {
    let mut screen = Screen::new(10, 5, 100);
    feed(&mut screen, b"\x1b[3;4H");
    assert_eq!(screen.cursor(), (2, 3));
    feed(&mut screen, b"\x1b[10A");
    assert_eq!(screen.cursor(), (0, 3));
    feed(&mut screen, b"\x1b[99C");
    assert_eq!(screen.cursor(), (0, 9));
    feed(&mut screen, b"\x1b[2;2H\x1b[1B\x1b[2D");
    assert_eq!(screen.cursor(), (2, 0));
    feed(&mut screen, b"\x1b[0;0H");
    assert_eq!(screen.cursor(), (0, 0));
}

#[test]
fn text_wraps_at_the_right_margin_and_scrolls_into_scrollback() {
    let mut screen = Screen::new(4, 2, 100);
    feed(&mut screen, b"abcdefghijk");
    assert_eq!(text(&screen, 0), "efgh");
    assert_eq!(text(&screen, 1), "ijk");
    assert_eq!(screen.scrollback_len(), 1);
    assert_eq!(screen.line_abs(0).map(|l| l[0].ch), Some('a'));
}

#[test]
fn decawm_off_overwrites_the_last_column() {
    let mut screen = Screen::new(3, 2, 10);
    feed(&mut screen, b"\x1b[?7labcdef");
    assert_eq!(text(&screen, 0), "abf");
    assert_eq!(screen.cursor().0, 0);
}

#[test]
fn sgr_sets_attributes_and_colours_in_all_forms() {
    let mut screen = Screen::new(40, 2, 10);
    feed(
        &mut screen,
        b"\x1b[1;3;4;7mA\x1b[0m\x1b[31;42mB\x1b[38;5;200mC\x1b[38;2;1;2;3;48:2::9:8:7mD\x1b[91mE",
    );
    let row = screen.line(0);
    assert_eq!(
        row[0].attrs,
        attr::BOLD | attr::ITALIC | attr::UNDERLINE | attr::INVERSE
    );
    assert_eq!(
        (row[1].fg, row[1].bg, row[1].attrs),
        (Color::Indexed(1), Color::Indexed(2), 0)
    );
    assert_eq!(row[2].fg, Color::Indexed(200));
    assert_eq!(
        (row[3].fg, row[3].bg),
        (Color::Rgb(1, 2, 3), Color::Rgb(9, 8, 7))
    );
    assert_eq!(row[4].fg, Color::Indexed(9));
}

#[test]
fn scroll_region_limits_linefeed_scrolling_and_keeps_outside_rows() {
    let mut screen = Screen::new(5, 5, 100);
    feed(&mut screen, b"1\r\n2\r\n3\r\n4\r\n5");
    feed(&mut screen, b"\x1b[2;4r\x1b[4;1H\n");
    assert_eq!(text(&screen, 0), "1");
    assert_eq!(text(&screen, 1), "3");
    assert_eq!(text(&screen, 2), "4");
    assert_eq!(text(&screen, 3), "");
    assert_eq!(text(&screen, 4), "5");
    assert_eq!(
        screen.scrollback_len(),
        0,
        "a partial region must not feed scrollback"
    );
}

#[test]
fn reverse_index_at_top_of_region_scrolls_down() {
    let mut screen = Screen::new(3, 3, 10);
    feed(&mut screen, b"a\r\nb\r\nc\x1b[1;1H\x1bM");
    assert_eq!(text(&screen, 0), "");
    assert_eq!(text(&screen, 1), "a");
    assert_eq!(text(&screen, 2), "b");
}

#[test]
fn alt_screen_1049_saves_cursor_and_restores_primary_contents() {
    let mut screen = Screen::new(6, 3, 10);
    feed(&mut screen, b"main\x1b[2;3H");
    feed(&mut screen, b"\x1b[?1049h");
    assert!(screen.alt_active());
    assert_eq!(text(&screen, 0), "", "alt screen starts blank");
    feed(&mut screen, b"alt");
    feed(&mut screen, b"\x1b[?1049l");
    assert!(!screen.alt_active());
    assert_eq!(text(&screen, 0), "main");
    assert_eq!(screen.cursor(), (1, 2));
}

#[test]
fn erase_in_line_and_display_use_the_current_background() {
    let mut screen = Screen::new(4, 2, 10);
    feed(&mut screen, b"abcd\x1b[42m\x1b[1;2H\x1b[K");
    assert_eq!(text(&screen, 0), "a");
    assert_eq!(screen.line(0)[3].bg, Color::Indexed(2));
    feed(&mut screen, b"\x1b[2J");
    assert_eq!(text(&screen, 0), "");
}

#[test]
fn insert_and_delete_characters_shift_the_line() {
    let mut screen = Screen::new(5, 1, 0);
    feed(&mut screen, b"abcde\x1b[1;2H\x1b[P");
    assert_eq!(text(&screen, 0), "acde");
    feed(&mut screen, b"\x1b[@");
    assert_eq!(text(&screen, 0), "a cde");
}

#[test]
fn device_status_and_attributes_produce_replies() {
    let mut screen = Screen::new(10, 5, 0);
    feed(&mut screen, b"\x1b[3;7H\x1b[6n\x1b[c\x1b[5n");
    assert_eq!(
        String::from_utf8(screen.take_replies()).unwrap(),
        "\x1b[3;7R\x1b[?62;22c\x1b[0n"
    );
}

#[test]
fn modes_and_title_are_tracked() {
    let mut screen = Screen::new(10, 5, 0);
    feed(
        &mut screen,
        b"\x1b[?1h\x1b[?2004h\x1b[?25l\x1b]2;vim\x07\x1b[6 q",
    );
    assert!(screen.app_cursor);
    assert!(screen.bracketed_paste);
    assert!(!screen.cursor_visible);
    assert_eq!(screen.title.as_deref(), Some("vim"));
    assert_eq!(screen.cursor_shape, CursorShape::Bar);
    assert!(!screen.cursor_blink);
}

#[test]
fn line_drawing_charset_maps_box_characters() {
    let mut screen = Screen::new(4, 1, 0);
    feed(&mut screen, b"\x1b(0lqk\x1b(Bx");
    assert_eq!(text(&screen, 0), "┌─┐x");
}

#[test]
fn wide_characters_occupy_two_cells() {
    let mut screen = Screen::new(4, 1, 0);
    feed(&mut screen, "a界b".as_bytes());
    assert_eq!(screen.line(0)[1].attrs & attr::WIDE, attr::WIDE);
    assert_eq!(screen.line(0)[2].attrs & attr::SPACER, attr::SPACER);
    assert_eq!(screen.cursor(), (0, 4 - 1));
    assert_eq!(text(&screen, 0), "a界b");
}

#[test]
fn resize_moves_rows_into_scrollback_and_keeps_cursor_visible() {
    let mut screen = Screen::new(4, 4, 100);
    feed(&mut screen, b"1\r\n2\r\n3\r\n4");
    screen.resize(4, 2);
    assert_eq!(screen.rows(), 2);
    assert_eq!(text(&screen, 0), "3");
    assert_eq!(text(&screen, 1), "4");
    assert_eq!(screen.scrollback_len(), 2);
    screen.resize(6, 4);
    assert_eq!(screen.cols(), 6);
    assert_eq!(screen.rows(), 4);
    assert_eq!(text(&screen, 0), "1");
}

#[test]
fn scrollback_is_capped() {
    let mut screen = Screen::new(2, 1, 3);
    feed(&mut screen, b"a\r\nb\r\nc\r\nd\r\ne");
    assert_eq!(screen.scrollback_len(), 3);
}

#[test]
fn rewriting_identical_cells_leaves_rows_clean() {
    let mut screen = Screen::new(10, 3, 0);
    feed(&mut screen, b"\x1b[31mab\x1b[0m");
    assert_eq!(screen.take_dirty(), Dirty::All);
    feed(&mut screen, b"\x1b[H\x1b[31mab\x1b[0m\x1b[K");
    assert_eq!(
        screen.take_dirty(),
        Dirty::Rows(vec![]),
        "identical text and erase of blank cells mark nothing"
    );
    feed(&mut screen, b"\x1b[1;2Hz");
    assert_eq!(
        screen.take_dirty(),
        Dirty::Rows(vec![0]),
        "a changed cell marks its row"
    );
}

#[test]
fn rep_repeats_the_last_printed_character() {
    let mut screen = Screen::new(10, 2, 10);
    feed(&mut screen, b"x\x1b[3b|");
    assert_eq!(text(&screen, 0), "xxxx|");
}

#[test]
fn block_art_row_stays_within_the_run_pool() {
    // One run per colour change: a row of alternating truecolour block cells needs one run per cell.
    let mut screen = Screen::new(40, 1, 10);
    let mut bytes = Vec::new();
    for i in 0..40 {
        bytes.extend_from_slice(
            format!("\x1b[38;2;{};0;0m█", if i % 2 == 0 { 200 } else { 100 }).as_bytes(),
        );
    }
    feed(&mut screen, &bytes);
    let palette = crate::themes::builtin_themes()[0].palette.clone();
    let options = crate::view::GridOptions {
        view_offset: 0,
        selection: None,
        cursor: false,
        cursor_shape: CursorShape::Block,
    };
    let runs = &crate::view::grid_runs(&screen, &palette, &options)[0];
    assert_eq!(runs.len(), 40);
    assert!(
        runs.len() <= crate::pool::TIERS[2].runs,
        "a row past the pool would lose its block shapes"
    );
}

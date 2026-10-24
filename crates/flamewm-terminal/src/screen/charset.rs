//! Character width (East Asian wide, combining marks) and the DEC Special Graphics line-drawing map.

/// Display width of one scalar: 0 for combining marks, 2 for East Asian wide and emoji ranges, else 1.
pub fn char_width(c: char) -> usize {
    let u = c as u32;
    if matches!(u, 0x0300..=0x036f | 0x200b..=0x200f | 0xfe00..=0xfe0f | 0x20d0..=0x20ff) {
        return 0;
    }
    let wide = matches!(u,
        0x1100..=0x115f | 0x2e80..=0x303e | 0x3041..=0x33ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff
        | 0xa000..=0xa4cf | 0xac00..=0xd7a3 | 0xf900..=0xfaff | 0xfe30..=0xfe4f | 0xff00..=0xff60
        | 0xffe0..=0xffe6 | 0x1f300..=0x1f64f | 0x1f900..=0x1f9ff | 0x20000..=0x3fffd);
    if wide { 2 } else { 1 }
}

/// DEC Special Graphics (ESC ( 0): the line-drawing characters used by curses programs.
pub(super) fn dec_graphics(c: char) -> char {
    match c {
        '`' => '◆',
        'a' => '▒',
        'f' => '°',
        'g' => '±',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        other => other,
    }
}

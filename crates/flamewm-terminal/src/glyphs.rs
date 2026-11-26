//! Box-drawing (U+2500 to U+257F) and block (U+2580 to U+259F) glyphs as filled rectangles instead of font glyphs.
//! Font glyphs for these do not reach the cell edges on every line metric, so stacked borders and bars show gaps;
//! rectangles meet exactly. Geometry is in cell fractions: x runs 0..=1 across the cell, y 0..=1 down it.

/// A filled rectangle inside a cell span. `x` is in cells relative to the run's first column, so a merged
/// full-width band spans several cells; `y` is a fraction of the row height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arm {
    pub x0: f32,
    pub x1: f32,
    pub y0: f32,
    pub y1: f32,
}

impl Arm {
    const fn new(x0: f32, x1: f32, y0: f32, y1: f32) -> Self {
        Self { x0, x1, y0, y1 }
    }
}

/// Stroke overlap past the cell centre, so arms meet at the junction without a notch.
const EPS: f32 = 1.0 / 16.0;

/// Arms (left, right, up, down) of each supported box glyph; weight 1 light, 2 heavy, 3 double, 0 none.
/// A direct match keeps the lookup O(1) for the border-heavy screens (`htop`, curses UIs) that redraw every tick.
fn box_weights(ch: char) -> Option<[u8; 4]> {
    Some(match ch {
        '─' => [1, 1, 0, 0],
        '━' => [2, 2, 0, 0],
        '═' => [3, 3, 0, 0],
        '│' => [0, 0, 1, 1],
        '┃' => [0, 0, 2, 2],
        '║' => [0, 0, 3, 3],
        '┌' | '╭' => [0, 1, 0, 1],
        '┏' => [0, 2, 0, 2],
        '╔' => [0, 3, 0, 3],
        '┐' | '╮' => [1, 0, 0, 1],
        '┓' => [2, 0, 0, 2],
        '╗' => [3, 0, 0, 3],
        '└' | '╰' => [0, 1, 1, 0],
        '┗' => [0, 2, 2, 0],
        '╚' => [0, 3, 3, 0],
        '┘' | '╯' => [1, 0, 1, 0],
        '┛' => [2, 0, 2, 0],
        '╝' => [3, 0, 3, 0],
        '├' => [0, 1, 1, 1],
        '┣' => [0, 2, 2, 2],
        '╠' => [0, 3, 3, 3],
        '┤' => [1, 0, 1, 1],
        '┫' => [2, 0, 2, 2],
        '╣' => [3, 0, 3, 3],
        '┬' => [1, 1, 0, 1],
        '┳' => [2, 2, 0, 2],
        '╦' => [3, 3, 0, 3],
        '┴' => [1, 1, 1, 0],
        '┻' => [2, 2, 2, 0],
        '╩' => [3, 3, 3, 0],
        '┼' => [1, 1, 1, 1],
        '╋' => [2, 2, 2, 2],
        '╬' => [3, 3, 3, 3],
        '╴' => [1, 0, 0, 0],
        '╵' => [0, 0, 1, 0],
        '╶' => [0, 1, 0, 0],
        '╷' => [0, 0, 0, 1],
        _ => return None,
    })
}

/// Most rectangles any supported glyph needs (a double junction).
const MAX_RECTS: usize = 4;

/// Up to four filled rectangles for one glyph, enough for a double junction. Fixed size on the stack, so a screen
/// of borders and bars costs no heap allocation per cell.
#[derive(Clone, Copy, Debug)]
pub struct Rects {
    arms: [(Arm, u32); MAX_RECTS],
    len: usize,
}

impl Rects {
    const EMPTY: (Arm, u32) = (Arm::new(0.0, 0.0, 0.0, 0.0), 0);

    const fn new() -> Self {
        Self {
            arms: [Self::EMPTY; MAX_RECTS],
            len: 0,
        }
    }

    fn push(&mut self, arm: Arm, colour: u32) {
        if let Some(slot) = self.arms.get_mut(self.len) {
            *slot = (arm, colour);
            self.len += 1;
        }
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[must_use]
    pub fn as_slice(&self) -> &[(Arm, u32)] {
        &self.arms[..self.len]
    }
}

/// Thickness bands (y) of a horizontal stroke by weight. Light is one pixel at a 16 pixel cell.
fn h_strokes(weight: u8) -> &'static [(f32, f32)] {
    match weight {
        1 => &[(7.0 / 16.0, 8.0 / 16.0)],
        2 => &[(7.0 / 16.0, 9.0 / 16.0)],
        _ => &[(4.0 / 16.0, 5.0 / 16.0), (11.0 / 16.0, 12.0 / 16.0)],
    }
}

/// Thickness bands (x) of a vertical stroke by weight. Light is one pixel at an 8 pixel cell.
fn v_strokes(weight: u8) -> &'static [(f32, f32)] {
    match weight {
        1 => &[(3.0 / 8.0, 4.0 / 8.0)],
        2 => &[(3.0 / 8.0, 5.0 / 8.0)],
        _ => &[(1.0 / 8.0, 2.0 / 8.0), (6.0 / 8.0, 7.0 / 8.0)],
    }
}

fn box_arms([left, right, up, down]: [u8; 4], colour: u32) -> Rects {
    let mut rects = Rects::new();
    // A pair of arms of equal weight is one full-length stroke, which lets a run of them merge into one rectangle.
    if left > 0 && right > 0 {
        for &(y0, y1) in h_strokes(left) {
            rects.push(Arm::new(0.0, 1.0, y0, y1), colour);
        }
    } else if left + right > 0 {
        let (weight, x0, x1) = if left > 0 {
            (left, 0.0, 0.5 + EPS)
        } else {
            (right, 0.5 - EPS, 1.0)
        };
        for &(y0, y1) in h_strokes(weight) {
            rects.push(Arm::new(x0, x1, y0, y1), colour);
        }
    }
    if up > 0 && down > 0 {
        for &(x0, x1) in v_strokes(up) {
            rects.push(Arm::new(x0, x1, 0.0, 1.0), colour);
        }
    } else if up + down > 0 {
        let (weight, y0, y1) = if up > 0 {
            (up, 0.0, 0.5 + EPS)
        } else {
            (down, 0.5 - EPS, 1.0)
        };
        for &(x0, x1) in v_strokes(weight) {
            rects.push(Arm::new(x0, x1, y0, y1), colour);
        }
    }
    rects
}

/// `from` blended toward `to` by `num / den` per channel.
#[must_use]
pub fn blend(from: u32, to: u32, num: u32, den: u32) -> u32 {
    let channel = |shift: u32| {
        let (a, b) = ((from >> shift) & 0xff, (to >> shift) & 0xff);
        (a * (den - num) + b * num) / den
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// Rectangles that draw `ch`, each with its colour, or `None` when the glyph is drawn as text. `fg` and `bg` are the
/// cell's resolved colours, so shades blend the two. The rectangles live in the returned value, not on the heap.
#[must_use]
pub fn glyph_rects(ch: char, fg: u32, bg: u32) -> Option<Rects> {
    // Ordinary text never reaches the table: one range test instead of the whole match per cell.
    if !('\u{2500}'..='\u{259f}').contains(&ch) {
        return None;
    }
    if let Some(weights) = box_weights(ch) {
        return Some(box_arms(weights, fg));
    }
    let full = Arm::new(0.0, 1.0, 0.0, 1.0);
    let code = ch as u32;
    let mut rects = Rects::new();
    let arm = match code {
        0x2580 => Arm::new(0.0, 1.0, 0.0, 0.5),
        // Lower n/8 blocks: 2581 is 1/8, 2587 is 7/8.
        0x2581..=0x2587 => Arm::new(0.0, 1.0, 1.0 - (code - 0x2580) as f32 / 8.0, 1.0),
        0x2588 => full,
        // Left blocks: 2589 is 7/8 wide, 258F is 1/8.
        0x2589..=0x258f => Arm::new(0.0, (0x2590 - code) as f32 / 8.0, 0.0, 1.0),
        0x2590 => Arm::new(0.5, 1.0, 0.0, 1.0),
        // Shades: 2591 is 1/4, 2592 is 1/2, 2593 is 3/4 of the foreground over the background.
        0x2591..=0x2593 => {
            rects.push(full, blend(bg, fg, code - 0x2590, 4));
            return Some(rects);
        }
        0x2594 => Arm::new(0.0, 1.0, 0.0, 1.0 / 8.0),
        0x2595 => Arm::new(7.0 / 8.0, 1.0, 0.0, 1.0),
        // Quadrants 2596 to 259F as bit sets: upper-left 1, upper-right 2, lower-left 4, lower-right 8.
        0x2596..=0x259f => {
            const QUADRANTS: [u8; 10] = [4, 8, 1, 13, 9, 7, 11, 2, 6, 14];
            let bits = QUADRANTS[(code - 0x2596) as usize];
            let quarters = [
                (1, Arm::new(0.0, 0.5, 0.0, 0.5)),
                (2, Arm::new(0.5, 1.0, 0.0, 0.5)),
                (4, Arm::new(0.0, 0.5, 0.5, 1.0)),
                (8, Arm::new(0.5, 1.0, 0.5, 1.0)),
            ];
            for (bit, arm) in quarters {
                if bits & bit != 0 {
                    rects.push(arm, fg);
                }
            }
            return Some(rects);
        }
        _ => return None,
    };
    rects.push(arm, fg);
    Some(rects)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn horizontal_and_vertical_light_lines_are_single_full_bands() {
        let line = glyph_rects('─', 0x00ff_ffff, 0).expect("box glyph");
        assert_eq!(
            line.as_slice(),
            &[(Arm::new(0.0, 1.0, 7.0 / 16.0, 8.0 / 16.0), 0x00ff_ffff)]
        );
        let bar = glyph_rects('│', 1, 0).expect("box glyph");
        assert_eq!(bar.len(), 1);
        let (arm, _) = bar.as_slice()[0];
        assert_eq!((arm.y0, arm.y1), (0.0, 1.0));
    }

    #[test]
    fn cross_is_two_full_bands_and_corner_has_two_arms() {
        assert_eq!(glyph_rects('┼', 1, 0).expect("cross").len(), 2);
        assert_eq!(glyph_rects('┌', 1, 0).expect("corner").len(), 2);
    }

    #[test]
    fn double_lines_are_two_strokes_per_direction() {
        assert_eq!(glyph_rects('═', 1, 0).expect("double").len(), 2);
        assert_eq!(glyph_rects('║', 1, 0).expect("double").len(), 2);
    }

    #[test]
    fn block_elements_cover_their_fraction_of_the_cell() {
        let lower_half = glyph_rects('\u{2584}', 1, 0).expect("block");
        assert_eq!(lower_half.as_slice()[0].0, Arm::new(0.0, 1.0, 0.5, 1.0));
        let left_eighth = glyph_rects('\u{258f}', 1, 0).expect("block");
        assert_eq!(left_eighth.as_slice()[0].0, Arm::new(0.0, 0.125, 0.0, 1.0));
        let upper_left = glyph_rects('\u{2598}', 1, 0).expect("quadrant");
        assert_eq!(upper_left.as_slice()[0].0, Arm::new(0.0, 0.5, 0.0, 0.5));
        assert_eq!(glyph_rects('\u{259f}', 1, 0).expect("quadrant").len(), 3);
    }

    #[test]
    fn shades_blend_foreground_into_background() {
        let half = glyph_rects('\u{2592}', 0x00ff_ffff, 0x0000_0000).expect("shade");
        assert_eq!(half.as_slice()[0].1, 0x007f_7f7f);
        let quarter = glyph_rects('\u{2591}', 0x00ff_ffff, 0x0000_0000).expect("shade");
        assert_eq!(quarter.as_slice()[0].1, 0x003f_3f3f);
    }

    #[test]
    fn other_characters_stay_text() {
        assert!(glyph_rects('a', 1, 0).is_none());
        assert!(glyph_rects('中', 1, 0).is_none());
        assert!(
            glyph_rects('┄', 1, 0).is_none(),
            "dashed lines are not in the table"
        );
    }
}

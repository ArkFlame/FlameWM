//! Taskbar popover geometry and dismissal. `flamewm-assistant --popover X Y W H` opens the
//! assistant as an override-redirect window above (or below, for a top bar) the taskbar button
//! whose device rect is `X Y W H`. Outside presses and Escape dismiss it; a press on the button
//! itself is left to the shell, whose click reaches the toggle in `instance::claim`.

use flamewm_ui_x11::RawPress;

/// Error text that ends the popover loop; `run_popover` turns it into a clean exit.
pub const DISMISSED: &str = "assistant popover dismissed";

/// Button rect in root device pixels: `[x, y, width, height]`.
pub type Anchor = [i32; 4];

/// Gap between the button and the popover edge, in pixels.
pub const GAP: i32 = 8;

/// Parses the four anchor numbers that follow `--popover`.
#[must_use]
pub fn parse_args(args: &[String]) -> Option<Anchor> {
    match args {
        [x, y, w, h] => {
            let anchor = [x, y, w, h].map(|value| value.parse::<i32>().ok());
            let [Some(x), Some(y), Some(w), Some(h)] = anchor else {
                return None;
            };
            (w > 0 && h > 0).then_some([x, y, w, h])
        }
        _ => None,
    }
}

/// Tallest popover that fits beside the button: above it for a lower-half button, else below.
#[must_use]
pub fn max_height(root: (u32, u32), anchor: Anchor) -> i32 {
    let [_, ay, _, ah] = anchor;
    let root_h = root.1 as i32;
    let room = if ay + ah / 2 > root_h / 2 { ay - GAP } else { root_h - (ay + ah) - GAP };
    room.max(240)
}

/// Top-left of a `size` popover centred on the button and kept on the root. It sits above the
/// button when the button is in the lower half of the root, otherwise below it.
#[must_use]
pub fn origin(root: (u32, u32), size: (i32, i32), anchor: Anchor) -> (i32, i32) {
    let [ax, ay, aw, ah] = anchor;
    let (root_w, root_h) = (root.0 as i32, root.1 as i32);
    let (w, h) = size;
    let x = (ax + aw / 2 - w / 2).clamp(0, (root_w - w).max(0));
    let below_bottom = ay + ah + GAP;
    let above_top = ay - GAP - h;
    let y = if ay + ah / 2 > root_h / 2 { above_top } else { below_bottom };
    (x, y.clamp(0, (root_h - h).max(0)))
}

/// True when a root press lies outside every rect (`[x, y, w, h]`, root device pixels). Presses
/// are reported in logical pixels, which equal device pixels at the default scale.
#[must_use]
pub fn is_outside(press: &RawPress, rects: &[Anchor]) -> bool {
    let (px, py) = (press.x, press.y);
    !rects.iter().any(|&[x, y, w, h]| {
        px >= x as f32 && py >= y as f32 && px < (x + w) as f32 && py < (y + h) as f32
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(x: f32, y: f32) -> RawPress {
        RawPress { x, y, button: 1, time_ms: 0 }
    }

    #[test]
    fn parse_requires_four_positive_numbers() {
        let args = |values: &[&str]| values.iter().map(|v| (*v).to_owned()).collect::<Vec<_>>();
        assert_eq!(parse_args(&args(&["10", "20", "30", "40"])), Some([10, 20, 30, 40]));
        assert_eq!(parse_args(&args(&["10", "20", "30"])), None);
        assert_eq!(parse_args(&args(&["10", "20", "0", "40"])), None);
        assert_eq!(parse_args(&args(&["x", "20", "30", "40"])), None);
    }

    #[test]
    fn bottom_bar_popover_sits_above_the_button_and_stays_on_root() {
        // 1280x800 root, button at the right end of a 40 px bottom bar.
        let anchor = [1200, 760, 30, 40];
        let (x, y) = origin((1280, 800), (460, 600), anchor);
        assert_eq!(y, 760 - GAP - 600);
        assert!(x + 460 <= 1280 && x >= 0);
        // Near the left edge the popover is pulled back onto the root.
        assert_eq!(origin((1280, 800), (460, 600), [0, 760, 30, 40]).0, 0);
    }

    #[test]
    fn top_bar_popover_sits_below_the_button() {
        assert_eq!(origin((1280, 800), (460, 600), [600, 0, 30, 40]).1, 40 + GAP);
    }

    #[test]
    fn outside_press_excludes_the_popover_and_the_button() {
        let rects = [[100, 100, 460, 780], [900, 760, 30, 40]];
        assert!(!is_outside(&press(120.0, 120.0), &rects));
        assert!(!is_outside(&press(910.0, 770.0), &rects));
        assert!(is_outside(&press(50.0, 50.0), &rects));
        assert!(is_outside(&press(930.0, 800.0), &rects));
    }
}

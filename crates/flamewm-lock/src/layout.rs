//! Lock screen geometry on a 701x438 reference canvas, mapped onto any output.

/// Reference canvas width.
pub const REF_W: f64 = 701.0;
/// Reference canvas height.
pub const REF_H: f64 = 438.0;
/// Largest scale applied, so 4K outputs do not blow up the layout.
pub const MAX_SCALE: f64 = 2.5;
/// Smallest font size in output pixels.
pub const MIN_FONT_PX: u32 = 8;

/// Rectangle in reference-canvas units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl RefRect {
    const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// Rectangle of size `w`x`h` whose centre is at (`cx`, `cy`).
    const fn centred(cx: f64, cy: f64, w: f64, h: f64) -> Self {
        Self::new(cx - w / 2.0, cy - h / 2.0, w, h)
    }

    fn bottom(self) -> f64 {
        self.y + self.h
    }
}

/// Uniform scale and centring offset that maps the reference canvas onto an output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub s: f64,
    pub ox: f64,
    pub oy: f64,
}

impl Fit {
    pub fn map_point(&self, x: f64, y: f64) -> (f64, f64) {
        (self.ox + x * self.s, self.oy + y * self.s)
    }

    pub fn map_rect(&self, rect: RefRect) -> RefRect {
        let (x, y) = self.map_point(rect.x, rect.y);
        RefRect::new(x, y, rect.w * self.s, rect.h * self.s)
    }
}

/// Scales the reference canvas to fit `out_w` x `out_h`, centred, clamped to [`MAX_SCALE`].
pub fn fit(out_w: u32, out_h: u32) -> Fit {
    let (w, h) = (f64::from(out_w), f64::from(out_h));
    let s = (w / REF_W).min(h / REF_H).min(MAX_SCALE);
    Fit {
        s,
        ox: (w - REF_W * s) / 2.0,
        oy: (h - REF_H * s) / 2.0,
    }
}

/// A placed element in output pixels. `font_px` is zero for non-text elements.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub rect: RefRect,
    pub font_px: u32,
}

const CLOCK_FONT: f64 = 31.0;
const DATE_FONT: f64 = 15.0;
const USERNAME_FONT: f64 = 12.0;
const LABEL_FONT: f64 = 9.0;
const FOOTER_FONT: f64 = 8.0;

/// Reference-canvas geometry of every lock screen element.
pub const CLOCK: RefRect = RefRect::centred(350.0, 61.0, 120.0, 30.0);
pub const DATE: RefRect = RefRect::new(270.0, 89.0, 160.0, 12.0);
pub const AVATAR: RefRect = RefRect::centred(350.0, 179.0, 68.0, 68.0);
pub const USERNAME: RefRect = RefRect::centred(350.0, 233.0, 160.0, 12.0);
pub const PASSWORD: RefRect = RefRect::new(271.0, 253.0, 138.0, 17.0);
pub const SUBMIT: RefRect = RefRect::new(413.0, 253.0, 17.0, 17.0);
pub const POWER_CENTRES_X: [f64; 4] = [245.0, 315.0, 386.0, 455.0];
pub const POWER_CENTRE_Y: f64 = 348.0;
pub const POWER_ICON: f64 = 23.0;
pub const POWER_LABEL_BASELINE: f64 = 375.0;
pub const POWER_LABEL_W: f64 = 80.0;
pub const FOOTER_BASELINE: f64 = 432.0;
pub const FOOTER: RefRect = RefRect::new(4.0, FOOTER_BASELINE - FOOTER_FONT, 300.0, 10.0);

/// Lock screen elements in output pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct LockLayout {
    pub fit: Fit,
    pub clock: Placed,
    pub date: Placed,
    pub avatar: Placed,
    pub username: Placed,
    pub password: Placed,
    pub submit: Placed,
    pub power_icons: [Placed; 4],
    pub power_labels: [Placed; 4],
    pub footer: Placed,
}

impl LockLayout {
    pub fn for_output(out_w: u32, out_h: u32) -> Self {
        let fit = fit(out_w, out_h);
        let place = |rect: RefRect, font: f64| Placed {
            rect: fit.map_rect(rect),
            font_px: scaled_font(font, fit.s),
        };
        let icon = |cx: f64| RefRect::centred(cx, POWER_CENTRE_Y, POWER_ICON, POWER_ICON);
        let label = |cx: f64| {
            RefRect::new(
                cx - POWER_LABEL_W / 2.0,
                POWER_LABEL_BASELINE - LABEL_FONT,
                POWER_LABEL_W,
                LABEL_FONT,
            )
        };
        Self {
            fit,
            clock: place(CLOCK, CLOCK_FONT),
            date: place(DATE, DATE_FONT),
            avatar: place(AVATAR, 0.0),
            username: place(USERNAME, USERNAME_FONT),
            password: place(PASSWORD, 0.0),
            submit: place(SUBMIT, 0.0),
            power_icons: POWER_CENTRES_X.map(|cx| place(icon(cx), 0.0)),
            power_labels: POWER_CENTRES_X.map(|cx| place(label(cx), LABEL_FONT)),
            footer: place(FOOTER, FOOTER_FONT),
        }
    }
}

fn scaled_font(reference_px: f64, s: f64) -> u32 {
    if reference_px == 0.0 {
        return 0;
    }
    let px = (reference_px * s).round();
    px.max(f64::from(MIN_FONT_PX)) as u32
}

/// Vertical gaps (reference px) between stacked elements, for layout checks.
pub fn gap_time_to_date() -> f64 {
    DATE.y - CLOCK.bottom()
}

pub fn gap_date_to_avatar() -> f64 {
    AVATAR.y - DATE.bottom()
}

pub fn gap_avatar_to_username() -> f64 {
    USERNAME.y - AVATAR.bottom()
}

pub fn gap_username_to_password() -> f64 {
    PASSWORD.y - USERNAME.bottom()
}

pub fn gap_password_to_power_row() -> f64 {
    POWER_CENTRE_Y - POWER_ICON / 2.0 - PASSWORD.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside(p: &Placed, w: u32, h: u32) -> bool {
        let r = p.rect;
        let eps = 1e-6;
        r.x >= -eps
            && r.y >= -eps
            && r.x + r.w <= f64::from(w) + eps
            && r.bottom() <= f64::from(h) + eps
    }

    fn all(l: &LockLayout) -> Vec<Placed> {
        let mut v = vec![
            l.clock,
            l.date,
            l.avatar,
            l.username,
            l.password,
            l.submit,
            l.footer,
        ];
        v.extend(l.power_icons);
        v.extend(l.power_labels);
        v
    }

    #[test]
    fn identity_at_reference_size() {
        let f = fit(701, 438);
        assert_eq!(f.s, 1.0);
        assert_eq!((f.ox, f.oy), (0.0, 0.0));
        let l = LockLayout::for_output(701, 438);
        assert_eq!(l.clock.rect, CLOCK);
        assert_eq!(l.password.rect, PASSWORD);
        assert_eq!(l.clock.font_px, 31);
        assert_eq!(l.footer.font_px, 8);
    }

    #[test]
    fn scale_follows_limiting_axis() {
        let f = fit(1280, 720);
        assert!((f.s - 720.0 / 438.0).abs() < 1e-9);
        assert!((f.s - 1.644).abs() < 0.001);
        let f = fit(760, 500);
        assert!((f.s - 760.0 / 701.0).abs() < 1e-9);
        let f = fit(320, 240);
        assert!((f.s - 320.0 / 701.0).abs() < 1e-9);
    }

    #[test]
    fn scale_is_clamped_at_4k() {
        assert_eq!(fit(3840, 2160).s, 2.5);
        let f = fit(1920, 1080);
        assert!((f.s - 1080.0 / 438.0).abs() < 1e-9);
    }

    #[test]
    fn fonts_scale_and_never_drop_below_minimum() {
        let l = LockLayout::for_output(3840, 2160);
        assert_eq!(l.clock.font_px, 78); // 31 * 2.5 = 77.5 rounds up
        let tiny = LockLayout::for_output(320, 240);
        assert_eq!(tiny.footer.font_px, MIN_FONT_PX);
        assert_eq!(tiny.power_labels[0].font_px, MIN_FONT_PX);
        assert_eq!(tiny.avatar.font_px, 0);
    }

    #[test]
    fn every_element_is_inside_the_output() {
        for (w, h) in [(701, 438), (1280, 720), (1920, 1080), (3840, 2160), (760, 500), (320, 240)] {
            let l = LockLayout::for_output(w, h);
            for p in all(&l) {
                assert!(inside(&p, w, h), "{p:?} outside {w}x{h}");
            }
        }
    }

    #[test]
    fn anchors_are_centred_on_the_output() {
        for (w, h) in [(1280, 720), (1920, 1080), (760, 500), (320, 240)] {
            let l = LockLayout::for_output(w, h);
            let centre = f64::from(w) / 2.0;
            let tolerance = l.fit.s; // one reference pixel
            for p in [l.clock, l.date, l.avatar, l.username] {
                let cx = p.rect.x + p.rect.w / 2.0;
                assert!((cx - centre).abs() <= tolerance + 1e-6, "{cx} vs {centre} at {w}x{h}");
            }
        }
    }

    #[test]
    fn spacing_invariants_hold_in_reference_pixels() {
        assert!((gap_time_to_date() - 13.0).abs() <= 1.0);
        assert!((gap_date_to_avatar() - 43.0).abs() <= 2.0);
        assert!((gap_avatar_to_username() - 17.0).abs() <= 3.0);
        assert!((gap_username_to_password() - 15.0).abs() <= 2.0);
        assert!((gap_password_to_power_row() - 68.0).abs() <= 3.0);
    }

    #[test]
    fn power_row_is_evenly_spaced_and_labels_sit_below_icons() {
        let l = LockLayout::for_output(701, 438);
        let centres: Vec<f64> = l.power_icons.iter().map(|p| p.rect.x + p.rect.w / 2.0).collect();
        assert_eq!(centres, vec![245.0, 315.0, 386.0, 455.0]);
        for (icon, label) in l.power_icons.iter().zip(l.power_labels.iter()) {
            assert!(label.rect.y >= icon.rect.bottom());
        }
    }
}

use super::*;
use crate::model::WindowModel;
use flamewm_api::settings::{SettingValue, SettingsSnapshot};

fn decorated() -> Decorations {
    let mut decorations = Decorations::default();
    assert!(decorations.set_outline(0x00ff_ffff, 127, 1));
    decorations
}

/// The settings come from the shared surface owner the X11 frame already reads, so the
/// Wayland ring cannot drift from it: the committed defaults are neutral gray (#999999) at half opacity.
#[test]
fn outline_defaults_to_gray_at_half_opacity() {
    let stroke = flamewm_ui_core::surface::window_outline_stroke(&SettingsSnapshot::new(1));
    assert_eq!((stroke.rgb, stroke.alpha), (0x0099_9999, 127));
    let mut decorations = Decorations::default();
    assert!(
        decorations
            .ring_strips(1, Rect::new(0, 0, 300, 200), 24)
            .is_none(),
        "nothing is drawn before the first settings sync"
    );
    assert!(decorations.set_outline(stroke.rgb, stroke.alpha, stroke.width));
    let ring = decorations
        .ring_strips(1, Rect::new(0, 0, 300, 200), 24)
        .expect("ring");
    assert_eq!(
        (ring.paint.rgb, ring.paint.alpha, ring.paint.width),
        (0x0099_9999, 127, 1)
    );
    assert!(
        !decorations.set_outline(stroke.rgb, stroke.alpha, stroke.width),
        "an unchanged snapshot must not request a redraw"
    );
}

#[test]
fn outlines_off_or_zero_opacity_draw_nothing() {
    let mut snapshot = SettingsSnapshot::new(1);
    snapshot
        .values
        .insert("outlines".into(), SettingValue::Boolean(false));
    let stroke = flamewm_ui_core::surface::window_outline_stroke(&snapshot);
    assert_eq!(stroke.alpha, 0, "outlines off means no ring");
    let mut decorations = Decorations::default();
    assert!(decorations.set_outline(stroke.rgb, stroke.alpha, stroke.width));
    assert!(
        decorations
            .ring_strips(1, Rect::new(0, 0, 300, 200), 24)
            .is_none()
    );
    // A zero `windowOutlineOpacity` is the same: no strips, so no elements.
    decorations.set_outline(0x00ff_ffff, 0, 1);
    assert!(
        decorations
            .ring_strips(1, Rect::new(0, 0, 300, 200), 24)
            .is_none()
    );
}

/// The ring is cached per window: an unchanged frame and a moved frame both reuse the strips,
/// while a resize, a new radius and new outline settings each rebuild them.
#[test]
fn outline_cache_rebuilds_only_when_an_input_changes() {
    let mut decorations = decorated();
    let frame = Rect::new(10, 20, 300, 200);
    let ring = decorations.ring_strips(1, frame, 24).expect("ring");
    assert_eq!(
        (ring.width, ring.height, ring.radius, ring.builds),
        (300, 200, 24, 1)
    );
    let strips = ring.strips.len();
    let ring = decorations.ring_strips(1, frame, 24).expect("ring");
    assert_eq!(ring.builds, 1, "unchanged inputs reuse the cache");
    assert_eq!(ring.strips.len(), strips);

    let moved = Rect::new(11, 20, 300, 200);
    let ring = decorations.ring_strips(1, moved, 24).expect("ring");
    assert_eq!(
        ring.builds, 1,
        "a move reuses the strips: they are frame-relative"
    );

    let resized = Rect::new(11, 20, 301, 200);
    let ring = decorations.ring_strips(1, resized, 24).expect("ring");
    assert_eq!((ring.width, ring.builds), (301, 2), "a resize rebuilds");

    let ring = decorations.ring_strips(1, resized, 12).expect("ring");
    assert_eq!((ring.radius, ring.builds), (12, 3), "a new radius rebuilds");

    assert!(decorations.set_outline(0x0011_2233, 127, 2));
    let ring = decorations.ring_strips(1, resized, 12).expect("ring");
    assert_eq!(
        (ring.paint.rgb, ring.paint.width, ring.builds),
        (0x0011_2233, 2, 1),
        "new settings drop the cached strips"
    );
}

/// Strip origins plus buffer sizes are the frame rect: every corner and edge is uploaded and
/// the interior is not, so a caller can place the strips from the frame origin alone.
#[test]
fn ring_strips_tile_the_frame_rect() {
    let mut decorations = decorated();
    let frame = Rect::new(40, 60, 320, 240);
    let (w, h) = (frame.w as u32, frame.h as u32);
    let ring = decorations.ring_strips(1, frame, 24).expect("ring");
    let mut covered = vec![false; (w * h) as usize];
    for strip in &ring.strips {
        let (x, y, sw, sh) = (strip.offset.0, strip.offset.1, strip.size.0, strip.size.1);
        assert!(sw > 0 && sh > 0);
        assert!(
            x >= 0 && y >= 0 && x as u32 + sw as u32 <= w && y as u32 + sh as u32 <= h,
            "strip at {x},{y} {sw}x{sh} leaves the frame"
        );
        for row in 0..sh as u32 {
            for column in 0..sw as u32 {
                covered[((y as u32 + row) * w + x as u32 + column) as usize] = true;
            }
        }
    }
    let covered = |x: u32, y: u32| covered[(y * w + x) as usize];
    assert!(
        covered(0, 0) && covered(w - 1, 0) && covered(0, h - 1) && covered(w - 1, h - 1),
        "all four corners are uploaded"
    );
    assert!(
        covered(w / 2, 0) && covered(w / 2, h - 1) && covered(0, h / 2) && covered(w - 1, h / 2),
        "both edges are uploaded"
    );
    assert!(
        !covered(w / 2, h / 2),
        "the interior stays out of the upload"
    );
}

/// The ring radius is the client clip radius, so the arc and the clipped corner are
/// concentric; maximized, fullscreen and squared windows get a square ring.
#[test]
fn ring_radius_matches_the_client_clip_radius() {
    let work = Rect::new(0, 0, 300, 200);
    let mut model = WindowModel::new(1);
    model.add(1, Rect::new(10, 40, 300, 200), 0);
    let ring = |model: &WindowModel| outline_radius(model.get(1).expect("window"));
    let clip = |model: &WindowModel| {
        let window = model.get(1).expect("window");
        client_corner_radii(window, window.titlebars)
    };
    assert_eq!(ring(&model), clip(&model)[2], "bottom arc is concentric");
    model.configure(false, 0);
    assert_eq!(ring(&model), clip(&model)[0], "no band: same radius");
    model.configure(true, 0);
    model.maximize_to(1, work, Rect::new(10, 40, 300, 200));
    assert_eq!(ring(&model), 0, "maximized is square");
    model.set_fullscreen(1, work, true);
    assert_eq!(ring(&model), 0, "fullscreen is square");
}

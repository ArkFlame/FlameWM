use super::*;
use flamewm_api::shortcuts::{ACTION_LAUNCHER, ACTION_WINDOW_SWITCH};
use flamewm_api::workspace::GridDirection as D;
use flamewm_platform::shortcuts::ShortcutRegistry;

const WORK: Rect = Rect::new(0, 0, 1000, 700);

fn model_with(ids: &[WindowId]) -> WindowModel {
    let mut model = WindowModel::new(4);
    for id in ids {
        let rect = model.cascade_rect(0);
        model.add(*id, rect, 0);
    }
    model
}

#[test]
fn stacking_is_bottom_to_top_and_raise_moves_to_top() {
    let mut model = model_with(&[1, 2, 3]);
    assert_eq!(model.visible_top_first(), vec![3, 2, 1]);
    model.raise(1);
    assert_eq!(model.visible_top_first(), vec![1, 3, 2]);
}

#[test]
fn alt_tab_raises_second_window_and_toggles() {
    let mut model = model_with(&[1, 2, 3]);
    assert_eq!(model.cycle_focus(), Some(2));
    assert_eq!(model.visible_top_first()[0], 2);
    assert_eq!(model.cycle_focus(), Some(3));
    assert_eq!(WindowModel::new(1).cycle_focus(), None);
}

/// `Super+K` steps backwards: from the topmost window it wraps to the bottom of the stack,
/// which is where X11 `cycle_window(-1)` lands. Minimized windows and other workspaces are
/// never candidates, and a lone window stays focused.
#[test]
fn cycle_focus_prev_wraps_to_the_bottom_of_the_stack() {
    let mut model = model_with(&[1, 2, 3]);
    assert_eq!(model.cycle_focus_prev(), Some(1));
    assert_eq!(model.visible_top_first(), vec![1, 3, 2]);
    assert_eq!(model.cycle_focus_prev(), Some(2));
    assert_eq!(model.visible_top_first(), vec![2, 1, 3]);

    // One window is its own predecessor, as X11's modular step returns it unchanged.
    assert_eq!(model_with(&[7]).cycle_focus_prev(), Some(7));
    assert_eq!(WindowModel::new(1).cycle_focus_prev(), None);

    // Skipped candidates: a minimized window and a window on another workspace.
    let mut model = model_with(&[1, 2]);
    model.add(9, Rect::new(0, 0, 200, 200), 1);
    model.minimize(2);
    assert_eq!(model.cycle_focus_prev(), Some(1));
}

/// The directional search scores distance along the axis four times the drift across it, so the
/// nearest window on that side wins even when a farther one is better aligned. Anything not on
/// the requested side is never a target.
#[test]
fn focus_direction_picks_the_nearest_window_on_that_side() {
    let mut model = WindowModel::new(2);
    model.add(1, Rect::new(100, 100, 200, 200), 0);
    model.add(2, Rect::new(500, 100, 200, 200), 0);
    model.add(3, Rect::new(100, 500, 200, 200), 0);
    assert_eq!(model.focus_direction(1, D::Right), Some(2));
    assert_eq!(model.focus_direction(1, D::Down), Some(3));
    assert_eq!(model.focus_direction(1, D::Left), None);
    assert_eq!(model.focus_direction(1, D::Up), None);
    // Both windows sit to the left of 2 at the same distance along the axis; the one that also
    // lines up horizontally wins on the across term.
    assert_eq!(model.focus_direction(2, D::Left), Some(1));

    // A minimized window is not a candidate, and neither is one on another workspace.
    model.minimize(2);
    assert_eq!(model.focus_direction(1, D::Right), None);
    model.restore(2);
    assert_eq!(model.focus_direction(1, D::Right), Some(2));
    model.move_to_workspace(2, 1);
    assert_eq!(model.focus_direction(1, D::Right), None);
}

/// `Super+equal` / `Super+minus` follow X11 `resize_active`: 40x30 per press, floored at
/// 160x96, and only for a floating window.
#[test]
fn resize_step_grows_shrinks_and_floors_a_floating_window() {
    let mut model = WindowModel::new(1);
    model.add(1, Rect::new(10, 10, 200, 200), 0);
    model.resize_step(1, true);
    assert_eq!(model.get(1).unwrap().rect, Rect::new(10, 10, 240, 230));
    model.resize_step(1, false);
    assert_eq!(model.get(1).unwrap().rect, Rect::new(10, 10, 200, 200));
    for _ in 0..20 {
        model.resize_step(1, false);
    }
    assert_eq!(model.get(1).unwrap().rect, Rect::new(10, 10, 160, 96));

    // A maximized window is not floating: the step leaves its geometry alone.
    model.set_rect(1, Rect::new(10, 10, 500, 400));
    model.toggle_maximize(1, WORK);
    let maximized = model.get(1).unwrap().rect;
    model.resize_step(1, false);
    assert_eq!(model.get(1).unwrap().rect, maximized);
}

/// `Super+Shift+arrows` nudge a floating window 40 px and a maximized one never moves.
#[test]
fn nudge_moves_only_a_floating_window() {
    let mut model = WindowModel::new(1);
    model.add(1, Rect::new(100, 100, 200, 200), 0);
    model.nudge(1, D::Left);
    model.nudge(1, D::Down);
    assert_eq!(model.get(1).unwrap().rect, Rect::new(60, 140, 200, 200));

    model.toggle_maximize(1, WORK);
    let maximized = model.get(1).unwrap().rect;
    model.nudge(1, D::Right);
    assert_eq!(model.get(1).unwrap().rect, maximized);
}

/// `Super+V` is X11 `toggle_float`: a floating window fills the work area, and pressing again
/// returns it to its floating rect. From fullscreen the first press lands on the floating rect,
/// dropping the maximize underneath.
#[test]
fn toggle_float_fills_the_work_area_and_returns_to_floating() {
    let mut model = model_with(&[1]);
    let floating = model.get(1).unwrap().rect;
    model.toggle_float(1, WORK, WORK);
    assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
    assert_eq!(model.get(1).unwrap().frame(), WORK);
    model.toggle_float(1, WORK, WORK);
    assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
    assert_eq!(model.get(1).unwrap().rect, floating);

    // Fullscreen over a maximized layout: leaving it returns to floating, not to maximized.
    model.toggle_maximize(1, WORK);
    model.set_fullscreen(1, WORK, true);
    model.toggle_float(1, WORK, WORK);
    assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
    assert_eq!(model.get(1).unwrap().rect, floating);
}

#[test]
fn workspaces_hide_other_windows_and_respect_grid_bounds() {
    let mut model = model_with(&[1]);
    model.add(9, Rect::new(0, 0, 200, 200), 1);
    assert_eq!(model.visible_top_first(), vec![1]);
    assert!(model.switch_workspace(1));
    assert_eq!(model.visible_top_first(), vec![9]);
    assert!(!model.switch_workspace(1), "same workspace is a no-op");
    assert!(!model.switch_workspace(4), "out of range is rejected");
    // 4 workspaces form a 2x2 grid: right of 0 is 1, down of 1 is 3, nothing right of 1.
    assert_eq!(model.workspace_step(D::Right), None);
    assert_eq!(model.workspace_step(D::Down), Some(3));
}

#[test]
fn resize_edges_pick_sides_and_corners_in_the_grip_band() {
    let rect = Rect::new(100, 100, 300, 200);
    let edge = ResizeEdge::at(rect, 101.0, 150.0);
    assert!(edge.left && !edge.right && !edge.top && !edge.bottom);
    let corner = ResizeEdge::at(rect, 398.0, 298.0);
    assert!(corner.right && corner.bottom && !corner.left && !corner.top);
    assert!(in_resize_grip(rect, 150.0, 101.0));
    assert!(!in_resize_grip(rect, 200.0, 200.0));
}

#[test]
fn left_and_top_edges_move_the_origin_and_clamp_to_minimum() {
    let start = Rect::new(100, 100, 300, 200);
    let left = ResizeEdge {
        left: true,
        right: false,
        top: false,
        bottom: false,
    };
    assert_eq!(left.resized(start, -40, 0), Rect::new(60, 100, 340, 200));
    let top = ResizeEdge {
        left: false,
        right: false,
        top: true,
        bottom: false,
    };
    assert_eq!(top.resized(start, 0, 20), Rect::new(100, 120, 300, 180));
    // Dragging past the minimum keeps the opposite side fixed.
    let clamped = left.resized(start, 500, 0);
    assert_eq!(clamped.x + clamped.w, start.x + start.w);
    assert_eq!(clamped.w, MIN_CLIENT_W);
}

#[test]
fn quadrant_picks_the_corner_nearest_the_grab() {
    let rect = Rect::new(0, 0, 200, 100);
    let q = ResizeEdge::quadrant(rect, 150.0, 80.0);
    assert!(q.right && q.bottom && !q.left && !q.top);
    assert_eq!(q.resized(rect, 10, 10), Rect::new(0, 0, 210, 110));
}

#[test]
fn hit_test_prefers_topmost_and_classifies_titlebar_and_buttons() {
    let mut model = WindowModel::new(1);
    model.add(1, Rect::new(100, 100, 300, 300), 0);
    model.add(2, Rect::new(150, 150, 300, 300), 0);
    // Window 2 covers (200,200) client area; window 1 is below it.
    assert_eq!(model.hit(200.0, 200.0), Some(Hit::Client(2)));
    // Titlebar of window 1 above its client rect, away from buttons.
    assert_eq!(model.hit(110.0, 100.0 - 10.0), Some(Hit::Titlebar(1)));
    // Close button sits at the right edge of the frame; its corner-grip overlap is tested
    // separately, so sample the centre.
    let frame = model.get(1).unwrap().frame();
    let close = button_rect(frame, Button::Close);
    let (close_x, close_y) = (
        f64::from(close.x + close.w / 2),
        f64::from(close.y + close.h / 2),
    );
    assert_eq!(
        model.hit(close_x, close_y),
        Some(Hit::Button(1, Button::Close))
    );
}

#[test]
fn resize_grip_is_bottom_right_corner_of_client() {
    let mut model = WindowModel::new(1);
    model.add(7, Rect::new(0, 28, 200, 200), 0);
    assert_eq!(model.hit(195.0, 225.0), Some(Hit::Resize(7)));
    assert_eq!(model.hit(50.0, 50.0), Some(Hit::Client(7)));
}

/// A press on any frame corner starts a resize, including the two top corners in the titlebar
/// band where the client grip band cannot reach. With `titlebars=false` the frame is the client
/// rect, so the same squares sit exactly on the client corners.
#[test]
fn frame_corners_resize_with_and_without_titlebars() {
    let mut model = WindowModel::new(1);
    model.add(1, Rect::new(100, 100, 300, 300), 0);
    let frame = model.get(1).unwrap().frame();
    let left = f64::from(frame.x + 2);
    let right = f64::from(frame.x + frame.w - 2);
    let top = f64::from(frame.y + 2);
    let bottom = f64::from(frame.y + frame.h - 2);
    for (px, py) in [(left, top), (right, top), (left, bottom), (right, bottom)] {
        assert_eq!(model.hit(px, py), Some(Hit::Resize(1)), "frame corner");
    }

    model.configure(false, 0);
    assert_eq!(model.get(1).unwrap().frame(), Rect::new(100, 100, 300, 300));
    for (px, py) in [
        (102.0, 102.0),
        (398.0, 102.0),
        (102.0, 398.0),
        (398.0, 398.0),
    ] {
        assert_eq!(model.hit(px, py), Some(Hit::Resize(1)), "client corner");
    }
}

/// The resolver reports both axes inside a frame corner square and keeps the single-edge band
/// result for presses between the corners.
#[test]
fn resize_edges_pick_frame_corners_before_the_client_band() {
    let rect = Rect::new(100, 100, 300, 200);
    let frame = rect.frame();
    let top = f64::from(frame.y + 2);
    let bottom = f64::from(frame.y + frame.h - 2);
    let corners = [
        ((102.0, top), (true, false, true, false)),
        ((398.0, top), (false, true, true, false)),
        ((102.0, bottom), (true, false, false, true)),
        ((398.0, bottom), (false, true, false, true)),
    ];
    for ((px, py), (left, right, top_edge, bottom_edge)) in corners {
        assert_eq!(
            resize_edge_at(frame, rect, px, py),
            ResizeEdge {
                left,
                right,
                top: top_edge,
                bottom: bottom_edge,
            },
            "corner at {px}, {py}"
        );
    }
    // Middle of the top frame edge and the left client band stay single-edge.
    assert_eq!(
        resize_edge_at(frame, rect, 250.0, top),
        ResizeEdge {
            left: false,
            right: false,
            top: true,
            bottom: false,
        }
    );
    assert_eq!(
        resize_edge_at(frame, rect, 101.0, 150.0),
        ResizeEdge {
            left: true,
            right: false,
            top: false,
            bottom: false,
        }
    );
}

/// A top corner grabbed in the titlebar band resizes from the client rect: the pointer delta
/// moves the client top, so the frame top moves with it and the bottom stays fixed.
#[test]
fn top_corner_resize_moves_the_frame_top_from_the_client_rect() {
    let rect = Rect::new(100, 100, 300, 200);
    let frame = rect.frame();
    let edge = resize_edge_at(frame, rect, 102.0, f64::from(frame.y + 2));
    assert!(edge.left && edge.top && !edge.right && !edge.bottom);
    let resized = edge.resized(rect, -10, 10);
    assert_eq!(resized, Rect::new(90, 110, 310, 190));
    assert_eq!(resized.frame().y, frame.y + 10, "frame top tracks the drag");
}

/// The top-right handle and the close button share one 16 px square; the handle wins only
/// there, so the button centre still closes and the rest of the titlebar still drags.
#[test]
fn corner_grip_shares_only_its_square_with_the_close_button() {
    let mut model = WindowModel::new(1);
    model.add(1, Rect::new(100, 100, 300, 300), 0);
    let frame = model.get(1).unwrap().frame();
    let close = button_rect(frame, Button::Close);
    let centre = (
        f64::from(close.x + close.w / 2),
        f64::from(close.y + close.h / 2),
    );
    assert_eq!(
        model.hit(centre.0, centre.1),
        Some(Hit::Button(1, Button::Close))
    );
    assert_eq!(
        model.hit(f64::from(frame.x + frame.w - 2), f64::from(frame.y + 2)),
        Some(Hit::Resize(1)),
        "the shared square belongs to the handle"
    );
    // One full grip below the frame top, at its middle, is a titlebar drag.
    assert_eq!(
        model.hit(
            f64::from(frame.x + frame.w / 2),
            f64::from(frame.y + CORNER_GRIP)
        ),
        Some(Hit::Titlebar(1))
    );
    assert_eq!(model.hit(200.0, 200.0), Some(Hit::Client(1)));
}

#[test]
fn minimize_and_show_desktop_round_trip() {
    let mut model = model_with(&[1, 2]);
    model.minimize(2);
    assert_eq!(model.visible_top_first(), vec![1]);
    model.restore(2);
    assert_eq!(model.visible_top_first(), vec![2, 1]);

    model.toggle_show_desktop();
    assert!(model.visible_top_first().is_empty());
    model.toggle_show_desktop();
    assert_eq!(model.visible_top_first().len(), 2);
}

#[test]
fn maximize_restores_previous_rect_and_drops_titlebar_offset() {
    let mut model = model_with(&[1]);
    let before = model.get(1).unwrap().rect;
    model.toggle_maximize(1, WORK);
    let maximized = model.get(1).unwrap().clone();
    assert_eq!(maximized.mode, Mode::Maximized);
    assert_eq!(maximized.frame(), Rect::new(0, 0, 1000, 700));
    model.toggle_maximize(1, WORK);
    assert_eq!(model.get(1).unwrap().rect, before);
    assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
}

/// A model that is never configured keeps the defaults, which are also the regression baseline:
/// titlebars on and no margin, i.e. exactly the geometry and hit testing this model had before
/// the two settings were read.
#[test]
fn defaults_are_titlebars_on_and_no_margin() {
    for model in [WindowModel::new(2), WindowModel::default()] {
        assert!(model.titlebars);
        assert_eq!(model.titlebars, DEFAULT_TITLEBARS);
        assert_eq!(model.window_margin, 0);
        assert_eq!(model.window_margin, DEFAULT_WINDOW_MARGIN);
        assert_eq!(model.work_area, None);
    }
}

/// `titlebars=false` is X11 `reflow_titlebars(false)`: the frame's top edge moves down by the
/// band, so the client keeps its on-screen position and the frame equals the client rect. The
/// band above the client is then outside the frame and hits nothing at all.
#[test]
fn titlebars_off_drops_the_band_from_geometry_and_hit_testing() {
    let mut model = WindowModel::new(1);
    model.add(1, Rect::new(100, 100, 300, 300), 0);
    // Baseline, titlebars on: the band is part of the frame and grabs the click.
    assert!(model.get(1).unwrap().decorated());
    assert_eq!(
        model.get(1).unwrap().frame(),
        Rect::new(100, 100 - TITLEBAR_HEIGHT, 300, 300 + TITLEBAR_HEIGHT)
    );
    assert_eq!(model.hit(110.0, 100.0 - 10.0), Some(Hit::Titlebar(1)));

    model.configure(false, 0);
    let window = model.get(1).unwrap().clone();
    assert!(!window.decorated());
    assert_eq!(
        window.rect,
        Rect::new(100, 100, 300, 300),
        "client stays put"
    );
    assert_eq!(
        window.frame(),
        window.rect,
        "frame no longer includes a band"
    );
    assert_eq!(
        model.hit(110.0, 100.0 - 10.0),
        None,
        "band is not hit-testable"
    );
    // Inside the client the hit is a plain client hit: no titlebar and no buttons.
    assert_eq!(model.hit(200.0, 200.0), Some(Hit::Client(1)));
    assert_eq!(model.hit(350.0, 120.0), Some(Hit::Client(1)));
    // The resize grip still belongs to the client rect.
    assert_eq!(model.hit(102.0, 200.0), Some(Hit::Resize(1)));
}

/// Turning titlebars back on restores the band above the unchanged client rect, which is the
/// same reflow X11 applies when the setting flips, so the hit test classifies it again.
#[test]
fn titlebars_on_again_restores_the_band_above_the_same_client() {
    let mut model = WindowModel::new(1);
    model.add(1, Rect::new(100, 100, 300, 300), 0);
    model.configure(false, 0);
    model.configure(true, 0);
    let window = model.get(1).unwrap().clone();
    assert!(window.decorated());
    assert_eq!(window.rect, Rect::new(100, 100, 300, 300));
    assert_eq!(
        window.frame(),
        Rect::new(100, 100 - TITLEBAR_HEIGHT, 300, 300 + TITLEBAR_HEIGHT)
    );
    assert_eq!(model.hit(110.0, 100.0 - 10.0), Some(Hit::Titlebar(1)));
    let close = button_rect(window.frame(), Button::Close);
    assert_eq!(
        model.hit(
            f64::from(close.x + close.w / 2),
            f64::from(close.y + close.h / 2)
        ),
        Some(Hit::Button(1, Button::Close))
    );
}

/// Windows added after the settings were applied inherit them; a negative margin is "no margin",
/// as `apply_window_margin` itself treats it.
#[test]
fn configure_reaches_new_windows_and_clamps_a_negative_margin() {
    let mut model = WindowModel::new(1);
    model.configure(false, -5);
    assert_eq!(model.window_margin, 0, "a negative margin means none");
    model.add(2, Rect::new(0, 0, 200, 200), 0);
    assert!(!model.get(2).unwrap().decorated());
    assert_eq!(model.get(2).unwrap().frame(), Rect::new(0, 0, 200, 200));
}

/// `windowMargin` parity with X11: a maximized frame is the work area inset by
/// `flamewm_window_core::apply_window_margin`, and the titlebar band comes out of the client.
#[test]
fn maximize_insets_the_work_area_by_the_window_margin() {
    let mut model = model_with(&[1]);
    model.configure(true, 2);
    model.toggle_maximize(1, WORK);
    let window = model.get(1).unwrap().clone();
    let frame = Rect::new(2, 2, 996, 696);
    assert_eq!(window.frame(), frame);
    assert_eq!(
        window.rect,
        Rect::new(
            frame.x,
            frame.y + TITLEBAR_HEIGHT,
            frame.w,
            frame.h - TITLEBAR_HEIGHT
        )
    );
    // Same rectangle the shared policy produces for a tile that fills the work area.
    assert_eq!(
        flamewm_window_core::apply_window_margin(
            flamewm_api::Rect::new(0, 0, 1000, 700),
            flamewm_api::Rect::new(0, 0, 1000, 700),
            2
        ),
        flamewm_api::Rect::new(frame.x, frame.y, frame.w, frame.h)
    );
    // Leaving maximized restores the floating rect untouched by the margin.
    model.toggle_maximize(1, WORK);
    assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
}

/// Margin 0 is today's geometry byte for byte: the frame fills the work area exactly.
#[test]
fn zero_margin_maximize_is_unchanged_from_today() {
    let mut model = model_with(&[1]);
    model.configure(true, 0);
    model.toggle_maximize(1, WORK);
    let frame = model.get(1).unwrap().frame();
    assert_eq!(frame, WORK);
    assert_eq!(
        model.get(1).unwrap().rect,
        Rect::new(0, TITLEBAR_HEIGHT, WORK.w, WORK.h - TITLEBAR_HEIGHT)
    );
    model.configure(true, 0);
    assert_eq!(
        model.get(1).unwrap().frame(),
        frame,
        "re-applying is a no-op"
    );
}

/// Without titlebars the maximized client fills the margined work area, as X11 does when
/// `titlebar_height()` is 0 and the frame equals the client.
#[test]
fn maximize_without_titlebars_fills_the_margined_work_area() {
    let mut model = model_with(&[1]);
    model.configure(false, 2);
    model.toggle_maximize(1, WORK);
    let window = model.get(1).unwrap().clone();
    assert!(!window.decorated());
    assert_eq!(window.rect, Rect::new(2, 2, 996, 696));
    assert_eq!(window.frame(), window.rect);
}

/// `Super+V` goes through the same maximize path, so it picks the margin up too.
#[test]
fn toggle_float_fills_the_margined_work_area() {
    let mut model = model_with(&[1]);
    model.configure(true, 2);
    model.toggle_float(1, WORK, WORK);
    assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
    assert_eq!(model.get(1).unwrap().frame(), Rect::new(2, 2, 996, 696));
}

/// An output size change re-applies the margin through `refit_output`, for the live maximized
/// window and for the maximized rect a fullscreen window restores to.
#[test]
fn refit_output_reapplies_the_margin_after_an_output_change() {
    let mut model = model_with(&[1]);
    model.configure(true, 2);
    model.toggle_maximize(1, WORK);

    let resized = Rect::new(0, 0, 1280, 720);
    let usable = Rect::new(0, 0, 1280, 688);
    model.refit_output(1, resized, usable);
    assert_eq!(model.get(1).unwrap().frame(), Rect::new(2, 2, 1276, 684));

    // Fullscreen over the maximized layout: the saved maximize rect tracks the new margin too.
    model.set_fullscreen(1, resized, true);
    let bigger = Rect::new(0, 0, 1600, 900);
    let bigger_work = Rect::new(0, 0, 1600, 868);
    model.refit_output(1, bigger, bigger_work);
    assert_eq!(
        model.get(1).unwrap().rect,
        bigger,
        "fullscreen keeps the output rect"
    );
    model.set_fullscreen(1, bigger, false);
    assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
    assert_eq!(model.get(1).unwrap().frame(), Rect::new(2, 2, 1596, 864));
}

/// A live settings change reflows a maximized frame at once, the way X11 `reflow_titlebars`
/// plus `reconcile_work_area` do, without waiting for the next maximize keypress.
#[test]
fn configure_reflows_a_maximized_frame_immediately() {
    let mut model = model_with(&[1]);
    model.configure(true, 2);
    model.toggle_maximize(1, WORK);
    assert_eq!(model.get(1).unwrap().frame(), Rect::new(2, 2, 996, 696));

    // Margin change: same work area, new inset, no second maximize call.
    model.configure(true, 8);
    assert_eq!(model.get(1).unwrap().frame(), Rect::new(8, 8, 984, 684));

    // Titlebars off: the frame still fills the margined work area, no band to subtract.
    model.configure(false, 8);
    let window = model.get(1).unwrap().clone();
    assert!(!window.decorated());
    assert_eq!(window.frame(), Rect::new(8, 8, 984, 684));
    assert_eq!(window.rect, window.frame());

    // Back to the defaults: the unconfigured geometry this model always produced.
    model.configure(true, 0);
    assert_eq!(model.get(1).unwrap().frame(), WORK);
}

/// `configure` never touches a floating window and never overwrites its restore rect.
#[test]
fn configure_leaves_floating_and_restore_geometry_alone() {
    let mut model = model_with(&[1]);
    let floating = model.get(1).unwrap().rect;
    model.configure(false, 4);
    assert_eq!(model.get(1).unwrap().rect, floating);

    model.configure(true, 0);
    model.toggle_maximize(1, WORK);
    model.configure(true, 6);
    model.toggle_maximize(1, WORK);
    assert_eq!(model.get(1).unwrap().rect, floating);
}

/// A new window cascades below the band only when the model draws one.
#[test]
fn cascade_reserves_the_band_only_with_titlebars() {
    let mut model = WindowModel::new(1);
    let with_band = model.cascade_rect(0);
    assert_eq!(with_band.y, 60 + TITLEBAR_HEIGHT);
    model.configure(false, 0);
    let without_band = model.cascade_rect(0);
    assert_eq!(without_band.y, 60);
    assert_eq!(
        without_band,
        Rect::new(with_band.x, 60, with_band.w, with_band.h)
    );
}

#[test]
fn fullscreen_resize_and_restore_maximized_window() {
    let mut model = model_with(&[1, 2]);
    let initial = model.get(1).unwrap().rect;
    model.toggle_maximize(1, WORK);
    let maximized = model.get(1).unwrap().rect;
    model.set_fullscreen(1, WORK, true);
    let resized = Rect::new(0, 0, 1280, 720);
    model.set_fullscreen(1, resized, true);
    assert_eq!(model.get(1).unwrap().rect, resized);
    model.set_fullscreen(1, resized, false);
    assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
    assert_eq!(model.get(1).unwrap().rect, maximized);
    model.toggle_maximize(1, WORK);
    assert_eq!(model.get(1).unwrap().rect, initial);
}

#[test]
fn output_resize_refits_fullscreen_maximized_and_inactive_windows() {
    let mut model = model_with(&[1, 2, 3]);
    let original = model.get(1).unwrap().rect;
    model.toggle_maximize(1, WORK);
    model.set_fullscreen(1, WORK, true);
    model.move_to_workspace(1, 1);
    model.toggle_maximize(2, WORK);
    let resized = Rect::new(0, 0, 1280, 720);
    let usable = Rect::new(0, 0, 1280, 688);
    for id in [1, 2, 3] {
        model.refit_output(id, resized, usable);
    }
    assert_eq!(model.get(1).unwrap().rect, resized);
    assert_eq!(model.get(2).unwrap().frame(), usable);
    assert_eq!(model.get(3).unwrap().mode, Mode::Normal);
    model.set_fullscreen(1, resized, false);
    assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
    assert_eq!(model.get(1).unwrap().frame(), usable);
    model.toggle_maximize(1, WORK);
    assert_eq!(model.get(1).unwrap().rect, original);
}

#[test]
fn fullscreen_resized_while_on_other_workspace_is_not_clipped() {
    let mut model = model_with(&[1]);
    model.move_to_workspace(1, 1);
    model.set_fullscreen(1, WORK, true);
    model.set_fullscreen(1, Rect::new(0, 0, 640, 360), true);
    assert_eq!(model.get(1).unwrap().rect, Rect::new(0, 0, 640, 360));
    model.set_fullscreen(1, WORK, false);
    assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
}

#[test]
fn set_fullscreen_is_idempotent() {
    let mut model = model_with(&[1]);
    let before = model.get(1).unwrap().rect;
    model.set_fullscreen(1, WORK, true);
    model.set_fullscreen(1, WORK, true);
    model.set_fullscreen(1, WORK, false);
    model.set_fullscreen(1, WORK, false);
    assert_eq!(model.get(1).unwrap().rect, before);
}

#[test]
fn fullscreen_has_no_frame_and_restores() {
    let mut model = model_with(&[1]);
    let before = model.get(1).unwrap().rect;
    model.set_fullscreen(1, WORK, true);
    assert_eq!(model.get(1).unwrap().frame(), WORK);
    assert!(!model.get(1).unwrap().decorated());
    model.set_fullscreen(1, WORK, false);
    assert_eq!(model.get(1).unwrap().rect, before);
}

#[test]
fn snapped_modes_commit_every_shared_edge_and_corner_target() {
    let placements = [
        (SnapTarget::LeftHalf, Rect::new(0, 0, 500, 700)),
        (SnapTarget::RightHalf, Rect::new(500, 0, 500, 700)),
        (SnapTarget::TopHalf, Rect::new(0, 0, 1000, 350)),
        (SnapTarget::BottomHalf, Rect::new(0, 350, 1000, 350)),
        (SnapTarget::TopLeftQuarter, Rect::new(0, 0, 500, 350)),
        (SnapTarget::TopRightQuarter, Rect::new(500, 0, 500, 350)),
        (SnapTarget::BottomLeftQuarter, Rect::new(0, 350, 500, 350)),
        (
            SnapTarget::BottomRightQuarter,
            Rect::new(500, 350, 500, 350),
        ),
    ];
    let floating = Rect::new(120, 130, 640, 420);
    for (target, expected_frame) in placements {
        let mut model = WindowModel::new(1);
        model.configure(true, 0);
        model.add(1, floating, 0);
        model.snap_to(1, target, WORK, floating);
        let window = model.get(1).unwrap();
        assert_eq!(window.mode, Mode::Snapped(target));
        assert_eq!(window.frame(), expected_frame, "{target:?}");
        assert_eq!(
            window.restore, floating,
            "{target:?} keeps floating restore"
        );
    }

    let mut model = WindowModel::new(1);
    model.configure(true, 0);
    model.add(1, floating, 0);
    model.maximize_to(1, WORK, floating);
    assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
    assert_eq!(model.get(1).unwrap().frame(), WORK);
    assert_eq!(model.get(1).unwrap().restore, floating);
}

#[test]
fn snap_preview_and_commit_share_margin_and_titlebar_conversion() {
    let work = Rect::new(0, 0, 1000, 700);
    let floating = Rect::new(120, 130, 640, 420);
    let mut model = WindowModel::new(1);
    model.configure(true, 2);
    model.add(1, floating, 0);
    let preview = model
        .snap_client_geometry(1, SnapTarget::LeftHalf, work)
        .unwrap();
    model.snap_to(1, SnapTarget::LeftHalf, work, floating);
    let window = model.get(1).unwrap();
    assert_eq!(window.rect, preview);
    assert_eq!(window.frame(), Rect::new(2, 2, 497, 696));
    assert_eq!(window.rect.y, 2 + TITLEBAR_HEIGHT);
    assert_eq!(window.rect.h, 696 - TITLEBAR_HEIGHT);
    assert_eq!(window.restore, floating);

    model.configure(false, 2);
    model.snap_to(1, SnapTarget::BottomRightQuarter, work, floating);
    assert_eq!(model.get(1).unwrap().frame(), Rect::new(501, 351, 497, 347));
    assert_eq!(model.get(1).unwrap().rect, model.get(1).unwrap().frame());

    let mut model = WindowModel::new(1);
    model.configure(true, 2);
    model.add(1, floating, 0);
    let preview = model
        .snap_client_geometry(1, SnapTarget::Maximize, work)
        .unwrap();
    model.maximize_to(1, work, floating);
    assert_eq!(model.get(1).unwrap().rect, preview);
    assert_eq!(model.get(1).unwrap().frame(), Rect::new(2, 2, 996, 696));
}

#[test]
fn resizing_a_snapped_window_leaves_layout_without_snapping_on_release() {
    let floating = Rect::new(120, 130, 640, 420);
    let resized = Rect::new(118, 158, 644, 390);
    let mut model = WindowModel::new(1);
    model.add(1, floating, 0);
    model.snap_to(1, SnapTarget::LeftHalf, WORK, floating);
    model.set_rect(1, resized);
    assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
    assert_eq!(model.get(1).unwrap().rect, resized);
    assert_eq!(model.get(1).unwrap().restore, resized);
}

#[test]
fn moving_out_of_snapped_or_maximized_restores_floating_size() {
    let floating = Rect::new(120, 130, 640, 420);
    let moved = Rect::new(260, 90, 640, 420);
    for from_maximized in [false, true] {
        let mut model = WindowModel::new(1);
        model.add(1, floating, 0);
        if from_maximized {
            model.maximize_to(1, WORK, floating);
        } else {
            model.snap_to(1, SnapTarget::BottomRightQuarter, WORK, floating);
        }
        model.commit_move(1, moved);
        let window = model.get(1).unwrap();
        assert_eq!(window.mode, Mode::Normal);
        assert_eq!(window.rect, moved);
        assert_eq!(window.restore, moved);
    }
}

#[test]
fn activated_fullscreen_move_commits_free_and_snapped_placements() {
    let floating = Rect::new(120, 130, 640, 420);
    let moved = Rect::new(260, 90, 640, 420);
    let mut model = WindowModel::new(1);
    model.add(1, floating, 0);
    model.set_fullscreen(1, WORK, true);
    model.commit_move(1, moved);
    let window = model.get(1).unwrap();
    assert_eq!(window.mode, Mode::Normal);
    assert_eq!(window.rect, moved);
    assert_eq!(window.restore, moved);

    // A later fullscreen cycle restores the committed floating geometry, not the old rect.
    model.set_fullscreen(1, WORK, true);
    model.set_fullscreen(1, WORK, false);
    assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
    assert_eq!(model.get(1).unwrap().rect, moved);

    model.set_fullscreen(1, WORK, true);
    model.snap_to(1, SnapTarget::LeftHalf, WORK, moved);
    let window = model.get(1).unwrap();
    assert_eq!(window.mode, Mode::Snapped(SnapTarget::LeftHalf));
    assert_eq!(window.frame(), Rect::new(0, 0, 500, 700));
    assert_eq!(window.restore, moved);

    model.set_fullscreen(1, WORK, true);
    model.maximize_to(1, WORK, moved);
    let window = model.get(1).unwrap();
    assert_eq!(window.mode, Mode::Maximized);
    assert_eq!(window.frame(), WORK);
    assert_eq!(window.restore, moved);
}

#[test]
fn snapped_maximized_fullscreen_and_refit_keep_the_floating_restore() {
    let floating = Rect::new(120, 130, 640, 420);
    let mut model = WindowModel::new(1);
    model.configure(true, 2);
    model.add(1, floating, 0);
    model.snap_to(1, SnapTarget::LeftHalf, WORK, floating);
    let snapped = model.get(1).unwrap().frame();
    model.toggle_maximize(1, WORK);
    assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
    model.set_fullscreen(1, WORK, true);

    let output = Rect::new(0, 0, 1280, 800);
    let work = Rect::new(0, 0, 1280, 760);
    model.refit_output(1, output, work);
    assert_eq!(model.get(1).unwrap().rect, output);
    model.set_fullscreen(1, output, false);
    assert_eq!(model.get(1).unwrap().mode, Mode::Maximized);
    assert_eq!(model.get(1).unwrap().frame(), Rect::new(2, 2, 1276, 756));

    model.toggle_maximize(1, work);
    assert_eq!(
        model.get(1).unwrap().mode,
        Mode::Snapped(SnapTarget::LeftHalf)
    );
    assert_eq!(model.get(1).unwrap().frame(), Rect::new(2, 2, 637, 756));
    assert_ne!(
        model.get(1).unwrap().frame(),
        snapped,
        "snap refits to new work area"
    );
    assert_eq!(model.get(1).unwrap().restore, floating);

    let moved = Rect::new(240, 90, floating.w, floating.h);
    model.commit_move(1, moved);
    assert_eq!(model.get(1).unwrap().mode, Mode::Normal);
    assert_eq!(model.get(1).unwrap().rect, moved);
    assert_eq!(model.get(1).unwrap().restore, moved);
}

#[test]
fn resize_is_clamped_to_minimum_client_size() {
    let mut model = model_with(&[1]);
    model.resize_to(1, 1, 1);
    let rect = model.get(1).unwrap().rect;
    assert_eq!((rect.w, rect.h), (MIN_CLIENT_W, MIN_CLIENT_H));
}

#[test]
fn parses_shared_default_bindings_into_chords() {
    assert_eq!(
        parse_chord("Ctrl+Super+Left"),
        Some(Chord {
            mods: Mods {
                ctrl: true,
                logo: true,
                ..Mods::default()
            },
            key: "left".into(),
        })
    );
    assert_eq!(
        parse_chord("Super_L").map(|c| c.key),
        Some("super_l".into())
    );
    assert_eq!(parse_chord("   "), None);
}

#[test]
fn bare_letters_never_trigger_show_desktop_and_both_chords_do() {
    use flamewm_api::shortcuts::{ACTION_SHOW_DESKTOP, ACTION_SHOW_DESKTOP_CTRL_ALT};

    let registry = ShortcutRegistry::default();
    let shortcuts = Shortcuts::from_bindings(registry.bindings());

    // Regression: plain `d` (and every other bare letter) must never toggle show-desktop.
    assert_eq!(shortcuts.action_for(Mods::default(), "d"), None);
    for letter in 'a'..='z' {
        assert_eq!(
            shortcuts.action_for(Mods::default(), &letter.to_string()),
            None,
            "bare {letter} must not match any chord"
        );
    }

    let logo = Mods {
        logo: true,
        ..Mods::default()
    };
    let ctrl_alt = Mods {
        ctrl: true,
        alt: true,
        ..Mods::default()
    };
    assert_eq!(shortcuts.action_for(logo, "d"), Some(ACTION_SHOW_DESKTOP));
    assert_eq!(
        shortcuts.action_for(ctrl_alt, "d"),
        Some(ACTION_SHOW_DESKTOP_CTRL_ALT)
    );
    // Super+d must not be reachable through a stale bare-`d` entry either.
    assert_eq!(shortcuts.action_for(logo, "D"), Some(ACTION_SHOW_DESKTOP));
}

#[test]
fn stray_bare_letter_binding_can_never_fire_while_typing() {
    use flamewm_api::shortcuts::{ACTION_SHOW_DESKTOP, ACTION_WINDOW_CLOSE};

    // A legacy or hand-edited registry entry that lost its modifier.
    let stale = BTreeMap::from([
        (ACTION_SHOW_DESKTOP.to_owned(), KeyBinding::assigned("d")),
        (
            ACTION_WINDOW_CLOSE.to_owned(),
            KeyBinding::assigned("Shift+q"),
        ),
    ]);
    let shortcuts = Shortcuts::from_bindings(&stale);
    assert_eq!(shortcuts.action_for(Mods::default(), "d"), None);
    assert_eq!(
        shortcuts.action_for(
            Mods {
                shift: true,
                ..Mods::default()
            },
            "q"
        ),
        None,
        "Shift alone must not make a typing key a shortcut"
    );
    // Media keys and named keys stay bindable without a modifier.
    let named = BTreeMap::from([(
        "VolumeUp".to_owned(),
        KeyBinding::assigned("XF86AudioRaiseVolume"),
    )]);
    let shortcuts = Shortcuts::from_bindings(&named);
    assert_eq!(
        shortcuts.action_for(Mods::default(), "xf86audioraisevolume"),
        Some("VolumeUp")
    );
    // An empty key never matches, and neither does an empty query.
    assert_eq!(shortcuts.action_for(Mods::default(), ""), None);
}

#[test]
fn shared_registry_resolves_actions_for_wayland_keys() {
    let registry = ShortcutRegistry::default();
    let shortcuts = Shortcuts::from_bindings(registry.bindings());
    let logo = Mods {
        logo: true,
        ..Mods::default()
    };
    let alt = Mods {
        alt: true,
        ..Mods::default()
    };
    assert_eq!(shortcuts.action_for(logo, "space"), Some(ACTION_LAUNCHER));
    assert_eq!(shortcuts.action_for(alt, "Tab"), Some(ACTION_WINDOW_SWITCH));
    assert_eq!(shortcuts.action_for(Mods::default(), "Tab"), None);
}
/// A removal must carry windows off the removed workspace. The plan's own mapping is what does
/// it, so a window that sat on the removed index follows the mapping instead of ending up on a
/// workspace that no longer exists.
#[test]
fn a_workspace_removal_carries_windows_through_the_plan_mapping() {
    use flamewm_api::workspace::{WorkspacePlan, WorkspacePlanKind};

    let mut model = WindowModel::new(3);
    model.add(1, Rect::new(0, 0, 100, 100), 0);
    model.add(2, Rect::new(0, 0, 100, 100), 2);
    model.switch_workspace(2);

    // Removing workspace 1 (index 1): old 0 stays, old 2 moves down to 1.
    let plan = WorkspacePlan {
        kind: WorkspacePlanKind::Remove,
        expected_revision: 1,
        new_revision: 2,
        old_count: 3,
        new_count: 2,
        requested_index: 1,
        old_to_new: vec![0, 1, 1],
        new_active: 1,
        new_last: None,
        new_names: vec!["1".to_owned(), "2".to_owned()],
    };
    plan.validate().unwrap();
    let moved = model.apply_workspace_plan(&plan);

    assert_eq!(model.workspace_count(), 2);
    assert!(moved, "the visible workspace changed from 2 to 1");
    assert_eq!(model.current_workspace(), 1);
    assert_eq!(
        model.get(1).unwrap().workspace,
        0,
        "a window on a surviving workspace keeps its place"
    );
    assert_eq!(
        model.get(2).unwrap().workspace,
        1,
        "a window above the removed index follows the mapping down"
    );
    // Nothing may point outside the new topology.
    for window in model.managed() {
        assert!(
            window.workspace < model.workspace_count(),
            "window {} landed outside the topology",
            window.id
        );
    }
}

/// Inserting a workspace shifts later windows up and leaves the visible one where the plan says.
#[test]
fn a_workspace_insert_shifts_later_windows() {
    use flamewm_api::workspace::{WorkspacePlan, WorkspacePlanKind};

    let mut model = WindowModel::new(2);
    model.add(1, Rect::new(0, 0, 100, 100), 0);
    model.add(2, Rect::new(0, 0, 100, 100), 1);

    let plan = WorkspacePlan {
        kind: WorkspacePlanKind::Insert,
        expected_revision: 1,
        new_revision: 2,
        old_count: 2,
        new_count: 3,
        requested_index: 1,
        old_to_new: vec![0, 2],
        new_active: 1,
        new_last: None,
        new_names: vec!["1".to_owned(), "2".to_owned(), "3".to_owned()],
    };
    plan.validate().unwrap();
    model.apply_workspace_plan(&plan);

    assert_eq!(model.workspace_count(), 3);
    assert_eq!(model.get(1).unwrap().workspace, 0);
    assert_eq!(
        model.get(2).unwrap().workspace,
        2,
        "a window after the inserted slot shifts up"
    );
}

/// The persisted `workspaces.count` is adopted as-is when it fits the product maximum.
#[test]
fn a_live_count_is_adopted_and_windows_stay_inside_it() {
    let mut model = WindowModel::new(4);
    model.add(1, Rect::new(0, 0, 100, 100), 3);
    model.switch_workspace(3);

    assert!(model.set_count(2), "the visible workspace moved down");
    assert_eq!(model.workspace_count(), 2);
    assert_eq!(model.current_workspace(), 1);
    assert_eq!(
        model.get(1).unwrap().workspace,
        1,
        "a window on a removed workspace lands on the new last one"
    );
    assert!(!model.set_count(2), "the same count is a no-op");
}

/// A persisted count outside the product range is clamped, never trusted verbatim.
#[test]
fn a_live_count_is_clamped_to_the_product_range() {
    let mut model = WindowModel::new(4);
    model.set_count(0);
    assert_eq!(model.workspace_count(), 1);
    model.set_count(usize::MAX);
    assert_eq!(
        model.workspace_count(),
        flamewm_api::workspace::MAX_WORKSPACES
    );
}

/// Unplugging or shrinking an output pulls floating windows that now sit off-screen back inside,
/// and leaves windows that are still visible where the user put them.
#[test]
fn clamp_floating_into_pulls_offscreen_windows_back_after_an_output_shrink() {
    let mut model = model_with(&[1, 2]);
    model.configure(true, 0);
    model.get_mut(1).unwrap().rect = Rect::new(900, 100, 300, 200);
    model.get_mut(2).unwrap().rect = Rect::new(10, 100, 200, 200);

    let output = Rect::new(0, 0, 1000, 700);
    let moved = model.clamp_floating_into(output);
    assert_eq!(moved, vec![1], "only the window past the right edge moves");
    assert_eq!(
        model.get(1).unwrap().rect.x,
        700,
        "right edge sits on the output edge"
    );
    assert_eq!(model.get(2).unwrap().rect, Rect::new(10, 100, 200, 200));
}

/// A floating window larger than a shrunk output is shrunk to fit, with the titlebar band kept.
#[test]
fn clamp_floating_into_shrinks_a_window_larger_than_the_output() {
    let mut model = model_with(&[1]);
    model.configure(true, 0);
    model.get_mut(1).unwrap().rect = Rect::new(0, 0, 800, 600);

    let output = Rect::new(0, 0, 500, 400);
    assert_eq!(model.clamp_floating_into(output), vec![1]);
    let frame = model.get(1).unwrap().frame();
    assert!(
        frame.x >= 0 && frame.y >= 0 && frame.x + frame.w <= 500 && frame.y + frame.h <= 400,
        "frame {frame:?} must fit the output"
    );
    assert_eq!(frame.w, 500);
}

/// Maximized and fullscreen windows are owned by the refit path, not this clamp.
#[test]
fn clamp_floating_into_leaves_maximized_windows_alone() {
    let mut model = model_with(&[1]);
    model.configure(true, 0);
    model.toggle_maximize(1, WORK);
    let before = model.get(1).unwrap().rect;
    assert!(
        model
            .clamp_floating_into(Rect::new(0, 0, 100, 100))
            .is_empty()
    );
    assert_eq!(model.get(1).unwrap().rect, before);
}

/// A zero or negative output has no usable area: nothing moves and nothing panics.
#[test]
fn clamp_floating_into_ignores_zero_sized_outputs() {
    let mut model = model_with(&[1]);
    model.get_mut(1).unwrap().rect = Rect::new(40, 40, 300, 200);
    assert!(model.clamp_floating_into(Rect::new(0, 0, 0, 0)).is_empty());
    assert!(
        model
            .clamp_floating_into(Rect::new(0, 0, -5, 300))
            .is_empty()
    );
    assert_eq!(model.get(1).unwrap().rect, Rect::new(40, 40, 300, 200));
}

/// A pointer left past a shrunk or unplugged output lands on that output's last pixel.
#[test]
fn clamp_point_into_pulls_pointer_onto_shrunk_output() {
    let shrunk = Rect::new(0, 0, 800, 600);
    assert_eq!(clamp_point_into(1900.0, 1000.0, shrunk), (799.0, 599.0));
    assert_eq!(clamp_point_into(-5.0, -3.0, shrunk), (0.0, 0.0));
    assert_eq!(clamp_point_into(120.5, 40.0, shrunk), (120.5, 40.0));
    let offset = Rect::new(1920, 0, 1280, 1024);
    assert_eq!(clamp_point_into(100.0, 2000.0, offset), (1920.0, 1023.0));
    let empty = Rect::new(0, 0, 0, 0);
    assert_eq!(clamp_point_into(7.0, 9.0, empty), (7.0, 9.0));
}

/// Odd-sized outputs (1px, non-even dimensions) clamp without an inverted range.
#[test]
fn clamp_into_output_handles_one_pixel_and_odd_outputs() {
    let one = clamp_into_output(Rect::new(50, 50, 30, 30), Rect::new(0, 0, 1, 1));
    assert_eq!(one, Rect::new(0, 0, 1, 1));
    let odd = clamp_into_output(Rect::new(1000, -20, 333, 333), Rect::new(0, 0, 1921, 1081));
    assert_eq!(odd, Rect::new(1000, 0, 333, 333));
    let inside = Rect::new(3, 4, 5, 6);
    assert_eq!(clamp_into_output(inside, Rect::new(0, 0, 999, 999)), inside);
}

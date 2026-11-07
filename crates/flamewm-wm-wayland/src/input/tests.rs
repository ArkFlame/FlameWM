//! Unit tests for the input module.

use super::buttons::*;
use super::keyboard::*;
use super::pointer::*;
use super::settings::*;
use super::snap::*;
use super::*;
use crate::model::{Mode, Rect, TITLEBAR_HEIGHT, WindowModel};
use crate::state::Grab;
use flamewm_platform::shortcuts::{
    ACTION_BRIGHTNESS_DOWN, ACTION_BRIGHTNESS_UP, ACTION_VOLUME_DOWN, ACTION_VOLUME_MUTE,
    ACTION_VOLUME_UP,
};
use flamewm_platform::system::MediaAudioKey;
use flamewm_window_core::SnapTarget;
use smithay::utils::{Logical, Point};

/// The default registry binds these media actions, so dispatch must recognize the exact ids
/// instead of swallowing them in the unhandled-shortcut arm.
#[test]
fn media_shortcut_actions_route_to_the_system_service() {
    assert_eq!(
        media_key(ACTION_BRIGHTNESS_UP),
        Some(MediaKey::Brightness(true))
    );
    assert_eq!(
        media_key(ACTION_BRIGHTNESS_DOWN),
        Some(MediaKey::Brightness(false))
    );
    assert_eq!(
        media_key(ACTION_VOLUME_UP),
        Some(MediaKey::Audio(MediaAudioKey::VolumeUp))
    );
    assert_eq!(
        media_key(ACTION_VOLUME_DOWN),
        Some(MediaKey::Audio(MediaAudioKey::VolumeDown))
    );
    assert_eq!(
        media_key(ACTION_VOLUME_MUTE),
        Some(MediaKey::Audio(MediaAudioKey::VolumeMute))
    );
    assert_eq!(
        media_key("WindowClose"),
        None,
        "other actions keep their path"
    );
}

#[test]
fn move_activation_matches_x11_per_axis_inclusive_threshold() {
    assert!(move_activated(4, 0));
    assert!(move_activated(0, 4));
    assert!(
        !move_activated(3, 3),
        "diagonal distance does not accumulate"
    );
    assert!(!move_activated(3, 0), "below threshold remains a click");
    assert!(
        move_activated(5, 0),
        "motion beyond the threshold activates"
    );
    assert!(
        move_activated(i32::MIN, 0),
        "absolute comparisons do not overflow"
    );
}

#[test]
fn all_activated_move_origins_can_snap_and_restore_layout_geometry() {
    let floating = Rect::new(0, 0, 640, 420);
    let mut model = WindowModel::new(1);
    model.configure(true, 0);
    model.add(1, floating, 0);
    for mode in [Mode::Snapped(SnapTarget::LeftHalf), Mode::Maximized] {
        if mode == Mode::Maximized {
            model.maximize_to(1, Rect::new(0, 0, 1000, 700), floating);
        } else {
            model.snap_to(
                1,
                SnapTarget::LeftHalf,
                Rect::new(0, 0, 1000, 700),
                floating,
            );
        }
        let managed = model.get(1).unwrap().clone();
        let frame = managed.frame();
        let start_pointer = (f64::from(frame.x + frame.w / 2), f64::from(frame.y + 10));
        // XDG move requests and modifier moves both create non-titlebar Move grabs.
        let Grab::Move {
            start,
            start_mode,
            restore,
            anchor_num,
            anchor_den,
            anchor_y,
            ..
        } = Grab::move_window(1, start_pointer, &managed, false)
        else {
            unreachable!()
        };
        let restored = move_candidate(
            start,
            start_mode,
            restore,
            start_pointer,
            Point::<f64, Logical>::from((start_pointer.0 + 100.0, start_pointer.1 + 25.0)),
            anchor_num,
            anchor_den,
            anchor_y,
            true,
        );
        assert_eq!(
            restored,
            Rect::new(
                start_pointer.0 as i32 + 100 - floating.w / 2,
                start_pointer.1 as i32 + 25 - 10 + TITLEBAR_HEIGHT,
                floating.w,
                floating.h,
            ),
            "mode={mode:?}"
        );
    }

    // Ordinary floating drags remain free moves, including over a snap zone.
    let free = move_candidate(
        Rect::new(100, 80, 640, 420),
        Mode::Normal,
        Rect::new(100, 80, 640, 420),
        (300.0, 50.0),
        Point::<f64, Logical>::from((305.0, 55.0)),
        50,
        640,
        10,
        true,
    );
    assert_eq!(free, Rect::new(105, 85, 640, 420));
}

#[test]
fn restored_free_move_stays_quarter_visible_on_the_physical_output() {
    let output = Rect::new(0, 0, 1350, 900);
    let candidate = Rect::new(1169, -10_000, 880, 600);

    // The 880px frame must leave ceil(880 / 4) = 220px on the 1350px output.
    assert_eq!(
        clamp_move_candidate(candidate, output, false),
        Rect::new(1130, -450, 880, 600)
    );
    // With decorations, the 631px outer height needs 158px visible. Clamp that frame, then
    // convert the resulting origin and height back to client geometry exactly once.
    let decorated = clamp_move_candidate(candidate, output, true);
    assert_eq!(decorated, Rect::new(1130, -442, 880, 600));
}

#[test]
fn floating_move_snap_preserves_the_final_free_move_as_restore() {
    let start = Rect::new(120, 130, 640, 420);
    let pointer = (300.0, 150.0);
    let moved_pointer = Point::<f64, Logical>::from((335.0, 168.0));
    let candidate = move_candidate(
        start,
        Mode::Normal,
        start,
        pointer,
        moved_pointer,
        50,
        640,
        10,
        true,
    );
    assert_eq!(candidate, Rect::new(155, 148, 640, 420));

    let work = Rect::new(0, 0, 1000, 700);
    let mut model = WindowModel::new(1);
    model.add(1, start, 0);
    commit_move_snap(&mut model, 1, SnapTarget::Maximize, work, candidate);

    let maximized = model.get(1).unwrap();
    assert_eq!(maximized.mode, Mode::Maximized);
    assert_eq!(maximized.restore, candidate);
    model.toggle_maximize(1, work);
    let restored = model.get(1).unwrap();
    assert_eq!(restored.mode, Mode::Normal);
    assert_eq!(restored.rect, candidate);
    assert_ne!(restored.rect, start);
}

#[test]
fn activated_moves_from_any_mode_can_snap_but_clicks_cannot() {
    let output = flamewm_api::Rect::new(0, 0, 1280, 800);
    let work = flamewm_api::Rect::new(0, 0, 1280, 752);
    let left_bottom_corner = flamewm_api::Point::new(0, 799);
    // Titlebar drags, client-initiated moves and modifier moves all resolve via this same
    // activated-Move policy; the origin is deliberately not part of the snap gate.
    assert_eq!(
        snap_target_for_move(true, left_bottom_corner, output, work),
        SnapTarget::BottomLeftQuarter
    );
    assert_eq!(
        snap_target_for_move(true, flamewm_api::Point::new(15, 31), output, output,),
        SnapTarget::TopLeftQuarter
    );
    assert_eq!(
        snap_target_for_move(true, flamewm_api::Point::new(16, 31), output, output,),
        SnapTarget::Maximize
    );
    assert_eq!(
        snap_target_for_move(true, flamewm_api::Point::new(640, 32), output, output,),
        SnapTarget::None
    );
    assert_eq!(
        snap_target_for_move(true, flamewm_api::Point::new(640, 0), output, work,),
        SnapTarget::Maximize
    );
    assert_eq!(
        snap_target_for_move(true, flamewm_api::Point::new(1279, 799), output, work,),
        SnapTarget::BottomRightQuarter
    );
    assert_eq!(
        snap_target_for_move(true, flamewm_api::Point::new(640, 799), output, work,),
        SnapTarget::None,
        "bottom-center stays a free move"
    );
    assert_eq!(
        snap_target_for_move(false, left_bottom_corner, output, work),
        SnapTarget::None,
        "an unactivated click never snaps"
    );
}

#[test]
fn activated_fullscreen_move_snaps_using_saved_floating_geometry() {
    assert!(
        move_activated(4, 0),
        "inclusive threshold activates the Move"
    );
    let output = Rect::new(0, 0, 1280, 800);
    let work = Rect::new(0, 0, 1280, 752);
    let floating = Rect::new(120, 130, 640, 420);
    let mut model = WindowModel::new(1);
    model.configure(true, 0);
    model.add(1, floating, 0);
    model.set_fullscreen(1, output, true);
    let fullscreen = model.get(1).unwrap().clone();
    let start_pointer = (400.0, 20.0);
    let Grab::Move {
        start,
        start_mode,
        restore,
        anchor_num,
        anchor_den,
        anchor_y,
        ..
    } = Grab::move_window(1, start_pointer, &fullscreen, false)
    else {
        unreachable!()
    };
    let pointer = Point::<f64, Logical>::from((0.0, 799.0));
    let candidate = move_candidate(
        start,
        start_mode,
        restore,
        start_pointer,
        pointer,
        anchor_num,
        anchor_den,
        anchor_y,
        true,
    );
    let target = snap_target_for_move(
        true,
        flamewm_api::Point::new(pointer.x as i32, pointer.y as i32),
        flamewm_api::Rect::new(output.x, output.y, output.w, output.h),
        flamewm_api::Rect::new(work.x, work.y, work.w, work.h),
    );
    assert_eq!(target, SnapTarget::BottomLeftQuarter);
    assert_eq!((candidate.w, candidate.h), (floating.w, floating.h));

    model.snap_to(1, target, work, candidate);
    let snapped = model.get(1).unwrap();
    assert_eq!(snapped.mode, Mode::Snapped(target));
    assert_eq!(snapped.restore, candidate);
    assert_eq!(snapped.frame(), Rect::new(0, 376, 640, 376));
}

#[test]
fn unactivated_fullscreen_move_keeps_fullscreen_and_never_snaps() {
    let output = Rect::new(0, 0, 1280, 800);
    let floating = Rect::new(120, 130, 640, 420);
    let mut model = WindowModel::new(1);
    model.add(1, floating, 0);
    model.set_fullscreen(1, output, true);
    let start = model.get(1).unwrap().rect;
    let candidate = move_candidate(
        start,
        Mode::Fullscreen,
        floating,
        (400.0, 20.0),
        Point::<f64, Logical>::from((0.0, 799.0)),
        400,
        output.w,
        20,
        false,
    );
    assert_eq!(candidate, start, "a click does not leave fullscreen");
    assert_eq!(
        snap_target_for_move(
            false,
            flamewm_api::Point::new(0, 799),
            flamewm_api::Rect::new(0, 0, output.w, output.h),
            flamewm_api::Rect::new(0, 0, output.w, output.h),
        ),
        SnapTarget::None
    );
    assert_eq!(model.get(1).unwrap().mode, Mode::Fullscreen);
    assert_eq!(model.get(1).unwrap().rect, output);
    model.set_fullscreen(1, output, false);
    assert_eq!(model.get(1).unwrap().rect, floating);
}

/// The saved layout reaches the keymap, and a value outside the supported set falls back
/// instead of producing a keymap that would leave the session without a keyboard.
#[test]
fn keyboard_layout_reaches_the_xkb_config() {
    assert_eq!(xkb_config("de").layout, "de");
    assert_eq!(xkb_config("us").layout, "us");
    assert_eq!(
        xkb_config("not-a-layout").layout,
        flamewm_api::settings::DEFAULT_KEYBOARD_LAYOUT
    );
}

/// This is the discriminating scene-pick regression: the actual topmost Dock must win over
/// a Desktop covering the same pixels, or the shell taskbar never receives its button.
#[test]
fn foreground_dock_wins_over_desktop_in_button_pick_order() {
    #[derive(Debug, PartialEq, Eq)]
    enum Owner {
        Dock,
        Desktop,
    }
    let candidates =
        crate::scene::compose_front_to_back([], [Owner::Dock], [], [], [Owner::Desktop]);
    let winner = first_hit(candidates, Some).expect("both X11 surfaces cover the pointer");
    assert_eq!(winner, Owner::Dock);
}

/// A managed or unmanaged tree miss does not retain the geometry-only managed action: the
/// next actual scene owner wins instead.
#[test]
fn surface_tree_holes_fall_through_without_a_managed_action() {
    let lower = PointerTarget {
        focus: None,
        managed_hit: None,
        scene_hit: true,
    };
    let target = first_hit([None, Some(lower)], |target| target).unwrap();
    assert!(target.managed_hit.is_none());
    assert!(target.scene_hit);
}

/// Every surface class uses the same first-hit rule; a popup or top layer cannot expose an
/// underlying managed client, and managed decorations remain compositor-owned.
#[test]
fn higher_scene_surfaces_win_and_managed_decoration_is_not_client_input() {
    #[derive(Debug, PartialEq, Eq)]
    enum Owner {
        Popup,
        ManagedClient,
        TopLayer,
        BottomLayer,
    }
    assert_eq!(
        first_hit([Owner::Popup, Owner::ManagedClient], Some),
        Some(Owner::Popup)
    );
    assert_eq!(
        first_hit([Owner::TopLayer, Owner::BottomLayer], Some),
        Some(Owner::TopLayer)
    );
    assert_eq!(
        pointer_route(Some(Hit::Titlebar(4))),
        PointerRoute::Compositor
    );
    assert_eq!(pointer_route(Some(Hit::Client(4))), PointerRoute::Client(4));
}

/// A titlebar press must belong to the compositor, never to the desktop beneath it: the press
/// starts the window move, so routing it onward would also start the desktop's rubber band.
#[test]
fn decoration_pixels_never_route_to_the_desktop() {
    let id = 7u64;
    assert_eq!(
        pointer_route(Some(Hit::Titlebar(id))),
        PointerRoute::Compositor
    );
    assert_eq!(
        pointer_route(Some(Hit::Resize(id))),
        PointerRoute::Compositor
    );
    assert_eq!(
        pointer_route(Some(Hit::Button(id, Button::Close))),
        PointerRoute::Compositor
    );
    // The client area still reaches its own surface.
    assert_eq!(
        pointer_route(Some(Hit::Client(id))),
        PointerRoute::Client(id)
    );
    // Empty desktop space is the only case that falls through to the unmanaged stack.
    assert_eq!(pointer_route(None), PointerRoute::Unmanaged);
}

#[test]
fn chord_lookup_tries_the_unshifted_symbol_then_the_level_symbol() {
    // Plain `d`: both names agree.
    assert_eq!(
        name_candidates("d", "d").collect::<Vec<_>>(),
        vec!["d", "d"]
    );
    // Level-3 layout: `Ctrl+Alt+d` reports another keysym, the raw name still matches.
    assert_eq!(
        name_candidates("d", "eth").collect::<Vec<_>>(),
        vec!["d", "eth"]
    );
    // A dead key with no symbol never produces an empty lookup key.
    assert_eq!(name_candidates("", "d").collect::<Vec<_>>(), vec!["d"]);
}
/// A second click on the same title within the window is a double click; a slow one, a distant
/// one, or one on another window is two separate clicks.
#[test]
fn double_click_needs_the_same_window_spot_and_time() {
    let first = TitleClick {
        id: 4,
        time_ms: 1_000,
        pointer: (100.0, 50.0),
    };
    assert!(completes_double_click(Some(first), 4, 1_200, 101.0, 51.0));
    assert!(
        !completes_double_click(Some(first), 4, 1_600, 100.0, 50.0),
        "beyond the 500ms window"
    );
    assert!(
        !completes_double_click(Some(first), 4, 1_200, 140.0, 50.0),
        "beyond the movement slop"
    );
    assert!(
        !completes_double_click(Some(first), 9, 1_200, 100.0, 50.0),
        "a different window"
    );
    assert!(
        !completes_double_click(None, 4, 1_200, 100.0, 50.0),
        "there must be a first click"
    );
}
/// Mouse-driven focus is opt-in: only an explicit `focusFollowsMouse=true` turns it on, and an
/// unreadable file leaves it off rather than guessing.
#[test]
fn focus_follows_mouse_is_opt_in() {
    assert!(focus_follows_mouse_from("focusFollowsMouse=true\n"));
    assert!(!focus_follows_mouse_from("focusFollowsMouse=false\n"));
    assert!(!focus_follows_mouse_from(""));
    assert!(!focus_follows_mouse_from("not = [valid"));
}
/// The readers must reach a real file, not only a string: a settings file on disk is what the
/// compositor actually sees at startup.
#[test]
fn settings_readers_work_against_a_real_file() {
    let dir = std::env::temp_dir().join(format!("flamewm-wayland-settings-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.toml");
    std::fs::write(&path, "focusFollowsMouse=true\nkeyboardLayout=fr\n").unwrap();
    assert!(focus_follows_mouse_at(&path));
    assert_eq!(keyboard_layout_at(&path), "fr");

    // A missing file falls back instead of panicking: default layout, focus off.
    let missing = dir.join("absent.toml");
    assert!(!focus_follows_mouse_at(&missing));
    assert_eq!(
        keyboard_layout_at(&missing),
        flamewm_api::settings::DEFAULT_KEYBOARD_LAYOUT
    );

    std::fs::remove_dir_all(&dir).ok();
}

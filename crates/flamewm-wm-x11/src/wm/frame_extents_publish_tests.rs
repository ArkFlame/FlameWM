use super::*;

/// Mirrors `set_frame_extents`: a publish is sent only when the value differs.
fn publish_all(values: &[[u32; 4]]) -> usize {
    let mut published = HashMap::new();
    values
        .iter()
        .filter(|&&extents| {
            let needs = frame_extents_needs_publish(&published, 7, extents);
            if needs {
                published.insert(7, extents);
            }
            needs
        })
        .count()
}

#[test]
fn first_publish_always_sends_and_identical_repeats_are_suppressed() {
    let floating = [1, 1, 25, 1];
    let published: HashMap<Window, [u32; 4]> = HashMap::new();
    assert!(frame_extents_needs_publish(&published, 7, floating));
    assert_eq!(publish_all(&[floating, floating, floating, floating]), 1);
}

#[test]
fn any_change_republishes_including_border_mode_switch() {
    let floating = [1, 1, 25, 1];
    let snapped = [0, 0, 25, 0];
    let client_side = [0, 0, 0, 0];
    // Floating -> snapped (border 1 -> 0) -> floating -> client-side decoration.
    assert_eq!(publish_all(&[floating, snapped, floating, client_side]), 4);
    // Titlebar-only change must also be detected (no border-only shortcut).
    assert_eq!(publish_all(&[floating, [1, 1, 0, 1], [1, 1, 0, 1]]), 2);
}

/// Mirrors `publish_client_list`: returns how many (list, stacking) publishes are sent.
fn client_list_sends(snapshots: &[(Vec<Window>, Vec<Window>)]) -> usize {
    let mut published = None;
    snapshots
        .iter()
        .filter(|(list, stack)| {
            let needs = client_list_needs_publish(published.as_ref(), list, stack);
            if needs {
                published = Some((list.clone(), stack.clone()));
            }
            needs
        })
        .count()
}

#[test]
fn client_list_identical_repeats_skip_both_property_writes() {
    let same = (vec![1, 2], vec![2, 1]);
    // Burst of identical restack-driven publishes: only the first is sent.
    assert_eq!(
        client_list_sends(&[same.clone(), same.clone(), same.clone()]),
        1
    );
    // Stacking-only change (same managed list) and list-only change both republish.
    let restacked = (vec![1, 2], vec![1, 2]);
    let added = (vec![1, 2, 3], vec![1, 2, 3]);
    assert_eq!(
        client_list_sends(&[same.clone(), restacked, same, added]),
        4
    );
}

#[test]
fn frame_extents_follow_effective_border_per_mode() {
    let (border, titlebar) = (1, 31);
    let floating = [1, 1, 32, 1];
    let snapped = [0, 0, 31, 0];
    assert_eq!(
        frame_extents_for_mode(PlacementMode::Floating, border, titlebar, 0),
        floating
    );
    assert_eq!(
        frame_extents_for_mode(
            PlacementMode::Snapped(crate::frame::model::SnapTarget::Left),
            border,
            titlebar,
            0
        ),
        snapped
    );
    assert_eq!(
        frame_extents_for_mode(PlacementMode::Maximized, border, titlebar, 0),
        snapped
    );
    // Fullscreen has no chrome, even with a band: the client covers the whole output.
    assert_eq!(
        frame_extents_for_mode(PlacementMode::Fullscreen, border, titlebar, 2),
        [0, 0, 0, 0]
    );
    // Unsnap returns to the floating border and titlebar extents.
    assert_eq!(
        frame_extents_for_mode(PlacementMode::Floating, border, titlebar, 0),
        floating
    );
    // The outline band is part of every mode's left/right/bottom extents.
    assert_eq!(
        frame_extents_for_mode(PlacementMode::Floating, border, titlebar, 2),
        [3, 3, 32, 3]
    );
    assert_eq!(
        frame_extents_for_mode(
            PlacementMode::Snapped(crate::frame::model::SnapTarget::Left),
            border,
            titlebar,
            2
        ),
        [2, 2, 31, 2]
    );
}

/// Client interior per mode: the published EWMH side/bottom extents equal
/// the effective X border (the only side inset), and the client rect sits
/// at (0, titlebar) spanning the full frame width. No side inset leaks
/// into the client for snapped/maximized/fullscreen frames.
#[test]
fn client_interior_matches_effective_border_per_mode() {
    let outer = crate::frame::coords::RootRect {
        x: 10,
        y: 20,
        w: 800,
        h: 600,
    };
    let titlebar: u16 = 31;
    let configured: u16 = 1;
    let modes = [
        PlacementMode::Floating,
        PlacementMode::Snapped(crate::frame::model::SnapTarget::TopLeft),
        PlacementMode::Maximized,
        PlacementMode::Fullscreen,
    ];
    for mode in modes {
        let border = frame_border_for_mode(mode, configured);
        // Fullscreen has no titlebar either; every other mode keeps it.
        let titlebar = if matches!(mode, PlacementMode::Fullscreen) {
            0
        } else {
            titlebar
        };
        // No outline band: the client keeps the full frame width below the titlebar.
        let [left, right, top, bottom] = frame_extents_for_mode(mode, configured, titlebar, 0);
        assert_eq!(left, u32::from(border), "{mode:?} left extent");
        assert_eq!(right, u32::from(border), "{mode:?} right extent");
        assert_eq!(bottom, u32::from(border), "{mode:?} bottom extent");
        assert_eq!(
            top,
            u32::from(titlebar) + u32::from(border),
            "{mode:?} top extent"
        );
        let ext = FrameExtents::new(i32::from(titlebar), i32::from(border));
        let (x, y, w, h) = lifecycle::client_configure(outer, ext);
        assert_eq!((x, y), (0, i32::from(titlebar)), "{mode:?} client origin");
        assert_eq!(
            w, outer.w as u32,
            "{mode:?} client width equals frame width"
        );
        assert_eq!(
            h,
            (outer.h - i32::from(titlebar)) as u32,
            "{mode:?} client height"
        );
    }
    // Client-drawn side insets must never appear for borderless modes.
    let snapped = FrameExtents::new(i32::from(titlebar), 0);
    assert_eq!(lifecycle::client_configure(outer, snapped).2, 800);
}

/// Regression: the frame-band path places the client `t` px inside the frame on the left, right
/// and bottom edges, so the painted ring cannot be covered by the client (or by a shape the
/// application installs). The compositing-manager overlay path keeps the full client rect.
#[test]
fn outline_band_insets_the_client_rect_and_the_overlay_does_not() {
    let outer = crate::frame::coords::RootRect {
        x: 10,
        y: 20,
        w: 800,
        h: 600,
    };
    let titlebar: u16 = 31;
    let border: u16 = 1;
    let t: u16 = 2;
    // Frame-band path: origin (t, titlebar), size reduced by (2t, t).
    let band = FrameExtents::new(i32::from(titlebar), i32::from(border)).with_inset(i32::from(t));
    assert_eq!(band.client_offset(), (i32::from(t), i32::from(titlebar)));
    assert_eq!(
        band.delta(),
        (2 * i32::from(t), i32::from(titlebar) + i32::from(t))
    );
    let (x, y, w, h) = lifecycle::client_configure(outer, band);
    assert_eq!((x, y), (i32::from(t), i32::from(titlebar)));
    assert_eq!(w, outer.w as u32 - 2 * u32::from(t));
    assert_eq!(h, (outer.h - i32::from(titlebar) - i32::from(t)) as u32);
    // The published EWMH extents carry the band, so the application still gets the size it asked
    // for: the frame grows by exactly what the client lost.
    let [left, right, top, bottom] =
        frame_extents_for_mode(PlacementMode::Floating, border, titlebar, t);
    assert_eq!(
        (left, right, bottom),
        (
            u32::from(border) + u32::from(t),
            u32::from(border) + u32::from(t),
            u32::from(border) + u32::from(t)
        )
    );
    assert_eq!(top, u32::from(titlebar) + u32::from(border));
    let client_origin =
        crate::frame::coords::RootPoint::new(outer.x + i32::from(t), outer.y + i32::from(titlebar));
    assert_eq!(
        crate::frame::geometry::client_rect_to_frame(client_origin, (w, h), band),
        outer,
        "the client rect and the frame rect round-trip through the band"
    );
    // Overlay path: the ring is composited over the client, so the client keeps the full rect.
    let overlay = FrameExtents::new(i32::from(titlebar), i32::from(border));
    let (x, y, w, h) = lifecycle::client_configure(outer, overlay);
    assert_eq!((x, y), (0, i32::from(titlebar)));
    assert_eq!(
        (w, h),
        (outer.w as u32, (outer.h - i32::from(titlebar)) as u32)
    );
    assert_eq!(
        frame_extents_for_mode(PlacementMode::Floating, border, titlebar, 0),
        [1, 1, u32::from(titlebar) + 1, 1]
    );
}

/// The `ConfigureFrameRoot` arm commits the border/geometry, then publishes
/// the effective extents through the single owner, `set_frame_extents`.
#[test]
fn configure_frame_root_arm_publishes_extents_after_commit() {
    let src = WM_SOURCE;
    let start = src
        .find("FrameEffect::ConfigureFrameRoot { x, y, w, h } => {")
        .expect("ConfigureFrameRoot arm");
    let end = start
        + src[start..]
            .find("FrameEffect::MoveFrameRoot")
            .expect("arm following ConfigureFrameRoot");
    let arm = &src[start..end];
    let commit = arm
        .find("self.commit_frame_rect(client, x, y, w, h);")
        .expect("arm commits frame rect");
    let publish = arm
        .find("self.set_frame_extents(client)?;")
        .expect("arm publishes frame extents");
    assert!(
        publish > commit,
        "extents must publish after the rect is committed"
    );
}

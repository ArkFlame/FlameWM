//! Frame input regions: source-XID-is-truth event targeting.
//!
//! Pure value objects. No X calls, no x11rb.
//!
//! The wm registers each frame child window with its [`FrameRegion`].
//! [`target_for_xid`] looks the source XID up in that registry; a known
//! child keeps its registered region (identity, no geometric re-hit-test).
//! Unknown sources (including the client window) resolve to
//! [`FrameRegion::Client`].

use std::collections::HashMap;

use super::model::{FrameControl, FrameRegion};
use super::resources::FrameEventTarget;

/// Title-button hit target from skin `control_button_geometries`.
/// Hit rects derive from skin geometry; glyph pixels never affect them.
#[must_use]
pub fn frame_control_at(frame_width: u32, titlebar_height: u16, x: i16) -> Option<FrameControl> {
    use super::layout::{
        controls_width_for_titlebar, layout_frame_children, resize_thickness_for_titlebar,
    };
    let width = i32::try_from(frame_width).unwrap_or(i32::MAX);
    let titlebar_h = i32::from(titlebar_height);
    let rects = layout_frame_children(
        width,
        titlebar_h + resize_thickness_for_titlebar(titlebar_h),
        titlebar_h,
        controls_width_for_titlebar(titlebar_h),
        resize_thickness_for_titlebar(titlebar_h),
    )
    .controls;
    let x = i32::from(x);
    if x < 0 || x >= width {
        return None;
    }
    let roles = [
        FrameControl::Minimize,
        FrameControl::MaximizeRestore,
        FrameControl::Close,
    ];
    rects
        .into_iter()
        .zip(roles)
        .find_map(|(rect, role)| (x >= rect.x && x < rect.x + rect.w).then_some(role))
}

/// Resolve an event source XID via the registry map (value-side).
/// Known child XID -> its registered region (source XID is truth, no geometry);
/// anything else -> client.
#[must_use]
pub fn target_for_xid(
    registry: &HashMap<u32, FrameRegion>,
    client: u32,
    source: u32,
) -> FrameEventTarget {
    let region = registry
        .get(&source)
        .copied()
        .unwrap_or(FrameRegion::Client);
    FrameEventTarget { client, region }
}

/// Idle-hover fallback for events whose source XID is not a control child:
/// `frame_control_at` inside the titlebar band, else `None`.
#[must_use]
pub fn frame_control_fallback(
    frame_width: u32,
    titlebar_height: u16,
    x: i16,
    y: i16,
    _current: Option<FrameControl>,
) -> Option<FrameControl> {
    if titlebar_height == 0 {
        return None;
    }
    if y < 0 || y >= titlebar_height.max(1) as i16 {
        return None;
    }
    frame_control_at(frame_width, titlebar_height, x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::coords::{FrameRect, RootPoint, RootRect};
    use crate::frame::model::{FrameControl, ResizeEdges};

    const W: i32 = 400;
    const H: i32 = 300;

    fn test_registry() -> (HashMap<u32, FrameRegion>, u32) {
        let client = 20_u32;
        let mut registry = HashMap::new();
        for (index, edges) in crate::frame::resources::RESIZE_ORDER.iter().enumerate() {
            registry.insert(100 + index as u32, FrameRegion::Resize(*edges));
        }
        registry.insert(200, FrameRegion::TitleDrag);
        registry.insert(201, FrameRegion::Control(FrameControl::Minimize));
        registry.insert(202, FrameRegion::Control(FrameControl::MaximizeRestore));
        registry.insert(203, FrameRegion::Control(FrameControl::Close));
        (registry, client)
    }

    #[test]
    fn every_control_pixel_matches_native_child_at_100_135_200_percent() {
        use crate::frame::layout::{
            controls_width_for_titlebar, layout_frame_children, resize_thickness_for_titlebar,
        };
        for titlebar_h in [31_u16, 42, 62] {
            for width in [100_u32, 160, 400, 800] {
                let height = i32::from(titlebar_h);
                let layout = layout_frame_children(
                    width as i32,
                    300,
                    height,
                    controls_width_for_titlebar(height),
                    resize_thickness_for_titlebar(height),
                );
                for (index, expected) in [
                    FrameControl::Minimize,
                    FrameControl::MaximizeRestore,
                    FrameControl::Close,
                ]
                .into_iter()
                .enumerate()
                {
                    let rect = layout.controls[index];
                    for x in rect.x..rect.x + rect.w {
                        assert_eq!(
                            frame_control_at(width, titlebar_h, x as i16),
                            Some(expected),
                            "width={width} height={height} x={x}"
                        );
                    }
                }
                assert_eq!(frame_control_at(width, titlebar_h, -1), None);
                assert_eq!(frame_control_at(width, titlebar_h, width as i16), None);
                assert_eq!(
                    frame_control_fallback(
                        width,
                        titlebar_h,
                        width as i16 - 1,
                        titlebar_h as i16,
                        None
                    ),
                    None
                );
            }
        }
    }

    #[test]
    fn known_child_is_identity_no_rehittest() {
        let (registry, client) = test_registry();
        // Resize child xid wins even though the "point" conceptually sits
        // elsewhere: no geometric re-hit-test is consulted.
        let target = target_for_xid(&registry, client, 104);
        assert_eq!(
            target,
            FrameEventTarget {
                client,
                region: FrameRegion::Resize(ResizeEdges::top()),
            }
        );
        assert_eq!(
            target_for_xid(&registry, client, 203).region,
            FrameRegion::Control(FrameControl::Close)
        );
    }

    #[test]
    fn client_source_is_client_not_resize() {
        let (registry, client) = test_registry();
        // The client xid is not registered as a child: source != resize.
        assert!(!registry.contains_key(&client));
        let target = target_for_xid(&registry, client, client);
        assert_eq!(target.region, FrameRegion::Client);
        assert_eq!(target.client, client);
        // Unknown xid is also client.
        assert_eq!(
            target_for_xid(&registry, client, 9999).region,
            FrameRegion::Client
        );
    }

    #[test]
    fn frame_at_root_does_not_change_local_region_result() {
        // Registry resolution is XID-based, so moving the frame outer to
        // root (500,200) cannot change any local region result.
        let origin = RootPoint::new(500, 200);
        let root = RootRect::new(500, 200, W, H);
        let frame = root.to_frame(origin);
        assert_eq!(frame, FrameRect::new(0, 0, W, H));
        let (registry, client) = test_registry();
        let before = target_for_xid(&registry, client, 200);
        let after = target_for_xid(&registry, client, 200);
        assert_eq!(before, after);
        assert_eq!(before.region, FrameRegion::TitleDrag);
        assert_eq!(frame.to_root(origin), root);
    }

    #[test]
    fn eight_resize_zones_unique_via_registry() {
        let (registry, client) = test_registry();
        let mut seen = std::collections::HashSet::new();
        for xid in 100..108 {
            let target = target_for_xid(&registry, client, xid);
            let seen_key = match target.region {
                FrameRegion::Resize(edges) => (edges.left, edges.right, edges.top, edges.bottom),
                other => panic!("expected resize, got {other:?}"),
            };
            assert!(seen.insert(seen_key), "{seen_key:?} dup");
        }
        assert_eq!(seen.len(), 8);
    }
}

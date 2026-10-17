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

/// Title-button hit target from skin `control_button_geometries`.
/// Hit rects derive from skin geometry; glyph pixels never affect them.
#[must_use]
pub fn frame_control_at(frame_width: u32, titlebar_height: u16, x: i16) -> Option<FrameControl> {
    use flamewm_skin::recipes::window_chrome::{
        SceneRect, WindowControlRole, control_button_geometries,
    };
    let width = i32::try_from(frame_width).unwrap_or(i32::MAX);
    let titlebar = SceneRect::new(0, 0, width, i32::from(titlebar_height));
    let geometries = control_button_geometries(titlebar);
    let x = i32::from(x);
    for geometry in geometries.iter().rev() {
        if x >= geometry.bounds.x {
            return Some(match geometry.role {
                WindowControlRole::Minimize => FrameControl::Minimize,
                WindowControlRole::Close => FrameControl::Close,
                _ => FrameControl::MaximizeRestore,
            });
        }
    }
    None
}

/// Event target: client XID placeholder plus the resolved region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameEventTarget {
    pub client: u32,
    pub region: FrameRegion,
}

/// Identity for a known child region: source XID is truth, never geometry.
#[must_use]
pub fn region_for_known_child(region: FrameRegion) -> FrameRegion {
    region
}

/// Resolve an event source XID via the registry map (value-side).
/// Known child XID -> its registered region; anything else -> client.
#[must_use]
pub fn target_for_xid(
    registry: &HashMap<u32, FrameRegion>,
    client: u32,
    source: u32,
) -> FrameEventTarget {
    let region = registry
        .get(&source)
        .copied()
        .map_or(FrameRegion::Client, region_for_known_child);
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
            region_for_known_child(FrameRegion::TitleDrag),
            FrameRegion::TitleDrag
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

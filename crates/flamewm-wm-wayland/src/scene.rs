//! Shared front-to-back scene order for painting and pointer picking.
//!
//! This is deliberately a small traversal over the compositor's existing owners, not a second
//! scene graph. `render` paints these entries in order and `input` stops at the first hit.

use smithay::desktop::{LayerSurface, PopupManager, layer_map_for_output};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::model::WindowId;
use crate::state::State;

#[derive(Clone)]
pub(crate) enum SceneItem {
    Layer {
        surface: LayerSurface,
        origin: Point<i32, Logical>,
    },
    /// Index into `State::unmanaged`; keeping the owner in State avoids duplicating X11 handles.
    Unmanaged(usize),
    Popup {
        surface: WlSurface,
        origin: Point<i32, Logical>,
    },
    /// Includes the managed client surface and its non-overlapping compositor decoration band.
    Managed(WindowId),
}

/// Compose scene owners in the canonical front-to-back order. Kept separate from Smithay handles
/// so ordering regressions can be tested without a compositor/backend fixture.
pub(crate) fn compose_front_to_back<T>(
    overlay_top: impl IntoIterator<Item = T>,
    unmanaged_above: impl IntoIterator<Item = T>,
    managed: impl IntoIterator<Item = T>,
    bottom_background: impl IntoIterator<Item = T>,
    unmanaged_below: impl IntoIterator<Item = T>,
) -> Vec<T> {
    overlay_top
        .into_iter()
        .chain(unmanaged_above)
        .chain(managed)
        .chain(bottom_background)
        .chain(unmanaged_below)
        .collect()
}

/// Walk scene owners in the same front-to-back order used for both drawing and input.
pub(crate) fn front_to_back(state: &State) -> Vec<SceneItem> {
    let map = layer_map_for_output(&state.output);
    let layers: Vec<_> = map.layers().cloned().collect();
    let mut overlay_top = Vec::new();
    for group in [Layer::Overlay, Layer::Top] {
        for surface in layers.iter().rev().filter(|layer| layer.layer() == group) {
            if let Some(geometry) = map.layer_geometry(surface) {
                overlay_top.push(SceneItem::Layer {
                    surface: surface.clone(),
                    origin: geometry.loc,
                });
            }
        }
    }
    let bottom_background = layers
        .iter()
        .rev()
        .filter(|layer| matches!(layer.layer(), Layer::Bottom | Layer::Background))
        .filter_map(|surface| {
            map.layer_geometry(surface)
                .map(|geometry| SceneItem::Layer {
                    surface: surface.clone(),
                    origin: geometry.loc,
                })
        })
        .collect::<Vec<_>>();
    drop(map);

    let unmanaged_above = state
        .unmanaged
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, entry)| !entry.below)
        .map(|(index, _)| SceneItem::Unmanaged(index));
    let unmanaged_below = state
        .unmanaged
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, entry)| entry.below)
        .map(|(index, _)| SceneItem::Unmanaged(index));
    let mut managed_items = Vec::new();
    for id in state.model.visible_top_first() {
        let Some(surface) = state.window_wl_surface(id) else {
            continue;
        };
        let Some(managed) = state.model.get(id) else {
            continue;
        };
        for (popup, offset) in PopupManager::popups_for_surface(&surface) {
            managed_items.push(SceneItem::Popup {
                surface: popup.wl_surface().clone(),
                origin: Point::from((managed.rect.x + offset.x, managed.rect.y + offset.y)),
            });
        }
        managed_items.push(SceneItem::Managed(id));
    }

    compose_front_to_back(
        overlay_top,
        unmanaged_above,
        managed_items,
        bottom_background,
        unmanaged_below,
    )
}

#[cfg(test)]
mod tests {
    use super::compose_front_to_back;

    #[test]
    fn scene_stack_places_docks_popups_and_layers_above_the_surfaces_they_cover() {
        let order = compose_front_to_back(
            ["overlay", "top"],
            ["dock", "menu"],
            ["popup", "managed"],
            ["bottom", "background"],
            ["desktop"],
        );
        assert_eq!(
            order,
            [
                "overlay",
                "top",
                "dock",
                "menu",
                "popup",
                "managed",
                "bottom",
                "background",
                "desktop",
            ]
        );
        let dock = order.iter().position(|owner| *owner == "dock").unwrap();
        let desktop = order.iter().position(|owner| *owner == "desktop").unwrap();
        assert!(dock < desktop, "foreground dock must receive the first hit");
        let popup = order.iter().position(|owner| *owner == "popup").unwrap();
        let managed = order.iter().position(|owner| *owner == "managed").unwrap();
        assert!(
            popup < managed,
            "popup tree must win over its managed parent"
        );
        let top = order.iter().position(|owner| *owner == "top").unwrap();
        let bottom = order.iter().position(|owner| *owner == "bottom").unwrap();
        assert!(top < bottom, "Top layer must win over Bottom layer");
    }
}

//! Pointer input: motion, hit routing, focus-follows-mouse, and axis scrolling.

use super::*;

/// Pick the first owner that actually contains the point. The same helper makes front-to-back
/// semantics explicit in tests using foreground Dock/Desktop, popup, layer, and managed fixtures.
pub(super) fn first_hit<I, T>(
    items: impl IntoIterator<Item = I>,
    hit: impl FnMut(I) -> Option<T>,
) -> Option<T> {
    items.into_iter().find_map(hit)
}

#[derive(Default)]
pub(super) struct PointerTarget {
    pub(super) focus: Option<(WlSurface, Point<f64, Logical>)>,
    pub(super) managed_hit: Option<Hit>,
    /// True even when the scene owner is compositor-only and has no client surface to focus.
    pub(super) scene_hit: bool,
}

/// Which part of the scene owns the pixels under the pointer.
///
/// A managed window splits three ways: its client area belongs to the client surface, its
/// decoration belongs to the compositor, and neither may leak to a lower scene owner. The same
/// front-to-back traversal also prevents popups, layer surfaces and unmanaged X11 surfaces from
/// exposing a covered managed window to focus or button handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PointerRoute {
    /// A managed window's client area: forward to that window's surface.
    Client(crate::model::WindowId),
    /// The compositor's own decoration: no client surface receives the pointer.
    Compositor,
    /// Nothing managed here; fall through to docks, menus and the desktop.
    Unmanaged,
}

pub(super) fn pointer_route(hit: Option<Hit>) -> PointerRoute {
    match hit {
        Some(Hit::Client(id)) => PointerRoute::Client(id),
        Some(Hit::Titlebar(_) | Hit::Resize(_) | Hit::Button(_, _)) => PointerRoute::Compositor,
        None => PointerRoute::Unmanaged,
    }
}

/// Route to the actual surface in a client tree; a miss is an input-region/tree hole and lets
/// the front-to-back scene traversal consider the next owner.
pub(super) fn tree_focus_at(
    root: &WlSurface,
    location: Point<f64, Logical>,
    origin: Point<f64, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    under_from_surface_tree(root, location - origin, (0, 0), WindowSurfaceType::ALL).map(
        |(surface, offset)| {
            (
                surface,
                origin + Point::from((f64::from(offset.x), f64::from(offset.y))),
            )
        },
    )
}

/// Pointer focus on an unmanaged X11 window, only when its surface tree accepts the point.
pub(super) fn unmanaged_focus(
    entry: &crate::xwayland::Unmanaged,
    location: Point<f64, Logical>,
) -> Option<(WlSurface, Point<f64, Logical>)> {
    let geometry = entry.surface.geometry();
    let origin = geometry.loc.to_f64();
    let inside = location.x >= origin.x
        && location.y >= origin.y
        && location.x < origin.x + f64::from(geometry.size.w)
        && location.y < origin.y + f64::from(geometry.size.h);
    if !inside {
        return None;
    }
    let surface = entry.surface.wl_surface()?;
    tree_focus_at(&surface, location, origin)
}

impl State {
    pub(crate) fn on_motion(&mut self, location: Point<f64, Logical>, time: u32) {
        self.pointer = location;
        if let Some(grab) = self.grab {
            match grab {
                Grab::Move {
                    id,
                    pointer,
                    start,
                    start_mode,
                    restore,
                    anchor_num,
                    anchor_den,
                    anchor_y,
                    ..
                } => {
                    let dx = (location.x - pointer.0).round() as i32;
                    let dy = (location.y - pointer.1).round() as i32;
                    let activated = move_activated(dx, dy);
                    let floating = clamp_move_candidate(
                        move_candidate(
                            start, start_mode, restore, pointer, location, anchor_num, anchor_den,
                            anchor_y, activated,
                        ),
                        self.output_rect(),
                        self.model.titlebars(),
                    );
                    let target = snap_target_at(self, location, activated);
                    let frame = if target == SnapTarget::None {
                        frame_for_client(floating, self.model.titlebars())
                    } else {
                        self.model
                            .snap_client_geometry(id, target, self.work_area())
                            .map_or_else(
                                || frame_for_client(floating, self.model.titlebars()),
                                |client| frame_for_client(client, self.model.titlebars()),
                            )
                    };
                    self.move_outline = Some((id, frame));
                }
                Grab::Resize {
                    id,
                    pointer,
                    start,
                    edge,
                } => {
                    let dx = (location.x - pointer.0).round() as i32;
                    let dy = (location.y - pointer.1).round() as i32;
                    let rect = edge.resized(start, dx, dy);
                    // Sub-step pointer motion leaves the rectangle unchanged: no configure. A real change
                    // is deferred to the next paint, which flushes it once (see `flush_pending_configure`).
                    let unchanged = self.model.get(id).is_some_and(|m| m.rect == rect);
                    self.model.set_rect(id, rect);
                    if !unchanged {
                        self.pending_configure = Some(id);
                    }
                }
            }
        }
        let target = self.pointer_target(location);
        // `focusFollowsMouse` is off by default: without it, entry only updates hover and the
        // click path still owns focus. A drag never steals focus from the window it is carrying.
        if self.focus_follows_mouse && self.grab.is_none() {
            if let Some(Hit::Client(id) | Hit::Titlebar(id)) = target.managed_hit {
                self.focus_hover(id);
            }
        }
        self.update_pointer_focus(location, target.focus, time);
    }

    /// Synchronize Smithay's pointer focus before routing any pointer event. Calling motion while
    /// an implicit grab is active preserves that grab's delivery target, while stationary presses
    /// and axes still observe the current scene stack.
    pub(super) fn update_pointer_focus(
        &mut self,
        location: Point<f64, Logical>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        time: u32,
    ) {
        if let Some(pointer) = self.seat.get_pointer() {
            pointer.motion(
                self,
                focus,
                &MotionEvent {
                    location,
                    serial: next_serial(),
                    time,
                },
            );
            pointer.frame(self);
        }
    }

    /// Resolve the first front-to-back scene owner under the pointer. It supplies both the
    /// client surface focus and, only for a managed winner, its compositor hit classification.
    pub(super) fn pointer_target(&self, location: Point<f64, Logical>) -> PointerTarget {
        use crate::scene::SceneItem;

        first_hit(crate::scene::front_to_back(self), |item| match item {
            SceneItem::Layer { surface, origin } => {
                let origin = origin.to_f64();
                surface
                    .surface_under(location - origin, WindowSurfaceType::ALL)
                    .map(|(surface, offset)| PointerTarget {
                        focus: Some((
                            surface,
                            origin + Point::from((f64::from(offset.x), f64::from(offset.y))),
                        )),
                        managed_hit: None,
                        scene_hit: true,
                    })
            }
            SceneItem::Unmanaged(index) => self
                .unmanaged
                .get(index)
                .and_then(|entry| unmanaged_focus(entry, location))
                .map(|focus| PointerTarget {
                    focus: Some(focus),
                    managed_hit: None,
                    scene_hit: true,
                }),
            SceneItem::Popup { surface, origin } => {
                let origin = origin.to_f64();
                tree_focus_at(&surface, location, origin).map(|focus| PointerTarget {
                    focus: Some(focus),
                    managed_hit: None,
                    scene_hit: true,
                })
            }
            SceneItem::Managed(id) => {
                let hit = self.model.hit(location.x, location.y)?;
                let hit_id = match hit {
                    Hit::Button(window, _)
                    | Hit::Titlebar(window)
                    | Hit::Resize(window)
                    | Hit::Client(window) => window,
                };
                if hit_id != id {
                    return None;
                }
                match pointer_route(Some(hit)) {
                    PointerRoute::Client(id) => {
                        let rect = self.model.get(id)?.rect;
                        let origin = Point::from((f64::from(rect.x), f64::from(rect.y)));
                        let surface = self.window_wl_surface(id)?;
                        let focus = tree_focus_at(&surface, location, origin)?;
                        Some(PointerTarget {
                            focus: Some(focus),
                            managed_hit: Some(hit),
                            scene_hit: true,
                        })
                    }
                    // Decorations are an opaque compositor owner, not a reason to continue
                    // down the scene and focus a covered dock or desktop.
                    PointerRoute::Compositor => Some(PointerTarget {
                        focus: None,
                        managed_hit: Some(hit),
                        scene_hit: true,
                    }),
                    PointerRoute::Unmanaged => None,
                }
            }
        })
        .unwrap_or_default()
    }

    pub(super) fn on_axis<I: InputBackend>(&mut self, event: &impl PointerAxisEvent<I>) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
        for axis in [Axis::Horizontal, Axis::Vertical] {
            if let Some(amount) = event.amount(axis) {
                frame = frame.value(axis, amount);
            }
            if event.source() == AxisSource::Wheel {
                if let Some(steps) = event.amount_v120(axis) {
                    frame = frame.v120(axis, steps as i32);
                }
            }
        }
        let location = self.pointer;
        let target = self.pointer_target(location);
        self.update_pointer_focus(location, target.focus, event.time_msec());
        pointer.axis(self, frame);
        pointer.frame(self);
    }
}

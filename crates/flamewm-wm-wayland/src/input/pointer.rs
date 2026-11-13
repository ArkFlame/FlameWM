//! Pointer input: motion, hit routing, focus-follows-mouse, and axis scrolling.

use super::*;
use smithay::input::pointer::{CursorIcon, PointerHandle, RelativeMotionEvent};
use smithay::wayland::pointer_constraints::{PointerConstraint, with_pointer_constraint};

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

/// Pointer image for a compositor resize handle: the edge or corner arrow that names the drag.
pub(super) fn resize_cursor(edge: ResizeEdge) -> Option<CursorIcon> {
    Some(match (edge.left, edge.right, edge.top, edge.bottom) {
        (true, false, true, false) => CursorIcon::NwResize,
        (false, true, true, false) => CursorIcon::NeResize,
        (true, false, false, true) => CursorIcon::SwResize,
        (false, true, false, true) => CursorIcon::SeResize,
        (true, false, false, false) | (false, true, false, false) => CursorIcon::EwResize,
        (false, false, true, false) | (false, false, false, true) => CursorIcon::NsResize,
        _ => return None,
    })
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
        // Backends report the host or device position in `raw_pointer`; the cursor (`pointer`) may
        // stay put while a lock is active, so the relative delta is taken from the raw position.
        let delta = relative_delta(self.raw_pointer, location);
        self.raw_pointer = location;
        if let Some(surface) = self.locked_focus() {
            self.send_relative_motion(&surface, delta, time);
            return;
        }
        let location = self.confined_location(self.pointer, location);
        let primary = [self.output_rect()];
        let rects = crate::output_layout::pointer_layout(&self.pointer_layout, &primary);
        let (x, y) = crate::output_layout::clamp_point_to_layout(location.x, location.y, rects);
        let location = Point::from((x, y));
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
                    let titlebars = self.model.titlebars();
                    let target = snap_target_at(self, location, activated);
                    // `snapPreviewEnabled` off hides the zone ring only; release still snaps.
                    let zone =
                        (self.snap_preview_enabled && target != SnapTarget::None).then(|| {
                            self.model
                                .snap_client_geometry(id, target, self.pointer_work_area())
                                .map_or_else(
                                    || frame_for_client(floating, titlebars),
                                    |client| frame_for_client(client, titlebars),
                                )
                        });
                    let feedback = move_feedback(
                        self.show_content_while_dragging,
                        zone,
                        floating,
                        frame_for_client(floating, titlebars),
                    );
                    // Content mode moves the real window; outline mode leaves it until release.
                    let rect_changed = feedback.live.is_some_and(|rect| {
                        let unchanged = self.model.get(id).is_some_and(|m| m.rect == rect);
                        self.model.set_rect(id, rect);
                        !unchanged
                    });
                    if rect_changed {
                        self.refresh_window_outputs();
                        self.pending_configure = Some(id);
                    }
                    let ring_changed = match feedback.ring {
                        Some(ring) => self.snap_preview.show(ring),
                        None => self.snap_preview.hide(),
                    };
                    if needs_scene_rebuild(ring_changed, rect_changed) {
                        self.request_paint();
                    }
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
        let decor_cursor = match target.managed_hit {
            Some(Hit::Resize(id)) => self.model.get(id).and_then(|window| {
                resize_cursor(resize_edge_at(
                    window.frame(),
                    window.rect,
                    location.x,
                    location.y,
                ))
            }),
            _ => None,
        };
        if self.decor_cursor != decor_cursor {
            self.decor_cursor = decor_cursor;
            self.refresh_cursor_status();
        }
        // `focusFollowsMouse` is off by default: without it, entry only updates hover and the
        // click path still owns focus. A drag never steals focus from the window it is carrying.
        if self.focus_follows_mouse
            && self.grab.is_none()
            && let Some(Hit::Client(id) | Hit::Titlebar(id)) = target.managed_hit
        {
            self.focus_hover(id);
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
            let previous = pointer.current_focus();
            let next = focus.clone();
            pointer.motion(
                self,
                focus,
                &MotionEvent {
                    location,
                    serial: next_serial(),
                    time,
                },
            );
            self.sync_pointer_constraints(&pointer, previous, next.as_ref(), location);
            pointer.frame(self);
        }
    }

    /// The focused surface when its pointer lock is active: motion then stays relative only.
    /// Origin for the next device-relative move. Only a lock keeps the cursor still while the device
    /// keeps moving, so only then is the raw device position the base; otherwise the cursor is.
    #[cfg_attr(not(feature = "drm"), allow(dead_code))]
    pub(crate) fn relative_base(&self) -> Point<f64, Logical> {
        pick_relative_base(
            self.locked_focus().is_some(),
            self.pointer,
            self.raw_pointer,
        )
    }

    fn locked_focus(&self) -> Option<WlSurface> {
        let pointer = self.seat.get_pointer()?;
        let surface = pointer.current_focus()?;
        let locked = with_pointer_constraint(&surface, &pointer, |c| {
            c.is_some_and(|c| c.is_active() && matches!(*c, PointerConstraint::Locked(_)))
        });
        locked.then_some(surface)
    }

    fn send_relative_motion(&mut self, surface: &WlSurface, delta: Point<f64, Logical>, time: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        // The origin of the focus is not used by the relative path; the cursor location stands in.
        let focus = Some((surface.clone(), self.pointer));
        let event = RelativeMotionEvent {
            delta,
            // No pointer acceleration is applied on this path, so both vectors are the same delta.
            delta_unaccel: delta,
            utime: u64::from(time) * 1000,
        };
        pointer.relative_motion(self, focus, &event);
        pointer.frame(self);
    }

    /// Clamps a candidate position for a confined pointer: it keeps `previous` when the candidate
    /// would leave the confining region (or the focused surface).
    fn confined_location(
        &self,
        previous: Point<f64, Logical>,
        candidate: Point<f64, Logical>,
    ) -> Point<f64, Logical> {
        let Some(pointer) = self.seat.get_pointer() else {
            return candidate;
        };
        let Some(surface) = pointer.current_focus() else {
            return candidate;
        };
        let confined = with_pointer_constraint(&surface, &pointer, |c| {
            c.is_some_and(|c| c.is_active() && matches!(*c, PointerConstraint::Confined(_)))
        });
        if !confined {
            return candidate;
        }
        let inside = match self.pointer_target(candidate).focus {
            Some((hit, origin)) if hit == surface => {
                let local = candidate - origin;
                let contains = with_pointer_constraint(&surface, &pointer, |c| {
                    c.and_then(|c| c.region().map(|r| region_hit(r, local)))
                });
                constraint_wanted(true, contains)
            }
            _ => false,
        };
        confined_step(previous, candidate, inside)
    }

    /// Activates or deactivates the pointer constraints of the surfaces the pointer left or entered.
    /// Smithay sends `locked`/`confined` only when asked, so the compositor owns this policy.
    fn sync_pointer_constraints(
        &self,
        pointer: &PointerHandle<Self>,
        previous: Option<WlSurface>,
        next: Option<&(WlSurface, Point<f64, Logical>)>,
        location: Point<f64, Logical>,
    ) {
        if let Some(old) = previous.filter(|old| next.is_none_or(|(new, _)| new != old)) {
            with_pointer_constraint(&old, pointer, |c| {
                if let Some(c) = c.filter(|c| c.is_active()) {
                    c.deactivate();
                }
            });
        }
        let Some((surface, origin)) = next else {
            return;
        };
        let local = location - *origin;
        let Some((active, contains)) = with_pointer_constraint(surface, pointer, |c| {
            c.map(|c| (c.is_active(), c.region().map(|r| region_hit(r, local))))
        }) else {
            return;
        };
        match constraint_transition(active, constraint_wanted(true, contains)) {
            Some(true) => with_pointer_constraint(surface, pointer, |c| {
                if let Some(c) = c {
                    c.activate();
                }
            }),
            Some(false) => with_pointer_constraint(surface, pointer, |c| {
                if let Some(c) = c {
                    c.deactivate();
                }
            }),
            None => {}
        }
    }

    /// Resolve the first front-to-back scene owner under the pointer. It supplies both the
    /// client surface focus and, only for a managed winner, its compositor hit classification.
    pub(super) fn pointer_target(&self, location: Point<f64, Logical>) -> PointerTarget {
        use crate::scene::SceneItem;

        if !crate::session_lock::windows_reachable(self.session_locked) {
            // Locked: the pointer reaches the lock surface alone. Windows behind the lock get no
            // focus, decoration hits (close, maximize) or move/resize grabs.
            return PointerTarget {
                focus: self
                    .lock_surface()
                    .map(|lock| (lock.wl_surface().clone(), Point::from((0.0, 0.0)))),
                managed_hit: None,
                scene_hit: true,
            };
        }

        // Hit lookups resolve against the placed row the pointer is clamped to: a point on no
        // output (a gap or past the row) has no scene owner. Single output keeps the open lookup.
        if !crate::output_layout::hit_on_layout(&self.pointer_layout, location.x, location.y) {
            return PointerTarget::default();
        }

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
            if event.source() == AxisSource::Wheel
                && let Some(steps) = event.amount_v120(axis)
            {
                frame = frame.v120(axis, steps as i32);
            }
        }
        let location = self.pointer;
        let target = self.pointer_target(location);
        self.update_pointer_focus(location, target.focus, event.time_msec());
        pointer.axis(self, frame);
        pointer.frame(self);
    }
}

/// A constraint is wanted while the pointer is over its surface and, when it has a region, inside.
pub(super) fn constraint_wanted(over_surface: bool, region_contains: Option<bool>) -> bool {
    over_surface && region_contains.unwrap_or(true)
}

/// The activation change a constraint needs, or `None` when it already matches the wanted state.
pub(super) fn constraint_transition(active: bool, wanted: bool) -> Option<bool> {
    (active != wanted).then_some(wanted)
}

/// A confined pointer keeps its previous position when the candidate would leave the region.
pub(super) fn confined_step(
    previous: Point<f64, Logical>,
    candidate: Point<f64, Logical>,
    inside: bool,
) -> Point<f64, Logical> {
    if inside { candidate } else { previous }
}

/// The base of a device-relative move: the raw position when locked, the cursor otherwise.
#[cfg_attr(not(feature = "drm"), allow(dead_code))]
pub(super) fn pick_relative_base(
    locked: bool,
    pointer: Point<f64, Logical>,
    raw: Point<f64, Logical>,
) -> Point<f64, Logical> {
    if locked { raw } else { pointer }
}

/// Relative motion is the raw position change; with no acceleration the unaccelerated delta matches it.
pub(super) fn relative_delta(
    raw_previous: Point<f64, Logical>,
    raw_next: Point<f64, Logical>,
) -> Point<f64, Logical> {
    raw_next - raw_previous
}

/// Surface-local region test; the protocol region is in integer surface coordinates.
fn region_hit(
    region: &smithay::wayland::compositor::RegionAttributes,
    local: Point<f64, Logical>,
) -> bool {
    region.contains(Point::<i32, Logical>::from((
        local.x.round() as i32,
        local.y.round() as i32,
    )))
}

#[cfg(test)]
mod pointer_constraint_tests {
    use super::*;

    #[test]
    fn wanted_requires_surface_and_region() {
        assert!(constraint_wanted(true, None));
        assert!(constraint_wanted(true, Some(true)));
        assert!(!constraint_wanted(true, Some(false)));
        assert!(!constraint_wanted(false, None));
        assert!(!constraint_wanted(false, Some(true)));
    }

    #[test]
    fn transition_only_when_state_differs() {
        assert_eq!(constraint_transition(false, true), Some(true));
        assert_eq!(constraint_transition(true, false), Some(false));
        assert_eq!(constraint_transition(true, true), None);
        assert_eq!(constraint_transition(false, false), None);
    }

    #[test]
    fn confined_keeps_previous_position_outside() {
        let previous = Point::<f64, Logical>::from((10.0, 10.0));
        let candidate = Point::<f64, Logical>::from((500.0, 10.0));
        assert_eq!(confined_step(previous, candidate, true), candidate);
        assert_eq!(confined_step(previous, candidate, false), previous);
    }

    #[test]
    fn relative_base_is_raw_only_when_locked() {
        let pointer = Point::<f64, Logical>::from((10.0, 10.0));
        let raw = Point::<f64, Logical>::from((50.0, 60.0));
        assert_eq!(pick_relative_base(false, pointer, raw), pointer);
        assert_eq!(pick_relative_base(true, pointer, raw), raw);
        // Without any constraint the raw position equals the cursor, so both bases agree.
        assert_eq!(
            pick_relative_base(false, pointer, pointer),
            pick_relative_base(true, pointer, pointer)
        );
    }

    #[test]
    fn relative_delta_is_raw_change() {
        let a = Point::<f64, Logical>::from((100.0, 40.0));
        let b = Point::<f64, Logical>::from((103.5, 37.0));
        assert_eq!(relative_delta(a, b), Point::from((3.5, -3.0)));
        assert_eq!(relative_delta(b, b), Point::from((0.0, 0.0)));
    }
}

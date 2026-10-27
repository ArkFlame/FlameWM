//! X side of the frame shadow (pure shading and planning live in `frame::shadow`).
//!
//! Per floating frame: one override-redirect, input-transparent window stacked
//! below every managed frame. It holds the desktop under it darkened in place,
//! so exposes repaint without WM work. Pixels are refreshed only on map,
//! resize, focus change and hide.
//!
//! The pixels come from the frame the desktop surface presents (its backbuffer
//! pixmap, published on the root), never from the root: a root read held other
//! windows and unpainted areas as they were at that instant, and baked them into
//! the shadow as stale ghosts and dark marks. The shadow sits below every frame,
//! so the desktop is all it can ever show. No server grab and no unmap is needed.
//!
//! Every reactor turn asks `frame::shadow::plan_shadow` what each frame needs:
//!
//! * `Noop` — nothing changed (the common case, including every motion step of a
//!   frame nobody is dragging).
//! * `Move` — the window slides with its frame, `configure_window` only.
//! * `Reshade` — one `get_image` of the desktop frame pixmap.
//!
//! A live move/resize never re-shades *anything*: every frame is planned with
//! `gesture_active`, so the dragged frame's shadow just slides and its neighbours stay
//! untouched. The release advances the generation once, so the next idle turn is a single
//! re-shade pass over the frames (one per frame, never per motion step). The same holds while
//! a window open/close flight runs, so an animation costs shadow configures, not readbacks.
//! Staleness behind a *resting* frame is caught by a root generation that advances only
//! on wallpaper/output-layout changes, map/unmap/workspace changes, settled gestures and
//! (when idle) managed geometry changes — never on a motion step.

use super::*;
use crate::frame::shadow::{self, ShadowAction, ShadowState};
use crate::frame::shadow_rubber;
use crate::geometry::Rect;
use flamewm_api::root_rubber::RootRubber;
use std::hash::{Hash, Hasher};
use x11rb::protocol::shape::{SK as ShapeSk, SO as ShapeOp};

/// Resolve the bottom managed frame once per reconcile batch. A failed X11 query retains the
/// previous per-frame fallback (`own`) and permits the next frame to retry the query.
fn cached_shadow_sibling(
    cached: &mut Option<Window>,
    own: Window,
    query: impl FnOnce() -> Option<Window>,
) -> Window {
    if let Some(bottom) = *cached {
        return bottom;
    }
    match query() {
        Some(bottom) => {
            *cached = Some(bottom);
            bottom
        }
        None => own,
    }
}

#[cfg(test)]
mod reconcile_batch_tests {
    use super::cached_shadow_sibling;
    use std::cell::Cell;

    #[test]
    fn successful_tree_lookup_is_shared_but_failure_preserves_per_frame_fallback() {
        let count = Cell::new(0);
        let mut cached = None;
        let query = || {
            count.set(count.get() + 1);
            Some(42)
        };
        assert_eq!(cached_shadow_sibling(&mut cached, 10, query), 42);
        assert_eq!(cached_shadow_sibling(&mut cached, 20, query), 42);
        assert_eq!(count.get(), 1);
        let mut failed = None;
        assert_eq!(cached_shadow_sibling(&mut failed, 10, || None), 10);
        assert_eq!(cached_shadow_sibling(&mut failed, 20, || None), 20);
        assert_eq!(failed, None);
    }
}

/// Shape is expressed in shadow-window coordinates, so position and colour changes leave
/// the X Shape mask unchanged. A new window or different dimensions require the mask again.
fn needs_shadow_shape_update(previous: Option<(Window, u32, u32)>, win: Window, outer: Rect) -> bool {
    previous.is_none_or(|(old_win, width, height)| {
        old_win != win || width != outer.width || height != outer.height
    })
}

#[cfg(test)]
mod shadow_shape_tests {
    use super::{needs_shadow_shape_update, Rect};

    #[test]
    fn only_size_or_shadow_window_identity_invalidates_shape() {
        let old = Rect::new(100, 200, 1280, 720);
        assert!(needs_shadow_shape_update(None, 7, old));
        assert!(!needs_shadow_shape_update(Some((7, 1280, 720)), 7, old));
        assert!(!needs_shadow_shape_update(
            Some((7, 1280, 720)), 7, Rect::new(-1920, -500, 1280, 720)
        ));
        assert!(needs_shadow_shape_update(Some((7, 1280, 720)), 8, old));
        assert!(needs_shadow_shape_update(
            Some((7, 1280, 720)), 7, Rect::new(100, 200, 1281, 720)
        ));
        assert!(needs_shadow_shape_update(
            Some((7, 1280, 720)), 7, Rect::new(100, 200, 1280, 721)
        ));
    }
}

/// One shadow window of a frame and what it currently shows.
pub(super) struct FrameShadow {
    win: Window,
    radius: u32,
    state: ShadowState,
    /// Pixmap behind `win`'s background. Kept (not freed after use) so a live desktop rubber band
    /// can be recomposed into it without a readback.
    pixmap: Pixmap,
    /// Unshaded root snapshot of `shadow_rect(state.shaded)`, BGRX.
    raw: shadow_rubber::ShadowBands,
}

/// One frame the current turn wants shadowed.
struct WantedShadow {
    frame: Window,
    outer: Rect,
    focused: bool,
}

/// Cross-frame shadow bookkeeping. Only `generation` reaches the pure planner; the rest
/// decides when it advances.
#[derive(Default)]
pub(super) struct ShadowScene {
    generation: u64,
    /// Visible managed set (map/unmap/workspace) at the last reconcile.
    membership: u64,
    /// Wallpaper source + output layout at the last reconcile.
    backdrop: u64,
    /// Managed geometry at the last idle reconcile. Untouched while a gesture runs, so the
    /// first idle turn after it always differs and re-shades once.
    geometry: Option<u64>,
    /// Clients owning an activated move/resize at the last reconcile (sorted).
    dragging: Vec<Window>,
    // Retain temporary capacity across reactor turns instead of allocating one list per wake.
    // Both drag buffers are swapped after comparing settled gestures.
    next_dragging: Vec<Window>,
    wanted_scratch: Vec<WantedShadow>,
    membership_scratch: Vec<(Window, usize, bool)>,
    geometry_scratch: Vec<(Window, i32, i32, u32, u32)>,
    outputs_scratch: Vec<(i32, i32, u32, u32)>,
    seeded: bool,
    /// Live desktop rubber band currently composed into the shadows (root property mirror).
    rubber: Option<RootRubber>,
    /// Presented desktop frame (root property mirror); `None` when the desktop publishes none
    /// (no shadows then).
    desktop: Option<DesktopFrame>,
    /// `desktop` matches the property; cleared by its `PropertyNotify`.
    pub(super) desktop_read: bool,
}

/// The desktop surface's presented frame: its backbuffer pixmap, the window's root rect and the
/// present serial. Every present changes the serial, so a read that raced a repaint is redone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct DesktopFrame {
    pixmap: Pixmap,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    serial: u32,
}

impl DesktopFrame {
    /// Parses `flamewm_render_x11::backdrop::DESKTOP_FRAME_PROPERTY`; `None` when malformed.
    fn from_cardinals(data: &[u32]) -> Option<Self> {
        let [pixmap, x, y, width, height, serial] = *data else {
            return None;
        };
        (pixmap != 0 && width > 0 && height > 0).then_some(Self {
            pixmap,
            x: x as i32,
            y: y as i32,
            width,
            height,
            serial,
        })
    }
}

impl ShadowScene {
    /// Advance the root generation for this turn and return its new value.
    fn advance(
        &mut self,
        membership: u64,
        backdrop: u64,
        geometry: Option<u64>,
    ) -> u64 {
        self.next_dragging.sort_unstable();
        self.next_dragging.dedup();
        let mut bump = !self.seeded;
        self.seeded = true;
        if self.backdrop != backdrop {
            self.backdrop = backdrop;
            bump = true;
        }
        if self.membership != membership {
            self.membership = membership;
            bump = true;
        }
        // A window that travelled over the root leaves different pixels behind once it settles.
        if self
            .dragging
            .iter()
            .any(|client| !self.next_dragging.contains(client))
        {
            bump = true;
        }
        std::mem::swap(&mut self.dragging, &mut self.next_dragging);
        self.next_dragging.clear();
        if let Some(geometry) = geometry {
            if self.geometry != Some(geometry) {
                // The very first idle turn only seeds the baseline.
                if self.geometry.is_some() {
                    bump = true;
                }
                self.geometry = Some(geometry);
            }
        }
        if bump {
            self.generation = self.generation.wrapping_add(1);
        }
        self.generation
    }
}

fn hash_of<T: Hash>(value: T) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod scratch_equivalence_tests {
    use super::{ShadowScene, hash_of};

    #[test]
    fn borrowed_reused_vectors_hash_identically_to_legacy_owned_snapshots() {
        let membership = vec![(9_u32, 1_usize, false), (12, 2, true)];
        let geometry = vec![(9_u32, -100_i32, 40_i32, 1600_u32, 900_u32)];
        let outputs = vec![(-1920_i32, 0_i32, 1920_u32, 1080_u32), (0, 0, 1280, 800)];
        assert_eq!(hash_of((2_usize, membership.clone())), hash_of((2_usize, &membership)));
        assert_eq!(hash_of((0_usize, geometry.clone())), hash_of((0_usize, &geometry)));
        assert_eq!(
            hash_of((4_u64, 1920_u16, 1080_u16, outputs.clone(), Some(3_u32))),
            hash_of((4_u64, 1920_u16, 1080_u16, &outputs, Some(3_u32))),
        );
    }

    #[test]
    fn swapping_reusable_drag_buffers_preserves_settle_generation_semantics() {
        let mut scene = ShadowScene::default();
        let g0 = scene.advance(11, 22, Some(33));
        assert_eq!(scene.advance(11, 22, Some(33)), g0);

        scene.next_dragging.extend([15, 5, 15]);
        let start = scene.advance(11, 22, None);
        assert_eq!(start, g0, "beginning a gesture is not a settled redraw");
        assert_eq!(scene.dragging, vec![5, 15]);
        assert!(scene.next_dragging.is_empty());
        let capacity = scene.next_dragging.capacity();

        scene.next_dragging.extend([5, 15]);
        assert_eq!(scene.advance(11, 22, None), start);
        scene.next_dragging.push(15);
        let first_settled = scene.advance(11, 22, None);
        assert_eq!(
            first_settled,
            start.wrapping_add(1),
            "a frame leaving the gesture re-shades"
        );
        let settled = scene.advance(11, 22, Some(33));
        assert_eq!(settled, first_settled.wrapping_add(1));
        assert!(scene.dragging.is_empty());
        assert!(scene.next_dragging.is_empty());
        assert!(scene.next_dragging.capacity() >= capacity);
        assert_eq!(scene.advance(11, 22, Some(33)), settled);
        assert_eq!(scene.advance(11, 23, Some(33)), settled.wrapping_add(1));
    }
}

fn shadow_geometry(rect: Rect) -> ConfigureWindowAux {
    ConfigureWindowAux::new()
        .x(rect.x)
        .y(rect.y)
        .width(rect.width.max(1))
        .height(rect.height.max(1))
}

fn shadow_aux(rect: Rect, sibling: Window) -> ConfigureWindowAux {
    shadow_geometry(rect)
        .sibling(sibling)
        .stack_mode(StackMode::BELOW)
}

impl<C: Connection> Wm<'_, C> {
    /// Bring shadow windows in line with the current frames. Called once per
    /// reactor turn; cheap when nothing changed.
    pub(super) fn reconcile_frame_shadows(&mut self) -> Result<(), ReplyOrIdError> {
        let usable = self.decoration_tokens.window_shadows
            && self.screen().root_depth == 24
            && self.conn.setup().image_byte_order == ImageOrder::LSB_FIRST
            && self.desktop_frame()?.is_some();
        // Local ownership permits native mutations below; return the allocation to the scene
        // after the batch so ordinary pointer and provider wakes do not reallocate per frame.
        let mut wanted = std::mem::take(&mut self.shadow_scene.wanted_scratch);
        wanted.clear();
        if usable {
            wanted.extend(
                self.clients
                    .values()
                    .filter(|c| c.is_floating() && c.visible_on(self.current_workspace))
                    .map(|c| WantedShadow {
                        frame: c.frame,
                        outer: c.outer,
                        focused: self.active == Some(c.client),
                    }),
            );
        }
        let gone: Vec<Window> = self
            .frame_shadows
            .keys()
            .copied()
            .filter(|frame| !wanted.iter().any(|w| w.frame == *frame))
            .collect();
        for frame in gone {
            if let Some(shadow) = self.frame_shadows.remove(&frame) {
                self.conn.destroy_window(shadow.win)?;
                self.conn.free_pixmap(shadow.pixmap)?;
            }
        }
        // With no eligible frames and no retained shadows, nothing depends on the current
        // generation. Seed it on the first frame instead of allocating/sorting three separate
        // state snapshots for every pointer, input, or provider wake in an empty/fullscreen desk.
        if wanted.is_empty() && self.frame_shadows.is_empty() {
            self.shadow_scene.wanted_scratch = wanted;
            return Ok(());
        }
        let (generation, motion) = self.advance_shadow_generation();
        // Re-shading a batch must not synchronously query the same X11 stacking tree once per
        // frame. Managed frames cannot change stacking during this synchronous reconcile pass.
        let mut bottom_frame = None;
        let current_radius = u32::from(crate::frame::geometry_shape::scaled_corner_radius(
            true, self.decoration_tokens.style, self.titlebar_height(),
        ));
        for want in &wanted {
            let prev = self.frame_shadows.get(&want.frame).map(|held| held.state);
            // A frame first seen while frames are still moving waits for the settle: shading it
            // now would need the root readback a live move must never do.
            if prev.is_none() && motion {
                continue;
            }
            let shape_changed = self.frame_shadows.get(&want.frame)
                .is_some_and(|held| held.radius != current_radius);
            let decision = if shape_changed && !motion {
                ShadowAction::Reshade
            } else {
                shadow::plan_shadow(prev, want.outer, want.focused, motion, generation)
            };
            match decision {
                ShadowAction::Noop => {}
                ShadowAction::Move => self.move_frame_shadow(want.frame, want.outer)?,
                ShadowAction::Reshade => {
                    // A readback now would bake the live rubber into the snapshot. The frame keeps
                    // its state and is re-shaded by the first turn after the rubber ends.
                    if self.shadow_scene.rubber.is_some() {
                        continue;
                    }
                    let win = match self.frame_shadows.get(&want.frame) {
                        Some(held) => held.win,
                        None => self.create_shadow_window()?,
                    };
                    let sibling = cached_shadow_sibling(&mut bottom_frame, want.frame, || {
                        self.shadow_sibling()
                    });
                    self.refresh_frame_shadow(
                        want.frame,
                        want.outer,
                        want.focused,
                        win,
                        generation,
                        sibling,
                    )?;
                }
            }
        }
        self.shadow_scene.wanted_scratch = wanted;
        Ok(())
    }

    /// Advance the root generation: bump it when the root behind some strip may have changed.
    /// Also reports whether frames are still moving this turn (a gesture or a flight), in which
    /// case nothing may re-shade.
    fn advance_shadow_generation(&mut self) -> (u64, bool) {
        self.shadow_scene.next_dragging.clear();
        self.shadow_scene.next_dragging.extend(
            self.sessions
                .iter()
                .filter(|(_, state)| drag_activated(&state.session))
                .map(|(client, _)| *client),
        );
        // The same "something is flying" predicate `transition_wait` uses to cap the reactor
        // wait: geometry is only meaningful once both are quiet.
        let motion = !self.shadow_scene.next_dragging.is_empty() || self.animator.is_active();
        let membership = self.shadow_membership_hash();
        let backdrop = self.shadow_backdrop_hash();
        // Geometry is only comparable between two quiet turns: a dragged or animating frame
        // changes every step and must not invalidate the others.
        let geometry = if motion {
            None
        } else {
            Some(self.shadow_geometry_hash())
        };
        let generation = self
            .shadow_scene
            .advance(membership, backdrop, geometry);
        (generation, motion)
    }

    /// Which windows the root currently shows: a map/unmap/workspace change re-shades.
    fn shadow_membership_hash(&mut self) -> u64 {
        let items = &mut self.shadow_scene.membership_scratch;
        items.clear();
        items.extend(
            self.clients
                .values()
                .filter(|c| c.visible_on(self.current_workspace))
                .map(|c| (c.frame, c.workspace, c.sticky)),
        );
        items.sort_unstable();
        hash_of((self.current_workspace, &*items))
    }

    /// Wallpaper source plus output layout: a change invalidates every strip's pixels.
    fn shadow_backdrop_hash(&mut self) -> u64 {
        let (screen_width, screen_height) = {
            let screen = self.screen();
            (screen.width_in_pixels, screen.height_in_pixels)
        };
        let outputs = &mut self.shadow_scene.outputs_scratch;
        outputs.clear();
        outputs.extend(self.output_work_areas.iter().map(|output| {
            let rect = output.geometry;
            (rect.x, rect.y, rect.width, rect.height)
        }));
        hash_of((
            self.decoration_tokens.backdrop_revision,
            screen_width,
            screen_height,
            &*outputs,
            self.shadow_scene.desktop,
        ))
    }

    /// Managed geometry while idle: a window that moved (or settled) re-shades the rest once.
    fn shadow_geometry_hash(&mut self) -> u64 {
        let items = &mut self.shadow_scene.geometry_scratch;
        items.clear();
        items.extend(
            self.clients
                .values()
                .filter(|c| c.visible_on(self.current_workspace))
                .map(|c| (c.frame, c.outer.x, c.outer.y, c.outer.width, c.outer.height)),
        );
        items.sort_unstable();
        hash_of((self.current_workspace, &*items))
    }

    /// The lowest managed frame, used as the sibling for every shadow so the shadow sits below
    /// all managed windows (and above the desktop). Falls back to the frame's own window.
    fn shadow_sibling(&self) -> Option<Window> {
        let root = self.screen().root;
        let reply = self.conn.query_tree(root).ok()?.reply().ok()?;
        let frames: std::collections::HashSet<Window> =
            self.clients.values().map(|client| client.frame).collect();
        reply
            .children
            .iter()
            .copied()
            .find(|window| frames.contains(window))
    }

    /// Slide the shadow with its frame. One `configure_window`, no readback.
    fn move_frame_shadow(&mut self, frame: Window, outer: Rect) -> Result<(), ReplyOrIdError> {
        let Some(held) = self.frame_shadows.get(&frame) else {
            return Ok(());
        };
        self.conn
            .configure_window(held.win, &shadow_geometry(shadow::shadow_rect(outer)))?;
        if let Some(held) = self.frame_shadows.get_mut(&frame) {
            held.state.placed = outer;
        }
        Ok(())
    }

    fn create_shadow_window(&self) -> Result<Window, ReplyOrIdError> {
        let root = self.screen().root;
        let win = self.conn.generate_id()?;
        self.conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            win,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .override_redirect(1)
                .event_mask(EventMask::NO_EVENT),
        )?;
        // Empty input shape: the shadow never takes pointer input.
        self.conn.shape_rectangles(
            ShapeOp::SET,
            ShapeSk::INPUT,
            ClipOrdering::UNSORTED,
            win,
            0,
            0,
            &[],
        )?;
        Ok(win)
    }

    /// Shade the shadow from the desktop frame under it. The window stays mapped: the source is
    /// the desktop's own pixmap, so the shadow can never read itself or another window.
    fn refresh_frame_shadow(
        &mut self,
        frame: Window,
        outer: Rect,
        focused: bool,
        win: Window,
        generation: u64,
        sibling: Window,
    ) -> Result<(), ReplyOrIdError> {
        let _guard = flamewm_profiler::start("wm.shadow.refresh");
        // The caller resolved the bottom managed frame once for this batch. Placing all shadows
        // beneath it prevents a stale shadow snapshot from covering another client's titlebar.
        let previous = self.frame_shadows.get(&frame).map(|held| held.pixmap);
        // X Shape is window-local: position, focus and backdrop changes cannot alter its
        // silhouette. Only a new shadow window or a changed frame size needs a new mask.
        let previous_shape = self.frame_shadows.get(&frame).map(|held| {
            (held.win, held.state.shaded.width, held.state.shaded.height)
        });
        let radius = u32::from(crate::frame::geometry_shape::scaled_corner_radius(
            true, self.decoration_tokens.style, self.titlebar_height(),
        ));
        let update_shape = needs_shadow_shape_update(previous_shape, win, outer)
            || self.frame_shadows.get(&frame).is_some_and(|held| held.radius != radius);
        let shown = self.refresh_shadow(outer, focused, win, sibling, update_shape, radius)?;
        if let Some((raw, pixmap)) = shown {
            self.frame_shadows.insert(
                frame,
                FrameShadow {
                    win,
                    radius,
                    state: ShadowState {
                        shaded: outer,
                        focused,
                        generation,
                        placed: outer,
                    },
                    pixmap,
                    raw,
                },
            );
        } else {
            self.conn.destroy_window(win)?;
            self.frame_shadows.remove(&frame);
        }
        if let Some(old) = previous {
            self.conn.free_pixmap(old)?;
        }
        Ok(())
    }

    /// Returns the unshaded snapshot and the pixmap now behind the window, or `None` when the
    /// shadow could not be shaded (no snapshot, request too large); the caller then hides it.
    fn refresh_shadow(
        &self,
        outer: Rect,
        focused: bool,
        win: Window,
        sibling: Window,
        update_shape: bool,
        radius: u32,
    ) -> Result<Option<(shadow_rubber::ShadowBands, Pixmap)>, ReplyOrIdError> {
        let root = self.screen().root;
        let rect = shadow::shadow_rect(outer);
        let Some(raw) = self.desktop_snapshot(rect, outer)? else {
            return Ok(None);
        };
        let raw_bands = shadow_rubber::ShadowBands::capture_rounded(&raw, rect, outer, radius);
        let mut pixels = raw;
        shadow::shade_visible_shadow_rounded(outer, focused, radius, &mut pixels);
        if pixels.len() + 32 > self.conn.maximum_request_bytes() {
            return Ok(None);
        }
        let gc = self.conn.generate_id()?;
        self.conn.create_gc(gc, root, &CreateGCAux::new())?;
        let pixmap = self.conn.generate_id()?;
        let (w, h) = (rect.width.max(1) as u16, rect.height.max(1) as u16);
        self.conn.create_pixmap(24, pixmap, root, w, h)?;
        self.conn.put_image(
            ImageFormat::Z_PIXMAP,
            pixmap,
            gc,
            w,
            h,
            0,
            0,
            0,
            24,
            &pixels,
        )?;
        // The window holds the pixmap by reference once it is its background; the caller frees it
        // when the shadow is replaced or destroyed.
        self.conn.change_window_attributes(
            win,
            &ChangeWindowAttributesAux::new().background_pixmap(pixmap),
        )?;
        // A new background does not repaint a mapped window: repaint it from the new pixels.
        self.conn.clear_area(false, win, 0, 0, 0, 0)?;
        self.conn.free_gc(gc)?;
        self.conn
            .configure_window(win, &shadow_aux(rect, sibling))?;
        // One rounded square: the fixed small radius cuts all four corners. The
        // window area inside the frame is cut away too, so only the cast shows.
        if update_shape {
            let mask: Vec<x11rb::protocol::xproto::Rectangle> =
                shadow::rounded_shadow_mask_rectangles(rect, outer, radius)
                    .into_iter()
                    .map(|(x, y, width, height)| x11rb::protocol::xproto::Rectangle {
                        x: x as i16,
                        y: y as i16,
                        width: width as u16,
                        height: height as u16,
                    })
                    .collect();
            self.conn.shape_rectangles(
                ShapeOp::SET,
                ShapeSk::BOUNDING,
                ClipOrdering::UNSORTED,
                win,
                0,
                0,
                &mask,
            )?;
        }
        self.conn.map_window(win)?;
        Ok(Some((raw_bands, pixmap)))
    }

    /// The desktop published or withdrew the live rubber band. Recompose only the shadow regions
    /// the old or new band crosses, from each shadow's raw snapshot: no root readback, no grab.
    pub(super) fn on_root_rubber_changed(&mut self) -> Result<(), ReplyOrIdError> {
        let root = self.screen().root;
        let values: Vec<u32> = self
            .conn
            .get_property(
                false,
                root,
                self.atoms.root_rubber,
                AtomEnum::CARDINAL,
                0,
                6,
            )?
            .reply()?
            .value32()
            .map(std::iter::Iterator::collect)
            .unwrap_or_default();
        let next = RootRubber::from_cardinals(&values);
        let previous = std::mem::replace(&mut self.shadow_scene.rubber, next);
        if previous == next {
            return Ok(());
        }
        // One graphics context per property change, not per intersecting shadow. Keep the
        // output allocation across all strips/windows in this update, while dropping it before
        // the next event so large selection operations do not inflate the idle working set.
        let mut gc = None;
        let mut pixels = Vec::new();
        let update = (|| -> Result<(), ReplyOrIdError> {
            for held in self.frame_shadows.values() {
                let area = shadow::shadow_rect(held.state.shaded);
                let regions = shadow_rubber::visible_motion_damage_regions_rounded(
                    area,
                    held.state.shaded,
                    held.radius,
                    previous.as_ref(),
                    next.as_ref(),
                );
                if regions.iter().all(Option::is_none) {
                    continue;
                }
                let shared_gc = if let Some(gc) = gc {
                    gc
                } else {
                    let created = self.conn.generate_id()?;
                    self.conn.create_gc(created, root, &CreateGCAux::new())?;
                    gc = Some(created);
                    created
                };
                for region in regions.into_iter().flatten() {
                    shadow_rubber::compose_visible_region_into(
                        &held.raw,
                        region,
                        held.state.shaded,
                        held.state.focused,
                        next.as_ref(),
                        &mut pixels,
                    );
                    self.put_shadow_region(held, area, region, &pixels, shared_gc)?;
                }
            }
            Ok(())
        })();
        // Attempt GC cleanup even if an X11 image upload failed partway through the batch.
        let cleanup = gc.map(|created| self.conn.free_gc(created)).transpose();
        update?;
        cleanup?;
        Ok(())
    }

    /// Writes composed pixels for `region` (root space, inside `area`) into the shadow's pixmap
    /// and repaints that window area from it. Rows go in bands that fit one request.
    fn put_shadow_region(
        &self,
        held: &FrameShadow,
        area: Rect,
        region: Rect,
        pixels: &[u8],
        gc: u32,
    ) -> Result<(), ReplyOrIdError> {
        let stride = region.width as usize * 4;
        let rows_per_request =
            (self.conn.maximum_request_bytes().saturating_sub(64) / stride.max(1)).max(1);
        let (dst_x, dst_y) = (region.x - area.x, region.y - area.y);
        for (index, band) in pixels.chunks(rows_per_request * stride).enumerate() {
            let rows = band.len() / stride;
            self.conn.put_image(
                ImageFormat::Z_PIXMAP,
                held.pixmap,
                gc,
                region.width as u16,
                rows as u16,
                dst_x as i16,
                (dst_y as usize + index * rows_per_request) as i16,
                0,
                24,
                band,
            )?;
        }
        self.conn.clear_area(
            false,
            held.win,
            dst_x as i16,
            dst_y as i16,
            region.width as u16,
            region.height as u16,
        )?;
        Ok(())
    }

    /// The presented desktop frame, read from the root property once per change.
    fn desktop_frame(&mut self) -> Result<Option<DesktopFrame>, ReplyOrIdError> {
        if self.shadow_scene.desktop_read {
            return Ok(self.shadow_scene.desktop);
        }
        let values: Vec<u32> = self
            .conn
            .get_property(
                false,
                self.screen().root,
                self.atoms.desktop_frame,
                AtomEnum::CARDINAL,
                0,
                6,
            )?
            .reply()?
            .value32()
            .map(std::iter::Iterator::collect)
            .unwrap_or_default();
        let frame = DesktopFrame::from_cardinals(&values);
        self.shadow_scene.desktop = frame;
        self.shadow_scene.desktop_read = true;
        Ok(frame)
    }

    /// BGRX desktop pixels of the shadow `rect` of frame `outer`, read from the presented desktop
    /// frame. Only the visible cast is read (the band right of and below the frame); the area the
    /// frame covers and anything outside the desktop frame stay zero (never visible). `None`
    /// without a desktop frame, or when the frame was replaced between its publish and this read
    /// (its property change re-shades).
    fn desktop_snapshot(&self, rect: Rect, outer: Rect) -> Result<Option<Vec<u8>>, ReplyOrIdError> {
        let Some(desktop) = self.shadow_scene.desktop else {
            return Ok(None);
        };
        let (w, h) = (rect.width.max(1) as usize, rect.height.max(1) as usize);
        let mut pixels = vec![0u8; w * h * 4];
        let right = (outer.x + outer.width as i32 - rect.x).clamp(0, w as i32);
        let bottom = (outer.y + outer.height as i32 - rect.y).clamp(0, h as i32);
        let bands = [
            (right, 0, w as i32 - right, bottom),
            (0, bottom, w as i32, h as i32 - bottom),
        ];
        for (bx, by, bw, bh) in bands {
            let x0 = (rect.x + bx).max(desktop.x);
            let y0 = (rect.y + by).max(desktop.y);
            let x1 = (rect.x + bx + bw).min(desktop.x + desktop.width as i32);
            let y1 = (rect.y + by + bh).min(desktop.y + desktop.height as i32);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let (cw, ch) = ((x1 - x0) as usize, (y1 - y0) as usize);
            let Ok(reply) = self
                .conn
                .get_image(
                    ImageFormat::Z_PIXMAP,
                    desktop.pixmap,
                    (x0 - desktop.x) as i16,
                    (y0 - desktop.y) as i16,
                    cw as u16,
                    ch as u16,
                    u32::MAX,
                )?
                .reply()
            else {
                return Ok(None);
            };
            if !matches!(reply.depth, 24 | 32) || reply.data.len() != cw * ch * 4 {
                return Ok(None);
            }
            let (ox, oy) = ((x0 - rect.x) as usize, (y0 - rect.y) as usize);
            for row in 0..ch {
                let src = &reply.data[row * cw * 4..(row + 1) * cw * 4];
                let dst = ((oy + row) * w + ox) * 4;
                pixels[dst..dst + cw * 4].copy_from_slice(src);
            }
        }
        Ok(Some(pixels))
    }
}

#[cfg(test)]
mod desktop_frame_tests {
    use super::DesktopFrame;

    #[test]
    fn parses_the_published_frame_and_rejects_malformed_ones() {
        let frame = DesktopFrame::from_cardinals(&[0x0c0_0003, 0, (-4i32) as u32, 1280, 800, 7])
            .expect("valid frame");
        assert_eq!(
            (
                frame.pixmap,
                frame.x,
                frame.y,
                frame.width,
                frame.height,
                frame.serial
            ),
            (0x0c0_0003, 0, -4, 1280, 800, 7)
        );
        assert!(DesktopFrame::from_cardinals(&[0, 0, 0, 1280, 800, 1]).is_none());
        assert!(DesktopFrame::from_cardinals(&[5, 0, 0, 0, 800, 1]).is_none());
        assert!(DesktopFrame::from_cardinals(&[5, 0, 0, 1280, 800]).is_none());
    }
}

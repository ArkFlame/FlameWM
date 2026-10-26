//! X side of the frame shadow (pure shading and planning live in `frame::shadow`).
//!
//! Per floating frame: one override-redirect, input-transparent window stacked
//! directly below the frame. It holds a single root snapshot darkened in place,
//! so exposes repaint without WM work. Pixels are refreshed only on map,
//! resize, focus change and hide.
//!
//! Every reactor turn asks `frame::shadow::plan_shadow` what each frame needs:
//!
//! * `Noop` — nothing changed (the common case, including every motion step of a
//!   frame nobody is dragging).
//! * `Move` — the window slides with its frame, `configure_window` only.
//! * `Reshade` — one `get_image` readback, under a short server grab.
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

/// One shadow window of a frame and what it currently shows.
pub(super) struct FrameShadow {
    win: Window,
    state: ShadowState,
    /// Pixmap behind `win`'s background. Kept (not freed after use) so a live desktop rubber band
    /// can be recomposed into it without a readback.
    pixmap: Pixmap,
    /// Unshaded root snapshot of `shadow_rect(state.shaded)`, BGRX.
    raw: Vec<u8>,
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
    seeded: bool,
    /// Live desktop rubber band currently composed into the shadows (root property mirror).
    rubber: Option<RootRubber>,
}

impl ShadowScene {
    /// Advance the root generation for this turn and return its new value.
    fn advance(
        &mut self,
        membership: u64,
        backdrop: u64,
        geometry: Option<u64>,
        mut dragging: Vec<Window>,
    ) -> u64 {
        dragging.sort_unstable();
        dragging.dedup();
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
            .any(|client| !dragging.contains(client))
        {
            bump = true;
        }
        self.dragging = dragging;
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
            && self.conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
        let wanted: Vec<WantedShadow> = if usable {
            self.clients
                .values()
                .filter(|c| c.is_floating() && c.visible_on(self.current_workspace))
                .map(|c| WantedShadow {
                    frame: c.frame,
                    outer: c.outer,
                    focused: self.active == Some(c.client),
                })
                .collect()
        } else {
            Vec::new()
        };
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
        let (generation, motion) = self.advance_shadow_generation();
        for want in wanted {
            let prev = self.frame_shadows.get(&want.frame).map(|held| held.state);
            // A frame first seen while frames are still moving waits for the settle: shading it
            // now would need the root readback a live move must never do.
            if prev.is_none() && motion {
                continue;
            }
            match shadow::plan_shadow(prev, want.outer, want.focused, motion, generation) {
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
                    self.refresh_frame_shadow(
                        want.frame,
                        want.outer,
                        want.focused,
                        win,
                        generation,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Advance the root generation: bump it when the root behind some strip may have changed.
    /// Also reports whether frames are still moving this turn (a gesture or a flight), in which
    /// case nothing may re-shade.
    fn advance_shadow_generation(&mut self) -> (u64, bool) {
        let dragging: Vec<Window> = self
            .sessions
            .iter()
            .filter(|(_, state)| drag_activated(&state.session))
            .map(|(client, _)| *client)
            .collect();
        // The same "something is flying" predicate `transition_wait` uses to cap the reactor
        // wait: geometry is only meaningful once both are quiet.
        let motion = !dragging.is_empty() || self.animator.is_active();
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
            .advance(membership, backdrop, geometry, dragging);
        (generation, motion)
    }

    /// Which windows the root currently shows: a map/unmap/workspace change re-shades.
    fn shadow_membership_hash(&self) -> u64 {
        let mut items: Vec<(Window, usize, bool)> = self
            .clients
            .values()
            .filter(|c| c.visible_on(self.current_workspace))
            .map(|c| (c.frame, c.workspace, c.sticky))
            .collect();
        items.sort_unstable();
        hash_of((self.current_workspace, items))
    }

    /// Wallpaper source plus output layout: a change invalidates every strip's pixels.
    fn shadow_backdrop_hash(&self) -> u64 {
        let outputs: Vec<(i32, i32, u32, u32)> = self
            .output_work_areas
            .iter()
            .map(|output| {
                let rect = output.geometry;
                (rect.x, rect.y, rect.width, rect.height)
            })
            .collect();
        let screen = self.screen();
        hash_of((
            self.decoration_tokens.backdrop_revision,
            screen.width_in_pixels,
            screen.height_in_pixels,
            outputs,
        ))
    }

    /// Managed geometry while idle: a window that moved (or settled) re-shades the rest once.
    fn shadow_geometry_hash(&self) -> u64 {
        let mut items: Vec<(Window, i32, i32, u32, u32)> = self
            .clients
            .values()
            .filter(|c| c.visible_on(self.current_workspace))
            .map(|c| (c.frame, c.outer.x, c.outer.y, c.outer.width, c.outer.height))
            .collect();
        items.sort_unstable();
        hash_of((self.current_workspace, items))
    }

    /// The lowest managed frame, used as the sibling for every shadow so the shadow sits below
    /// all managed windows (and above the desktop). Falls back to the frame's own window.
    fn shadow_sibling(&self, own: Window) -> Window {
        let root = self.screen().root;
        let Ok(cookie) = self.conn.query_tree(root) else {
            return own;
        };
        let Ok(reply) = cookie.reply() else {
            return own;
        };
        let frames: std::collections::HashSet<Window> =
            self.clients.values().map(|client| client.frame).collect();
        reply
            .children
            .iter()
            .copied()
            .find(|window| frames.contains(window))
            .unwrap_or(own)
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

    /// Shade the shadow from a root snapshot, under one server grab. The window
    /// is unmapped during the read so it never shades itself.
    ///
    /// The grab is kept, but only here: it makes unmap-then-read atomic, so a
    /// repaint that lands between the two cannot be baked into the shadow. It
    /// never spans a motion turn, because a live move only takes the `Move` path.
    fn refresh_frame_shadow(
        &mut self,
        frame: Window,
        outer: Rect,
        focused: bool,
        win: Window,
        generation: u64,
    ) -> Result<(), ReplyOrIdError> {
        let _guard = flamewm_profiler::start("wm.shadow.refresh");
        // Stack below every managed frame, not just below this one: an opaque snapshot above a
        // neighbour's titlebar paints stale/black pixels over that titlebar. Bottom-of-stack keeps
        // the shadow on the desktop and never over another window.
        let sibling = self.shadow_sibling(frame);
        let previous = self.frame_shadows.get(&frame).map(|held| held.pixmap);
        self.conn.grab_server()?;
        let result = self.refresh_shadow(outer, focused, win, sibling);
        self.conn.ungrab_server()?;
        let shown = result?;
        if let Some((raw, pixmap)) = shown {
            self.frame_shadows.insert(
                frame,
                FrameShadow {
                    win,
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
    ) -> Result<Option<(Vec<u8>, Pixmap)>, ReplyOrIdError> {
        let root = self.screen().root;
        let rect = shadow::shadow_rect(outer);
        self.conn.unmap_window(win)?;
        let Some(raw) = self.root_snapshot(rect)? else {
            return Ok(None);
        };
        let mut pixels = raw.clone();
        shadow::shade_shadow(outer, focused, &mut pixels);
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
        self.conn.free_gc(gc)?;
        self.conn
            .configure_window(win, &shadow_aux(rect, sibling))?;
        // One rounded square: the fixed small radius cuts all four corners. The
        // window area inside the frame is cut away too, so only the cast shows.
        let mask: Vec<x11rb::protocol::xproto::Rectangle> =
            shadow::clip_spans_outside_frame(shadow::shadow_spans(rect), rect, outer)
                .into_iter()
                .map(|(row, start, end)| x11rb::protocol::xproto::Rectangle {
                    x: start as i16,
                    y: row as i16,
                    width: end.saturating_sub(start) as u16,
                    height: 1,
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
        self.conn.map_window(win)?;
        Ok(Some((raw, pixmap)))
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
        for held in self.frame_shadows.values() {
            let area = shadow::shadow_rect(held.state.shaded);
            let Some(region) = shadow_rubber::damage_region(area, previous.as_ref(), next.as_ref())
            else {
                continue;
            };
            let pixels = shadow_rubber::compose_region(
                &held.raw,
                area,
                region,
                held.state.shaded,
                held.state.focused,
                next.as_ref(),
            );
            self.put_shadow_region(held, area, region, &pixels)?;
        }
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
    ) -> Result<(), ReplyOrIdError> {
        let root = self.screen().root;
        let stride = region.width as usize * 4;
        let rows_per_request =
            (self.conn.maximum_request_bytes().saturating_sub(64) / stride.max(1)).max(1);
        let (dst_x, dst_y) = (region.x - area.x, region.y - area.y);
        let gc = self.conn.generate_id()?;
        self.conn.create_gc(gc, root, &CreateGCAux::new())?;
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
        self.conn.free_gc(gc)?;
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

    /// BGRX pixels of `rect`, clipped to the screen. Off-screen parts are zero
    /// (never visible). Returns `None` when nothing of the rect is on screen.
    fn root_snapshot(&self, rect: Rect) -> Result<Option<Vec<u8>>, ReplyOrIdError> {
        let screen = self.screen();
        let x0 = rect.x.max(0);
        let y0 = rect.y.max(0);
        let x1 = (rect.x + rect.width as i32).min(i32::from(screen.width_in_pixels));
        let y1 = (rect.y + rect.height as i32).min(i32::from(screen.height_in_pixels));
        if x1 <= x0 || y1 <= y0 {
            return Ok(None);
        }
        let (cw, ch) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let reply = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                screen.root,
                x0 as i16,
                y0 as i16,
                cw as u16,
                ch as u16,
                u32::MAX,
            )?
            .reply()?;
        if reply.depth != 24 || reply.data.len() != cw * ch * 4 {
            return Ok(None);
        }
        let (w, h) = (rect.width.max(1) as usize, rect.height.max(1) as usize);
        let mut pixels = vec![0u8; w * h * 4];
        let (ox, oy) = ((x0 - rect.x) as usize, (y0 - rect.y) as usize);
        for row in 0..ch {
            let src = &reply.data[row * cw * 4..(row + 1) * cw * 4];
            let dst = ((oy + row) * w + ox) * 4;
            pixels[dst..dst + cw * 4].copy_from_slice(src);
        }
        Ok(Some(pixels))
    }
}

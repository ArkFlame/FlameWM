//! Pointer buttons: titlebar clicks, double-click detection, and window grabs.

use super::*;

/// A title-bar release that activated no move, awaiting a possible second click.
///
/// Mirrors the X11 engine: only a release that never became a drag counts, and a second one from
/// the same window within [`DOUBLE_CLICK_TIMEOUT_MS`] at the same spot toggles maximize.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TitleClick {
    pub id: WindowId,
    pub time_ms: u32,
    pub pointer: (f64, f64),
}

/// Double-click window and movement slop, matching the X11 engine so both sessions feel alike.
pub(super) const DOUBLE_CLICK_TIMEOUT_MS: u32 = 500;
pub(super) const DOUBLE_CLICK_SLOP_PX: f64 = 4.0;

/// Classifies a title-bar release: `true` when it completes a double click.
///
/// A press that turned into a drag never records a click, so "press, drag, press" is not a double
/// click even at the same pixel. Elapsed time and distance are compared the way X11 compares them.
pub(super) fn completes_double_click(
    previous: Option<TitleClick>,
    id: WindowId,
    time_ms: u32,
    x: f64,
    y: f64,
) -> bool {
    previous.is_some_and(|prev| {
        let dx = x - prev.pointer.0;
        let dy = y - prev.pointer.1;
        prev.id == id
            && time_ms.wrapping_sub(prev.time_ms) <= DOUBLE_CLICK_TIMEOUT_MS
            && dx * dx + dy * dy <= DOUBLE_CLICK_SLOP_PX * DOUBLE_CLICK_SLOP_PX
    })
}

impl State {
    pub(super) fn on_button<I: InputBackend>(&mut self, event: &impl PointerButtonEvent<I>) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let button = event.button_code();
        let pressed = event.state() == ButtonState::Pressed;
        let mods = self
            .seat
            .get_keyboard()
            .map(|keyboard| mods_of(&keyboard.modifier_state()))
            .unwrap_or_default();
        let location = self.pointer;
        let pointer_pos = (location.x, location.y);
        let target = self.pointer_target(location);
        self.update_pointer_focus(location, target.focus, event.time_msec());
        let serial = SERIAL_COUNTER.next_serial();
        if pressed {
            // Any button press cancels a bare-Super tap, as X11 `press_other` does.
            self.super_tap = false;
        }
        // A release that ends a compositor grab is not forwarded: the client never saw the press.
        let ends_grab = !pressed && self.grab.is_some();
        let mut consumed = ends_grab;

        if !pressed {
            if let Some(Grab::Move {
                id,
                pointer,
                start,
                start_mode,
                restore,
                anchor_num,
                anchor_den,
                anchor_y,
                titlebar,
            }) = self.grab
            {
                let time_ms = event.time_msec();
                let (px, py) = pointer;
                let location = self.pointer;
                let dx = (location.x - pointer.0).round() as i32;
                let dy = (location.y - pointer.1).round() as i32;
                let moved = move_activated(dx, dy);
                self.snap_preview.hide();
                if moved {
                    let candidate = clamp_move_candidate(
                        move_candidate(
                            start, start_mode, restore, pointer, location, anchor_num, anchor_den,
                            anchor_y, true,
                        ),
                        self.output_rect(),
                        self.model.titlebars(),
                    );
                    // A Dock may have changed its strut while this drag was active. Refresh only
                    // at commit, after the motion hot path has kept its cached work area.
                    self.refresh_dock_struts();
                    let target = snap_target_at(self, location, true);
                    if target == SnapTarget::None {
                        self.model.commit_move(id, candidate);
                    } else {
                        let work = self.work_area();
                        commit_move_snap(&mut self.model, id, target, work, candidate);
                    }
                    self.apply_geometry(id);
                    self.refresh_window_outputs();
                    // A real drag never counts as either half of a double click.
                    self.last_title_click = None;
                } else if titlebar
                    && completes_double_click(self.last_title_click, id, time_ms, px, py)
                {
                    self.last_title_click = None;
                    self.toggle_maximize(id);
                } else if titlebar {
                    self.last_title_click = Some(TitleClick {
                        id,
                        time_ms,
                        pointer: (px, py),
                    });
                } else {
                    self.last_title_click = None;
                }
            } else if self.grab.is_some() {
                // Resize grabs are not title clicks.
                self.last_title_click = None;
            }
            self.grab = None;
        }
        // Super+left moves and Ctrl+Super+right resizes from anywhere on the frame or client, the
        // X11 pointer.rs rules. The press is consumed so the client does not see the drag.
        if pressed && mods.logo && (button == BTN_LEFT || (button == BTN_RIGHT && mods.ctrl)) {
            let target_window = match target.managed_hit {
                Some(Hit::Client(id) | Hit::Titlebar(id) | Hit::Resize(id)) => Some(id),
                _ => None,
            };
            if let Some(id) = target_window {
                self.set_focus(Some(id));
                if button == BTN_LEFT {
                    self.refresh_dock_struts();
                }
                if let Some(managed) = self.model.get(id).cloned() {
                    consumed = true;
                    self.grab = Some(if button == BTN_LEFT {
                        Grab::move_window(id, pointer_pos, &managed, false)
                    } else {
                        Grab::Resize {
                            id,
                            pointer: pointer_pos,
                            start: managed.rect,
                            edge: ResizeEdge::quadrant(managed.rect, location.x, location.y),
                        }
                    });
                }
            }
        }

        if !pressed {
            self.pressed_hit = None;
        }
        if pressed && !consumed && button == BTN_LEFT {
            match target.managed_hit {
                Some(Hit::Button(id, action)) => {
                    self.pressed_hit = Some(Hit::Button(id, action));
                    self.set_focus(Some(id));
                    self.run_titlebar_button(id, action);
                    consumed = true;
                }
                Some(Hit::Titlebar(id)) => {
                    self.set_focus(Some(id));
                    self.refresh_dock_struts();
                    if let Some(managed) = self.model.get(id).cloned() {
                        consumed = true;
                        self.grab = Some(Grab::move_window(id, pointer_pos, &managed, true));
                    }
                }
                Some(Hit::Resize(id)) => {
                    self.set_focus(Some(id));
                    if let Some(window) = self.model.get(id) {
                        let rect = window.rect;
                        consumed = true;
                        self.grab = Some(Grab::Resize {
                            id,
                            pointer: pointer_pos,
                            // The client rect is the origin even for a top corner grabbed in the
                            // titlebar band: `resized` applies the pointer delta to the dragged
                            // edge, so the frame keeps its grab offset and its top tracks the
                            // pointer because the titlebar shift is constant.
                            start: rect,
                            edge: resize_edge_at(window.frame(), rect, location.x, location.y),
                        });
                    }
                }
                Some(Hit::Client(id)) => self.set_focus(Some(id)),
                None if !target.scene_hit => self.set_focus(None),
                None => {}
            }
        }

        // Clicks on decorations are consumed by the compositor; only client areas see them.
        if !consumed && pointer.current_focus().is_some() {
            pointer.button(
                self,
                &ButtonEvent {
                    serial,
                    time: event.time_msec(),
                    button,
                    state: event.state(),
                },
            );
            pointer.frame(self);
        }
    }

    pub(super) fn run_titlebar_button(&mut self, id: WindowId, button: Button) {
        match button {
            Button::Close => self.close_window(id),
            Button::Minimize => {
                self.model.minimize(id);
                self.refocus();
            }
            Button::Maximize => self.toggle_maximize(id),
        }
    }
}

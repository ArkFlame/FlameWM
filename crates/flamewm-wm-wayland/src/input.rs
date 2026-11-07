//! Keyboard and pointer input: shortcut dispatch, pointer focus, move/resize grabs.

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
    KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::desktop::WindowSurfaceType;
use smithay::desktop::utils::under_from_surface_tree;
use smithay::input::keyboard::xkb;
use smithay::input::keyboard::{FilterResult, ModifiersState, XkbConfig};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use flamewm_api::settings::{DEFAULT_KEYBOARD_LAYOUT, KEYBOARD_LAYOUTS};
use flamewm_api::shortcuts::{
    ACTION_FULLSCREEN_ESCAPE, ACTION_LAUNCHER, ACTION_SCREENSHOT_FULL, ACTION_SCREENSHOT_REGION,
    ACTION_SCREENSHOT_WINDOW, ACTION_SESSION_LOCK, ACTION_SHOW_DESKTOP,
    ACTION_SHOW_DESKTOP_CTRL_ALT, ACTION_TOGGLE_START_MENU, ACTION_WINDOW_CLOSE,
    ACTION_WINDOW_CYCLE_NEXT, ACTION_WINDOW_CYCLE_PREV, ACTION_WINDOW_FLOAT,
    ACTION_WINDOW_FULLSCREEN, ACTION_WINDOW_MAXIMIZE, ACTION_WINDOW_MINIMIZE,
    ACTION_WINDOW_RESIZE_GROW, ACTION_WINDOW_RESIZE_SHRINK, ACTION_WINDOW_SWITCH, focus_direction,
    move_direction, workspace_activate_index, workspace_direction, workspace_move_index,
};
use flamewm_platform::shortcuts::{
    ACTION_BRIGHTNESS_DOWN, ACTION_BRIGHTNESS_UP, ACTION_OPEN_ASSISTANT_CTRL_ALT,
    ACTION_OPEN_ASSISTANT_SUPER, ACTION_OPEN_FILES_CTRL_ALT, ACTION_OPEN_FILES_SUPER,
    ACTION_OPEN_FINDER, ACTION_OPEN_FINDER_CTRL_ALT, ACTION_OPEN_TERMINAL_CTRL_ALT,
    ACTION_OPEN_TERMINAL_SUPER, ACTION_VOLUME_DOWN, ACTION_VOLUME_MUTE, ACTION_VOLUME_UP,
};
use flamewm_platform::system::MediaAudioKey;

use crate::model::{
    Button, Hit, Mode, Mods, Rect, ResizeEdge, TITLEBAR_HEIGHT, WindowId, resize_edge_at,
};
use crate::state::{Grab, State, next_serial};
use flamewm_window_core::{SnapTarget, keep_visible};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// One media-key action the compositor routes through the platform system service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaKey {
    Brightness(bool),
    Audio(MediaAudioKey),
}

/// Maps a shortcut action id to its media key, so the registry binding and the dispatch table
/// cannot drift apart. Every other action yields `None`.
fn media_key(action: &str) -> Option<MediaKey> {
    match action {
        ACTION_BRIGHTNESS_UP => Some(MediaKey::Brightness(true)),
        ACTION_BRIGHTNESS_DOWN => Some(MediaKey::Brightness(false)),
        ACTION_VOLUME_UP => Some(MediaKey::Audio(MediaAudioKey::VolumeUp)),
        ACTION_VOLUME_DOWN => Some(MediaKey::Audio(MediaAudioKey::VolumeDown)),
        ACTION_VOLUME_MUTE => Some(MediaKey::Audio(MediaAudioKey::VolumeMute)),
        _ => None,
    }
}

fn mods_of(state: &ModifiersState) -> Mods {
    Mods {
        ctrl: state.ctrl,
        alt: state.alt,
        shift: state.shift,
        logo: state.logo,
    }
}

/// Keysym names that are modifier keys themselves.
fn is_super_key(name: &str) -> bool {
    name == "super_l" || name == "super_r"
}

/// Keysym names to look up for one key event, in priority order: the unshifted symbol first,
/// then the symbol the active level produces. `Ctrl+Alt` is level 3 on several layouts, where
/// `Ctrl+Alt+d` would otherwise be reported under a different keysym and the chord would miss.
fn name_candidates<'a>(raw: &'a str, modified: &'a str) -> impl Iterator<Item = &'a str> {
    std::iter::once(raw)
        .chain(std::iter::once(modified))
        .filter(|name| !name.is_empty())
}

impl State {
    pub fn process_input<I: InputBackend>(&mut self, event: InputEvent<I>) {
        // Plain motion (no grab, no focus-follows-mouse) only moves the cursor: it needs a paint but
        // not a shell-mirror rebuild, so it takes the cheaper request.
        let mut plain_motion = false;
        match event {
            InputEvent::Keyboard { event } => {
                // A forwarded key changes no pixel by itself: the client repaints on its commit. Only a
                // shortcut action (which may change the scene) asks the compositor for a frame.
                if !self.on_key::<I>(&event) {
                    return;
                }
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let location = event.position_transformed(self.output_size);
                let hover_before = self.model.hit(self.pointer.x, self.pointer.y);
                self.on_motion(location, event.time_msec());
                // Leaving the held control cancels its pressed look; re-entering does not restore it.
                if self.pressed_hit.is_some()
                    && self.model.hit(self.pointer.x, self.pointer.y) != self.pressed_hit
                {
                    self.pressed_hit = None;
                }
                // A hit change can move a titlebar button's hover, which the scene draws.
                plain_motion = self.grab.is_none()
                    && !self.focus_follows_mouse
                    && self.model.hit(self.pointer.x, self.pointer.y) == hover_before;
            }
            InputEvent::PointerButton { event } => self.on_button::<I>(&event),
            InputEvent::PointerAxis { event } => self.on_axis::<I>(&event),
            _ => {}
        }
        if plain_motion {
            // Nested winit with a host-drawn pointer: the host moves its own cursor, so no frame.
            if self.backend.is_none() || !Self::host_draws_pointer_status(&self.cursor_status) {
                self.request_cursor_paint();
            }
        } else {
            self.request_redraw();
        }
    }

    /// Routes one key. Returns true when it ran a shortcut action, the only key outcome that needs a frame.
    fn on_key<I: InputBackend>(&mut self, event: &impl KeyboardKeyEvent<I>) -> bool {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return false;
        };
        let key_state = event.state();
        let keycode = event.key_code();
        let time = event.time_msec();
        let serial = next_serial();
        let shortcuts = std::mem::take(&mut self.shortcuts);

        let action = keyboard.input::<Option<String>, _>(
            self,
            keycode,
            key_state,
            serial,
            time,
            |state, mods, handle| {
                // Unshifted symbol first: `Ctrl+Alt` is level 3 on several layouts, where
                // `modified_sym` names a different keysym than the chord (or no single keysym).
                let raw = handle
                    .raw_latin_sym_or_raw_current_sym()
                    .map_or_else(String::new, |sym| {
                        xkb::keysym_get_name(sym).to_ascii_lowercase()
                    });
                let modified = xkb::keysym_get_name(handle.modified_sym()).to_ascii_lowercase();
                let is_super = is_super_key(&raw) || is_super_key(&modified);
                if key_state == KeyState::Pressed {
                    if is_super {
                        // Bare Super: decided on release, so Super+x chords never also toggle the menu.
                        state.super_tap = true;
                        return FilterResult::Intercept(None);
                    }
                    state.super_tap = false;
                    let mods = mods_of(mods);
                    let found = name_candidates(&raw, &modified)
                        .find_map(|name| shortcuts.action_for(mods, name))
                        .map(str::to_owned);
                    return match found {
                        Some(action) => {
                            state.swallowed_keys.push(keycode.raw());
                            FilterResult::Intercept(Some(action))
                        }
                        None => FilterResult::Forward,
                    };
                }
                if is_super {
                    let tapped = std::mem::take(&mut state.super_tap);
                    let action = tapped.then(|| ACTION_TOGGLE_START_MENU.to_owned());
                    return FilterResult::Intercept(action);
                }
                match state
                    .swallowed_keys
                    .iter()
                    .position(|code| *code == keycode.raw())
                {
                    Some(index) => {
                        state.swallowed_keys.remove(index);
                        FilterResult::Intercept(None)
                    }
                    None => FilterResult::Forward,
                }
            },
        );
        self.shortcuts = shortcuts;
        if let Some(Some(action)) = action {
            self.dispatch_action(&action);
            return true;
        }
        false
    }

    /// Runs one shortcut action. Actions are the shared `flamewm-api` names.
    pub fn dispatch_action(&mut self, action: &str) {
        // Media keys are bound by the default registry, so they must reach the shared system
        // service instead of falling through to the unhandled log.
        if let Some(key) = media_key(action) {
            self.dispatch_media_key(key);
            return;
        }
        if let Some(direction) = workspace_direction(action) {
            if let Some(target) = self.model.workspace_step(direction) {
                if self.model.switch_workspace(target) {
                    self.refocus();
                }
            }
            return;
        }
        // Hyprland-style binds whose target is encoded in the action id: absolute workspace
        // activation/move and directional focus/move. An out-of-range index or a missing
        // neighbour is a no-op, exactly as in the X11 engine.
        if let Some(index) = workspace_activate_index(action) {
            if self.model.switch_workspace(index) {
                self.refocus();
            }
            return;
        }
        if let Some(index) = workspace_move_index(action) {
            // Out of range is a no-op, as in the X11 engine: nothing moves and focus stays put.
            if index < self.model.workspace_count() {
                if let Some(id) = self.focused {
                    self.model.move_to_workspace(id, index);
                    self.apply_geometry(id);
                    self.refocus();
                }
            }
            return;
        }
        if let Some(direction) = focus_direction(action) {
            if let Some(id) = self.focused {
                if let Some(target) = self.model.focus_direction(id, direction) {
                    self.set_focus(Some(target));
                }
            }
            return;
        }
        if let Some(direction) = move_direction(action) {
            if let Some(id) = self.focused {
                self.model.nudge(id, direction);
                self.apply_geometry(id);
            }
            return;
        }
        match action {
            ACTION_WINDOW_CLOSE => {
                if let Some(id) = self.focused {
                    self.close_window(id);
                }
            }
            ACTION_WINDOW_MINIMIZE => {
                if let Some(id) = self.focused {
                    self.model.minimize(id);
                    self.refocus();
                }
            }
            ACTION_WINDOW_MAXIMIZE => {
                if let Some(id) = self.focused {
                    self.toggle_maximize(id);
                }
            }
            ACTION_WINDOW_FULLSCREEN => {
                if let Some(id) = self.focused {
                    let on = self
                        .model
                        .get(id)
                        .is_some_and(|window| window.mode != Mode::Fullscreen);
                    let output = self.output_rect();
                    self.model.set_fullscreen(id, output, on);
                    self.apply_geometry(id);
                }
            }
            // Floating fills the work area and back, the X11 `toggle_float`; leaving fullscreen
            // lands on the floating rect instead of the layout it was fullscreened over.
            ACTION_WINDOW_FLOAT => {
                if let Some(id) = self.focused {
                    self.refresh_dock_struts();
                    let work = self.work_area();
                    let output = self.output_rect();
                    self.model.toggle_float(id, work, output);
                    self.apply_geometry(id);
                }
            }
            ACTION_WINDOW_CYCLE_NEXT => self.cycle_focus(true),
            ACTION_WINDOW_CYCLE_PREV => self.cycle_focus(false),
            ACTION_WINDOW_RESIZE_GROW => {
                if let Some(id) = self.focused {
                    self.model.resize_step(id, true);
                    self.apply_geometry(id);
                }
            }
            ACTION_WINDOW_RESIZE_SHRINK => {
                if let Some(id) = self.focused {
                    self.model.resize_step(id, false);
                    self.apply_geometry(id);
                }
            }
            ACTION_WINDOW_SWITCH => match self.control.clone() {
                Some(service) => service.switcher_step(self),
                None => {
                    if let Some(id) = self.model.cycle_focus() {
                        self.set_focus(Some(id));
                    }
                }
            },
            ACTION_SHOW_DESKTOP | ACTION_SHOW_DESKTOP_CTRL_ALT => {
                self.model.toggle_show_desktop();
                self.refocus();
            }
            ACTION_FULLSCREEN_ESCAPE => {
                if let Some(id) = self.focused {
                    let output = self.output_rect();
                    self.model.set_fullscreen(id, output, false);
                    self.apply_geometry(id);
                }
            }
            ACTION_OPEN_FINDER | ACTION_OPEN_FINDER_CTRL_ALT => self.spawn("flamewm-finder", &[]),
            // The assistant binary toggles its own window; a missing binary is logged by `spawn`.
            ACTION_OPEN_ASSISTANT_SUPER | ACTION_OPEN_ASSISTANT_CTRL_ALT => {
                self.spawn("flamewm-assistant", &[]);
            }
            ACTION_OPEN_TERMINAL_SUPER | ACTION_OPEN_TERMINAL_CTRL_ALT => {
                self.spawn("flamewm-terminal", &[]);
            }
            ACTION_OPEN_FILES_SUPER | ACTION_OPEN_FILES_CTRL_ALT => {
                let home = std::env::var_os("HOME").unwrap_or_else(|| "/".into());
                self.spawn("xdg-open", &[&home.to_string_lossy()]);
            }
            ACTION_SCREENSHOT_FULL => self.spawn("flamewm-screenshot", &["--full", "--copy"]),
            ACTION_SCREENSHOT_REGION => {
                self.spawn("flamewm-screenshot", &["--region", "--annotate", "--copy"]);
            }
            ACTION_SCREENSHOT_WINDOW => self.spawn("flamewm-screenshot", &["--active", "--copy"]),
            // The shell owns the launcher and start menu; it listens for the same control signals
            // the X11 WM emits.
            ACTION_LAUNCHER => self.emit_control_signal("LauncherToggleRequested"),
            ACTION_TOGGLE_START_MENU => self.emit_control_signal("StartMenuToggleRequested"),
            ACTION_SESSION_LOCK => eprintln!("flamewm-wayland: session lock is a later phase"),
            other => eprintln!("flamewm-wayland: unhandled shortcut action {other}"),
        }
    }

    /// Routes a media key to the control service. The service owns the backlight and the audio
    /// helper; without it the key is a no-op and the missing installation is logged once.
    fn dispatch_media_key(&self, key: MediaKey) {
        let Some(service) = self.control.clone() else {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                eprintln!("flamewm-wayland: media keys need the control service; ignoring them");
            });
            return;
        };
        match key {
            MediaKey::Brightness(up) => service.media_brightness_key(up),
            MediaKey::Audio(key) => service.media_audio_key(key),
        }
    }

    fn toggle_maximize(&mut self, id: WindowId) {
        self.refresh_dock_struts();
        let work = self.work_area();
        self.model.toggle_maximize(id, work);
        self.apply_geometry(id);
    }

    /// `Super+J` / `Super+K`: focus the next/previous visible window in stack order, the X11
    /// `cycle_window` step.
    fn cycle_focus(&mut self, forward: bool) {
        let target = if forward {
            self.model.cycle_focus()
        } else {
            self.model.cycle_focus_prev()
        };
        if let Some(id) = target {
            self.set_focus(Some(id));
        }
    }

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
    fn update_pointer_focus(
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
    fn pointer_target(&self, location: Point<f64, Logical>) -> PointerTarget {
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

    fn on_button<I: InputBackend>(&mut self, event: &impl PointerButtonEvent<I>) {
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
                self.move_outline = None;
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

    fn run_titlebar_button(&mut self, id: WindowId, button: Button) {
        match button {
            Button::Close => self.close_window(id),
            Button::Minimize => {
                self.model.minimize(id);
                self.refocus();
            }
            Button::Maximize => self.toggle_maximize(id),
        }
    }

    fn on_axis<I: InputBackend>(&mut self, event: &impl PointerAxisEvent<I>) {
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

fn move_activated(dx: i32, dy: i32) -> bool {
    dx.unsigned_abs() >= MOVE_ACTIVATION_THRESHOLD_PX
        || dy.unsigned_abs() >= MOVE_ACTIVATION_THRESHOLD_PX
}

#[allow(clippy::too_many_arguments)]
fn move_candidate(
    start: Rect,
    start_mode: Mode,
    restore: Rect,
    start_pointer: (f64, f64),
    pointer: Point<f64, Logical>,
    anchor_num: i32,
    anchor_den: i32,
    anchor_y: i32,
    activated: bool,
) -> Rect {
    if start_mode == Mode::Normal {
        return Rect::new(
            start.x + (pointer.x - start_pointer.0).round() as i32,
            start.y + (pointer.y - start_pointer.1).round() as i32,
            start.w,
            start.h,
        );
    }
    if !activated {
        return start;
    }
    let anchor_x = (f64::from(restore.w) * f64::from(anchor_num) / f64::from(anchor_den.max(1)))
        .round() as i32;
    Rect::new(
        pointer.x.round() as i32 - anchor_x,
        pointer.y.round() as i32 - anchor_y + TITLEBAR_HEIGHT,
        restore.w,
        restore.h,
    )
}

/// Clamp a free move/restore candidate on the physical output, accounting for the decorated frame.
/// Candidates and the window model use client geometry; only this boundary converts to/from frame
/// geometry, so titlebar height is included exactly once in the visibility calculation.
fn clamp_move_candidate(candidate: Rect, output: Rect, titlebars: bool) -> Rect {
    let frame = if titlebars {
        Rect::new(
            candidate.x,
            candidate.y.saturating_sub(TITLEBAR_HEIGHT),
            candidate.w,
            candidate.h.saturating_add(TITLEBAR_HEIGHT),
        )
    } else {
        candidate
    };
    let visible = keep_visible(
        flamewm_api::Rect::new(frame.x, frame.y, frame.w, frame.h),
        flamewm_api::Rect::new(output.x, output.y, output.w, output.h),
    );
    let frame = Rect::new(visible.x, visible.y, visible.width, visible.height);
    if titlebars {
        Rect::new(
            frame.x,
            frame.y.saturating_add(TITLEBAR_HEIGHT),
            frame.w,
            frame.h.saturating_sub(TITLEBAR_HEIGHT),
        )
    } else {
        frame
    }
}

fn commit_move_snap(
    model: &mut crate::model::WindowModel,
    id: WindowId,
    target: SnapTarget,
    work: Rect,
    candidate: Rect,
) {
    if target == SnapTarget::Maximize {
        model.maximize_to(id, work, candidate);
    } else {
        model.snap_to(id, target, work, candidate);
    }
}

fn frame_for_client(client: Rect, titlebar: bool) -> Rect {
    if titlebar { client.frame() } else { client }
}

fn snap_target_at(state: &State, pointer: Point<f64, Logical>, activated: bool) -> SnapTarget {
    let output = state.output_rect();
    let work = state.work_area();
    let (edge_snap, edge_px) = state.model.edge_snap();
    if !edge_snap {
        return SnapTarget::None;
    }
    snap_target_for_move_with(
        activated,
        flamewm_api::Point::new(pointer.x.round() as i32, pointer.y.round() as i32),
        flamewm_api::Rect::new(output.x, output.y, output.w, output.h),
        flamewm_api::Rect::new(work.x, work.y, work.w, work.h),
        edge_px,
    )
}

/// Default-threshold fixture used by the snap tests; the live path passes `edgeSnap.thresholdPx`.
#[cfg(test)]
fn snap_target_for_move(
    activated: bool,
    pointer: flamewm_api::Point,
    output: flamewm_api::Rect,
    work: flamewm_api::Rect,
) -> SnapTarget {
    snap_target_for_move_with(
        activated,
        pointer,
        output,
        work,
        flamewm_window_core::EDGE_THRESHOLD,
    )
}

/// Snap target for a move with an explicit `edgeSnap.thresholdPx`.
fn snap_target_for_move_with(
    activated: bool,
    pointer: flamewm_api::Point,
    output: flamewm_api::Rect,
    work: flamewm_api::Rect,
    edge_px: i32,
) -> SnapTarget {
    if !activated {
        return SnapTarget::None;
    }
    flamewm_window_core::snap_target_in_output_with(pointer, output, work, edge_px)
}

/// Pick the first owner that actually contains the point. The same helper makes front-to-back
/// semantics explicit in tests using foreground Dock/Desktop, popup, layer, and managed fixtures.
fn first_hit<I, T>(
    items: impl IntoIterator<Item = I>,
    hit: impl FnMut(I) -> Option<T>,
) -> Option<T> {
    items.into_iter().find_map(hit)
}

#[derive(Default)]
struct PointerTarget {
    focus: Option<(WlSurface, Point<f64, Logical>)>,
    managed_hit: Option<Hit>,
    /// True even when the scene owner is compositor-only and has no client surface to focus.
    scene_hit: bool,
}

/// Which part of the scene owns the pixels under the pointer.
///
/// A managed window splits three ways: its client area belongs to the client surface, its
/// decoration belongs to the compositor, and neither may leak to a lower scene owner. The same
/// front-to-back traversal also prevents popups, layer surfaces and unmanaged X11 surfaces from
/// exposing a covered managed window to focus or button handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PointerRoute {
    /// A managed window's client area: forward to that window's surface.
    Client(crate::model::WindowId),
    /// The compositor's own decoration: no client surface receives the pointer.
    Compositor,
    /// Nothing managed here; fall through to docks, menus and the desktop.
    Unmanaged,
}

fn pointer_route(hit: Option<Hit>) -> PointerRoute {
    match hit {
        Some(Hit::Client(id)) => PointerRoute::Client(id),
        Some(Hit::Titlebar(_) | Hit::Resize(_) | Hit::Button(_, _)) => PointerRoute::Compositor,
        None => PointerRoute::Unmanaged,
    }
}

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
const DOUBLE_CLICK_TIMEOUT_MS: u32 = 500;
const DOUBLE_CLICK_SLOP_PX: f64 = 4.0;
const MOVE_ACTIVATION_THRESHOLD_PX: u32 = 4;

/// Classifies a title-bar release: `true` when it completes a double click.
///
/// A press that turned into a drag never records a click, so "press, drag, press" is not a double
/// click even at the same pixel. Elapsed time and distance are compared the way X11 compares them.
fn completes_double_click(
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

/// Route to the actual surface in a client tree; a miss is an input-region/tree hole and lets
/// the front-to-back scene traversal consider the next owner.
fn tree_focus_at(
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
fn unmanaged_focus(
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

/// Keymap for the seat's one keyboard, from the saved `keyboardLayout`.
///
/// The layout is resolved by the caller and checked against [`KEYBOARD_LAYOUTS`], so this stays
/// pure: a value outside the list falls back to [`DEFAULT_KEYBOARD_LAYOUT`] instead of leaving the
/// field empty, which would make `xkbcommon` read `XKB_DEFAULT_LAYOUT` from the environment and
/// ignore the setting. Everything else keeps smithay's defaults (system rules/model/variant).
fn xkb_config(layout: &str) -> XkbConfig<'static> {
    // Resolve to the `'static` entry from the list rather than the caller's borrow, so the config
    // can outlive the settings string it was parsed from.
    let resolved = KEYBOARD_LAYOUTS
        .iter()
        .copied()
        .find(|candidate| *candidate == layout)
        .unwrap_or(DEFAULT_KEYBOARD_LAYOUT);
    XkbConfig {
        layout: resolved,
        ..XkbConfig::default()
    }
}

/// The `keyboardLayout` saved in the `FlameWM` settings file, for the seat created at startup.
/// A missing or unreadable file leaves the default layout, exactly as an unset settings file does
/// elsewhere in the compositor.
fn saved_keyboard_layout() -> String {
    keyboard_layout_at(&crate::control::settings_path())
}

/// Reads `keyboardLayout` from `path`, falling back to the default layout.
pub fn keyboard_layout_at(path: &std::path::Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| {
            flamewm_platform::settings::ConfigStore::default()
                .parse_snapshot(&text)
                .ok()
        })
        .map_or_else(
            || DEFAULT_KEYBOARD_LAYOUT.to_owned(),
            |settings| settings.effective().keyboard_layout,
        )
}

/// `focusFollowsMouse` saved in the settings file.
///
/// Mouse-driven focus is opt-in, so a missing or unreadable file keeps the X11 default (off).
pub fn saved_focus_follows_mouse() -> bool {
    focus_follows_mouse_at(&crate::control::settings_path())
}

/// Reads `focusFollowsMouse` from `path`. Split from `settings_path` so the file is really read in
/// tests; the path itself comes from the environment, which is resolved in one place.
pub fn focus_follows_mouse_at(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .is_some_and(|text| focus_follows_mouse_from(&text))
}

/// Parses `focusFollowsMouse` out of settings text; an unreadable file means the default, off.
fn focus_follows_mouse_from(text: &str) -> bool {
    flamewm_platform::settings::ConfigStore::default()
        .parse_snapshot(text)
        .is_ok_and(|settings| settings.effective().focus_follows_mouse)
}

/// `titlebars` and `windowMargin` saved in the settings file, for the model built at startup.
///
/// A missing or unreadable file keeps the product defaults, so an unset settings file lays windows
/// out exactly as an unconfigured model does.
pub fn saved_model_settings() -> (bool, i32) {
    model_settings_at(&crate::control::settings_path())
}

/// Reads `titlebars` and `windowMargin` from `path`. Split from `settings_path` so the file is
/// really read in tests.
pub fn model_settings_at(path: &std::path::Path) -> (bool, i32) {
    std::fs::read_to_string(path)
        .ok()
        .map_or_else(default_model_settings, |text| model_settings_from(&text))
}

/// Parses `titlebars` and `windowMargin` out of settings text.
fn model_settings_from(text: &str) -> (bool, i32) {
    flamewm_platform::settings::ConfigStore::default()
        .parse_snapshot(text)
        .map_or_else(
            |_| default_model_settings(),
            |settings| {
                let settings = settings.effective();
                (settings.titlebars, i32::from(settings.window_margin))
            },
        )
}

/// The product defaults, used when the file is absent or rejected.
fn default_model_settings() -> (bool, i32) {
    use flamewm_platform::settings::ProductSettings;

    let defaults = ProductSettings::default();
    (defaults.titlebars, i32::from(defaults.window_margin))
}

/// The keymap the seat installs at startup, read from the saved settings file.
pub fn saved_xkb_config() -> XkbConfig<'static> {
    xkb_config(&saved_keyboard_layout())
}

#[cfg(test)]
mod tests {
    use super::{
        Button, Hit, MediaKey, PointerRoute, TitleClick, clamp_move_candidate, commit_move_snap,
        completes_double_click, first_hit, focus_follows_mouse_at, focus_follows_mouse_from,
        keyboard_layout_at, media_key, move_activated, move_candidate, name_candidates,
        pointer_route, snap_target_for_move, xkb_config,
    };
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
        let lower = super::PointerTarget {
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
        let dir =
            std::env::temp_dir().join(format!("flamewm-wayland-settings-{}", std::process::id()));
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
}

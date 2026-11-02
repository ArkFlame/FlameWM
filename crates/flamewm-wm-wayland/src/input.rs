//! Keyboard and pointer input: shortcut dispatch, pointer focus, move/resize grabs.

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
    KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::desktop::{WindowSurfaceType, layer_map_for_output};
use smithay::input::keyboard::xkb;
use smithay::input::keyboard::{FilterResult, ModifiersState, XkbConfig};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use smithay::wayland::shell::wlr_layer::Layer;

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
    ACTION_OPEN_ASSISTANT_CTRL_ALT, ACTION_OPEN_ASSISTANT_SUPER, ACTION_OPEN_FILES_CTRL_ALT,
    ACTION_OPEN_FILES_SUPER, ACTION_OPEN_FINDER, ACTION_OPEN_FINDER_CTRL_ALT,
    ACTION_OPEN_TERMINAL_CTRL_ALT, ACTION_OPEN_TERMINAL_SUPER,
};

use crate::model::{Button, Hit, Mode, Mods, ResizeEdge, WindowId};
use crate::state::{Grab, State, next_serial};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

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
        match event {
            InputEvent::Keyboard { event } => self.on_key::<I>(&event),
            InputEvent::PointerMotionAbsolute { event } => {
                let location = event.position_transformed(self.output_size);
                self.on_motion(location, event.time_msec());
            }
            InputEvent::PointerButton { event } => self.on_button::<I>(&event),
            InputEvent::PointerAxis { event } => self.on_axis::<I>(&event),
            _ => {}
        }
        self.request_redraw();
    }

    fn on_key<I: InputBackend>(&mut self, event: &impl KeyboardKeyEvent<I>) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
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
        }
    }

    /// Runs one shortcut action. Actions are the shared `flamewm-api` names.
    pub fn dispatch_action(&mut self, action: &str) {
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

    fn toggle_maximize(&mut self, id: WindowId) {
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
                Grab::Move { id, pointer, start } => {
                    let dx = (location.x - pointer.0).round() as i32;
                    let dy = (location.y - pointer.1).round() as i32;
                    let _ = start;
                    self.move_outline = Some((id, dx, dy));
                }
                Grab::Resize {
                    id,
                    pointer,
                    start,
                    edge,
                } => {
                    let dx = (location.x - pointer.0).round() as i32;
                    let dy = (location.y - pointer.1).round() as i32;
                    self.model.set_rect(id, edge.resized(start, dx, dy));
                    self.apply_geometry(id);
                }
            }
        }
        let focus = self.pointer_focus(location);
        // `focusFollowsMouse` is off by default: without it, entry only updates hover and the
        // click path still owns focus. A drag never steals focus from the window it is carrying.
        if self.focus_follows_mouse && self.grab.is_none() {
            if let Some(Hit::Client(id) | Hit::Titlebar(id)) =
                self.model.hit(location.x, location.y)
            {
                self.focus_hover(id);
            }
        }
        if let Some(pointer) = self.seat.get_pointer() {
            let serial = next_serial();
            pointer.motion(
                self,
                focus,
                &MotionEvent {
                    location,
                    serial,
                    time,
                },
            );
            pointer.frame(self);
        }
    }

    /// Surface under the pointer with its global origin, as smithay expects for focus.
    fn pointer_focus(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let map = layer_map_for_output(&self.output);
        for layer in [Layer::Overlay, Layer::Top] {
            if let Some(layer_surface) = map.layer_under(layer, location) {
                let origin = map
                    .layer_geometry(layer_surface)
                    .map(|g| g.loc.to_f64())
                    .unwrap_or_default();
                if let Some((surface, offset)) =
                    layer_surface.surface_under(location - origin, WindowSurfaceType::ALL)
                {
                    return Some((
                        surface,
                        origin + Point::from((f64::from(offset.x), f64::from(offset.y))),
                    ));
                }
            }
        }
        drop(map);
        // Unmanaged X11 windows: menus and docks first (topmost first), desktop last.
        for entry in self.unmanaged.iter().rev().filter(|entry| !entry.below) {
            if let Some(hit) = unmanaged_focus(entry, location) {
                return Some(hit);
            }
        }
        match pointer_route(self.model.hit(location.x, location.y)) {
            PointerRoute::Client(id) => {
                let rect = self.model.get(id)?.rect;
                let surface = self.window_wl_surface(id)?;
                return Some((surface, Point::from((f64::from(rect.x), f64::from(rect.y)))));
            }
            // The compositor's own decoration owns these pixels: no client surface is under the
            // pointer. Continuing to the below-unmanaged stack would hand the drag motion to the
            // desktop, whose rubber band would then run alongside the window move.
            PointerRoute::Compositor => return None,
            PointerRoute::Unmanaged => {}
        }
        self.unmanaged
            .iter()
            .rev()
            .filter(|entry| entry.below)
            .find_map(|entry| unmanaged_focus(entry, location))
    }

    fn on_button<I: InputBackend>(&mut self, event: &impl PointerButtonEvent<I>) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        let button = event.button_code();
        let pressed = event.state() == ButtonState::Pressed;
        let mods = self
            .seat
            .get_keyboard()
            .map(|keyboard| mods_of(&keyboard.modifier_state()))
            .unwrap_or_default();
        let location = self.pointer;
        let pointer_pos = (location.x, location.y);
        if pressed {
            // Any button press cancels a bare-Super tap, as X11 `press_other` does.
            self.super_tap = false;
        }
        // A release that ends a compositor grab is not forwarded: the client never saw the press.
        let ends_grab = !pressed && self.grab.is_some();
        let mut consumed = ends_grab;

        if !pressed {
            if let Some(Grab::Move { id, pointer, start }) = self.grab {
                let time_ms = event.time_msec();
                let (px, py) = pointer;
                // Motion inside the slop is still a click: X11 uses the same slop, and treating a
                // 1px twitch as a drag would silently disable double-click-to-maximize.
                let drag = self.move_outline.take().map(|(_, dx, dy)| (dx, dy));
                let moved = drag.is_some_and(|(dx, dy)| {
                    let (dx, dy) = (f64::from(dx), f64::from(dy));
                    dx * dx + dy * dy > DOUBLE_CLICK_SLOP_PX * DOUBLE_CLICK_SLOP_PX
                });
                if moved {
                    if let Some((dx, dy)) = drag {
                        self.model.move_to(id, start.x + dx, start.y + dy);
                        self.apply_geometry(id);
                    }
                    // A real drag never counts as either half of a double click.
                    self.last_title_click = None;
                } else if completes_double_click(self.last_title_click, id, time_ms, px, py) {
                    self.last_title_click = None;
                    self.toggle_maximize(id);
                } else {
                    self.last_title_click = Some(TitleClick {
                        id,
                        time_ms,
                        pointer: (px, py),
                    });
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
            let target = match self.model.hit(location.x, location.y) {
                Some(Hit::Client(id) | Hit::Titlebar(id) | Hit::Resize(id)) => Some(id),
                _ => None,
            };
            if let Some(id) = target {
                self.set_focus(Some(id));
                if let Some(rect) = self.model.get(id).map(|w| w.rect) {
                    consumed = true;
                    self.grab = Some(if button == BTN_LEFT {
                        Grab::Move {
                            id,
                            pointer: pointer_pos,
                            start: rect,
                        }
                    } else {
                        Grab::Resize {
                            id,
                            pointer: pointer_pos,
                            start: rect,
                            edge: ResizeEdge::quadrant(rect, location.x, location.y),
                        }
                    });
                }
            }
        }

        if pressed && !consumed && button == BTN_LEFT {
            match self.model.hit(location.x, location.y) {
                Some(Hit::Button(id, action)) => {
                    self.set_focus(Some(id));
                    self.run_titlebar_button(id, action);
                    consumed = true;
                }
                Some(Hit::Titlebar(id)) => {
                    self.set_focus(Some(id));
                    if let Some(start) = self.model.get(id).map(|w| w.rect) {
                        consumed = true;
                        self.grab = Some(Grab::Move {
                            id,
                            pointer: pointer_pos,
                            start,
                        });
                    }
                }
                Some(Hit::Resize(id)) => {
                    self.set_focus(Some(id));
                    if let Some(rect) = self.model.get(id).map(|w| w.rect) {
                        consumed = true;
                        self.grab = Some(Grab::Resize {
                            id,
                            pointer: pointer_pos,
                            start: rect,
                            edge: ResizeEdge::at(rect, location.x, location.y),
                        });
                    }
                }
                Some(Hit::Client(id)) => self.set_focus(Some(id)),
                None => self.set_focus(None),
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
        pointer.axis(self, frame);
        pointer.frame(self);
    }
}

/// Which part of the scene owns the pixels under the pointer.
///
/// A managed window splits three ways: its client area belongs to the client surface, its
/// decoration belongs to the compositor, and neither may leak to the unmanaged stack below
/// (menus, docks and finally the desktop). The X11 engine gets this split structurally — the
/// frame's own input children cover the decoration band and a pointer grab holds the drag, so the
/// desktop never sees it. Here the split has to be decided explicitly.
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

/// Pointer focus on an unmanaged X11 window, when the pointer is inside its geometry.
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
    entry.surface.wl_surface().map(|surface| (surface, origin))
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
            |settings| settings.keyboard_layout,
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
        .is_ok_and(|settings| settings.focus_follows_mouse)
}

/// The keymap the seat installs at startup, read from the saved settings file.
pub fn saved_xkb_config() -> XkbConfig<'static> {
    xkb_config(&saved_keyboard_layout())
}

#[cfg(test)]
mod tests {
    use super::{
        Button, Hit, PointerRoute, TitleClick, completes_double_click, focus_follows_mouse_at,
        focus_follows_mouse_from, keyboard_layout_at, name_candidates, pointer_route, xkb_config,
    };

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

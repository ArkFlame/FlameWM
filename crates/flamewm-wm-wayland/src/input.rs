//! Keyboard and pointer input: shortcut dispatch, pointer focus, move/resize grabs.

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
    KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::desktop::{WindowSurfaceType, layer_map_for_output};
use smithay::input::keyboard::xkb;
use smithay::input::keyboard::{FilterResult, ModifiersState};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use smithay::wayland::shell::wlr_layer::Layer;

use flamewm_api::shortcuts::{
    ACTION_FULLSCREEN_ESCAPE, ACTION_LAUNCHER, ACTION_SCREENSHOT_FULL, ACTION_SCREENSHOT_REGION,
    ACTION_SCREENSHOT_WINDOW, ACTION_SESSION_LOCK, ACTION_SHOW_DESKTOP,
    ACTION_SHOW_DESKTOP_CTRL_ALT, ACTION_TOGGLE_START_MENU, ACTION_WINDOW_CLOSE,
    ACTION_WINDOW_MAXIMIZE, ACTION_WINDOW_MINIMIZE, ACTION_WINDOW_SWITCH, workspace_direction,
};
use flamewm_platform::shortcuts::{
    ACTION_OPEN_FILES_CTRL_ALT, ACTION_OPEN_FILES_SUPER, ACTION_OPEN_TERMINAL_CTRL_ALT,
    ACTION_OPEN_TERMINAL_SUPER,
};

use crate::model::{Button, Hit, Mods, ResizeEdge, WindowId};
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

    pub(crate) fn on_motion(&mut self, location: Point<f64, Logical>, time: u32) {
        self.pointer = location;
        if let Some(grab) = self.grab {
            match grab {
                Grab::Move { id, pointer, start } => {
                    let dx = (location.x - pointer.0).round() as i32;
                    let dy = (location.y - pointer.1).round() as i32;
                    self.model.move_to(id, start.x + dx, start.y + dy);
                    self.apply_geometry(id);
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
        if let Some(Hit::Client(id)) = self.model.hit(location.x, location.y) {
            let rect = self.model.get(id)?.rect;
            let surface = self.window_wl_surface(id)?;
            return Some((surface, Point::from((f64::from(rect.x), f64::from(rect.y)))));
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
                }
                Some(Hit::Titlebar(id)) => {
                    self.set_focus(Some(id));
                    if let Some(start) = self.model.get(id).map(|w| w.rect) {
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

#[cfg(test)]
mod tests {
    use super::name_candidates;

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
}

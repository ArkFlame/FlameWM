//! Shortcut actions: dispatch table, media routing, maximize and focus cycling.

use super::*;

impl State {
    /// Runs one shortcut action. Actions are the shared `flamewm-api` names.
    pub fn dispatch_action(&mut self, action: &str) {
        if let Some(level) = crate::zoom::after_action(self.zoom_level, action) {
            self.zoom_level = level;
            self.zoom_origin = None;
            self.request_redraw();
            return;
        }
        // Media keys are bound by the default registry, so they must reach the shared system
        // service instead of falling through to the unhandled log.
        if let Some(key) = media_key(action) {
            self.dispatch_media_key(key);
            return;
        }
        if let Some(direction) = workspace_direction(action) {
            if let Some(target) = self.model.workspace_step(direction)
                && self.model.switch_workspace(target)
            {
                self.refocus();
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
            if index < self.model.workspace_count()
                && let Some(id) = self.focused
            {
                self.model.move_to_workspace(id, index);
                self.apply_geometry(id);
                self.refocus();
            }
            return;
        }
        // Super+arrows are keyboard tiling, as on X11.
        if let Some(direction) = keyboard_tile::tile_action(action) {
            self.keyboard_tile(direction);
            return;
        }
        // Super+Alt+arrows: focus the nearest visible window on that side, raised as X11 does.
        if let Some(direction) = focus_direction(action) {
            if let Some(target) = self
                .focused
                .and_then(|id| self.model.focus_direction(id, direction))
            {
                self.model.raise(target);
                self.set_focus(Some(target));
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
            // Same mapping as X11 (`flamewm_platform::shortcuts::screenshot_args`); the binary picks
            // the Wayland screencopy path itself when it runs on a native session.
            ACTION_SCREENSHOT_FULL | ACTION_SCREENSHOT_REGION | ACTION_SCREENSHOT_WINDOW => {
                if let Some(args) = flamewm_platform::shortcuts::screenshot_args(action) {
                    self.spawn("flamewm-screenshot", args);
                }
            }
            // The shell owns the launcher and start menu; it listens for the same control signals
            // the X11 WM emits.
            ACTION_LAUNCHER => self.emit_control_signal("LauncherToggleRequested"),
            ACTION_TOGGLE_START_MENU => self.emit_control_signal("StartMenuToggleRequested"),
            ACTION_SESSION_LOCK => eprintln!("flamewm-wayland: session lock is a later phase"),
            ACTION_DOCK_AUTO_HIDE_TOGGLE => match self.control.clone() {
                Some(service) => service.toggle_dock_auto_hide(),
                None => {
                    eprintln!("flamewm-wayland: dock auto-hide toggle needs the control service");
                }
            },
            other => eprintln!("flamewm-wayland: unhandled shortcut action {other}"),
        }
    }

    /// Routes a media key to the control service. The service owns the backlight and the audio
    /// helper; without it the key is a no-op and the missing installation is logged once.
    pub(super) fn dispatch_media_key(&self, key: MediaKey) {
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

    pub(super) fn toggle_maximize(&mut self, id: WindowId) {
        self.refresh_dock_struts();
        let work = self.work_area();
        self.model.toggle_maximize(id, work);
        self.apply_geometry(id);
    }

    /// `Super+J` / `Super+K`: focus the next/previous visible window in stack order, the X11
    /// `cycle_window` step.
    pub(super) fn cycle_focus(&mut self, forward: bool) {
        let target = if forward {
            self.model.cycle_focus()
        } else {
            self.model.cycle_focus_prev()
        };
        if let Some(id) = target {
            self.set_focus(Some(id));
        }
    }
}

//! Keyboard input: key dispatch, media keys, and XKB layout settings.

use super::*;

/// Keymap for the seat's one keyboard, from the saved `keyboardLayout`.
///
/// The layout is resolved by the caller and checked against [`KEYBOARD_LAYOUTS`], so this stays
/// pure: a value outside the list falls back to [`DEFAULT_KEYBOARD_LAYOUT`] instead of leaving the
/// field empty, which would make `xkbcommon` read `XKB_DEFAULT_LAYOUT` from the environment and
/// ignore the setting. Everything else keeps smithay's defaults (system rules/model/variant).
pub(super) fn xkb_config(layout: &str) -> XkbConfig<'static> {
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
pub(super) fn saved_keyboard_layout() -> String {
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

/// The keymap the seat installs at startup, read from the saved settings file.
pub fn saved_xkb_config() -> XkbConfig<'static> {
    xkb_config(&saved_keyboard_layout())
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
    pub(super) fn on_key<I: InputBackend>(&mut self, event: &impl KeyboardKeyEvent<I>) -> bool {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return false;
        };
        let key_state = event.state();
        let keycode = event.key_code();
        let time = event.time_msec();
        let serial = next_serial();
        if !crate::session_lock::shortcuts_active(self.session_locked) {
            // Locked: every key belongs to the lock client. No WM shortcut may act on the windows
            // behind the lock (close, focus cycling, launchers), so none is intercepted.
            let _ = self.pin_focus_to_lock();
            keyboard.input::<(), _>(self, keycode, key_state, serial, time, |_, _, _| {
                FilterResult::Forward
            });
            return false;
        }
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
                    // An open Alt+F7/F8 grab owns its arrow, Return and Escape presses.
                    if state.kbd_grab.is_some()
                        && state.keyboard_grab_key(handle.modified_sym().raw())
                    {
                        state.swallowed_keys.push(keycode.raw());
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
}

use std::borrow::Cow;

use super::input::apply_wheel_scroll;
use super::*;
use crate::WindowGeometrySlot;
use crate::runtime::{RawPress, RawPressWatch};
use crate::xinput::{GENERIC_EVENT, generic_cookie};

fn keyboard_text(symbol: u64) -> Option<String> {
    match symbol {
        XK_BACK_SPACE => Some("\u{8}".to_string()),
        XK_DELETE => Some("\u{7f}".to_string()),
        XK_RETURN => Some("\n".to_string()),
        XK_ESCAPE => Some("\u{1b}".to_string()),
        _ => printable_key_char(symbol).map(|ch| ch.to_string()),
    }
}

/// The desktop is intentionally below managed windows and is not normally
/// focusable through the WM taskbar. A primary click that selects desktop
/// entries must give it keyboard focus so Delete is delivered there. Other
/// roles retain their existing focus owner.
fn desktop_takes_focus_on_press(role: X11WindowRole, button: u32) -> bool {
    role == X11WindowRole::Desktop && matches!(button, 1 | 3)
}

const TYPED_KEY_ACTION: &str = "__flamewm.ui.key";
const KEY_PAYLOAD_SEPARATOR: char = '\u{1f}';

fn normalized_modifiers(state: u32) -> Vec<&'static str> {
    [
        (CONTROL_MASK, "Ctrl"),
        (SHIFT_MASK, "Shift"),
        (MOD1_MASK, "Alt"),
        (MOD4_MASK, "Super"),
    ]
    .into_iter()
    .filter(|(mask, _)| state & mask != 0)
    .map(|(_, name)| name)
    .collect()
}

fn canonical_key_name(symbol: u64) -> Cow<'static, str> {
    match symbol {
        XK_BACK_SPACE => Cow::Borrowed("BackSpace"),
        XK_DELETE => Cow::Borrowed("Delete"),
        XK_TAB => Cow::Borrowed("Tab"),
        XK_SPACE => Cow::Borrowed("Space"),
        XK_RETURN => Cow::Borrowed("Return"),
        XK_ESCAPE => Cow::Borrowed("Escape"),
        XK_LEFT => Cow::Borrowed("Left"),
        XK_UP => Cow::Borrowed("Up"),
        XK_RIGHT => Cow::Borrowed("Right"),
        XK_DOWN => Cow::Borrowed("Down"),
        XK_SUPER_L => Cow::Borrowed("Super_L"),
        XK_SUPER_R => Cow::Borrowed("Super_R"),
        0xffe1 => Cow::Borrowed("Shift_L"),
        0xffe2 => Cow::Borrowed("Shift_R"),
        0xffe3 => Cow::Borrowed("Control_L"),
        0xffe4 => Cow::Borrowed("Control_R"),
        0xffe7 => Cow::Borrowed("Meta_L"),
        0xffe8 => Cow::Borrowed("Meta_R"),
        0xffe9 => Cow::Borrowed("Alt_L"),
        0xffea => Cow::Borrowed("Alt_R"),
        0x1008_ff18 => Cow::Borrowed("XF86HomePage"),
        0x1008_ff2e => Cow::Borrowed("XF86WWW"),
        0x1008_ff1b => Cow::Borrowed("XF86Search"),
        0x1008_ff19 => Cow::Borrowed("XF86Mail"),
        0x1008_ff1d => Cow::Borrowed("XF86Calculator"),
        0x1008_ff5d => Cow::Borrowed("XF86Explorer"),
        0x1008_ff30 => Cow::Borrowed("XF86Favorites"),
        0x1008_ff33 => Cow::Borrowed("XF86MyComputer"),
        0x1008_ff81 => Cow::Borrowed("XF86Tools"),
        0x1008_ff2d => Cow::Borrowed("XF86ScreenSaver"),
        0x1008_ff2f => Cow::Borrowed("XF86Sleep"),
        0x1008_ff14 => Cow::Borrowed("XF86AudioPlay"),
        0x1008_ff15 => Cow::Borrowed("XF86AudioStop"),
        0x1008_ff16 => Cow::Borrowed("XF86AudioPrev"),
        0x1008_ff17 => Cow::Borrowed("XF86AudioNext"),
        0x1008_ff02 => Cow::Borrowed("XF86MonBrightnessUp"),
        0x1008_ff03 => Cow::Borrowed("XF86MonBrightnessDown"),
        0x1008_ff26 => Cow::Borrowed("XF86Back"),
        0x1008_ff27 => Cow::Borrowed("XF86Forward"),
        0x1008_ff29 => Cow::Borrowed("XF86Refresh"),
        0x1008_ff11 => Cow::Borrowed("XF86AudioLowerVolume"),
        0x1008_ff12 => Cow::Borrowed("XF86AudioMute"),
        0x1008_ff13 => Cow::Borrowed("XF86AudioRaiseVolume"),
        0xffbe..=0xffc9 => Cow::Owned(format!("F{}", symbol - 0xffbd)),
        _ => match printable_key_char(symbol) {
            Some(key) => Cow::Owned(key.to_string()),
            None => Cow::Owned(format!("Keysym(0x{symbol:x})")),
        },
    }
}

fn printable_key_char(symbol: u64) -> Option<char> {
    match symbol {
        0x20..=0x7e | 0xa0..=0xff => char::from_u32(symbol as u32),
        _ => None,
    }
}

#[cfg(test)]
fn printable_key_text(symbol: u64) -> Option<String> {
    printable_key_char(symbol).map(|key| key.to_string())
}

/// Reusable buffers for ActionEvent strings. A buffer is moved into the event
/// for the callback and reclaimed afterwards, so steady-state dispatch does not
/// allocate action names or typed-key payloads.
#[derive(Default)]
struct ActionBuffers {
    action: String,
    payload: String,
}

/// Invokes the callback with `event.action` taken from `buffers.action`, then
/// returns that buffer for reuse. `buffers.action` must already hold the name.
fn invoke_action<F>(
    on_action: &mut F,
    document: &mut RuntimeDocument,
    buffers: &mut ActionBuffers,
    event: &mut ActionEvent,
) -> Result<(), String>
where
    F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
{
    event.action = std::mem::take(&mut buffers.action);
    let result = on_action(&*event, document);
    buffers.action = std::mem::take(&mut event.action);
    result
}

/// Dispatches `event` under a static or caller-owned action name.
fn fire_static<F>(
    on_action: &mut F,
    document: &mut RuntimeDocument,
    buffers: &mut ActionBuffers,
    name: &str,
    event: &mut ActionEvent,
) -> Result<(), String>
where
    F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
{
    buffers.action.clear();
    buffers.action.push_str(name);
    invoke_action(on_action, document, buffers, event)
}

/// Dispatches `event` under the action name of `nodes[node]`. Returns Ok(false)
/// without invoking the callback when that name is empty.
fn fire_node<F>(
    on_action: &mut F,
    document: &mut RuntimeDocument,
    buffers: &mut ActionBuffers,
    node: u32,
    event: &mut ActionEvent,
) -> Result<bool, String>
where
    F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
{
    let name: &str = &document.document.nodes[node as usize].action;
    if name.is_empty() {
        return Ok(false);
    }
    buffers.action.clear();
    buffers.action.push_str(name);
    invoke_action(on_action, document, buffers, event)?;
    Ok(true)
}

/// Pointer or key ActionEvent; device coordinates are scaled to logical points.
fn action_event(
    scale: f32,
    phase: ActionPhase,
    button: u32,
    (x, y): (i32, i32),
    inside: bool,
    time_ms: u64,
    text: Option<String>,
) -> ActionEvent {
    ActionEvent {
        action: String::new(),
        phase,
        button,
        x: x as f32 / scale,
        y: y as f32 / scale,
        inside,
        time_ms,
        text,
    }
}

/// Slider value text for a drag whose track starts at `track_x` (logical points).
fn range_drag_text(track_x: f32, track_w: f32, min: f32, max: f32, x: f32) -> String {
    let track = Rect {
        x: track_x,
        y: 0.0,
        width: track_w,
        height: 8.0,
    };
    slider_value_from_pointer(track, min, max, 0.0, x).to_string()
}

/// Keysyms at shift level 0 and at the level the event's Shift state selects.
unsafe fn key_symbols(key: &mut XKeyEvent) -> (u64, u64) {
    let shift = if key.state & SHIFT_MASK != 0 { 1 } else { 0 };
    // SAFETY: key is an initialized XKeyEvent copied from a key XEvent.
    unsafe { (XLookupKeysym(key, 0), XLookupKeysym(key, shift)) }
}

fn emit_typed_key<F>(
    key: &XKeyEvent,
    symbol: u64,
    phase: ActionPhase,
    document: &mut RuntimeDocument,
    on_action: &mut F,
    buffers: &mut ActionBuffers,
) -> Result<(), String>
where
    F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
{
    let modifiers = normalized_modifiers(key.state);
    let key_name = canonical_key_name(symbol);
    let printable = (!modifiers.iter().any(|modifier| *modifier != "Shift"))
        .then(|| printable_key_char(symbol))
        .flatten();
    let mut payload = std::mem::take(&mut buffers.payload);
    payload.clear();
    for (index, modifier) in modifiers.iter().enumerate() {
        if index > 0 {
            payload.push('+');
        }
        payload.push_str(modifier);
    }
    payload.push(KEY_PAYLOAD_SEPARATOR);
    payload.push_str(&key_name);
    payload.push(KEY_PAYLOAD_SEPARATOR);
    if let Some(key) = printable {
        payload.push(key);
    }
    let mut event = action_event(
        document.ui_scale(),
        phase,
        0,
        (key.x, key.y),
        true,
        key.time as u64,
        Some(payload),
    );
    let result = fire_static(on_action, document, buffers, TYPED_KEY_ACTION, &mut event);
    buffers.payload = event.text.take().unwrap_or_default();
    result
}

impl X11App {
    pub(crate) unsafe fn event_loop_once<F>(
        &mut self,
        document: &mut RuntimeDocument,
        on_action: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
    {
        self.single_event = true;
        // SAFETY: This wrapper is itself called under the same live-display invariant.
        let result = unsafe { self.event_loop(document, on_action) };
        self.single_event = false;
        result
    }

    pub(crate) unsafe fn event_loop<F>(
        &mut self,
        document: &mut RuntimeDocument,
        on_action: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
    {
        // No unconditional redraw on entry: presenters start from the
        // retained scene; damage/expose drive paints via redraw_if_dirty.
        let mut buffers = ActionBuffers::default();
        loop {
            let mut event = MaybeUninit::<XEvent>::uninit();
            // SAFETY: XNextEvent initializes the supplied XEvent storage before returning.
            unsafe { XNextEvent(self.display, event.as_mut_ptr()) };
            // SAFETY: XNextEvent wrote a complete XEvent into the MaybeUninit storage.
            let event = unsafe { event.assume_init() };
            // Root raw presses carry no window of ours; handle them before the window filter.
            // SAFETY: type_ is the common discriminator at the start of every XEvent variant.
            if unsafe { event.type_ } == GENERIC_EVENT {
                if Some(unsafe { generic_cookie(&event) }.extension)
                    == self.press_selection.opcode()
                {
                    // SAFETY: the event is a GenericEvent of the selected XI extension.
                    unsafe { self.record_raw_press(&event, document.ui_scale()) };
                }
                if self.single_event {
                    return Ok(());
                }
                continue;
            }
            // SAFETY: xany is the common XEvent prefix and is valid for every event type.
            if self.single_event && unsafe { event.xany.window } != self.window {
                // SAFETY: The event was returned by XNextEvent and remains initialized.
                unsafe { XPutBackEvent(self.display, &event as *const XEvent as *mut XEvent) };
                return Ok(());
            }
            // SAFETY: type_ is the common discriminator at the start of every XEvent variant.
            match unsafe { event.type_ } {
                EXPOSE => {
                    // Expose is present-only: re-blit the retained scene,
                    // never recompute layout/paint. Coalesce counts.
                    if unsafe { event.xexpose.count } == 0 {
                        // SAFETY: The caller owns a live X11 display and app resources.
                        unsafe { self.present_retained(document)? };
                    }
                }
                MAP_NOTIFY => {
                    // Native presenter: MapNotify advances Painted -> Mapped.
                    // Stray MapNotify in any other state is ignored, never an error.
                    let _ = self.note_mapped();
                }
                CONFIGURE_NOTIFY => {
                    // Reconcile-only: adopt the WM-driven size into retained
                    // state (recreate backbuffer, mark Full, shape follows).
                    // No XMoveResizeWindow echo: the native move is owned by
                    // SurfaceController::move_resize only.
                    let configure = unsafe { event.xconfigure };
                    let width = configure.width.max(1) as u32;
                    let height = configure.height.max(1) as u32;
                    if width != self.width || height != self.height {
                        self.width = width;
                        self.height = height;
                        // SAFETY: The display, window, and retained backbuffer belong to this app.
                        unsafe { self.recreate_backbuffer()? };
                        self.mark_full();
                        // SAFETY: window and display are live; mask follows resize.
                        unsafe { self.refresh_shape_mask(document) };
                    }
                }
                MOTION_NOTIFY => {
                    // Pointer motion can arrive much faster than a full scene redraw.
                    // Always consume the newest queued motion for this window before
                    // mutating layout/painting so interaction never trails behind the
                    // physical pointer by a backlog of stale coordinates.
                    // SAFETY: The discriminator identifies XMotionEvent in this match arm.
                    let mut motion = unsafe { event.xmotion };
                    loop {
                        // Only coalesce contiguous MotionNotify events. Do not
                        // search past ButtonRelease/other events, because doing
                        // so would violate X event ordering and could extend a
                        // drag beyond its release point. QUEUED_AFTER_READING also
                        // pulls already-arrived socket input into Xlib when the
                        // in-memory queue empties, preventing a nested X server
                        // from feeding us stale motion one packet at a time.
                        // SAFETY: self.display is the live display used by this event loop.
                        if unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) } <= 0 {
                            break;
                        }
                        let mut peeked = MaybeUninit::<XEvent>::uninit();
                        // SAFETY: XPeekEvent initializes the supplied XEvent storage.
                        unsafe { XPeekEvent(self.display, peeked.as_mut_ptr()) };
                        // SAFETY: XPeekEvent wrote a complete XEvent into the storage.
                        let peeked = unsafe { peeked.assume_init() };
                        // SAFETY: type_ and xany are the common XEvent prefix.
                        if unsafe { peeked.type_ } != MOTION_NOTIFY
                            // SAFETY: xany is valid for the event after checking its common prefix.
                            || unsafe { peeked.xany.window } != self.window
                        {
                            break;
                        }
                        let mut queued = MaybeUninit::<XEvent>::uninit();
                        // SAFETY: XNextEvent initializes the supplied XEvent storage.
                        unsafe { XNextEvent(self.display, queued.as_mut_ptr()) };
                        // SAFETY: The peeked event established a contiguous same-window motion
                        // event; XNextEvent removes that exact event from the queue.
                        let queued = unsafe { queued.assume_init() };
                        // SAFETY: The contiguous event was checked as MOTION_NOTIFY above.
                        motion = unsafe { queued.xmotion };
                    }
                    let hover = self.hit_test(document, motion.x as f32, motion.y as f32);
                    let changed = hover != self.interaction.hover;
                    self.interaction.hover = hover;
                    self.update_cursor(document, hover);
                    // Range drag: active node is a slider track; emit semantic
                    // delta via existing ActionEvent path (Motion, text=value).
                    if let Some((drag_index, track_x, track_w, min, max)) = self.range_drag {
                        let scale = document.ui_scale();
                        let text =
                            range_drag_text(track_x, track_w, min, max, motion.x as f32 / scale);
                        let mut event = action_event(
                            scale,
                            ActionPhase::Motion,
                            self.pointer_button.unwrap_or(1),
                            (motion.x, motion.y),
                            hover == Some(drag_index),
                            motion.time as u64,
                            Some(text),
                        );
                        if fire_node(on_action, document, &mut buffers, drag_index, &mut event)? {
                            self.mark_full();
                            if self.single_event {
                                unsafe { self.redraw_if_dirty(document)? };
                                return Ok(());
                            }
                            continue;
                        }
                    }
                    if let (Some(button), Some(index)) =
                        (self.pointer_button, self.interaction.active)
                    {
                        let mut event = action_event(
                            document.ui_scale(),
                            ActionPhase::Motion,
                            button,
                            (motion.x, motion.y),
                            hover == Some(index),
                            motion.time as u64,
                            None,
                        );
                        if fire_node(on_action, document, &mut buffers, index, &mut event)?
                            || changed
                        {
                            self.mark_full();
                        }
                    } else {
                        // Hover is a *pointer-position* event, not only an enter event.
                        // Radial controls can cross distinct polar sectors while hit-testing
                        // the same retained node. Dispatch on each motion and let semantic
                        // owners deduplicate unchanged state. Only changes of hovered node
                        // force a renderer redraw; actual document mutations carry damage.
                        if let Some(index) = hover {
                            let mut event = action_event(
                                document.ui_scale(),
                                ActionPhase::Hover,
                                0,
                                (motion.x, motion.y),
                                true,
                                motion.time as u64,
                                None,
                            );
                            fire_node(on_action, document, &mut buffers, index, &mut event)?;
                        }
                        if changed {
                            self.mark_full();
                        }
                    }
                }
                LEAVE_NOTIFY => {
                    let had_hover = self.interaction.hover.take().is_some();
                    self.set_cursor(CursorKind::Default);
                    if had_hover {
                        self.mark_full();
                    }
                }
                KEY_PRESS => {
                    // SAFETY: The discriminator identifies XKeyEvent in this match arm.
                    let mut key = unsafe { event.xkey };
                    // SAFETY: The discriminator identifies XKeyEvent in this match arm.
                    let (symbol, typed_symbol) = unsafe { key_symbols(&mut key) };
                    emit_typed_key(
                        &key,
                        typed_symbol,
                        ActionPhase::Press,
                        document,
                        on_action,
                        &mut buffers,
                    )?;
                    let ctrl = key.state & CONTROL_MASK != 0;
                    let alt = key.state & MOD1_MASK != 0;
                    let super_key = key.state & MOD4_MASK != 0;
                    let action = if alt && symbol == XK_TAB {
                        self.super_chord_used |= super_key;
                        Some("keyboard.alt-tab")
                    } else if ctrl && !alt && !super_key && symbol == XK_SPACE {
                        Some("keyboard.start.1")
                    } else if ctrl && super_key {
                        self.super_chord_used = true;
                        match symbol {
                            XK_LEFT => Some("keyboard.desktop.left.0"),
                            XK_RIGHT => Some("keyboard.desktop.right.0"),
                            XK_UP => Some("keyboard.desktop.up.0"),
                            XK_DOWN => Some("keyboard.desktop.down.0"),
                            _ => None,
                        }
                    } else if alt && !ctrl && !super_key {
                        match symbol {
                            XK_LEFT => Some("keyboard.desktop.left.1"),
                            XK_RIGHT => Some("keyboard.desktop.right.1"),
                            XK_UP => Some("keyboard.desktop.up.1"),
                            XK_DOWN => Some("keyboard.desktop.down.1"),
                            _ => None,
                        }
                    } else {
                        if super_key && symbol != XK_SUPER_L && symbol != XK_SUPER_R {
                            self.super_chord_used = true;
                        }
                        None
                    };
                    if let Some(action) = action {
                        let mut event = action_event(
                            document.ui_scale(),
                            ActionPhase::Release,
                            0,
                            (key.x, key.y),
                            true,
                            key.time as u64,
                            None,
                        );
                        fire_static(on_action, document, &mut buffers, action, &mut event)?;
                        self.mark_full();
                    } else if !ctrl && !alt && !super_key {
                        if let Some(text) = keyboard_text(typed_symbol) {
                            let mut event = action_event(
                                document.ui_scale(),
                                ActionPhase::Release,
                                0,
                                (key.x, key.y),
                                true,
                                key.time as u64,
                                Some(text),
                            );
                            fire_static(
                                on_action,
                                document,
                                &mut buffers,
                                "keyboard.input",
                                &mut event,
                            )?;
                            self.mark_full();
                        }
                    }
                }
                KEY_RELEASE => {
                    // SAFETY: The discriminator identifies XKeyEvent in this match arm.
                    let mut key = unsafe { event.xkey };
                    // SAFETY: The discriminator identifies XKeyEvent in this match arm.
                    let (symbol, typed_symbol) = unsafe { key_symbols(&mut key) };
                    emit_typed_key(
                        &key,
                        typed_symbol,
                        ActionPhase::Release,
                        document,
                        on_action,
                        &mut buffers,
                    )?;
                    if symbol == XK_SUPER_L || symbol == XK_SUPER_R {
                        if !self.super_chord_used {
                            let mut event = action_event(
                                document.ui_scale(),
                                ActionPhase::Release,
                                0,
                                (key.x, key.y),
                                true,
                                key.time as u64,
                                None,
                            );
                            fire_static(
                                on_action,
                                document,
                                &mut buffers,
                                "keyboard.start.0",
                                &mut event,
                            )?;
                            self.mark_full();
                        }
                        self.super_chord_used = false;
                    }
                }
                BUTTON_PRESS => {
                    // SAFETY: The discriminator identifies XButtonEvent in this match arm.
                    let button = unsafe { event.xbutton };
                    let raw = pointer_button_from_raw(button.button);
                    if desktop_takes_focus_on_press(self.role, button.button) {
                        // SAFETY: The X11App owns the live display and mapped
                        // desktop window; focus changes only after a real
                        // primary button press, never on ordinary mouse motion.
                        unsafe {
                            crate::surface_controller::raw_press::XSetInputFocus(
                                self.display,
                                self.window,
                                crate::surface_controller::raw_press::REVERT_TO_POINTER_ROOT,
                                button.time,
                            );
                        }
                    }
                    // Wheel Button4/5: scroll delta via existing ActionEvent path
                    // (semantic "scroll" action, Motion phase), no press state.
                    if let Some((dx, dy)) = wheel_scroll_delta(raw) {
                        let scale = document.ui_scale();
                        if let Some(layout) = self.layout.as_ref() {
                            apply_wheel_scroll(
                                layout,
                                document,
                                self.interaction,
                                button.x as f32 / scale,
                                button.y as f32 / scale,
                                (dx, dy),
                            );
                        }
                        let mut event = action_event(
                            scale,
                            ActionPhase::Motion,
                            button.button,
                            (button.x, button.y),
                            true,
                            button.time as u64,
                            Some(format!("{dx},{dy}")),
                        );
                        fire_static(on_action, document, &mut buffers, "scroll", &mut event)?;
                        self.mark_full();
                        if self.single_event {
                            unsafe { self.redraw_if_dirty(document)? };
                            return Ok(());
                        }
                        continue;
                    }
                    let hit = self.hit_test(document, button.x as f32, button.y as f32);
                    if button.button == 1 {
                        if let Some(index) = hit {
                            let now = button.time as u64;
                            let name: &str = &document.document.nodes[index as usize].action;
                            let is_title_move =
                                name.starts_with("window.") && name.ends_with(".move");
                            let double_title = is_title_move
                                && self
                                    .last_title_press
                                    .is_some_and(|(last_index, last_time)| {
                                        last_index == index && now.saturating_sub(last_time) <= 350
                                    });
                            if double_title {
                                let maximize = name
                                    .strip_prefix("window.")
                                    .and_then(|rest| rest.strip_suffix(".move"))
                                    .map(|prefix| format!("{prefix}.maximize"));
                                self.last_title_press = None;
                                self.interaction.active = None;
                                self.pointer_button = None;
                                if let Some(maximize) = maximize {
                                    let mut event = action_event(
                                        document.ui_scale(),
                                        ActionPhase::Release,
                                        button.button,
                                        (button.x, button.y),
                                        true,
                                        button.time as u64,
                                        None,
                                    );
                                    fire_static(
                                        on_action,
                                        document,
                                        &mut buffers,
                                        &maximize,
                                        &mut event,
                                    )?;
                                }
                                self.mark_full();
                                unsafe { self.redraw_if_dirty(document)? };
                                return Ok(());
                            }
                            if is_title_move {
                                self.last_title_press = Some((index, now));
                            } else {
                                self.last_title_press = None;
                            }
                        } else {
                            self.last_title_press = None;
                        }
                        self.interaction.active = hit;
                        self.pointer_button = Some(button.button);
                        if let Some(index) = hit {
                            let mut event = action_event(
                                document.ui_scale(),
                                ActionPhase::Press,
                                button.button,
                                (button.x, button.y),
                                true,
                                button.time as u64,
                                None,
                            );
                            fire_node(on_action, document, &mut buffers, index, &mut event)?;
                        }
                        // Range drag arm: action names starting with "range." or
                        // "slider." capture the press as a slider drag; pointer
                        // geometry comes from the retained layout box.
                        if let Some(index) = hit {
                            let press_action: &str =
                                &document.document.nodes[index as usize].action;
                            if press_action.starts_with("range.")
                                || press_action.starts_with("slider.")
                            {
                                if let Some(rect) = self.node_global_rect(document, index) {
                                    self.range_drag = Some((
                                        index,
                                        rect.x / document.ui_scale(),
                                        rect.width / document.ui_scale(),
                                        0.0,
                                        100.0,
                                    ));
                                }
                            }
                        }
                        self.mark_full();
                    }
                }
                BUTTON_RELEASE => {
                    // SAFETY: The discriminator identifies XButtonEvent in this match arm.
                    let button = unsafe { event.xbutton };
                    // End any active range drag; emit final value via Release.
                    if button.button == 1 {
                        if let Some((drag_index, track_x, track_w, min, max)) =
                            self.range_drag.take()
                        {
                            let text = range_drag_text(
                                track_x,
                                track_w,
                                min,
                                max,
                                button.x as f32 / document.ui_scale(),
                            );
                            let mut event = action_event(
                                document.ui_scale(),
                                ActionPhase::Release,
                                button.button,
                                (button.x, button.y),
                                true,
                                button.time as u64,
                                Some(text),
                            );
                            fire_node(on_action, document, &mut buffers, drag_index, &mut event)?;
                        }
                    }
                    let hit = self.hit_test(document, button.x as f32, button.y as f32);
                    let dispatch = if button.button == 1 {
                        self.pointer_button = None;
                        self.interaction.active.take()
                    } else if button.button == 2 || button.button == 3 {
                        hit
                    } else {
                        None
                    };
                    if let Some(index) = dispatch {
                        let mut event = action_event(
                            document.ui_scale(),
                            ActionPhase::Release,
                            button.button,
                            (button.x, button.y),
                            hit == Some(index),
                            button.time as u64,
                            None,
                        );
                        fire_node(on_action, document, &mut buffers, index, &mut event)?;
                    }
                    self.mark_full();
                    let hover = self.hit_test(document, button.x as f32, button.y as f32);
                    self.interaction.hover = hover;
                    self.update_cursor(document, hover);
                }
                CLIENT_MESSAGE => {
                    // SAFETY: The discriminator identifies XClientMessageEvent in this arm.
                    let client = unsafe { event.xclient };
                    if self.wm_delete != 0
                        && client.format == 32
                        // SAFETY: XClientMessageEvent format 32 selects the initialized long data
                        // representation used by the WM_DELETE protocol.
                        && unsafe { client.data.l[0] } as u64 == self.wm_delete
                    {
                        self.close_requested = true;
                    }
                }
                _ => {}
            }
            // Coalesce: at most one redraw_if_dirty per drained event.
            // Damage marks above merge (max coverage); Expose presented
            // directly. single_event returns after that single paint.
            unsafe { self.redraw_if_dirty(document)? };
            if self.single_event {
                return Ok(());
            }
        }
    }

    pub(crate) unsafe fn event_loop_with_reactor<F, R, P>(
        &mut self,
        document: &mut RuntimeDocument,
        on_action: &mut F,
        reactor: &mut Reactor,
        on_reactor: &mut R,
        geometry: &WindowGeometrySlot,
        press_watch: &RawPressWatch,
        on_press: &mut P,
    ) -> Result<(), String>
    where
        F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
        R: FnMut(&mut RuntimeDocument) -> Result<(), String>,
        P: FnMut(&RawPress, &mut RuntimeDocument) -> Result<(), String>,
    {
        // No unconditional redraw on entry; reactor ticks/damage drive paints.
        loop {
            // SAFETY: the press watch is only read here; sync touches this app's live display.
            unsafe { self.sync_raw_press_observer(press_watch) };
            // XNextEvent can leave already-read events in Xlib's queue after
            // single_event returns. Avoid blocking on the fd reactor in that case.
            let timeout = (unsafe { XEventsQueued(self.display, 0) } > 0)
                .then_some(std::time::Duration::ZERO);
            reactor
                .dispatch(timeout)
                .map_err(|error| error.to_string())?;
            on_reactor(document)?;
            // Pending geometry request (if any) is applied before the damage
            // gate so the resized window repaints in this same wake.
            let requested = geometry.take();
            unsafe { self.apply_window_geometry(document, requested)? };
            // Reactor callbacks may mutate the document without producing
            // an X event. The retained damage/revision gate presents that
            // mutation once and skips no-op wakes.
            unsafe { self.redraw_if_dirty(document)? };
            // SAFETY: self.display is the live display owned by this app.
            if unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) } <= 0 {
                if self.close_requested {
                    return Ok(());
                }
                continue;
            }
            self.single_event = true;
            // SAFETY: The caller owns the live X11 display and app resources.
            unsafe { self.event_loop(document, on_action)? };
            self.single_event = false;
            // The event may have opened a menu: arm before the next wake.
            unsafe { self.sync_raw_press_observer(press_watch) };
            unsafe { self.deliver_raw_presses(document, on_press)? };
            if self.close_requested {
                return Ok(());
            }
        }
    }

    pub(crate) fn hit_test(&self, document: &RuntimeDocument, x: f32, y: f32) -> Option<u32> {
        // A pointer pixel is painted when its center is covered, so sample the
        // center: a fractional layout edge (e.g. y=451.3) must not hide the
        // top pixel row that the retained paint already shows as the control.
        let (x, y) = pointer_sample_point(x, y, document.ui_scale());
        self.layout
            .as_ref()
            .and_then(|layout| layout.hit_test_action(document, x, y))
    }

    pub(crate) unsafe fn refresh_shape_mask(&self, document: &RuntimeDocument) {
        // Retained path: derive VisualCoverage from the cached paint vector.
        // No layout clone, no command rebuild, no second coverage per frame.
        let Some(cached) = self.cached_paint.as_ref() else {
            return;
        };
        let backdrop_radius =
            super::render::backdrop_corner_radius_px(document, self.width, self.height);
        unsafe {
            self.refresh_shape_mask_from_coverage_scaled(&cached.commands, 1.0, backdrop_radius)
        };
    }

    pub(crate) unsafe fn recreate_backbuffer(&mut self) -> Result<(), String> {
        // Resize retarget lifecycle: checked replacement first, retarget
        // Xft/XRender (Pictures freed before Pixmaps on failure paths),
        // then swap + free the stale pixmap through the allocator ledger.
        // SAFETY: self.display and self.window are live handles owned by this app.
        let (root_w, root_h) = self.root_extent();
        let (replacement, replacement_bytes) = unsafe {
            self.allocator.create_pixmap(
                self.display,
                self.window,
                self.width,
                self.height,
                self.depth as u32,
                root_w,
                root_h,
            )
        }
        .map_err(|error| format!("retained backbuffer resize: {error}"))?;
        // Free the old XRender destination Picture before the old Pixmap:
        // retarget creates the replacement picture first, then the backend
        // drops the stale picture; the stale pixmap is freed only after swap.
        if let Some(xft) = self.xft.as_mut() {
            // SAFETY: `replacement` is a fresh pixmap on the live display.
            unsafe { xft.set_drawable(replacement) };
        }
        if let Some(xrender) = self.xrender.as_mut() {
            // SAFETY: `replacement` is a fresh pixmap on the live display.
            if let Err(error) = unsafe { xrender.set_drawable(replacement) } {
                if let Some(xft) = self.xft.as_mut() {
                    // SAFETY: restore the previous live backbuffer on the same display.
                    unsafe { xft.set_drawable(self.backbuffer) };
                }
                // SAFETY: replacement is allocator-owned on self.display.
                unsafe {
                    self.allocator
                        .free_pixmap(self.display, replacement, replacement_bytes)
                };
                return Err(error);
            }
        }
        let previous = self.backbuffer;
        let previous_bytes = self.backbuffer_bytes;
        self.backbuffer = replacement;
        self.backbuffer_bytes = replacement_bytes;
        refresh_backbuffer_gauge(replacement_bytes as u64);
        if let Some(surface) = self.surface.as_mut() {
            surface.pixmap = replacement;
        }
        if previous != 0 {
            // Keep the old backbuffer alive until the replacement is
            // painted and bound via
            // bind_presented_backbuffer_as_window_background (first full
            // redraw after this swap). The server may still reference the
            // old pixmap as the window background; freeing it now would
            // leave a dangling background reference. Never bind the
            // unpainted replacement here.
            if let Some((stale, stale_bytes)) =
                self.retired_backbuffer.replace((previous, previous_bytes))
            {
                // Defensive: only one retired generation is expected; free
                // any superseded stale id that is not the bound background.
                if stale != 0 && stale != self.background_bound && stale != replacement {
                    unsafe {
                        self.allocator.free_pixmap(self.display, stale, stale_bytes);
                    }
                }
            }
        }
        Ok(())
    }
}

/// Logical point sampled for a device pointer pixel: its center, so a fractional
/// layout edge cannot hide the top pixel row the retained paint already shows.
pub(crate) fn pointer_sample_point(x: f32, y: f32, scale: f32) -> (f32, f32) {
    ((x + 0.5) / scale, (y + 0.5) / scale)
}

#[cfg(test)]
mod pointer_sample_regressions {
    use super::{Rect, pointer_sample_point};

    #[test]
    fn fractional_track_top_row_is_hit_at_pixel_center() {
        // AP06 measured: the animation slider track laid out at y=451.30 and the
        // pointer pixel row 451 was painted as the track but missed the hit test.
        let track = Rect {
            x: 528.0,
            y: 451.30005,
            width: 180.0,
            height: 12.0,
        };
        assert!(
            !track.contains(531.0, 451.0),
            "corner sample misses the track"
        );
        let (x, y) = pointer_sample_point(531.0, 451.0, 1.0);
        assert!(
            track.contains(x, y),
            "pixel-center sample must hit the track"
        );
    }

    #[test]
    fn pixel_center_sampling_scales_device_pixels_to_logical_points() {
        assert_eq!(pointer_sample_point(10.0, 20.0, 2.0), (5.25, 10.25));
    }
}

#[cfg(test)]
mod reactor_presentation_regressions {
    use super::{
        CONTROL_MASK, MOD4_MASK, X11WindowRole, XK_ESCAPE, XK_LEFT, XK_SUPER_L, canonical_key_name,
        desktop_takes_focus_on_press, normalized_modifiers, printable_key_text,
    };

    fn reactor_loop_source() -> &'static str {
        const SOURCE: &str = include_str!("events.rs");
        let start = SOURCE
            .find("pub(crate) unsafe fn event_loop_with_reactor")
            .expect("reactor event-loop owner remains present");
        let end = SOURCE[start..]
            .find("pub(crate) fn hit_test")
            .map(|offset| start + offset)
            .expect("reactor event-loop owner has a bounded body");
        &SOURCE[start..end]
    }

    #[test]
    fn t18_reactor_only_document_mutation_presents_before_x_queue_gate() {
        let source = reactor_loop_source();
        let callback = source
            .find("on_reactor(document)?")
            .expect("reactor callback is the document mutation boundary");
        let after_callback = &source[callback..];
        let present = after_callback
            .find("redraw_if_dirty(document)?")
            .expect("reactor-only mutation must reach the retained redraw gate");
        let queue_gate = after_callback
            .find("XEventsQueued")
            .expect("X11 queue gate remains after reactor dispatch");
        assert!(
            present < queue_gate,
            "reactor document mutation must be presented before an empty X queue can continue"
        );
    }

    #[test]
    fn t19_noop_reactor_wakes_use_the_dirty_skip_not_an_unconditional_paint() {
        let source = reactor_loop_source();
        assert_eq!(
            source.matches("redraw_if_dirty(document)?").count(),
            1,
            "one retained redraw gate must admit each reactor turn"
        );
        assert!(
            !source.contains("self.redraw("),
            "reactor wake must not become an unconditional full repaint loop"
        );
        let render = include_str!("render.rs");
        assert!(render.contains("if self.pending_damage.is_empty() && !stale"));
        assert!(render.contains("return Ok(false);"));
    }

    #[test]
    fn t20_same_turn_reactor_mutations_coalesce_at_one_final_state_gate() {
        let source = reactor_loop_source();
        assert_eq!(
            source.matches("on_reactor(document)?").count(),
            1,
            "one reactor callback owns the same-turn mutation batch"
        );
        assert_eq!(
            source.matches("redraw_if_dirty(document)?").count(),
            1,
            "the mutation batch must have one retained presentation gate"
        );
        let callback = source
            .find("on_reactor(document)?")
            .expect("reactor callback is present");
        let present = source[callback..]
            .find("redraw_if_dirty(document)?")
            .expect("same-turn mutations must present through the retained gate");
        assert!(present > 0);
    }

    #[test]
    fn desktop_primary_press_claims_focus_for_selected_file_shortcuts() {
        assert!(desktop_takes_focus_on_press(X11WindowRole::Desktop, 1));
        assert!(desktop_takes_focus_on_press(X11WindowRole::Desktop, 3));
        assert!(!desktop_takes_focus_on_press(X11WindowRole::Desktop, 4));
        assert!(!desktop_takes_focus_on_press(X11WindowRole::Normal, 1));
        assert!(!desktop_takes_focus_on_press(X11WindowRole::Dock, 1));
    }

    #[test]
    fn typed_key_projection_uses_shortcut_modifier_order_and_canonical_names() {
        assert_eq!(
            normalized_modifiers(CONTROL_MASK | MOD4_MASK),
            vec!["Ctrl", "Super"]
        );
        assert_eq!(canonical_key_name(XK_LEFT), "Left");
        assert_eq!(canonical_key_name(XK_SUPER_L), "Super_L");
        assert_eq!(printable_key_text(u64::from(b'A')), Some("A".to_string()));
        assert_eq!(printable_key_text(XK_ESCAPE), None);
    }
}

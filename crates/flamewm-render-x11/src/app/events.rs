use std::borrow::Cow;
use std::os::raw::{c_char, c_int, c_uint, c_ulong};

use super::input::apply_wheel_scroll;
use super::xdnd::SELECTION_NOTIFY;
use super::*;
use crate::WindowGeometrySlot;
use crate::runtime::{RawPress, RawPressWatch};
use crate::xinput::{GENERIC_EVENT, generic_cookie};

/// Folds a run of queued ConfigureNotify events into the newest one. `next_matching` yields
/// the next queued event only while it belongs to this window; the first `None` ends the run.
pub(crate) fn newest_configure<T>(first: T, mut next_matching: impl FnMut() -> Option<T>) -> T {
    let mut newest = first;
    while let Some(next) = next_matching() {
        newest = next;
    }
    newest
}

/// How long the reactor may block. A handled event can latch damage without painting (a live
/// backdrop change, a window move that keeps its size); the paint gate runs after the dispatch,
/// so blocking with damage pending would show the stale frame until the next unrelated event.
pub(crate) fn reactor_wait_timeout(
    events_queued: bool,
    damage_pending: bool,
    backdrop_wait: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    let ready = (events_queued || damage_pending).then_some(std::time::Duration::ZERO);
    match (ready, backdrop_wait) {
        (Some(ready), Some(wait)) => Some(ready.min(wait)),
        (ready, wait) => ready.or(wait),
    }
}

#[cfg(test)]
#[path = "events/tests/key_text_regressions.rs"]
mod key_text_regressions;

/// Runtime proof against a private X display (never the host display). Run with
/// `FLAMEWM_KEY_TEST_DISPLAY=:200 cargo test -p flamewm-render-x11 xkb_lookup -- --ignored`.
/// Uses the server's keymap as-is: Shift+2 is `@` on the default us layout.
#[cfg(test)]
#[path = "events/tests/xkb_lookup_runtime.rs"]
mod xkb_lookup_runtime;

#[cfg(test)]
#[path = "events/tests/configure_coalescing_regressions.rs"]
mod configure_coalescing_regressions;

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
    matches!(role, X11WindowRole::Desktop | X11WindowRole::PopupMenu) && matches!(button, 1 | 3)
}

const TYPED_KEY_ACTION: &str = "__flamewm.ui.key";
const KEY_PAYLOAD_SEPARATOR: char = '\u{1f}';

fn normalized_modifiers(state: u32) -> impl Iterator<Item = &'static str> {
    [
        (CONTROL_MASK, "Ctrl"),
        (SHIFT_MASK, "Shift"),
        (MOD1_MASK, "Alt"),
        (MOD4_MASK, "Super"),
    ]
    .into_iter()
    .filter(move |(mask, _)| state & mask != 0)
    .map(|(_, name)| name)
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
#[allow(clippy::too_many_arguments)]
fn action_event(
    scale: f32,
    phase: ActionPhase,
    button: u32,
    (x, y): (i32, i32),
    inside: bool,
    time_ms: u64,
    text: Option<String>,
    modifiers: u32,
) -> ActionEvent {
    ActionEvent {
        action: String::new(),
        phase,
        button,
        modifiers,
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

/// Keysyms `(unshifted, typed)` resolved through the server keymap. Both honor the
/// keyboard group, Level3 (Mod5/AltGr) and CapsLock, so layouts such as `de` or `es`
/// decode `AltGr+Q` as `@`. `unshifted` drives shortcut matching; `typed` names the key.
unsafe fn key_symbols(key: &XKeyEvent) -> (u64, u64) {
    let unshifted = key.state & !(SHIFT_MASK | LOCK_MASK);
    // SAFETY: key is an initialized XKeyEvent copied from a key XEvent.
    unsafe { (lookup_keysym(key, unshifted), lookup_keysym(key, key.state)) }
}

/// Keysym for `key` under `state`. XLookupString ignores Ctrl/Alt/Super for level selection.
unsafe fn lookup_keysym(key: &XKeyEvent, state: c_uint) -> u64 {
    let mut event = *key;
    event.state = state;
    let mut keysym: c_ulong = 0;
    let mut buffer = [0 as c_char; 8];
    // SAFETY: event is a private copy; buffer and keysym outlive the call; no compose status.
    unsafe {
        XLookupString(
            &mut event,
            buffer.as_mut_ptr(),
            buffer.len() as c_int,
            &mut keysym,
            std::ptr::null_mut(),
        )
    };
    keysym as u64
}

/// Whether a key with `state` carries its printable character. Ctrl and Super are always
/// shortcuts. Alt carries the character too: terminals send it as `ESC` + char (Meta), and
/// the start menu types it. Bound shortcuts still win because they are matched before text.
/// Shift and CapsLock are part of the keysym already.
fn inserts_text(state: u32) -> bool {
    state & (CONTROL_MASK | MOD4_MASK) == 0
}

/// Typed-key payload `modifiers US key US text`, e.g. `Ctrl\x1fa\x1f` for Ctrl+A (no text).
/// Text is present only when `inserts_text` allows it; the key name is always set.
fn typed_key_payload(state: u32, symbol: u64, payload: &mut String) {
    payload.clear();
    for (index, modifier) in normalized_modifiers(state).enumerate() {
        if index > 0 {
            payload.push('+');
        }
        payload.push_str(modifier);
    }
    payload.push(KEY_PAYLOAD_SEPARATOR);
    payload.push_str(&canonical_key_name(symbol));
    payload.push(KEY_PAYLOAD_SEPARATOR);
    if let Some(key) = inserts_text(state)
        .then(|| printable_key_char(symbol))
        .flatten()
    {
        payload.push(key);
    }
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
    let mut payload = std::mem::take(&mut buffers.payload);
    typed_key_payload(key.state, symbol, &mut payload);
    let mut event = action_event(
        document.ui_scale(),
        phase,
        0,
        (key.x, key.y),
        true,
        key.time as u64,
        Some(payload),
        key.state,
    );
    let result = fire_static(on_action, document, buffers, TYPED_KEY_ACTION, &mut event);
    buffers.payload = event.text.take().unwrap_or_default();
    result
}

/// Logs a dropped foreign event: the first ones, then every power of two, so a storm stays
/// visible without flooding the log.
fn note_foreign_event_dropped(event: &XEvent) {
    static DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = DROPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if count <= 4 || count.is_power_of_two() {
        // SAFETY: type_ and xany are the common prefix of every XEvent.
        let (kind, window) = unsafe { (event.type_, event.xany.window) };
        eprintln!("FLAMEWM_EVENT_FOREIGN_DROPPED type={kind} window={window:#x} count={count}");
    }
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
            // Root stacking changes and damage on frames behind a live window backdrop carry no
            // window of ours; handle them before the window filter, which would put them back.
            if self.note_live_backdrop_event(&event) {
                if self.single_event {
                    return Ok(());
                }
                continue;
            }
            // SAFETY: type_ is the common discriminator at the start of every XEvent variant.
            if unsafe { event.type_ } == MAPPING_NOTIFY {
                // Keymap changed (layout switch, setxkbmap, xmodmap). Refresh Xlib's cache so
                // later KeyPress decoding uses the new keysyms. Mapping events carry no window,
                // so they are handled before the window filter, which would put them back forever
                // in single-event mode.
                // SAFETY: the discriminator selects XMappingEvent; the copy is a valid event.
                let mut mapping = unsafe { event.xmapping };
                if mapping.request != MAPPING_POINTER {
                    // SAFETY: the display and event are live; Xlib only updates its keymap cache.
                    unsafe { XRefreshKeyboardMapping(&mut mapping) };
                }
                if self.single_event {
                    return Ok(());
                }
                continue;
            }
            // SAFETY: xany is the common XEvent prefix and is valid for every event type.
            if self.single_event && unsafe { event.xany.window } != self.window {
                // Nothing else reads this display, so an event for another window is dropped.
                // Putting it back would hand the same event to the next pass forever: the loop
                // spins at full CPU and never reads input, paints or resizes again.
                note_foreign_event_dropped(&event);
                return Ok(());
            }
            // SAFETY: type_ is the common discriminator at the start of every XEvent variant.
            match unsafe { event.type_ } {
                EXPOSE => {
                    // The bound backbuffer already restored the exposed pixels when it holds the
                    // last frame at the current size. Otherwise a retained scene can predate a size
                    // or document change (a shrink), so the last Expose of a run relayouts and
                    // repaints. Coalesce counts.
                    if unsafe { event.xexpose.count } == 0
                        && !self.exposure_restored_by_background()
                    {
                        // SAFETY: The caller owns a live X11 display and app resources.
                        unsafe { self.repaint_full(document)? };
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
                    // Coalesce: a drag queues many ConfigureNotify; only the newest size is adopted.
                    let window = self.window;
                    let display = self.display;
                    let configure = newest_configure(unsafe { event.xconfigure }, || {
                        if unsafe { XEventsQueued(display, QUEUED_AFTER_READING) } <= 0 {
                            return None;
                        }
                        let mut peeked = MaybeUninit::<XEvent>::uninit();
                        // SAFETY: XPeekEvent initializes the supplied XEvent storage.
                        unsafe { XPeekEvent(display, peeked.as_mut_ptr()) };
                        // SAFETY: XPeekEvent wrote a complete XEvent into the storage.
                        let peeked = unsafe { peeked.assume_init() };
                        // SAFETY: type_ and xany are the common XEvent prefix.
                        if unsafe { peeked.type_ } != CONFIGURE_NOTIFY
                            // SAFETY: xany is valid after checking the common prefix.
                            || unsafe { peeked.xany.window } != window
                        {
                            return None;
                        }
                        let mut queued = MaybeUninit::<XEvent>::uninit();
                        // SAFETY: XNextEvent removes the exact event XPeekEvent reported.
                        unsafe { XNextEvent(display, queued.as_mut_ptr()) };
                        // SAFETY: the removed event was checked as CONFIGURE_NOTIFY above.
                        Some(unsafe { queued.assume_init().xconfigure })
                    });
                    let width = configure.width.max(1) as u32;
                    let height = configure.height.max(1) as u32;
                    if self.window_backdrop.is_some() {
                        self.mark_window_backdrop_stale();
                        self.request_window_backdrop_refresh();
                    }
                    if width != self.width || height != self.height {
                        self.width = width;
                        self.height = height;
                        self.pending_window_resize = Some((width, height));
                        // SAFETY: The display, window, and retained backbuffer belong to this app.
                        unsafe { self.recreate_backbuffer()? };
                        // Shrink only once the resize has settled (no queued X events).
                        // SAFETY: display is live; the queue probe does not consume events.
                        if unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) } <= 0 {
                            unsafe { self.shrink_backbuffer_if_settled()? };
                        }
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
                            motion.state,
                        );
                        if fire_node(on_action, document, &mut buffers, drag_index, &mut event)? {
                            self.mark_input_repaint();
                            if self.single_event && !self.input_backlog() {
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
                            motion.state,
                        );
                        if fire_node(on_action, document, &mut buffers, index, &mut event)?
                            || changed
                        {
                            self.mark_input_repaint();
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
                                motion.state,
                            );
                            fire_node(on_action, document, &mut buffers, index, &mut event)?;
                        }
                        if changed {
                            self.mark_input_repaint();
                        }
                    }
                }
                LEAVE_NOTIFY => {
                    let had_hover = self.interaction.hover.take().is_some();
                    self.set_cursor(CursorKind::Default);
                    if had_hover {
                        self.mark_input_repaint();
                    }
                }
                KEY_PRESS => {
                    // SAFETY: The discriminator identifies XKeyEvent in this match arm.
                    let key = unsafe { event.xkey };
                    // SAFETY: The discriminator identifies XKeyEvent in this match arm.
                    let (symbol, typed_symbol) = unsafe { key_symbols(&key) };
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
                            key.state,
                        );
                        fire_static(on_action, document, &mut buffers, action, &mut event)?;
                        self.mark_input_repaint();
                    } else if inserts_text(key.state) {
                        if let Some(text) = keyboard_text(typed_symbol) {
                            let mut event = action_event(
                                document.ui_scale(),
                                ActionPhase::Release,
                                0,
                                (key.x, key.y),
                                true,
                                key.time as u64,
                                Some(text),
                                key.state,
                            );
                            fire_static(
                                on_action,
                                document,
                                &mut buffers,
                                "keyboard.input",
                                &mut event,
                            )?;
                            self.mark_input_repaint();
                        }
                    }
                }
                KEY_RELEASE => {
                    // SAFETY: The discriminator identifies XKeyEvent in this match arm.
                    let key = unsafe { event.xkey };
                    // SAFETY: The discriminator identifies XKeyEvent in this match arm.
                    let (symbol, typed_symbol) = unsafe { key_symbols(&key) };
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
                                key.state,
                            );
                            fire_static(
                                on_action,
                                document,
                                &mut buffers,
                                "keyboard.start.0",
                                &mut event,
                            )?;
                            self.mark_input_repaint();
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
                            button.state,
                        );
                        fire_static(on_action, document, &mut buffers, "scroll", &mut event)?;
                        self.mark_input_repaint();
                        // A wheel burst shares one paint: defer while more input is queued.
                        if self.single_event && !self.input_backlog() {
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
                                        button.state,
                                    );
                                    fire_static(
                                        on_action,
                                        document,
                                        &mut buffers,
                                        &maximize,
                                        &mut event,
                                    )?;
                                }
                                self.mark_input_repaint();
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
                                button.state,
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
                        self.mark_input_repaint();
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
                                button.state,
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
                            button.state,
                        );
                        fire_node(on_action, document, &mut buffers, index, &mut event)?;
                    }
                    self.mark_input_repaint();
                    let hover = self.hit_test(document, button.x as f32, button.y as f32);
                    self.interaction.hover = hover;
                    self.update_cursor(document, hover);
                }
                SELECTION_NOTIFY => {
                    // SAFETY: The discriminator identifies a SelectionNotify on this display.
                    unsafe { self.handle_xdnd_selection_notify(&event, document, on_action)? };
                }
                CLIENT_MESSAGE => {
                    // SAFETY: The discriminator identifies XClientMessageEvent in this arm.
                    let client = unsafe { event.xclient };
                    // SAFETY: The client message is a live XClientMessageEvent on this display.
                    let xdnd = unsafe { self.handle_xdnd_message(&client, document, on_action)? };
                    if !xdnd && self.wm_delete != 0
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
            // Coalesce: paint only once the X queue is drained, so a burst of input shares one frame.
            // Damage marks above merge (max coverage); Expose presented directly. The reactor turn
            // paints any damage left by an early return once the queue is empty.
            if unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) } <= 0 {
                unsafe { self.redraw_if_dirty(document)? };
            }
            // Size reports coalesce: the latest settled size is delivered once the queue is empty.
            if self.pending_window_resize.is_some()
                && unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) } <= 0
                && let Some((width, height)) = self.pending_window_resize.take()
            {
                let resize = ActionEvent {
                    action: WINDOW_RESIZE_ACTION.to_string(),
                    phase: flamewm_render_core::ActionPhase::Release,
                    button: 0,
                    modifiers: 0,
                    x: width as f32,
                    y: height as f32,
                    inside: true,
                    time_ms: 0,
                    text: None,
                };
                on_action(&resize, document)?;
                // The resize hand-off relayouts the document after the paint above. Repaint
                // now, so a shrink shows the new layout (status strip) without waiting for
                // the next event.
                // SAFETY: the display and retained scene belong to this app's event loop.
                unsafe { self.repaint_full(document)? };
            }
            if self.single_event {
                return Ok(());
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
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
            // A throttled live backdrop refresh wakes the loop when it is due.
            let backdrop_wait = self
                .backdrop_deadline
                .map(|deadline| deadline.saturating_duration_since(std::time::Instant::now()));
            let timeout = reactor_wait_timeout(
                unsafe { XEventsQueued(self.display, 0) } > 0,
                !self.pending_damage.is_empty(),
                backdrop_wait,
            );
            reactor
                .dispatch(timeout)
                .map_err(|error| error.to_string())?;
            if self
                .backdrop_deadline
                .is_some_and(|deadline| std::time::Instant::now() >= deadline)
            {
                self.backdrop_deadline = None;
                self.mark_full();
            }
            on_reactor(document)?;
            // Pending geometry request (if any) is applied before the damage
            // gate so the resized window repaints in this same wake.
            let requested = geometry.take();
            unsafe { self.apply_window_geometry(document, requested)? };
            // Reactor callbacks may mutate the document without producing
            // an X event. The retained damage/revision gate presents that
            // mutation once and skips no-op wakes. Queued X events paint first.
            // SAFETY: self.display is the live display owned by this app.
            if unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) } <= 0 {
                unsafe { self.redraw_if_dirty(document)? };
                if self.close_requested {
                    return Ok(());
                }
                continue;
            }
            self.single_event = true;
            // SAFETY: The caller owns the live X11 display and app resources.
            unsafe { self.event_loop(document, on_action)? };
            self.single_event = false;
            // A UI request made while handling that event starts its modal XDND drag here.
            unsafe { self.start_pending_file_drag() };
            // The event may have opened a menu: arm before the next wake.
            unsafe { self.sync_raw_press_observer(press_watch) };
            unsafe { self.deliver_raw_presses(document, on_press)? };
            if self.close_requested {
                return Ok(());
            }
        }
    }

    /// True while more X input is already queued, so a paint can wait for the burst to drain.
    fn input_backlog(&self) -> bool {
        // SAFETY: the display is live; the probe does not consume events.
        unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) > 0 }
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

    /// Makes the backbuffer hold the current window size. Grow-only pool: a window that
    /// still fits the existing capacity reuses the pixmap and its XRender picture, so a
    /// resize step costs no allocation. Shrinking happens only on settle (see below).
    pub(crate) unsafe fn recreate_backbuffer(&mut self) -> Result<(), String> {
        let root = self.root_extent();
        let target = super::backbuffer_pool::grown_capacity(
            self.backbuffer_cap,
            (self.width, self.height),
            root,
        );
        if target == self.backbuffer_cap {
            return Ok(());
        }
        // SAFETY: forwarded caller contract; the swap owns the display-live handles.
        unsafe { self.swap_backbuffer(target) }
    }

    /// Settle-time shrink: releases an oversized backbuffer once the resize has stopped.
    pub(crate) unsafe fn shrink_backbuffer_if_settled(&mut self) -> Result<(), String> {
        let root = self.root_extent();
        match super::backbuffer_pool::settled_capacity(
            self.backbuffer_cap,
            (self.width, self.height),
            root,
        ) {
            // SAFETY: forwarded caller contract; the swap owns the display-live handles.
            Some(target) => unsafe { self.swap_backbuffer(target) },
            None => Ok(()),
        }
    }

    /// Replaces the backbuffer pixmap with one of `capacity` pixels.
    unsafe fn swap_backbuffer(&mut self, capacity: (u32, u32)) -> Result<(), String> {
        // Resize retarget lifecycle: checked replacement first, retarget
        // Xft/XRender (Pictures freed before Pixmaps on failure paths),
        // then swap + free the stale pixmap through the allocator ledger.
        // SAFETY: self.display and self.window are live handles owned by this app.
        let (root_w, root_h) = self.root_extent();
        let (replacement, replacement_bytes) = unsafe {
            self.allocator.create_pixmap(
                self.display,
                self.window,
                capacity.0,
                capacity.1,
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
        self.backbuffer_cap = capacity;
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
#[path = "events/tests/pointer_sample_regressions.rs"]
mod pointer_sample_regressions;

#[cfg(test)]
#[path = "events/tests/reactor_presentation_regressions.rs"]
mod reactor_presentation_regressions;

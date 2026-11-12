use std::borrow::Cow;
use std::os::raw::{c_char, c_int, c_uint, c_ulong};

use super::input::{apply_wheel_scroll, wheel_node_target};
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

mod event_loop;
mod window;

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
        XK_HOME => Cow::Borrowed("Home"),
        XK_END => Cow::Borrowed("End"),
        XK_PAGE_UP => Cow::Borrowed("Page_Up"),
        XK_PAGE_DOWN => Cow::Borrowed("Page_Down"),
        XK_ISO_LEFT_TAB => Cow::Borrowed("ISO_Left_Tab"),
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
    /// Delivers the latest settled client size once the X queue is empty. Called from both the
    /// event loop and the reactor turn: an early-return event (foreign window, backdrop, mapping)
    /// can drain the queue without reaching the event loop's own check, and the reactor then blocks
    /// until unrelated input, so the resize would otherwise stay unapplied.
    pub(crate) unsafe fn flush_pending_window_resize<F>(
        &mut self,
        document: &mut RuntimeDocument,
        on_action: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
    {
        if unsafe { XEventsQueued(self.display, QUEUED_AFTER_READING) } > 0 {
            return Ok(());
        }
        let Some((width, height)) = self.pending_window_resize.take() else {
            return Ok(());
        };
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
        Ok(())
    }

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

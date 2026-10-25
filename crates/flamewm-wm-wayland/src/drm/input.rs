//! libinput events: pointer motion, keyboard routing and the VT switch chord.

use super::*;

pub(super) fn handle_input(
    state: &mut State,
    drm: &Rc<RefCell<Drm>>,
    event: InputEvent<LibinputInputBackend>,
) {
    match event {
        InputEvent::PointerMotion { event } => {
            // Relative motion: the same absolute path the nested backend uses, clamped to the output.
            let size = state.output_size;
            let x = (state.pointer.x + event.delta_x()).clamp(0.0, f64::from((size.w - 1).max(0)));
            let y = (state.pointer.y + event.delta_y()).clamp(0.0, f64::from((size.h - 1).max(0)));
            state.on_motion((x, y).into(), event.time_msec());
            state.request_redraw();
        }
        InputEvent::Keyboard { event } => {
            match vt_for_key(state, &event) {
                // Ctrl+Alt+Fn is consumed here: the session switches and the key never reaches clients.
                Some(vt) => {
                    if let Err(error) = drm.borrow_mut().session.change_vt(vt) {
                        eprintln!("flamewm-wayland: VT switch to {vt} failed: {error}");
                    }
                }
                None => state.process_input::<LibinputInputBackend>(InputEvent::Keyboard { event }),
            }
        }
        other => state.process_input(other),
    }
}

/// Ctrl+Alt+F1..F12 (evdev KEY_F1..KEY_F12) select virtual terminals 1..12.
fn vt_for_key(state: &State, event: &impl KeyboardKeyEvent<LibinputInputBackend>) -> Option<i32> {
    if event.state() != KeyState::Pressed {
        return None;
    }
    let keyboard = state.seat.get_keyboard()?;
    let mods = keyboard.modifier_state();
    if !(mods.ctrl && mods.alt) {
        return None;
    }
    // xkb keycodes are evdev codes plus 8.
    let code = event.key_code().raw().checked_sub(8)?;
    match code {
        59..=68 => Some((code - 58) as i32),
        87 => Some(11),
        88 => Some(12),
        _ => None,
    }
}

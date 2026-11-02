//! libinput events: pointer motion, pointer acceleration, keyboard routing and the VT switch chord.

use super::*;
use smithay::reexports::input::{AccelProfile, Device, DeviceCapability};
use std::sync::OnceLock;

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
            // Ctrl+Alt+Fn is consumed here: the session switches and the key never reaches clients.
            if let Some(vt) = vt_for_key(state, &event) {
                if let Err(error) = drm.borrow_mut().session.change_vt(vt) {
                    eprintln!("flamewm-wayland: VT switch to {vt} failed: {error}");
                }
            } else {
                state.process_input::<LibinputInputBackend>(InputEvent::Keyboard { event });
                sync_leds(state, drm);
            }
        }
        InputEvent::DeviceAdded { mut device } => {
            if device.has_capability(DeviceCapability::Keyboard) {
                let mut d = drm.borrow_mut();
                device.led_update(d.leds.into());
                d.keyboards.push(device.clone());
            }
            // Every device is configured before the seat sees it, and a device whose pointer
            // acceleration cannot be configured still registers: the failure is a diagnostic.
            apply_pointer_acceleration(&mut device, pointer_input_settings());
            state.process_input::<LibinputInputBackend>(InputEvent::DeviceAdded { device });
        }
        InputEvent::DeviceRemoved { device } => {
            drm.borrow_mut().keyboards.retain(|known| *known != device);
            state.process_input::<LibinputInputBackend>(InputEvent::DeviceRemoved { device });
        }
        other => state.process_input(other),
    }
}

/// Writes the keyboard's Caps/Num/Scroll Lock state to every libinput keyboard when it changed. The
/// kernel no longer drives these LEDs once logind switched the VT keyboard off.
fn sync_leds(state: &State, drm: &Rc<RefCell<Drm>>) {
    let Some(keyboard) = state.seat.get_keyboard() else {
        return;
    };
    let leds = keyboard.led_state();
    let mut d = drm.borrow_mut();
    if d.leds == leds {
        return;
    }
    d.leds = leds;
    for device in &mut d.keyboards {
        device.led_update(leds.into());
    }
}

/// The pointer controls this backend applies to each pointer-capable device: `pointerSpeed` in
/// percent of libinput's normalized range, and `pointerRawInput` as the profile choice.
type PointerInput = (i8, bool);

/// `pointerSpeed` and `pointerRawInput` from the settings file, resolved once per process.
///
/// Read lazily on the first `DeviceAdded` and reused for every later device, so hotplugged and
/// resumed pointers get the same values without re-reading the file on each arrival.
fn pointer_input_settings() -> PointerInput {
    static SETTINGS: OnceLock<PointerInput> = OnceLock::new();
    *SETTINGS.get_or_init(|| pointer_input_at(&crate::control::settings_path()))
}

/// Reads the pointer controls from `path`, falling back to the documented defaults (speed 0,
/// adaptive profile) when the file is missing or a line does not parse.
///
/// Split from `settings_path` so a test can feed a real file instead of the environment.
pub fn pointer_input_at(path: &std::path::Path) -> PointerInput {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| {
            flamewm_platform::settings::ConfigStore::default()
                .parse_snapshot(&text)
                .ok()
        })
        .map_or((0, false), |settings| {
            (settings.pointer_speed, settings.pointer_raw_input)
        })
}

/// libinput's normalized acceleration speed for a `pointerSpeed` percent value.
///
/// libinput takes `[-1.0, 1.0]` with `0.0` at the device default, which is exactly `percent / 100.0`
/// for the accepted `-100..=100`. The result is clamped rather than wrapped, so a value that ever
/// leaves the schema's range (a hand-edited file, a future wider bound) still cannot put libinput
/// outside the range it accepts: the device gets the slowest or fastest setting instead of an error.
fn libinput_speed(percent: i64) -> f64 {
    (percent as f64 / 100.0).clamp(-1.0, 1.0)
}

/// libinput acceleration profile for `pointerRawInput`.
///
/// `Flat` is libinput's "flat" acceleration profile: it applies one constant, device-specific
/// factor to pointer motion instead of the speed-dependent curve, so the pointer does not get
/// progressively faster with motion speed. `Adaptive` is the speed-dependent default of most
/// devices. See [`apply_pointer_acceleration`] for what this deliberately does not do.
fn accel_profile(raw_input: bool) -> AccelProfile {
    if raw_input {
        AccelProfile::Flat
    } else {
        AccelProfile::Adaptive
    }
}

/// Applies [`PointerInput`] to one libinput device.
///
/// `pointerRawInput` is **not** a raw/unaccelerated device mode: libinput exposes no such switch,
/// and neither the kernel evdev node nor libinput's filter chain can be bypassed from here. What is
/// set is the acceleration *profile*: `Flat` (one constant factor per device, no speed-dependent
/// curve) instead of `Adaptive`. Acceleration is still applied, and `pointerSpeed` keeps working on
/// top of either profile.
///
/// Every call is guarded by libinput's own support check, so a device without pointer acceleration
/// (a pure keyboard, a virtual/test device, a touchpad whose driver reports none) is skipped rather
/// than driven into a libinput error. Failures are logged and never propagated: a device that
/// rejects its configuration must still be handed to `process_input` and keep working, and this
/// runs inside event delivery, where a panic would take the session down with it.
fn apply_pointer_acceleration(device: &mut Device, (speed_percent, raw_input): PointerInput) {
    if !device.config_accel_is_available() {
        return;
    }
    let profile = accel_profile(raw_input);
    if device.config_accel_profiles().contains(&profile) {
        if let Err(error) = device.config_accel_set_profile(profile) {
            eprintln!(
                "flamewm-wayland: libinput acceleration profile {profile:?} not applied: {error:?}"
            );
        }
    } else {
        eprintln!(
            "flamewm-wayland: device does not offer the {profile:?} acceleration profile; keeping \
             its current one"
        );
    }
    let speed = libinput_speed(i64::from(speed_percent));
    if let Err(error) = device.config_accel_set_speed(speed) {
        eprintln!("flamewm-wayland: libinput acceleration speed {speed} not applied: {error:?}");
    }
}

/// Ctrl+Alt+F1..F12 (evdev `KEY_F1..KEY_F12`) select virtual terminals 1..12.
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

#[cfg(test)]
mod tests {
    use super::{accel_profile, libinput_speed, pointer_input_at};
    use smithay::reexports::input::AccelProfile;

    /// The percent setting maps onto libinput's normalized range, with 0 as the device default.
    #[test]
    fn speed_percent_maps_onto_libinputs_normalized_range() {
        assert_eq!(libinput_speed(0), 0.0);
        assert_eq!(libinput_speed(100), 1.0);
        assert_eq!(libinput_speed(-100), -1.0);
        assert_eq!(libinput_speed(50), 0.5);
        assert_eq!(libinput_speed(-50), -0.5);
        assert_eq!(libinput_speed(1), 0.01);
    }

    /// The clamp is a boundary, not a wrap: an out-of-range value saturates and never leaves
    /// libinput's accepted `[-1.0, 1.0]`.
    #[test]
    fn speed_mapping_clamps_both_ends_instead_of_leaving_the_range() {
        assert_eq!(libinput_speed(101), 1.0);
        assert_eq!(libinput_speed(-101), -1.0);
        assert_eq!(libinput_speed(i64::MAX), 1.0);
        assert_eq!(libinput_speed(i64::MIN), -1.0);
        for percent in [i64::MIN, -1000, -101, 101, 1000, i64::MAX] {
            let speed = libinput_speed(percent);
            assert!(
                (-1.0..=1.0).contains(&speed),
                "percent {percent} escaped libinput's range with {speed}"
            );
        }
    }

    /// `pointerRawInput` selects libinput's flat profile; off keeps the adaptive default.
    #[test]
    fn raw_input_selects_the_flat_profile_and_off_keeps_adaptive() {
        assert_eq!(accel_profile(true), AccelProfile::Flat);
        assert_eq!(accel_profile(false), AccelProfile::Adaptive);
    }

    /// The readers must reach a real settings file, not only a string.
    #[test]
    fn pointer_settings_come_from_a_real_file_and_fall_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!(
            "flamewm-wayland-pointer-settings-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        std::fs::write(&path, "pointerSpeed=-50\npointerRawInput=true\n").unwrap();
        assert_eq!(pointer_input_at(&path), (-50, true));

        // A missing file, and a file whose value is rejected by the schema, keep the defaults.
        assert_eq!(pointer_input_at(&dir.join("absent.toml")), (0, false));
        let invalid = dir.join("invalid.toml");
        std::fs::write(&invalid, "pointerSpeed=101\n").unwrap();
        assert_eq!(pointer_input_at(&invalid), (0, false));

        std::fs::remove_dir_all(&dir).ok();
    }
}

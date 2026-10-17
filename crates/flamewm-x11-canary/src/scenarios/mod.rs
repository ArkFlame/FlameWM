//! J07 canary native scenarios (pre009): reusable T03/T06/T07/T09/T10/
//! T11/T12/T13 scenario functions with a real XTEST input path.
//!
//! Real-input helpers use the canary-local XTEST extension. Synthetic
//! `SendEvent` delivery may exist elsewhere as a diagnostic fallback but is
//! flagged NEVER-COUNTS-AS-PASS for R02/R03. Every scenario reports which
//! path produced its verdict.

pub mod clipboard;
pub mod debug_retention;
pub mod explicit_layout;
pub mod explicit_shell;
pub mod explicit_tray;
pub mod interaction_regressions;
pub mod popup_lifecycle;
pub mod reactor_presentation;
pub mod snap_border;
pub mod t15_t18;
pub mod window_chrome;
pub mod window_interaction;
pub mod workspaces;

use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConnectionExt as _, CreateWindowAux, EventMask,
    KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT, WindowClass,
};
use x11rb::protocol::xtest::ConnectionExt as _;

/// Verdict path marker: synthetic SendEvent delivery never counts as pass.
pub const SYNTHETIC_NEVER_PASSES: &str = "SYNTHETIC-NEVER-COUNTS-AS-PASS-R02-R03";

/// Which input path produced a scenario verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputPath {
    RealPointer,
    SyntheticFallback,
}

impl InputPath {
    pub fn as_str(self) -> &'static str {
        match self {
            InputPath::RealPointer => "real-pointer",
            InputPath::SyntheticFallback => SYNTHETIC_NEVER_PASSES,
        }
    }
}

const XTEST_DEVICE_ID: u8 = 0;

fn xtest_input(canary: &crate::Canary, type_: u8, detail: u8, root_x: i16, root_y: i16) -> bool {
    canary
        .conn
        .xtest_fake_input(
            type_,
            detail,
            CURRENT_TIME,
            canary.root,
            root_x,
            root_y,
            XTEST_DEVICE_ID,
        )
        .map(|cookie| cookie.check().is_ok())
        .unwrap_or(false)
}

/// Deliver real pointer motion through XTEST, never through `SendEvent`.
pub fn xtest_motion(canary: &crate::Canary, root_x: i16, root_y: i16) -> bool {
    xtest_input(canary, MOTION_NOTIFY_EVENT, 0, root_x, root_y)
}

/// Deliver a real XTEST button press at root coordinates.
pub fn xtest_button_press(canary: &crate::Canary, root_x: i16, root_y: i16, button: u8) -> bool {
    xtest_input(canary, BUTTON_PRESS_EVENT, button, root_x, root_y)
}

/// Deliver a real XTEST button release at root coordinates.
pub fn xtest_button_release(canary: &crate::Canary, root_x: i16, root_y: i16, button: u8) -> bool {
    xtest_input(canary, BUTTON_RELEASE_EVENT, button, root_x, root_y)
}

/// Deliver a bounded real XTEST drag path, including its press and release.
pub fn xtest_drag_path(
    canary: &crate::Canary,
    from: (i16, i16),
    to: (i16, i16),
    button: u8,
    steps: u32,
) -> bool {
    if !xtest_motion(canary, from.0, from.1) || !xtest_button_press(canary, from.0, from.1, button)
    {
        return false;
    }
    let steps = steps.max(1);
    let lerp = |a: i16, b: i16, i: u32| {
        let (a, b) = (i64::from(a), i64::from(b));
        (a + (b - a) * i64::from(i) / i64::from(steps)) as i16
    };
    (1..=steps).all(|i| xtest_motion(canary, lerp(from.0, to.0, i), lerp(from.1, to.1, i)))
        && xtest_button_release(canary, to.0, to.1, button)
}

/// Real click: move the actual pointer and deliver an XTEST button pair.
pub fn real_click(canary: &crate::Canary, root_x: i16, root_y: i16, button: u8) -> bool {
    xtest_motion(canary, root_x, root_y)
        && xtest_button_press(canary, root_x, root_y, button)
        && xtest_button_release(canary, root_x, root_y, button)
}

/// Bounded XTEST self-check: a private override-redirect input window must
/// receive both real XTEST button events. This path never uses `SendEvent`.
pub fn xtest_self_check(canary: &crate::Canary) -> (bool, String) {
    let version = match canary.conn.xtest_get_version(2, 2) {
        Ok(cookie) => match cookie.reply() {
            Ok(reply) => format!("{}.{}", reply.major_version, reply.minor_version),
            Err(error) => return (false, format!("XTEST version reply failed: {error}")),
        },
        Err(error) => return (false, format!("XTEST unavailable: {error}")),
    };
    let wid = match canary.conn.generate_id() {
        Ok(id) => id,
        Err(error) => return (false, format!("XTEST self-check id failed: {error}")),
    };
    let (root_x, root_y) = (1, 1);
    let create = canary.conn.create_window(
        0,
        wid,
        canary.root,
        root_x,
        root_y,
        2,
        2,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new()
            .override_redirect(1_u32)
            .event_mask(EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE),
    );
    if !create.is_ok_and(|cookie| cookie.check().is_ok()) {
        return (
            false,
            format!("XTEST self-check window create failed version={version}"),
        );
    }
    let mapped = canary
        .conn
        .map_window(wid)
        .is_ok_and(|cookie| cookie.check().is_ok())
        && canary.conn.flush().is_ok();
    let delivered = mapped
        && xtest_motion(canary, root_x + 1, root_y + 1)
        && xtest_button_press(canary, root_x + 1, root_y + 1, 1)
        && xtest_button_release(canary, root_x + 1, root_y + 1, 1);
    let mut pressed = false;
    let mut released = false;
    if delivered {
        for _ in 0..32 {
            match canary.conn.poll_for_event() {
                Ok(Some(Event::ButtonPress(event))) if event.event == wid => pressed = true,
                Ok(Some(Event::ButtonRelease(event))) if event.event == wid => released = true,
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
            if pressed && released {
                break;
            }
        }
    }
    let _ = canary.conn.destroy_window(wid);
    let _ = canary.conn.flush();
    let pass = delivered && pressed && released;
    (
        pass,
        format!(
            "version={version} window={wid} button_press={pressed} button_release={released} path=xtest-real-input send_event=false"
        ),
    )
}

/// Real Escape keypress through XTEST. The keycode is resolved from the server
/// keyboard mapping for keysym XK_Escape (0xff1b); false if unmapped or not
/// delivered.
pub fn real_escape(canary: &crate::Canary) -> bool {
    const XK_ESCAPE: u32 = 0xff1b;
    let setup = canary.conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let Ok(cookie) = canary.conn.get_keyboard_mapping(min, max - min + 1) else {
        return false;
    };
    let Ok(mapping) = cookie.reply() else {
        return false;
    };
    let per_code = usize::from(mapping.keysyms_per_keycode.max(1));
    let Some(index) = mapping
        .keysyms
        .chunks(per_code)
        .position(|syms| syms.contains(&XK_ESCAPE))
    else {
        return false;
    };
    let Ok(offset) = u8::try_from(index) else {
        return false;
    };
    let keycode = min + offset;
    xtest_input(canary, KEY_PRESS_EVENT, keycode, 0, 0)
        && xtest_input(canary, KEY_RELEASE_EVENT, keycode, 0, 0)
}

/// Root-coordinate center of a window info record.
pub fn center(w: &crate::WinInfo) -> (i16, i16) {
    (
        w.x.saturating_add(w.width as i16 / 2),
        w.y.saturating_add(w.height as i16 / 2),
    )
}

/// J07 dispatcher: `j07 --scenario <name|all> [--json]`, CLI-compat additive.
pub fn cmd_j07(canary: &crate::Canary, args: &[String]) {
    let scenario = crate::arg(args, "--scenario").unwrap_or_else(|| "all".to_owned());
    let json = crate::has(args, "--json");
    if scenario == "xtest-self-check" {
        let (pass, detail) = xtest_self_check(canary);
        if json {
            println!("{{\"scenario\":\"XTEST\",\"pass\":{pass},\"detail\":{detail:?}}}");
        } else {
            println!("CANARY_XTEST_SELF_CHECK pass={pass} {detail}");
        }
        if !pass {
            std::process::exit(1);
        }
        return;
    }
    let mut results: Vec<(String, bool, String)> = Vec::new();
    type Scenario = fn(&crate::Canary, &[String]) -> (bool, String);
    let s = scenario.as_str();
    // (selectors, verdict name, scenario). `all` selects only the default group.
    #[rustfmt::skip]
    let table: &[(&[&str], &str, Scenario)] = &[
        (&["all", "popup"], "t03-popup-lifecycle", popup_lifecycle::run_t03),
        (&["all", "move"], "t06-real-move", window_interaction::run_t06),
        (&["all", "resize"], "t07-real-resize-8", window_interaction::run_t07),
        (&["all", "regressions"], "j24-j25-interaction-regressions", interaction_regressions::run),
        (&["all", "chrome"], "t09-t10-chrome-client", window_chrome::run_t09_t10),
        (&["all", "max-snap"], "t11-max-snap-fullscreen", workspaces::run_t11),
        (&["all", "workspaces"], "t12-workspace-churn", workspaces::run_t12),
        (&["t21", "t22", "t21-t22"], "t21-t22-workspace-sticky-convergence", workspaces::run_t21_t22),
        (&["all", "retention"], "t13-debug-retention", |_, a| debug_retention::run_t13(a)),
        (&["t15", "t15-t18"], "t15-native-snap-preview", t15_t18::run_t15),
        (&["t16", "t15-t18"], "t16-mixed-responsiveness", t15_t18::run_t16),
        (&["t17", "t15-t18"], "t17-protected-interactions", t15_t18::run_t17),
        (&["t18", "t15-t18"], "t18-desktop-selection-admission", t15_t18::run_t18),
        (&["reactor", "reactor-presentation"], "t18-t20-reactor-presentation-admission", reactor_presentation::run),
        (&["snap-border", "s1"], "s1-physical-corner-quarters", snap_border::run_s1),
        (&["snap-border", "s2"], "s2-snapped-border-zero", snap_border::run_s2),
        (&["snap-border", "s3"], "s3-frame-extents-per-mode", snap_border::run_s3),
        (&["snap-border", "s4"], "s4-fixed-hints-client-stable", snap_border::run_s4),
        // Explicit-only shell scenarios (B14): never part of `all`; need --shot-dir.
        (&["explicit-taskbar-menu"], "shell-s1-taskbar-background-menu", explicit_shell::run_s1),
        (&["explicit-desktop-settings"], "shell-s2-desktop-menu-settings", explicit_shell::run_s2),
        (&["explicit-audio-tabs"], "shell-s3-audio-popup-tabs", explicit_shell::run_s3),
        // B15 explicit-only: private audio stack (tools/flamewm_osd_audio_acceptance.py), never in `all`.
        (&["explicit-osd"], "shell-osd-volume-card", explicit_shell::run_osd),
        (&["explicit-audio-apps"], "shell-audio-popup-applications", explicit_shell::run_audio_apps),
        // V7 A14 explicit-only layout probe (overflow limits, taskbar stacking, snap, maximize).
        (&["explicit-window-overflow"], "layout-window-overflow", explicit_layout::run_overflow),
        // G2 explicit-only ConfigureRequest clamp (3/4 overflow limit in four directions).
        (&["explicit-window-clamp"], "layout-window-clamp", explicit_layout::run_clamp),
        // A15 explicit-only tray toggle (start, audio, network): never part of `all`.
        (&["explicit-tray-toggle"], "tray-toggle-explicit", explicit_tray::run_toggle),
        // Explicit-only cross-client clipboard (CLIPBOARD+PRIMARY, TARGETS, INCR); never part of `all`.
        (&["clipboard"], "clipboard-cross-client", clipboard::run),
    ];
    for (selectors, name, f) in table {
        if selectors.contains(&s) {
            let (pass, detail) = f(canary, args);
            results.push((name.to_string(), pass, detail));
        }
    }
    if results.is_empty() {
        crate::fail(
            "j07 unknown --scenario (popup|move|resize|regressions|chrome|max-snap|workspaces|retention|t15|t16|t17|t18|t15-t18|t21|t22|t21-t22|reactor|reactor-presentation|snap-border|s1|s2|s3|s4|xtest-self-check|explicit-taskbar-menu|explicit-desktop-settings|explicit-audio-tabs|explicit-osd|explicit-audio-apps|explicit-window-overflow|explicit-window-clamp|explicit-tray-toggle|clipboard|all)",
        );
    }
    let pass = results.iter().all(|(_, ok, _)| *ok);
    if json {
        let items: Vec<String> = results
            .iter()
            .map(|(n, ok, d)| format!("{{\"name\":{n:?},\"pass\":{ok},\"detail\":{d:?}}}"))
            .collect();
        println!(
            "{{\"scenario\":\"J07\",\"pass\":{pass},\"checks\":[{}]}}",
            items.join(",")
        );
    } else {
        for (name, ok, detail) in &results {
            println!("CANARY_J07 {name} pass={ok} {detail}");
        }
        println!("CANARY_J07 pass={pass} checks={}", results.len());
    }
    if !pass {
        std::process::exit(1);
    }
}

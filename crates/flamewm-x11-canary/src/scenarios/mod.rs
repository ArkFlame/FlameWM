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
    for i in 1..=steps {
        let i = i64::from(i);
        let steps = i64::from(steps);
        let x = i64::from(from.0) + (i64::from(to.0) - i64::from(from.0)) * i / steps;
        let y = i64::from(from.1) + (i64::from(to.1) - i64::from(from.1)) * i / steps;
        if !xtest_motion(canary, x as i16, y as i16) {
            return false;
        }
    }
    xtest_button_release(canary, to.0, to.1, button)
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
    let run = |name: &str, pass: bool, detail: String| (name.to_owned(), pass, detail);
    if scenario == "all" || scenario == "popup" {
        let (pass, detail) = popup_lifecycle::run_t03(canary, args);
        results.push(run("t03-popup-lifecycle", pass, detail));
    }
    if scenario == "all" || scenario == "move" {
        let (pass, detail) = window_interaction::run_t06(canary, args);
        results.push(run("t06-real-move", pass, detail));
    }
    if scenario == "all" || scenario == "resize" {
        let (pass, detail) = window_interaction::run_t07(canary, args);
        results.push(run("t07-real-resize-8", pass, detail));
    }
    if scenario == "all" || scenario == "regressions" {
        let (pass, detail) = interaction_regressions::run(canary, args);
        results.push(run("j24-j25-interaction-regressions", pass, detail));
    }
    if scenario == "all" || scenario == "chrome" {
        let (pass, detail) = window_chrome::run_t09_t10(canary, args);
        results.push(run("t09-t10-chrome-client", pass, detail));
    }
    if scenario == "all" || scenario == "max-snap" {
        let (pass, detail) = workspaces::run_t11(canary, args);
        results.push(run("t11-max-snap-fullscreen", pass, detail));
    }
    if scenario == "all" || scenario == "workspaces" {
        let (pass, detail) = workspaces::run_t12(canary, args);
        results.push(run("t12-workspace-churn", pass, detail));
    }
    if scenario == "t21" || scenario == "t22" || scenario == "t21-t22" {
        let (pass, detail) = workspaces::run_t21_t22(canary, args);
        results.push(run("t21-t22-workspace-sticky-convergence", pass, detail));
    }
    if scenario == "all" || scenario == "retention" {
        let (pass, detail) = debug_retention::run_t13(args);
        results.push(run("t13-debug-retention", pass, detail));
    }
    if scenario == "t15" || scenario == "t15-t18" {
        let (pass, detail) = t15_t18::run_t15(canary, args);
        results.push(run("t15-native-snap-preview", pass, detail));
    }
    if scenario == "t16" || scenario == "t15-t18" {
        let (pass, detail) = t15_t18::run_t16(canary, args);
        results.push(run("t16-mixed-responsiveness", pass, detail));
    }
    if scenario == "t17" || scenario == "t15-t18" {
        let (pass, detail) = t15_t18::run_t17(canary, args);
        results.push(run("t17-protected-interactions", pass, detail));
    }
    if scenario == "t18" || scenario == "t15-t18" {
        let (pass, detail) = t15_t18::run_t18(canary, args);
        results.push(run("t18-desktop-selection-admission", pass, detail));
    }
    if scenario == "reactor" || scenario == "reactor-presentation" {
        let (pass, detail) = reactor_presentation::run(canary, args);
        results.push(run("t18-t20-reactor-presentation-admission", pass, detail));
    }
    if scenario == "snap-border" || scenario == "s1" {
        let (pass, detail) = snap_border::run_s1(canary, args);
        results.push(run("s1-physical-corner-quarters", pass, detail));
    }
    if scenario == "snap-border" || scenario == "s2" {
        let (pass, detail) = snap_border::run_s2(canary, args);
        results.push(run("s2-snapped-border-zero", pass, detail));
    }
    if scenario == "snap-border" || scenario == "s3" {
        let (pass, detail) = snap_border::run_s3(canary, args);
        results.push(run("s3-frame-extents-per-mode", pass, detail));
    }
    if scenario == "snap-border" || scenario == "s4" {
        let (pass, detail) = snap_border::run_s4(canary, args);
        results.push(run("s4-fixed-hints-client-stable", pass, detail));
    }
    // Explicit-only shell scenarios (B14): never part of `all`; need --shot-dir.
    if scenario == "explicit-taskbar-menu" {
        let (pass, detail) = explicit_shell::run_s1(canary, args);
        results.push(run("shell-s1-taskbar-background-menu", pass, detail));
    }
    if scenario == "explicit-desktop-settings" {
        let (pass, detail) = explicit_shell::run_s2(canary, args);
        results.push(run("shell-s2-desktop-menu-settings", pass, detail));
    }
    if scenario == "explicit-audio-tabs" {
        let (pass, detail) = explicit_shell::run_s3(canary, args);
        results.push(run("shell-s3-audio-popup-tabs", pass, detail));
    }
    // B15 explicit-only: private audio stack (tools/flamewm_osd_audio_acceptance.py), never in `all`.
    if scenario == "explicit-osd" {
        let (pass, detail) = explicit_shell::run_osd(canary, args);
        results.push(run("shell-osd-volume-card", pass, detail));
    }
    if scenario == "explicit-audio-apps" {
        let (pass, detail) = explicit_shell::run_audio_apps(canary, args);
        results.push(run("shell-audio-popup-applications", pass, detail));
    }
    // V7 A14 explicit-only layout probe (overflow limits, taskbar stacking, snap, maximize).
    if scenario == "explicit-window-overflow" {
        let (pass, detail) = explicit_layout::run_overflow(canary, args);
        results.push(run("layout-window-overflow", pass, detail));
    }
    // G2 explicit-only ConfigureRequest clamp (3/4 overflow limit in four directions).
    if scenario == "explicit-window-clamp" {
        let (pass, detail) = explicit_layout::run_clamp(canary, args);
        results.push(run("layout-window-clamp", pass, detail));
    }
    // A15 explicit-only tray toggle (start, audio, network): never part of `all`.
    if scenario == "explicit-tray-toggle" {
        let (pass, detail) = explicit_tray::run_toggle(canary, args);
        results.push(run("tray-toggle-explicit", pass, detail));
    }
    // Explicit-only cross-client clipboard (CLIPBOARD+PRIMARY, TARGETS, INCR); never part of `all`.
    if scenario == "clipboard" {
        let (pass, detail) = clipboard::run(canary, args);
        results.push(run("clipboard-cross-client", pass, detail));
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

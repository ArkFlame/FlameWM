//! Explicit-only tray toggle scenario (A15): start, audio and network buttons
//! toggle their popup with XTEST clicks. A second click on the opener must close
//! the popup and it must stay closed 700 ms later; a click on a closed opener
//! must open it. Never part of `all`.

use std::thread::sleep;
use std::time::{Duration, Instant};

use x11rb::protocol::xproto::MapState;

use super::{real_click, real_escape, InputPath};
use crate::{Canary, WinInfo};

const PATH: &str = "path=real-pointer";
const START_MENU: &str = "FlameWM start";
const AUDIO_POPUP: &str = "FlameWM audio";
const NETWORK_POPUP: &str = "FlameWM network";
const PANEL: &str = "FlameWM panel";
const CYCLES: usize = 10;

fn wait_for<F: FnMut() -> bool>(timeout: Duration, mut condition: F) -> bool {
    let end = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        sleep(Duration::from_millis(50));
    }
}

fn viewable(canary: &Canary, name: &str) -> Option<WinInfo> {
    canary
        .find(name)
        .into_iter()
        .find(|w| w.name == name && w.map_state == u8::from(MapState::VIEWABLE))
}

/// Toggle one opener CYCLES times. Returns (opens_ok, closes_ok, stays_closed_ok, detail).
fn toggle(canary: &Canary, point: (i16, i16), popup: &str) -> (usize, usize, usize, String) {
    let (mut opens, mut closes, mut stays) = (0usize, 0usize, 0usize);
    let mut failures = Vec::new();
    for i in 0..CYCLES {
        if viewable(canary, popup).is_some() {
            let _ = real_escape(canary);
            let _ = wait_for(Duration::from_secs(2), || viewable(canary, popup).is_none());
        }
        let opened = real_click(canary, point.0, point.1, 1)
            && wait_for(Duration::from_secs(3), || viewable(canary, popup).is_some());
        if opened {
            opens += 1;
        } else {
            failures.push(format!("cycle{i}:open"));
            continue;
        }
        let closed = real_click(canary, point.0, point.1, 1)
            && wait_for(Duration::from_secs(2), || viewable(canary, popup).is_none());
        if closed {
            closes += 1;
        } else {
            failures.push(format!("cycle{i}:close"));
            continue;
        }
        sleep(Duration::from_millis(700));
        if viewable(canary, popup).is_none() {
            stays += 1;
        } else {
            failures.push(format!("cycle{i}:reopened"));
        }
    }
    let detail = if failures.is_empty() {
        String::new()
    } else {
        format!("failures=[{}]", failures.join(","))
    };
    (opens, closes, stays, detail)
}

/// A15: start, audio and network tray buttons toggle their popups (10 cycles each).
pub fn run_toggle(canary: &Canary, _args: &[String]) -> (bool, String) {
    let Some(panel) = viewable(canary, PANEL) else {
        return (false, "FlameWM panel not viewable".to_owned());
    };
    let cy = panel.y + panel.height as i16 / 2;
    let right = panel.x + panel.width as i16;
    let targets = [
        ("start", (panel.x + 24, cy), START_MENU),
        ("audio", (right - 120, cy), AUDIO_POPUP),
        ("network", (right - 90, cy), NETWORK_POPUP),
    ];
    let mut pass = true;
    let mut parts = Vec::new();
    for (name, point, popup) in targets {
        let (opens, closes, stays, detail) = toggle(canary, point, popup);
        let ok = opens == CYCLES && closes == CYCLES && stays == CYCLES;
        pass &= ok;
        let sep = if detail.is_empty() { "" } else { " " };
        parts.push(format!(
            "{name}={ok}(opens={opens}/{CYCLES} closes={closes}/{CYCLES} stays_closed={stays}/{CYCLES}{sep}{detail})"
        ));
    }
    let input = InputPath::RealPointer.as_str();
    (pass, format!("{PATH} input={input} {}", parts.join(" ")))
}

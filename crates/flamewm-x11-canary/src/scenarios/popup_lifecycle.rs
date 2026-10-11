//! T03 popup lifecycle: click source -> popup Viewable, geometry matches
//! spec, popup not at 0,0; outside-click/Escape close; 20x repeat.

use x11rb::protocol::xproto::{ConnectionExt as _, MapState};

use super::{InputPath, real_click, real_escape, xtest_motion};

fn popup_candidates(canary: &crate::Canary) -> Vec<crate::WinInfo> {
    canary
        .all()
        .into_iter()
        .filter(|w| {
            w.map_state == u8::from(MapState::VIEWABLE) && w.width <= 600 && w.height <= 600
        })
        .collect()
}

fn wait_viewable_popup(
    canary: &crate::Canary,
    baseline: &[u32],
    timeout_ms: u64,
) -> Option<crate::WinInfo> {
    let start = std::time::Instant::now();
    let limit = std::time::Duration::from_millis(timeout_ms.max(200));
    while start.elapsed() < limit {
        for w in popup_candidates(canary) {
            if !baseline.contains(&w.id) && w.width > 0 && w.height > 0 {
                return Some(w);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    None
}

/// Start-button click point on the panel: 20px in from the left edge, vertically
/// centred. Opens the `FlameWM start` popup.
fn start_button(panel: &crate::WinInfo) -> (i16, i16) {
    (
        panel.x.saturating_add(20),
        panel.y.saturating_add(panel.height as i16 / 2),
    )
}

/// Focus/grab/pointer snapshot taken with the popup Viewable and before Escape.
fn focus_snapshot(canary: &crate::Canary, popup_id: u32) -> String {
    let focus = canary
        .conn
        .get_input_focus()
        .ok()
        .and_then(|c| c.reply().ok());
    let attrs = canary
        .conn
        .get_window_attributes(popup_id)
        .ok()
        .and_then(|c| c.reply().ok());
    let pointer = canary
        .conn
        .query_pointer(canary.root)
        .ok()
        .and_then(|c| c.reply().ok());
    let focus_text = match focus {
        Some(f) => format!("focus={} revert_to={:?}", f.focus, f.revert_to),
        None => "focus=unknown".to_owned(),
    };
    let mask = match attrs {
        Some(a) => format!("your_event_mask={:#x}", u32::from(a.your_event_mask)),
        None => "your_event_mask=unknown".to_owned(),
    };
    let child = match pointer {
        Some(p) => format!("pointer_child={}", p.child),
        None => "pointer_child=unknown".to_owned(),
    };
    format!("popup={popup_id} {focus_text} {mask} {child}")
}

fn wait_gone(canary: &crate::Canary, id: u32, timeout_ms: u64) -> bool {
    let start = std::time::Instant::now();
    let limit = std::time::Duration::from_millis(timeout_ms.max(200));
    while start.elapsed() < limit {
        match canary.by_id(id) {
            None => return true,
            Some(w) => {
                if w.map_state != u8::from(MapState::VIEWABLE) {
                    return true;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

/// T03 with real-pointer path. Diagnostic SendEvent fallback is recorded but
/// never counts as pass (returns fail with NEVER-COUNTS-AS-PASS marker).
pub fn run_t03(canary: &crate::Canary, args: &[String]) -> (bool, String) {
    let source_name = crate::arg(args, "--source").unwrap_or_else(|| "FlameWM panel".to_owned());
    let repeats: u32 = crate::arg(args, "--repeats")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20)
        .clamp(1, 20);
    let source = match canary.find(&source_name).into_iter().next() {
        Some(w) => w,
        None => {
            return (
                false,
                format!(
                    "no source {source_name:?} path={}",
                    InputPath::SyntheticFallback.as_str()
                ),
            );
        }
    };
    let (sx, sy) = start_button(&source);
    let mut opened = 0;
    let mut geom_ok = 0;
    let mut closed = 0;
    let mut closed_by_escape = 0;
    let mut real_ok = true;
    let mut escape_attempted = false;
    let mut escape_available = true;
    let mut focus_diag = String::new();
    for _ in 0..repeats {
        let baseline: Vec<u32> = popup_candidates(canary).iter().map(|w| w.id).collect();
        if !xtest_motion(canary, sx, sy) {
            real_ok = false;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
        if !real_click(canary, sx, sy, 1) {
            real_ok = false;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
        let popup = wait_viewable_popup(canary, &baseline, 1500);
        let Some(popup) = popup else { break };
        opened += 1;
        // Geometry matches spec: non-zero size, not pinned at 0,0 root origin
        // unless the source itself is there (spec popups anchor to source).
        if popup.width > 0
            && popup.height > 0
            && (popup.x != 0 || popup.y != 0 || (sx == 0 && sy == 0))
        {
            geom_ok += 1;
        }
        if focus_diag.is_empty() {
            focus_diag = focus_snapshot(canary, popup.id);
        }
        // Close: real XTEST Escape is the only close input; outside-click at
        // root 4,4 is cleanup when the popup survives and is not counted as an
        // Escape close.
        escape_attempted = true;
        if !real_escape(canary) {
            escape_available = false;
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
        let mut gone = wait_gone(canary, popup.id, 800);
        if gone {
            closed_by_escape += 1;
        } else {
            let _ = xtest_motion(canary, 4, 4);
            let _ = real_click(canary, 4, 4, 1);
            std::thread::sleep(std::time::Duration::from_millis(150));
            gone = wait_gone(canary, popup.id, 800);
        }
        if gone {
            closed += 1;
        }
    }
    let path = if real_ok && escape_attempted && escape_available && closed_by_escape == opened {
        InputPath::RealPointer
    } else {
        InputPath::SyntheticFallback
    };
    let pass = real_ok
        && escape_attempted
        && escape_available
        && opened as u32 == repeats
        && geom_ok == opened
        && closed == opened
        && closed_by_escape == opened;
    let detail = format!(
        "source={} repeats={repeats} opened={opened} geom_ok={geom_ok} closed={closed} closed_by_escape={closed_by_escape} focus_diag=[{focus_diag}] escape={} fallback=outside-click path={}",
        source.id,
        if !escape_attempted {
            "not-attempted"
        } else if escape_available {
            "xtest"
        } else {
            "unavailable"
        },
        path.as_str()
    );
    (pass, detail)
}

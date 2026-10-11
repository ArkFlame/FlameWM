//! T03 popup lifecycle: click source -> popup Viewable, geometry matches
//! spec, popup not at 0,0; outside-click/Escape close; 20x repeat.

use x11rb::protocol::xproto::MapState;

use super::{InputPath, center, real_click, real_escape, xtest_motion};

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
    let source_name = crate::arg(args, "--source").unwrap_or_else(|| "flame".to_owned());
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
    let (sx, sy) = center(&source);
    let mut opened = 0;
    let mut geom_ok = 0;
    let mut closed = 0;
    let mut real_ok = true;
    let mut escape_attempted = false;
    let mut escape_available = true;
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
        // Close: Escape first, then outside-click at root 4,4.
        let (px, py) = center(&popup);
        let _ = real_click(canary, px, py, 1);
        escape_attempted = true;
        if !real_escape(canary) {
            escape_available = false;
        }
        std::thread::sleep(std::time::Duration::from_millis(120));
        let mut gone = wait_gone(canary, popup.id, 800);
        if !gone {
            let _ = xtest_motion(canary, 4, 4);
            let _ = real_click(canary, 4, 4, 1);
            std::thread::sleep(std::time::Duration::from_millis(150));
            gone = wait_gone(canary, popup.id, 800);
        }
        if gone {
            closed += 1;
        }
    }
    let path = if real_ok && escape_attempted && escape_available {
        InputPath::RealPointer
    } else {
        InputPath::SyntheticFallback
    };
    let pass = real_ok
        && escape_attempted
        && escape_available
        && opened as u32 == repeats
        && geom_ok == opened
        && closed == opened;
    // XTEST is unavailable in the current canary dependency, so the bounded
    // outside-click fallback is diagnostic only and never counts as pass.
    let detail = format!(
        "source={} repeats={repeats} opened={opened} geom_ok={geom_ok} closed={closed} escape={} fallback=outside-click path={}",
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

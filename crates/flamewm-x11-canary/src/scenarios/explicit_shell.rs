//! Explicit-only shell popup scenarios (B14): taskbar background menu (S1),
//! desktop background menu settings row (S2) and audio popup tab switching
//! (S3). Pointer and keyboard input is XTEST only (real input path). Menu rows
//! are counted by sampling the menu pixels; screenshots are written with
//! ImageMagick `import` into the required `--shot-dir`. Never part of `all`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask, MapState,
    WindowClass,
};
use x11rb::protocol::Event;

use super::{real_click, real_escape, xtest_motion, InputPath};
use crate::{Canary, WinInfo};

const PATH: &str = "path=real-pointer";
const TASK_MENU: &str = "FlameWM task menu";
const START_MENU: &str = "FlameWM start";
const AUDIO_POPUP: &str = "FlameWM audio";
const PANEL: &str = "FlameWM panel";
const DESKTOP: &str = "FlameWM Desktop";
/// Desktop blank-menu rows: terminal, folder, [sticky note], desktop, settings.
/// Settings is always the last row (`blank_context_menu`); 4 rows = 134px,
/// 5 rows = 165px (`menu_height_for_rows`).
const DESKTOP_MENU_HEIGHT: u32 = 165;
/// Ink threshold: channel delta from the menu background.
const INK: i32 = 48;
const TAB_ITERATIONS: usize = 30;

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

/// Root-relative RGB sample of a rectangle (ZPixmap, 4 bytes per pixel).
fn grab(canary: &Canary, rect: (i32, i32, u32, u32)) -> Option<Vec<u8>> {
    let (x, y, w, h) = rect;
    let data = canary
        .sample(canary.root, x as i16, y as i16, w as u16, h as u16)
        .ok()?;
    (data.len() == (w * h * 4) as usize).then_some(data)
}

fn rgb(data: &[u8], w: u32, x: u32, y: u32) -> (i32, i32, i32) {
    let i = ((y * w + x) * 4) as usize;
    (
        i32::from(data[i + 2]),
        i32::from(data[i + 1]),
        i32::from(data[i]),
    )
}

fn differs(a: (i32, i32, i32), b: (i32, i32, i32), tol: i32) -> bool {
    (a.0 - b.0)
        .abs()
        .max((a.1 - b.1).abs())
        .max((a.2 - b.2).abs())
        > tol
}

/// How a pixel counts as menu ink (text/icon) for row banding.
#[derive(Clone, Copy)]
enum Ink {
    /// Opaque menu: channel delta from the menu background above `INK`.
    DeltaFromBackground,
    /// Frosted menu over wallpaper: every channel at least this bright (white labels).
    MinChannelAtLeast(i32),
}

/// Contiguous vertical bands (absolute top, bottom) of ink inside `rect`,
/// inset by `inset` on every side. Delta mode uses the inset top-left pixel as
/// background.
fn ink_bands(
    canary: &Canary,
    rect: (i32, i32, u32, u32),
    inset: u32,
    ink: Ink,
) -> Option<Vec<(i32, i32)>> {
    let (x, y, w, h) = rect;
    let (ix, iy) = (x + inset as i32, y + inset as i32);
    let (iw, ih) = (w.checked_sub(2 * inset)?, h.checked_sub(2 * inset)?);
    let data = grab(canary, (ix, iy, iw, ih))?;
    let bg = rgb(&data, iw, 0, 0);
    let is_ink = |px: (i32, i32, i32)| match ink {
        Ink::DeltaFromBackground => differs(px, bg, INK),
        Ink::MinChannelAtLeast(min) => px.0.min(px.1).min(px.2) >= min,
    };
    let mut bands = Vec::new();
    let mut start: Option<u32> = None;
    for row in 0..ih {
        let has_ink = (0..iw).any(|col| is_ink(rgb(&data, iw, col, row)));
        match (has_ink, start) {
            (true, None) => start = Some(row),
            (false, Some(s)) => {
                bands.push((iy + s as i32, iy + row as i32 - 1));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        bands.push((iy + s as i32, iy + ih as i32 - 1));
    }
    Some(bands)
}

/// Joins bands whose vertical gap is at most `max_gap` px (one label's glyphs).
fn merge_bands(bands: &[(i32, i32)], max_gap: i32) -> Vec<(i32, i32)> {
    let mut merged: Vec<(i32, i32)> = Vec::new();
    for &(top, bottom) in bands {
        match merged.last_mut() {
            Some(last) if top - last.1 <= max_gap => last.1 = bottom,
            _ => merged.push((top, bottom)),
        }
    }
    merged
}

fn screenshot(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let out = dir.join(name);
    let status = Command::new("import")
        .args(["-window", "root"])
        .arg(&out)
        .status()
        .map_err(|e| format!("import unavailable: {e}"))?;
    if status.success() {
        Ok(out)
    } else {
        Err(format!("import failed for {name}"))
    }
}

fn shot_dir(args: &[String]) -> Result<PathBuf, String> {
    let dir = PathBuf::from(crate::arg(args, "--shot-dir").ok_or("--shot-dir <path> required")?);
    fs::create_dir_all(&dir).map_err(|e| format!("shot dir: {e}"))?;
    Ok(dir)
}

fn panel_of(canary: &Canary) -> Result<WinInfo, String> {
    viewable(canary, PANEL).ok_or_else(|| "FlameWM panel not viewable".to_owned())
}

/// Empty taskbar point (panel centre) and the start button (left icon).
fn taskbar_points(panel: &WinInfo) -> ((i16, i16), (i16, i16)) {
    let cy = panel.y + panel.height as i16 / 2;
    let empty = (panel.x + panel.width as i16 / 2, cy);
    let start = (panel.x + 24, cy);
    (empty, start)
}

/// S1: taskbar background Button3 opens one "FlameWM Settings" row; Escape and
/// outside click dismiss; left click on start still opens the start menu.
pub fn run_s1(canary: &Canary, args: &[String]) -> (bool, String) {
    let dir = match shot_dir(args) {
        Ok(d) => d,
        Err(e) => return (false, e),
    };
    let panel = match panel_of(canary) {
        Ok(p) => p,
        Err(e) => return (false, e),
    };
    let (empty, start) = taskbar_points(&panel);
    let outside = (panel.x + panel.width as i16 / 2, 200);
    let mut checks: Vec<(&str, bool, String)> = Vec::new();

    let closed_before = viewable(canary, TASK_MENU).is_none();
    checks.push(("menu-closed-before", closed_before, String::new()));

    let opened = real_click(canary, empty.0, empty.1, 3)
        && wait_for(Duration::from_secs(3), || {
            viewable(canary, TASK_MENU).is_some()
        });
    checks.push(("button3-opens-task-menu", opened, String::new()));
    let mut row_count = 0usize;
    if let Some(menu) = viewable(canary, TASK_MENU) {
        let rect = (
            i32::from(menu.x),
            i32::from(menu.y),
            u32::from(menu.width),
            u32::from(menu.height),
        );
        let bands = ink_bands(canary, rect, 8, Ink::DeltaFromBackground).unwrap_or_default();
        row_count = bands.len();
        let shot = screenshot(&dir, "s1-taskbar-menu-open.png");
        checks.push((
            "one-settings-row",
            row_count == 1,
            format!("rows={row_count} rect={rect:?} shot={shot:?}"),
        ));
    } else {
        checks.push(("one-settings-row", false, "menu not viewable".to_owned()));
    }

    let escaped = real_escape(canary)
        && wait_for(Duration::from_secs(2), || {
            viewable(canary, TASK_MENU).is_none()
        });
    checks.push(("escape-dismisses", escaped, String::new()));

    let reopened = real_click(canary, empty.0, empty.1, 3)
        && wait_for(Duration::from_secs(3), || {
            viewable(canary, TASK_MENU).is_some()
        });
    let outside_closed = reopened
        && real_click(canary, outside.0, outside.1, 1)
        && wait_for(Duration::from_secs(2), || {
            viewable(canary, TASK_MENU).is_none()
        });
    checks.push(("outside-click-dismisses", outside_closed, String::new()));

    let start_opened = real_click(canary, start.0, start.1, 1)
        && wait_for(Duration::from_secs(3), || {
            viewable(canary, START_MENU).is_some()
        });
    let start_shot = screenshot(&dir, "s1-start-menu-open.png");
    checks.push((
        "left-click-start-opens-start",
        start_opened,
        format!("shot={start_shot:?}"),
    ));
    let _ = real_click(canary, outside.0, outside.1, 1);
    let _ = wait_for(Duration::from_secs(2), || {
        viewable(canary, START_MENU).is_none()
    });

    let pass = checks.iter().all(|(_, ok, _)| *ok);
    let detail = checks
        .iter()
        .map(|(n, ok, d)| {
            format!(
                "{n}={ok}{}",
                if d.is_empty() {
                    String::new()
                } else {
                    format!("({d})")
                }
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    (
        pass,
        format!("{PATH} input={} {detail}", InputPath::RealPointer.as_str()),
    )
}

/// Count live `flamewm-settings` processes (comm truncated to 15 bytes, zombies excluded).
pub fn settings_pids() -> Vec<u32> {
    let mut pids = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return pids;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let comm = fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        let state = stat
            .rsplit(')')
            .next()
            .unwrap_or("")
            .trim_start()
            .chars()
            .next();
        if comm.trim() == "flamewm-setting" && state != Some('Z') {
            pids.push(pid);
        }
    }
    pids
}

/// S2: desktop background Button3 shows a menu whose last (settings) row is the
/// only settings entry; clicking it launches exactly one flamewm-settings.
pub fn run_s2(canary: &Canary, args: &[String]) -> (bool, String) {
    let dir = match shot_dir(args) {
        Ok(d) => d,
        Err(e) => return (false, e),
    };
    let Some(desktop) = viewable(canary, DESKTOP) else {
        return (false, "FlameWM Desktop not viewable".to_owned());
    };
    let anchor = (desktop.x + 600, desktop.y + 200);
    let before = settings_pids();
    let area = (
        i32::from(anchor.0),
        i32::from(anchor.1),
        260u32,
        DESKTOP_MENU_HEIGHT + 20,
    );
    let baseline = grab(canary, area);
    if !real_click(canary, anchor.0, anchor.1, 3) {
        return (false, "button3 injection failed".to_owned());
    }
    let shown = wait_for(Duration::from_secs(3), || grab(canary, area) != baseline);
    sleep(Duration::from_millis(300));
    let shot = screenshot(&dir, "s2-desktop-menu-open.png");
    // Frosted menu: label text is near-white; banding uses the label column only.
    let rect = (
        i32::from(anchor.0) + 36,
        i32::from(anchor.1),
        170u32,
        DESKTOP_MENU_HEIGHT + 4,
    );
    let glyph_bands = ink_bands(canary, rect, 4, Ink::MinChannelAtLeast(200)).unwrap_or_default();
    // Glyph gaps inside one label are < 8px; row gaps are ~24px.
    let bands = merge_bands(&glyph_bands, 8);
    let rows = bands.len();
    let settings_row = bands.last().copied();
    let mut checks: Vec<(&str, bool, String)> = vec![
        ("desktop-menu-shown", shown, format!("shot={shot:?}")),
        (
            "settings-is-last-of-4-or_5-rows",
            (rows == 4 || rows == 5) && settings_row.is_some(),
            format!("rows={rows} bands={bands:?}"),
        ),
    ];
    let mut launched = 0usize;
    if let Some((top, bottom)) = settings_row {
        let click_y = ((top + bottom) / 2) as i16;
        let clicked = real_click(canary, anchor.0 + 60, click_y, 1);
        let spawned = wait_for(Duration::from_secs(8), || {
            settings_pids().len() > before.len()
        });
        sleep(Duration::from_secs(1));
        let after = settings_pids();
        launched = after.len().saturating_sub(before.len());
        checks.push((
            "settings-click-launches-one",
            clicked && spawned && launched == 1,
            format!("clicked={clicked} launched={launched}"),
        ));
        for pid in after.iter().filter(|p| !before.contains(p)) {
            let _ = Command::new("kill").arg(pid.to_string()).status();
        }
    } else {
        checks.push(("settings-click-launches-one", false, "no rows".to_owned()));
    }
    let pass = checks.iter().all(|(_, ok, _)| *ok);
    let detail = checks
        .iter()
        .map(|(n, ok, d)| format!("{n}={ok}({d})"))
        .collect::<Vec<_>>()
        .join(" ");
    (
        pass,
        format!(
            "{PATH} input={} launched={launched} {detail}",
            InputPath::RealPointer.as_str()
        ),
    )
}

/// Tab centres relative to the audio popup origin (Devices, Applications),
/// calibrated on the 459x460 popup: tab strip spans y 54..86 (rel.).
const TAB_DEVICES: (i16, i16) = (60, 70);
const TAB_APPS: (i16, i16) = (280, 70);
/// Tab content region relative to the popup origin (below the tab strip, above
/// the footer toggle).
const CONTENT_REL: (i32, i32, u32, u32) = (14, 96, 431, 250);
/// Permille of pixels whose channels differ by more than `INK` between two captures.
const SAME_TAB_MAX_PERMILLE: u32 = 5;
/// Permille of differing pixels required between the Devices and Applications captures.
const OTHER_TAB_MIN_PERMILLE: u32 = 15;

/// Permille of pixels that differ by more than `INK` in some channel.
fn differing_permille(a: &[u8], b: &[u8], w: u32) -> u32 {
    let n = (a.len() / 4).max(1) as u32;
    let differing = (0..n)
        .filter(|&i| differs(rgb(a, w, i % w, i / w), rgb(b, w, i % w, i / w), INK))
        .count() as u32;
    differing * 1000 / n
}

/// S3: audio popup Devices<->Applications switching 30x with pixel checks, and
/// the popup stays open while the pointer sweeps over the taskbar (taskbar
/// receives EnterNotify).
pub fn run_s3(canary: &Canary, args: &[String]) -> (bool, String) {
    let dir = match shot_dir(args) {
        Ok(d) => d,
        Err(e) => return (false, e),
    };
    let panel = match panel_of(canary) {
        Ok(p) => p,
        Err(e) => return (false, e),
    };
    let cy = panel.y + panel.height as i16 / 2;
    let audio_button = (panel.x + panel.width as i16 - 120, cy);
    // Precondition: the popup starts closed (the tray button toggles it).
    if viewable(canary, AUDIO_POPUP).is_some() {
        let _ = real_escape(canary);
        if !wait_for(Duration::from_secs(2), || {
            viewable(canary, AUDIO_POPUP).is_none()
        }) {
            return (
                false,
                "audio popup already open and Escape did not close it".to_owned(),
            );
        }
    }
    if !real_click(canary, audio_button.0, audio_button.1, 1) {
        return (false, "audio button injection failed".to_owned());
    }
    if !wait_for(Duration::from_secs(3), || {
        viewable(canary, AUDIO_POPUP).is_some()
    }) {
        return (false, "audio popup not viewable".to_owned());
    }
    let Some(popup) = viewable(canary, AUDIO_POPUP) else {
        return (false, "audio popup vanished".to_owned());
    };
    let _ = screenshot(&dir, "s3-audio-popup-open.png");
    let (ox, oy) = (i32::from(popup.x), i32::from(popup.y));
    let devices = (popup.x + TAB_DEVICES.0, popup.y + TAB_DEVICES.1);
    let apps = (popup.x + TAB_APPS.0, popup.y + TAB_APPS.1);
    let content = (
        ox + CONTENT_REL.0,
        oy + CONTENT_REL.1,
        CONTENT_REL.2,
        CONTENT_REL.3,
    );
    let capture = || grab(canary, content);

    // Reference captures: the first Devices and first Applications state.
    let mut checks: Vec<(&str, bool, String)> = Vec::new();
    let mut ref_devices: Option<Vec<u8>> = None;
    let mut ref_apps: Option<Vec<u8>> = None;
    let mut switches_ok = 0usize;
    let mut first_attempt_misses = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for i in 0..TAB_ITERATIONS {
        let (name, point, is_devices) = if i % 2 == 0 {
            ("devices", devices, true)
        } else {
            ("apps", apps, false)
        };
        let clicked = real_click(canary, point.0, point.1, 1);
        sleep(Duration::from_millis(250));
        let reference = if is_devices {
            &mut ref_devices
        } else {
            &mut ref_apps
        };
        match reference {
            None => match capture() {
                Some(now) => *reference = Some(now),
                None => {
                    failures.push(format!("#{i} {name}: capture failed"));
                    continue;
                }
            },
            Some(expected) => {
                // Bounded settle: the tab must show its reference state within 1.5s.
                // First-attempt misses are counted, never hidden.
                let differs_now = |canary: &Canary| {
                    capture()
                        .map(|now| differing_permille(&now, expected, content.2))
                        .unwrap_or(u32::MAX)
                };
                let mut diff = differs_now(canary);
                if diff > SAME_TAB_MAX_PERMILLE {
                    first_attempt_misses += 1;
                    let settled = wait_for(Duration::from_millis(1500), || {
                        diff = differs_now(canary);
                        diff <= SAME_TAB_MAX_PERMILLE
                    });
                    if !clicked || !settled {
                        failures.push(format!("#{i} {name}: diff={diff} clicked={clicked}"));
                        continue;
                    }
                } else if !clicked {
                    failures.push(format!("#{i} {name}: click not delivered"));
                    continue;
                }
            }
        }
        if let (Some(d), Some(a)) = (&ref_devices, &ref_apps) {
            if differing_permille(d, a, content.2) < OTHER_TAB_MIN_PERMILLE {
                failures.push(format!("#{i} tabs not distinguishable"));
                continue;
            }
        }
        switches_ok += 1;
    }
    let other_tab_diff = match (&ref_devices, &ref_apps) {
        (Some(d), Some(a)) => differing_permille(d, a, content.2) as i64,
        _ => -1,
    };
    checks.push((
        "tab-switch-30x-pixels",
        switches_ok == TAB_ITERATIONS && failures.is_empty(),
        format!(
            "ok={switches_ok}/{TAB_ITERATIONS} first_attempt_misses={first_attempt_misses} tab_diff_permille={other_tab_diff} first_fail={:?}",
            failures.first()
        ),
    ));
    let _ = screenshot(&dir, "s3-audio-apps-tab.png");

    // Pointer sweep over the taskbar strip: popup stays open; panel gets EnterNotify.
    let _ = canary.conn.change_window_attributes(
        panel.id,
        &ChangeWindowAttributesAux::new().event_mask(EventMask::ENTER_WINDOW),
    );
    let _ = canary.conn.flush();
    while let Ok(Some(_)) = canary.conn.poll_for_event() {}
    let (x0, x1) = (panel.x + 100, panel.x + panel.width as i16 - 300);
    let mut sweep_ok = xtest_motion(canary, x0, cy);
    let steps = 20i32;
    for step in 1..=steps {
        let x = x0 as i32 + (x1 as i32 - x0 as i32) * step / steps;
        sweep_ok &= xtest_motion(canary, x as i16, cy);
        sleep(Duration::from_millis(20));
    }
    sleep(Duration::from_millis(300));
    let mut enters = 0usize;
    while let Ok(Some(event)) = canary.conn.poll_for_event() {
        if let Event::EnterNotify(e) = event {
            if e.event == panel.id {
                enters += 1;
            }
        }
    }
    let still_open = viewable(canary, AUDIO_POPUP).is_some();
    let _ = screenshot(&dir, "s3-audio-after-taskbar-sweep.png");
    checks.push((
        "popup-open-after-taskbar-sweep",
        sweep_ok && still_open,
        format!("sweep={sweep_ok} open={still_open}"),
    ));
    checks.push((
        "taskbar-receives-enter",
        enters > 0,
        format!("enter_notify={enters}"),
    ));

    let closed = real_escape(canary)
        && wait_for(Duration::from_secs(2), || {
            viewable(canary, AUDIO_POPUP).is_none()
        });
    checks.push(("escape-closes-popup", closed, String::new()));

    let pass = checks.iter().all(|(_, ok, _)| *ok);
    let detail = checks
        .iter()
        .map(|(n, ok, d)| format!("{n}={ok}({d})"))
        .collect::<Vec<_>>()
        .join(" ");
    (
        pass,
        format!("{PATH} input={} {detail}", InputPath::RealPointer.as_str()),
    )
}

/// B15 OSD contract mirrored from `crates/flamewm-shell/src/osd.rs` (source wins) at 1x:
/// `OSD_WIDTH`/`OSD_HEIGHT` 215x37, `TRACK_X` 40, `TRACK_W` 129, `TRACK_H` 5 (track row 16..21),
/// icon `left:13px` 16px wide, so the icon scan spans x 0..35 (`ui/shell/osd.html`).
const OSD_TITLE: &str = "FlameWM volume";
const OSD_BODY: (u32, u32) = (215, 37);
const OSD_TRACK_X: u32 = 40;
const OSD_TRACK_W: u32 = 129;
const OSD_ICON_W: u32 = 35;
const OSD_VOLUME_MAX: f64 = 150.0;
const OSD_FILL_TOLERANCE: f64 = 0.03;
const OSD_HIDE_LIMIT: Duration = Duration::from_millis(1500);
const OSD_LEVELS: [u8; 5] = [0, 25, 50, 100, 150];
/// Private PipeWire null sink from tools/flamewm_osd_audio_acceptance.py (never the host sink).
const SINK_NAME: &str = "alsa_output.pci-0000_00_1f.3.analog-stereo";

fn summarize(checks: &[(&str, bool, String)]) -> (bool, String) {
    let pass = checks.iter().all(|(_, ok, _)| *ok);
    let detail = checks
        .iter()
        .map(|(n, ok, d)| format!("{n}={ok}({d})"))
        .collect::<Vec<_>>()
        .join(" ");
    (
        pass,
        format!("{PATH} input={} {detail}", InputPath::RealPointer.as_str()),
    )
}

/// Sink node id of the private null sink, from `pw-cli ls Node` (inherits the env of the harness).
fn sink_node_id() -> Option<u32> {
    let out = Command::new("pw-cli").args(["ls", "Node"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut current = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("id ") {
            current = rest
                .split(',')
                .next()
                .and_then(|v| v.trim().parse::<u32>().ok());
        } else if line == format!("node.name = \"{SINK_NAME}\"") {
            return current;
        }
    }
    None
}

fn pw_props(id: u32, props: &str) -> bool {
    Command::new("pw-cli")
        .args(["s", &id.to_string(), "Props", props])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Sets the private sink to `percent`. PipeWire maps pulse percent as 100 * cbrt(channel).
fn set_sink_percent(percent: u8) -> bool {
    let Some(id) = sink_node_id() else {
        return false;
    };
    let channel = (f64::from(percent) / 100.0).powi(3);
    pw_props(
        id,
        &format!("{{ channelVolumes: [ {channel:.6}, {channel:.6} ] }}"),
    )
}

fn set_sink_mute(muted: bool) -> bool {
    sink_node_id().is_some_and(|id| pw_props(id, &format!("{{ mute: {muted} }}")))
}

fn osd_cards(canary: &Canary) -> Vec<WinInfo> {
    canary
        .find(OSD_TITLE)
        .into_iter()
        .filter(|w| w.name == OSD_TITLE)
        .collect()
}

fn osd_visible(canary: &Canary) -> Option<WinInfo> {
    osd_cards(canary)
        .into_iter()
        .find(|w| w.map_state == u8::from(MapState::VIEWABLE))
}

fn wait_osd_shown(canary: &Canary, timeout: Duration) -> Option<WinInfo> {
    let mut found = None;
    wait_for(timeout, || {
        found = osd_visible(canary);
        found.is_some()
    });
    found
}

/// Time from `since` until no OSD card is viewable; `None` when it outlives `limit`.
fn wait_osd_hidden(canary: &Canary, since: Instant, limit: Duration) -> Option<Duration> {
    let end = since + limit;
    loop {
        if osd_visible(canary).is_none() {
            return Some(since.elapsed());
        }
        if Instant::now() >= end {
            return None;
        }
        sleep(Duration::from_millis(10));
    }
}

/// (input focus, `_NET_ACTIVE_WINDOW`) of the root, to prove the OSD never takes focus.
fn focus_state(canary: &Canary) -> Option<(u32, u32)> {
    let focus = canary.conn.get_input_focus().ok()?.reply().ok()?.focus;
    let atom = canary
        .conn
        .intern_atom(false, b"_NET_ACTIVE_WINDOW")
        .ok()?
        .reply()
        .ok()?
        .atom;
    let bytes = canary.prop_bytes(canary.root, atom);
    let active = bytes
        .get(..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))?;
    Some((focus, active))
}

/// Private override-redirect window that lies beneath the OSD card; it selects ButtonPress.
fn open_beneath(canary: &Canary, rect: (i16, i16, u16, u16)) -> Option<u32> {
    let wid = canary.conn.generate_id().ok()?;
    canary
        .conn
        .create_window(
            0,
            wid,
            canary.root,
            rect.0,
            rect.1,
            rect.2,
            rect.3,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new()
                .override_redirect(1_u32)
                .background_pixel(0x0033_6699)
                .event_mask(EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE),
        )
        .ok()?
        .check()
        .ok()?;
    canary.conn.map_window(wid).ok()?.check().ok()?;
    canary.conn.flush().ok()?;
    Some(wid)
}

/// True when a ButtonPress targeted at `wid` arrives within `timeout`.
fn press_seen(canary: &Canary, wid: u32, timeout: Duration) -> bool {
    let mut seen = false;
    wait_for(timeout, || {
        while let Ok(Some(event)) = canary.conn.poll_for_event() {
            if let Event::ButtonPress(press) = event {
                seen |= press.event == wid;
            }
        }
        seen
    });
    seen
}

/// Track row = the row where the full and the empty card differ most across the track.
fn track_row(empty: &[u8], full: &[u8], w: u32, h: u32) -> Option<u32> {
    let track = OSD_TRACK_X..OSD_TRACK_X + OSD_TRACK_W;
    (0..h)
        .map(|row| {
            let n = track
                .clone()
                .filter(|&x| differs(rgb(full, w, x, row), rgb(empty, w, x, row), INK))
                .count();
            (row, n)
        })
        .max_by_key(|&(_, n)| n)
        .filter(|&(_, n)| n >= (OSD_TRACK_W / 2) as usize)
        .map(|(row, _)| row)
}

/// Fraction of track columns that differ from the empty track at `row`.
fn fill_fraction(empty: &[u8], cap: &[u8], w: u32, row: u32) -> f64 {
    let n = (OSD_TRACK_X..OSD_TRACK_X + OSD_TRACK_W)
        .filter(|&x| differs(rgb(cap, w, x, row), rgb(empty, w, x, row), INK))
        .count();
    n as f64 / f64::from(OSD_TRACK_W)
}

/// B15 explicit-osd: each volume level shows the card at 215x37, centred, with a
/// track fill of level/150; muted shows an empty track; the card hides within 1.5 s;
/// two changes within 300 ms show one card; focus is unchanged; clicks pass through.
pub fn run_osd(canary: &Canary, args: &[String]) -> (bool, String) {
    let dir = match shot_dir(args) {
        Ok(d) => d,
        Err(e) => return (false, e),
    };
    let panel = match panel_of(canary) {
        Ok(p) => p,
        Err(e) => return (false, e),
    };
    let Some((root_w, root_h)) = canary
        .conn
        .get_geometry(canary.root)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|g| (u32::from(g.width), u32::from(g.height)))
    else {
        return (false, "root geometry failed".to_owned());
    };
    let mut checks: Vec<(&str, bool, String)> = Vec::new();
    let Some(beneath) = open_beneath(
        canary,
        (
            (root_w as i16 / 2) - 300,
            (root_h as i16 / 2) - 150,
            600,
            300,
        ),
    ) else {
        return (false, "beneath window create failed".to_owned());
    };
    let sink_ok = sink_node_id().is_some();
    checks.push(("private-sink-present", sink_ok, String::new()));
    let focus_before = focus_state(canary);

    // Output centre: the card centres on the physical output (osd.rs placement, B8).
    let expected_x = (root_w as i32 - OSD_BODY.0 as i32) / 2;
    let expected_y = (root_h as i32 - OSD_BODY.1 as i32) / 2;
    let mut captures: Vec<(String, Vec<u8>)> = Vec::new();
    let mut focus_ok = true;
    let mut hide_worst = Duration::ZERO;
    let mut shown_all = true;
    let mut steps: Vec<(String, bool)> = OSD_LEVELS
        .iter()
        .map(|&level| (format!("{level}"), false))
        .collect();
    steps.push(("muted".to_owned(), true));
    for (index, (label, muted)) in steps.iter().enumerate() {
        let started = Instant::now();
        let changed = if *muted {
            set_sink_mute(true)
        } else {
            if index == 0 {
                let _ = set_sink_mute(false);
            }
            set_sink_percent(label.parse().unwrap_or(0))
        };
        let shown = wait_osd_shown(canary, Duration::from_millis(1000));
        shown_all &= shown.is_some();
        let Some(card) = shown else {
            checks.push((
                "osd-shown",
                false,
                format!("level={label} changed={changed}"),
            ));
            let _ = set_sink_mute(false);
            continue;
        };
        if index == 0 {
            focus_ok &= focus_state(canary) == focus_before;
        }
        let size_ok = (card.width as u32, card.height as u32) == OSD_BODY;
        let centre_x_ok = (i32::from(card.x) - expected_x).abs() <= 1;
        let centre_y_ok = (i32::from(card.y) - expected_y).abs() <= 1;
        let rect = (
            i32::from(card.x),
            i32::from(card.y),
            card.width as u32,
            card.height as u32,
        );
        let cap = grab(canary, rect);
        let _ = screenshot(&dir, &format!("b15-osd-{label}.png"));
        checks.push((
            "osd-body-215x37",
            size_ok,
            format!("level={label} size={}x{}", card.width, card.height),
        ));
        checks.push((
            "osd-centred",
            centre_x_ok && centre_y_ok,
            format!(
                "level={label} at={},{} x_centre_ok={centre_x_ok} y_centre_ok={centre_y_ok} expected_xy={expected_x},{expected_y} output={root_w}x{root_h}",
                card.x, card.y
            ),
        ));
        match cap {
            Some(data) => captures.push((label.clone(), data)),
            None => checks.push(("osd-capture", false, format!("level={label}"))),
        }
        let hidden = wait_osd_hidden(
            canary,
            started,
            OSD_HIDE_LIMIT + Duration::from_millis(1000),
        );
        let hide_ok = hidden.is_some_and(|d| d <= OSD_HIDE_LIMIT);
        if let Some(d) = hidden {
            hide_worst = hide_worst.max(d);
        }
        checks.push((
            "osd-hides-1.5s",
            hide_ok,
            format!(
                "level={label} hidden_after_ms={}",
                hidden.map_or(-1, |d| d.as_millis() as i64)
            ),
        ));
        if *muted {
            let _ = set_sink_mute(false);
        }
    }
    checks.push(("osd-shown-each-level", shown_all, String::new()));
    checks.push((
        "osd-never-takes-focus",
        focus_ok,
        format!("before={focus_before:?} after={:?}", focus_state(canary)),
    ));

    // Fill analysis against the empty (0%) and full (150%) cards.
    let (card_w, card_h) = OSD_BODY;
    let empty = captures
        .iter()
        .find(|(l, _)| l == "0")
        .map(|(_, d)| d.clone());
    let full = captures
        .iter()
        .find(|(l, _)| l == "150")
        .map(|(_, d)| d.clone());
    match (empty, full) {
        (Some(empty), Some(full)) => match track_row(&empty, &full, card_w, card_h) {
            Some(row) => {
                for (label, data) in &captures {
                    let fraction = fill_fraction(&empty, data, card_w, row);
                    let (ok, expected) = if label == "muted" {
                        (fraction <= OSD_FILL_TOLERANCE, 0.0)
                    } else {
                        let level: f64 = label.parse().unwrap_or(0.0);
                        let expected = level / OSD_VOLUME_MAX;
                        ((fraction - expected).abs() <= OSD_FILL_TOLERANCE, expected)
                    };
                    checks.push((
                        "track-fill-fraction",
                        ok,
                        format!(
                            "level={label} fill={fraction:.3} expected={expected:.3} row={row}"
                        ),
                    ));
                }
                // Muted look: the speaker glyph changes between unmuted 100% and muted.
                let muted_look = captures
                    .iter()
                    .find(|(l, _)| l == "muted")
                    .zip(captures.iter().find(|(l, _)| l == "100"))
                    .map(|((_, m), (_, u))| {
                        (0..card_h)
                            .flat_map(|y| (0..OSD_ICON_W).map(move |x| (x, y)))
                            .filter(|&(x, y)| {
                                differs(rgb(m, card_w, x, y), rgb(u, card_w, x, y), INK)
                            })
                            .count()
                    });
                checks.push((
                    "muted-icon-differs",
                    muted_look.is_some_and(|n| n >= 20),
                    format!("icon_pixels_changed={muted_look:?}"),
                ));
            }
            None => checks.push((
                "track-row",
                false,
                "no fill track found between 0% and 150%".to_owned(),
            )),
        },
        _ => checks.push(("track-row", false, "missing 0% or 150% capture".to_owned())),
    }

    // Two changes 100 ms apart (inside 300 ms) must produce one visible card.
    let rapid_start = Instant::now();
    let rapid_ok = set_sink_percent(40) && {
        sleep(Duration::from_millis(100));
        set_sink_percent(60)
    };
    let mut max_visible = 0usize;
    let mut ids = std::collections::BTreeSet::new();
    while rapid_start.elapsed() < Duration::from_millis(400) {
        let cards = osd_cards(canary);
        for card in cards
            .iter()
            .filter(|w| w.map_state == u8::from(MapState::VIEWABLE))
        {
            ids.insert(card.id);
        }
        max_visible = max_visible.max(
            cards
                .iter()
                .filter(|w| w.map_state == u8::from(MapState::VIEWABLE))
                .count(),
        );
        sleep(Duration::from_millis(20));
    }
    let _ = wait_osd_hidden(
        canary,
        rapid_start,
        OSD_HIDE_LIMIT + Duration::from_millis(1000),
    );
    checks.push((
        "two-rapid-changes-one-card",
        rapid_ok && max_visible == 1 && ids.len() == 1 && osd_cards(canary).len() == 1,
        format!(
            "changed={rapid_ok} max_visible={max_visible} card_ids={}",
            ids.len()
        ),
    ));

    // Click-through: the card is on top of the beneath window, a real click reaches it.
    let clicked_card =
        set_sink_percent(50) && wait_osd_shown(canary, Duration::from_millis(1000)).is_some();
    let through = clicked_card
        && osd_visible(canary).is_some_and(|card| {
            let cx = card.x + card.width as i16 / 2;
            let cy = card.y + card.height as i16 / 2;
            real_click(canary, cx, cy, 1)
        })
        && press_seen(canary, beneath, Duration::from_millis(1000));
    checks.push((
        "click-reaches-window-beneath",
        through,
        format!("osd_shown={clicked_card} beneath={beneath}"),
    ));

    let _ = set_sink_mute(false);
    let _ = set_sink_percent(100);
    let _ = canary.conn.destroy_window(beneath);
    let _ = canary.conn.flush();
    checks.push((
        "hide-worst-ms",
        hide_worst <= OSD_HIDE_LIMIT,
        format!("worst_ms={}", hide_worst.as_millis()),
    ));
    summarize(&checks)
}

/// Words (lower-cased) with 1x centre positions from tesseract TSV on a 3x grey upscale.
fn ocr_words(png: &Path) -> Result<Vec<(String, i32, i32)>, String> {
    let big = png.with_extension("ocr.png");
    let scaled = Command::new("convert")
        .arg(png)
        .args(["-colorspace", "Gray", "-resize", "300%"])
        .arg(&big)
        .status()
        .map_err(|e| format!("convert unavailable: {e}"))?;
    if !scaled.success() {
        return Err("convert failed".to_owned());
    }
    let out = Command::new("tesseract")
        .arg(&big)
        .args(["stdout", "--psm", "11", "tsv"])
        .output()
        .map_err(|e| format!("tesseract unavailable: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    Ok(text
        .lines()
        .skip(1)
        .filter_map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 12 {
                return None;
            }
            let conf: f64 = cols[10].parse().ok()?;
            let word = cols[11].trim();
            if conf < 40.0 || word.is_empty() {
                return None;
            }
            let x: i32 = cols[6].parse().ok()?;
            let y: i32 = cols[7].parse().ok()?;
            Some((word.to_lowercase(), x / 3, y / 3))
        })
        .collect())
}

/// B15 explicit-audio-apps: the audio popup Applications tab lists QMMP and Firefox as two
/// rows and shows no raw `alsa_output` text (checked by OCR of the popup crop).
pub fn run_audio_apps(canary: &Canary, args: &[String]) -> (bool, String) {
    let dir = match shot_dir(args) {
        Ok(d) => d,
        Err(e) => return (false, e),
    };
    let panel = match panel_of(canary) {
        Ok(p) => p,
        Err(e) => return (false, e),
    };
    let cy = panel.y + panel.height as i16 / 2;
    let audio_button = (panel.x + panel.width as i16 - 120, cy);
    let mut checks: Vec<(&str, bool, String)> = Vec::new();
    if viewable(canary, AUDIO_POPUP).is_some() {
        let _ = real_escape(canary);
        let _ = wait_for(Duration::from_secs(2), || {
            viewable(canary, AUDIO_POPUP).is_none()
        });
    }
    let opened = real_click(canary, audio_button.0, audio_button.1, 1)
        && wait_for(Duration::from_secs(3), || {
            viewable(canary, AUDIO_POPUP).is_some()
        });
    checks.push(("popup-opens", opened, String::new()));
    let Some(popup) = viewable(canary, AUDIO_POPUP) else {
        checks.push(("popup-viewable", false, String::new()));
        return summarize(&checks);
    };
    // Let the popup finish opening, then click Applications until its content differs from
    // the Devices content (bounded: 3 attempts, 1.5 s settle each; attempts are reported).
    sleep(Duration::from_millis(500));
    let popup = viewable(canary, AUDIO_POPUP).unwrap_or(popup);
    let apps = (popup.x + TAB_APPS.0, popup.y + TAB_APPS.1);
    let content = (
        i32::from(popup.x) + CONTENT_REL.0,
        i32::from(popup.y) + CONTENT_REL.1,
        CONTENT_REL.2,
        CONTENT_REL.3,
    );
    let devices_content = grab(canary, content);
    let mut attempts = 0usize;
    let mut tab_switched = false;
    while attempts < 3 && !tab_switched {
        attempts += 1;
        let clicked = real_click(canary, apps.0, apps.1, 1);
        tab_switched = clicked
            && wait_for(Duration::from_millis(1500), || {
                match (&devices_content, grab(canary, content)) {
                    (Some(before), Some(now)) => {
                        differing_permille(&now, before, content.2) >= OTHER_TAB_MIN_PERMILLE
                    }
                    _ => false,
                }
            });
    }
    let popup = viewable(canary, AUDIO_POPUP).unwrap_or(popup);
    checks.push((
        "applications-tab-switched",
        tab_switched,
        format!("attempts={attempts}"),
    ));

    let rect = (
        i32::from(popup.x),
        i32::from(popup.y),
        u32::from(popup.width),
        u32::from(popup.height),
    );
    let crop = match screenshot(&dir, "b15-audio-apps-root.png") {
        Ok(root) => {
            let out = dir.join("b15-audio-apps-popup.png");
            let geometry = format!("{}x{}+{}+{}", rect.2, rect.3, rect.0, rect.1);
            let ok = Command::new("convert")
                .arg(&root)
                .args(["-crop", &geometry, "+repage"])
                .arg(&out)
                .status()
                .is_ok_and(|s| s.success());
            ok.then_some(out)
        }
        Err(_) => None,
    };
    match crop.map(|png| ocr_words(&png)) {
        Some(Ok(words)) => {
            let count = |needle: &str| {
                words
                    .iter()
                    .filter(|(w, _, _)| w.as_str() == needle)
                    .count()
            };
            let row_y = |needle: &str| {
                words
                    .iter()
                    .find(|(w, _, _)| w.as_str() == needle)
                    .map(|(_, _, y)| *y)
            };
            let (qmmp, firefox) = (count("qmmp"), count("firefox"));
            let distinct = match (row_y("qmmp"), row_y("firefox")) {
                (Some(a), Some(b)) => (a - b).abs() >= 16,
                _ => false,
            };
            let raw = words
                .iter()
                .filter(|(w, _, _)| {
                    w.contains("alsa") || w.contains("pci-0000") || w.contains("analog-stereo")
                })
                .count();
            checks.push(("row-qmmp-once", qmmp == 1, format!("count={qmmp}")));
            checks.push(("row-firefox-once", firefox == 1, format!("count={firefox}")));
            checks.push((
                "two-distinct-rows",
                distinct,
                format!(
                    "qmmp_y={:?} firefox_y={:?}",
                    row_y("qmmp"),
                    row_y("firefox")
                ),
            ));
            checks.push((
                "no-raw-alsa-text",
                raw == 0,
                format!("raw_matches={raw} words={}", words.len()),
            ));
        }
        Some(Err(error)) => checks.push(("ocr", false, error)),
        None => checks.push((
            "popup-crop",
            false,
            "convert or root shot failed".to_owned(),
        )),
    }
    let closed = real_escape(canary)
        && wait_for(Duration::from_secs(2), || {
            viewable(canary, AUDIO_POPUP).is_none()
        });
    checks.push(("escape-closes-popup", closed, String::new()));
    summarize(&checks)
}

#[cfg(test)]
mod tests {
    use super::{differs, merge_bands};

    #[test]
    fn glyph_gaps_merge_into_one_row() {
        let glyphs = [(216, 216), (218, 219), (221, 224), (247, 252), (254, 255)];
        assert_eq!(merge_bands(&glyphs, 8), vec![(216, 224), (247, 255)]);
    }

    #[test]
    fn ink_threshold_is_channel_wise() {
        assert!(differs((0, 0, 0), (0, 0, 60), 48));
        assert!(!differs((10, 10, 10), (20, 20, 20), 48));
    }
}

//! flamewm-debug: rate-limited per-process debug log writer. No async runtime.
//!
//! Files live under `$FLAMEWM_DEBUG_DIR/<name>.log` (default `<cwd>/.debug`).
//! Logging is active only when `FLAMEWM_DEBUG=1` is set in the environment.
//! Each file caps at 1 MiB; once capped a single `CAP` marker line is
//! appended and further lines are dropped. Cooldowns are keyed by
//! [`DebugEventId`] and checked before the message closure runs, so
//! suppressed events cost nothing. Never pass secrets: [`init_process`]
//! writes only `process` name + `pid` in its session header.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Rate-limit key. The inner `&'static str` is the stable event name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DebugEventId(pub &'static str);

/// icon.service.summary event; call sites pass `ICON_SERVICE_SUMMARY_COOLDOWN`.
pub const ICON_SERVICE_SUMMARY: DebugEventId = DebugEventId("icon.service.summary");
/// Default cooldown for [`ICON_SERVICE_SUMMARY`]; enforced via `emit` arg.
pub const ICON_SERVICE_SUMMARY_COOLDOWN: Duration = Duration::from_millis(5000);
/// icon.resolve.result event; call sites pass `ICON_RESOLVE_RESULT_COOLDOWN`.
pub const ICON_RESOLVE_RESULT: DebugEventId = DebugEventId("icon.resolve.result");
/// Default cooldown for [`ICON_RESOLVE_RESULT`]; enforced via `emit` arg.
pub const ICON_RESOLVE_RESULT_COOLDOWN: Duration = Duration::from_millis(1000);
/// icon.fallback event; call sites pass `ICON_FALLBACK_COOLDOWN`.
pub const ICON_FALLBACK: DebugEventId = DebugEventId("icon.fallback");
/// Default cooldown for [`ICON_FALLBACK`]; enforced via `emit` arg.
pub const ICON_FALLBACK_COOLDOWN: Duration = Duration::from_millis(1000);
/// icon.apply.summary event; call sites pass `ICON_APPLY_SUMMARY_COOLDOWN`.
pub const ICON_APPLY_SUMMARY: DebugEventId = DebugEventId("icon.apply.summary");
/// Default cooldown for [`ICON_APPLY_SUMMARY`]; enforced via `emit` arg.
pub const ICON_APPLY_SUMMARY_COOLDOWN: Duration = Duration::from_millis(5000);
/// wm.resize.summary event; call sites pass `WM_RESIZE_SUMMARY_COOLDOWN`.
// Kept for J15 migration; prefer `wm.frame.*` ids for new code.
pub const WM_RESIZE_SUMMARY: DebugEventId = DebugEventId("wm.resize.summary");
/// Default cooldown for [`WM_RESIZE_SUMMARY`]; enforced via `emit` arg.
pub const WM_RESIZE_SUMMARY_COOLDOWN: Duration = Duration::from_secs(1);
/// Maximum bytes per log file before the CAP marker + drop behaviour.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// New `wm.frame.*` ids. Static `&'static str` only (profiler/arch-guard).
macro_rules! frame_ids {
    ($(($sym:ident, $name:literal)),* $(,)?) => {
        $(pub const $sym: DebugEventId = DebugEventId($name);)*
        /// Allowlist of all `wm.frame.*` static ids for harness/profile checks.
        pub const WM_FRAME_IDS: &[&str] = &[$($name),*];
    };
}

frame_ids!(
    (WM_FRAME_EVENT_DISPATCH, "wm.frame.event.dispatch"),
    (WM_FRAME_SESSION_BEGIN, "wm.frame.session.begin"),
    (WM_FRAME_SESSION_MOTION, "wm.frame.session.motion"),
    (WM_FRAME_GEOMETRY_PLAN, "wm.frame.geometry.plan"),
    (WM_FRAME_GEOMETRY_COMMIT, "wm.frame.geometry.commit"),
    (
        WM_FRAME_GEOMETRY_MOVE_NATIVE,
        "wm.frame.geometry.move_native"
    ),
    (
        WM_FRAME_GEOMETRY_RESIZE_NATIVE,
        "wm.frame.geometry.resize_native"
    ),
    (WM_FRAME_INPUT_LAYOUT, "wm.frame.input_layout"),
    (WM_FRAME_CHROME_RENDER, "wm.frame.chrome.render"),
    (WM_FRAME_CHROME_TITLE, "wm.frame.chrome.title"),
    (WM_FRAME_CHROME_CONTROLS, "wm.frame.chrome.controls"),
    (WM_FRAME_CHROME_SHAPE, "wm.frame.chrome.shape"),
    (WM_FRAME_CONFIGURE_NOTIFY, "wm.frame.configure_notify"),
    (WM_FRAME_GRAB, "wm.frame.grab"),
    (WM_FRAME_MOTION_RECEIVED, "wm.frame.motion.received"),
    (WM_FRAME_MOTION_COALESCED, "wm.frame.motion.coalesced"),
    (WM_FRAME_GEOMETRY_REQUESTS, "wm.frame.geometry.requests"),
    (WM_FRAME_GEOMETRY_NOOP, "wm.frame.geometry.noop"),
    (WM_FRAME_GEOMETRY_COMMITS, "wm.frame.geometry.commits"),
    (WM_FRAME_MOVE_COMMITS, "wm.frame.move.commits"),
    (WM_FRAME_RESIZE_COMMITS, "wm.frame.resize.commits"),
    (WM_FRAME_INPUT_RELAYOUTS, "wm.frame.input.relayouts"),
    (WM_FRAME_CHROME_REPAINTS, "wm.frame.chrome.repaints"),
    (WM_FRAME_GRAB_FAIL, "wm.frame.grab.fail"),
    (WM_CONFIGURE_SUMMARY, "wm.configure.summary"),
    (WM_CONFIGURE_MISMATCH, "wm.configure.mismatch"),
);

struct State {
    /// Resolved log file for this process (None = unknown process name).
    file: Option<PathBuf>,
    /// Last emission per event id.
    last_emit: HashMap<&'static str, Instant>,
    /// Ids already emitted via [`emit_once`].
    once_emitted: std::collections::HashSet<&'static str>,
    /// Whether the CAP marker was already written for this file.
    capped: bool,
}

impl State {
    fn new() -> State {
        State {
            file: None,
            last_emit: HashMap::new(),
            once_emitted: std::collections::HashSet::new(),
            capped: false,
        }
    }
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::new()))
}

fn enabled() -> bool {
    enabled_for(std::env::var_os("FLAMEWM_DEBUG"))
}

fn debug_dir_for(raw: Option<std::ffi::OsString>) -> PathBuf {
    match raw.map(PathBuf::from) {
        Some(path) if !path.as_os_str().is_empty() => path,
        _ => PathBuf::from(".debug"),
    }
}

fn enabled_for(value: Option<std::ffi::OsString>) -> bool {
    value.is_some_and(|v| v == "1")
}

fn session_header(process_name: &str) -> String {
    format!(
        "debug.session process={process_name} pid={}",
        std::process::id()
    )
}

fn debug_dir() -> PathBuf {
    debug_dir_for(std::env::var_os("FLAMEWM_DEBUG_DIR"))
}

/// Map a process name to its log file name. Unknown names resolve to None.
fn file_for_process(process_name: &str) -> Option<&'static str> {
    match process_name {
        "wm" | "flamewm" | "flamewm-wm" => Some("wm.log"),
        "desktop" | "flamewm-desktop" => Some("desktop.log"),
        "shell" | "flamewm-shell" => Some("shell.log"),
        "quick-control" | "flamewm-quick-control" | "quick_control" => Some("quick-control.log"),
        _ => None,
    }
}

/// Register this process and touch its log file when enabled.
/// Safe to call more than once; the last call wins for the file mapping.
/// Writes a `debug.session process=<name> pid=<pid>` header (no secrets)
/// so enabled runs always produce a file even before any `emit`.
pub fn init_process(process_name: &str) {
    let dir = debug_dir();
    init_process_in(&dir, process_name, enabled());
}

/// Explicit-dir variant used by process startup after env resolution and by
/// tests (avoids process-env mutation, which the workspace forbids).
pub fn init_process_in(dir: &std::path::Path, process_name: &str, is_enabled: bool) {
    let dir = debug_dir_for(Some(dir.as_os_str().to_os_string()));
    let _ = fs::create_dir_all(&dir);
    let file = file_for_process(process_name).map(|name| dir.join(name));
    if let Ok(mut guard) = state().lock() {
        guard.file = file;
        guard.capped = false;
    }
    if !is_enabled {
        return;
    }
    let Some(path) = file_for_process(process_name).map(|name| dir.join(name)) else {
        return;
    };
    let line = session_header(process_name);
    if let Ok(mut guard) = state().lock() {
        write_line(&path, &line, &mut guard.capped);
    }
}

fn write_line(path: &std::path::Path, line: &str, capped: &mut bool) {
    if *capped {
        return;
    }
    let current_len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if current_len >= MAX_FILE_BYTES {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "CAP reached 1MiB, dropping further lines");
        }
        *capped = true;
        return;
    }
    // Truncate the single line if it alone would overflow the cap.
    let mut text = line;
    let owned;
    if current_len + line.len() as u64 + 1 > MAX_FILE_BYTES {
        let allowed = MAX_FILE_BYTES.saturating_sub(current_len + 1) as usize;
        owned = line
            .char_indices()
            .take_while(|(i, _)| *i < allowed)
            .map(|(_, c)| c)
            .collect::<String>();
        text = &owned;
        // Fall through to write the truncated line, then mark capped.
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{text}");
    }
    if current_len + line.len() as u64 + 1 >= MAX_FILE_BYTES {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "CAP reached 1MiB, dropping further lines");
        }
        *capped = true;
    }
}

/// Emit a rate-limited debug line. The cooldown is checked before `message`
/// runs; when disabled, on cooldown, or uninitialised, `message` never runs.
pub fn emit(id: DebugEventId, cooldown: Duration, message: impl FnOnce() -> String) {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    {
        let guard = state().lock();
        let Ok(guard) = guard else { return };
        if guard.file.is_none() {
            return;
        }
        if let Some(last) = guard.last_emit.get(id.0) {
            if now.duration_since(*last) < cooldown {
                return;
            }
        }
    }
    // Cooldown passed: build the message exactly once.
    let text = message();
    let mut guard = state().lock().ok();
    let Some(guard) = guard.as_deref_mut() else {
        return;
    };
    let Some(path) = guard.file.clone() else {
        return;
    };
    guard.last_emit.insert(id.0, now);
    write_line(&path, &format!("{} {}", id.0, text), &mut guard.capped);
}

/// Emit at most once per process lifetime for `id`. The closure runs only on
/// the first call (when enabled and initialised).
pub fn emit_once(id: DebugEventId, message: impl FnOnce() -> String) {
    if !enabled() {
        return;
    }
    {
        let guard = state().lock();
        let Ok(guard) = guard else { return };
        if guard.file.is_none() || guard.once_emitted.contains(id.0) {
            return;
        }
    }
    let text = message();
    let mut guard = state().lock().ok();
    let Some(guard) = guard.as_deref_mut() else {
        return;
    };
    let Some(path) = guard.file.clone() else {
        return;
    };
    // Re-check under the write lock so concurrent first calls emit once.
    if !guard.once_emitted.insert(id.0) {
        return;
    }
    write_line(&path, &format!("{} {}", id.0, text), &mut guard.capped);
}

#[cfg(test)]
mod tests {
    use super::{debug_dir_for, enabled_for, file_for_process, session_header};
    use std::ffi::OsString;
    use std::fs;

    #[test]
    fn debug_dir_and_init_lifecycle() {
        // Custom dir honored (pure helper, no process env mutation).
        assert_eq!(
            debug_dir_for(Some(OsString::from("/tmp/flamewm-f20-custom-debug"))),
            std::path::PathBuf::from("/tmp/flamewm-f20-custom-debug")
        );
        // Default is .debug.
        assert_eq!(debug_dir_for(None), std::path::PathBuf::from(".debug"));
        assert!(enabled_for(Some(OsString::from("1"))));
        assert!(!enabled_for(None));
        assert_eq!(file_for_process("wm"), Some("wm.log"));
        assert_eq!(file_for_process("flamewm-desktop"), Some("desktop.log"));
        assert_eq!(file_for_process("nope"), None);

        // Enabled creates file with session header (explicit dir seam).
        let dir = std::env::temp_dir().join(format!("flamewm-f20-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        super::init_process_in(&dir, "wm", true);
        let content = fs::read_to_string(dir.join("wm.log")).expect("enabled creates file");
        assert!(
            content.contains("debug.session process=wm pid="),
            "{content}"
        );
        // Disabled creates no file.
        let dir2 = std::env::temp_dir().join(format!("flamewm-f20-off-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir2);
        super::init_process_in(&dir2, "wm", false);
        assert!(
            !dir2.join("wm.log").exists(),
            "disabled must not create file"
        );
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dir2);
    }
}

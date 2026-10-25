//! `flamewm-wayland --probe`: prints which prerequisites of the Wayland session exist and exits 0
//! when every required one is present, 1 otherwise.
//!
//! Read-only by design: connector state is read from sysfs, logind is queried over D-Bus through
//! `busctl`, nothing is written, grabbed or launched, and no session control is ever taken. DRM
//! nodes are only opened when this process resolved its own active user-class logind session,
//! because a direct open without one proves nothing: the real path is logind's `TakeDevice`.
//! Libraries are checked in the loader cache (`ldconfig -p`), not dlopen-ed: the crate forbids
//! unsafe code and adds no dependency for this.
//!
//! Requirements follow the selected backend: the `drm` (native) backend needs GBM, libdrm, libinput
//! and libudev on top of EGL/GLESv2, the nested backend only needs EGL/GLESv2. libseat is never
//! required: the session comes from systemd-logind, so seatd/libseat is not part of the stack.

use std::env;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Backend, select_backend};

const LOGIND: &str = "org.freedesktop.login1";
const LOGIND_MANAGER: &str = "org.freedesktop.login1.Manager";
const LOGIND_SESSION: &str = "org.freedesktop.login1.Session";
const LOGIND_PATH: &str = "/org/freedesktop/login1";

/// When a runtime library is required.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Need {
    /// Required by every backend.
    Always,
    /// Required only by the native (`drm`) backend.
    NativeOnly,
    /// Reported, never a failure.
    Informational,
}

struct Library {
    soname: &'static str,
    why: &'static str,
    need: Need,
}

impl Library {
    fn required(&self, backend: Backend) -> bool {
        match self.need {
            Need::Always => true,
            Need::NativeOnly => backend == Backend::Native,
            Need::Informational => false,
        }
    }
}

/// Sonames the renderer and the selected backend load at runtime.
const LIBRARIES: &[Library] = &[
    Library {
        soname: "libEGL.so.1",
        why: "EGL, required by both backends",
        need: Need::Always,
    },
    Library {
        soname: "libGLESv2.so.2",
        why: "GLES, required by both backends",
        need: Need::Always,
    },
    Library {
        soname: "libgbm.so.1",
        why: "GBM allocator, drm backend",
        need: Need::NativeOnly,
    },
    Library {
        soname: "libdrm.so.2",
        why: "DRM/KMS, drm backend",
        need: Need::NativeOnly,
    },
    Library {
        soname: "libinput.so.10",
        why: "input, drm backend",
        need: Need::NativeOnly,
    },
    Library {
        soname: "libudev.so.1",
        why: "device discovery, drm backend",
        need: Need::NativeOnly,
    },
    Library {
        soname: "libseat.so.1",
        why: "not required: logind session",
        need: Need::Informational,
    },
];

/// Sonames that must be present for `backend`, in table order (test oracle for the requirement
/// table; the probe itself walks [`LIBRARIES`] so it can report every soname).
#[cfg(test)]
fn required_sonames(backend: Backend) -> Vec<&'static str> {
    LIBRARIES
        .iter()
        .filter(|library| library.required(backend))
        .map(|library| library.soname)
        .collect()
}

#[derive(Default)]
struct Report {
    lines: Vec<String>,
    failures: Vec<String>,
}

impl Report {
    fn check(&mut self, required: bool, ok: bool, name: &str, detail: &str) {
        let tag = match (ok, required) {
            (true, _) => "ok",
            (false, true) => "FAIL",
            (false, false) => "warn",
        };
        self.lines.push(format!("[{tag}] {name}: {detail}"));
        if !ok && required {
            self.failures.push(name.to_owned());
        }
    }

    fn info(&mut self, name: &str, detail: &str) {
        self.lines.push(format!("[info] {name}: {detail}"));
    }
}

/// Runs the probe, prints the report and returns the process exit code.
pub fn run() -> i32 {
    let mut report = Report::default();
    let backend = select_backend(env::var("FLAMEWM_WAYLAND_BACKEND").ok().as_deref());
    let native = backend == Backend::Native;
    let drm_built = cfg!(feature = "drm");
    report.info(
        "binary",
        &format!("flamewm-wayland {}", env!("CARGO_PKG_VERSION")),
    );
    report.info("pid", &std::process::id().to_string());
    report.info(
        "backend",
        &format!(
            "{} (FLAMEWM_WAYLAND_BACKEND when set, else the compiled feature)",
            backend.name()
        ),
    );
    report.check(
        native,
        drm_built,
        "drm backend compiled",
        if drm_built {
            "yes"
        } else {
            "no; rebuild with --features drm (the native backend exits 2 without it)"
        },
    );
    environment(&mut report);
    let usable_session = logind(&mut report, native);
    session_services(&mut report);
    drm_nodes(&mut report, native, usable_session);
    connectors(&mut report);
    input_devices(&mut report, native);
    libraries(&mut report, backend);
    let xwayland = find_in_path("Xwayland");
    report.check(
        true,
        xwayland.is_some(),
        "XWayland binary",
        &xwayland.map_or_else(
            || "not found on PATH".to_owned(),
            |path| path.display().to_string(),
        ),
    );

    for line in &report.lines {
        println!("{line}");
    }
    if report.failures.is_empty() {
        println!("result: PASS");
        0
    } else {
        println!("result: FAIL ({})", report.failures.join(", "));
        1
    }
}

fn environment(report: &mut Report) {
    let runtime = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let detail = match runtime.as_deref() {
        None => "unset".to_owned(),
        Some(path) => match fs::metadata(path) {
            Ok(meta) if meta.is_dir() => format!(
                "{} (mode {:o})",
                path.display(),
                meta.permissions().mode() & 0o7777
            ),
            _ => format!("{} (missing or not a directory)", path.display()),
        },
    };
    let ok = runtime.as_deref().is_some_and(Path::is_dir);
    report.check(true, ok, "XDG_RUNTIME_DIR", &detail);
    for var in [
        "XDG_SESSION_TYPE",
        "XDG_SESSION_ID",
        "XDG_SEAT",
        "XDG_VTNR",
        "XDG_CURRENT_DESKTOP",
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        report.info(var, &env::var(var).unwrap_or_else(|_| "<unset>".to_owned()));
    }
}

/// logind session of this process: by `XDG_SESSION_ID` when set, else `GetSessionByPID` (which fails
/// for processes outside any login1 session, e.g. under some sandboxes). Only `Get`/property reads
/// are issued: the probe never calls `TakeControl`.
///
/// Returns true when this process resolved its own session, it is class `user` and it is active.
fn logind(report: &mut Report, native: bool) -> bool {
    let session = match env::var("XDG_SESSION_ID").ok().filter(|id| !id.is_empty()) {
        Some(id) => busctl(&[
            "call",
            LOGIND,
            LOGIND_PATH,
            LOGIND_MANAGER,
            "GetSession",
            "s",
            &id,
        ]),
        None => {
            let pid = std::process::id().to_string();
            busctl(&[
                "call",
                LOGIND,
                LOGIND_PATH,
                LOGIND_MANAGER,
                "GetSessionByPID",
                "u",
                &pid,
            ])
        }
    }
    .and_then(|out| last_quoted(&out));
    let Some(path) = session else {
        report.check(
            true,
            false,
            "logind session",
            "no session found (GetSession by XDG_SESSION_ID or GetSessionByPID; no system bus?)",
        );
        return false;
    };
    report.check(true, true, "logind session", &path);
    let mut properties = Vec::new();
    for prop in [
        "Id", "Type", "Class", "Active", "State", "Seat", "VTNr", "Remote",
    ] {
        let value = busctl(&["get-property", LOGIND, &path, LOGIND_SESSION, prop]).map_or_else(
            || "unreadable".to_owned(),
            |out| {
                out.split_once(' ')
                    .map_or(out.clone(), |(_, v)| v.to_owned())
            },
        );
        report.info(&format!("logind {prop}"), &value);
        properties.push(value);
    }
    let class = quoted_value(&properties[2]);
    let active = bool_value(&properties[3]);
    let usable = class.as_deref() == Some("user") && active == Some(true);
    report.check(
        native,
        usable,
        "logind session usable",
        if usable {
            "active user session: a native start may TakeControl and TakeDevice"
        } else {
            "not an active user session; the native backend refuses to take it over"
        },
    );
    usable
}

/// The string inside a busctl reply such as `s "user"`.
fn quoted_value(raw: &str) -> Option<String> {
    last_quoted(raw)
}

/// The boolean inside a busctl reply such as `b true`.
fn bool_value(raw: &str) -> Option<bool> {
    match raw.split_whitespace().nth(1)? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// One line about the session manager the native backend depends on.
fn session_services(report: &mut Report) {
    let service = Command::new("systemctl")
        .args(["is-active", "systemd-logind"])
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|state| !state.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    let journal = if find_in_path("journalctl").is_some() {
        "present"
    } else {
        "missing"
    };
    report.info(
        "systemd-logind/journalctl",
        &format!("systemd-logind={service}, journalctl={journal}"),
    );
}

/// DRM nodes. Presence is only required for the native backend; opening them is a read-only
/// informational check and is skipped unless this process really owns an active user session,
/// because logind's `TakeDevice` is the path the compositor uses.
fn drm_nodes(report: &mut Report, native: bool, usable_session: bool) {
    let mut cards = list_names(Path::new("/dev/dri"), "card");
    cards.sort();
    if cards.is_empty() {
        report.check(native, false, "DRM card nodes", "none in /dev/dri");
        return;
    }
    report.check(
        true,
        true,
        "DRM card nodes",
        &format!("{} in /dev/dri", cards.len()),
    );
    if !usable_session {
        report.info("direct-open skipped", "no active user session");
    } else {
        for card in &cards {
            let path = Path::new("/dev/dri").join(card);
            match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(file) => {
                    drop(file);
                    report.info(card, "open O_RDWR ok (not master, closed again)");
                }
                Err(error) => report.info(
                    card,
                    &format!("open O_RDWR failed: {error} (informational: logind TakeDevice is the real path)"),
                ),
            }
        }
    }

    let mut renders = list_names(Path::new("/dev/dri"), "renderD");
    renders.sort();
    let render_open = usable_session
        && renders.iter().any(|name| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(Path::new("/dev/dri").join(name))
                .is_ok()
        });
    // Presence is informational and a failed direct open never fails the probe: the compositor
    // receives its render node from logind's `TakeDevice`.
    let open_note = if !usable_session {
        "skipped"
    } else if render_open {
        "ok"
    } else {
        "failed (informational)"
    };
    report.check(
        false,
        !renders.is_empty(),
        "DRM render node",
        &format!("{} present, direct open {open_note}", renders.len()),
    );
}

/// Connector status and first mode from sysfs (read-only, no ioctl).
fn connectors(report: &mut Report) {
    let mut entries: Vec<String> = list_names(Path::new("/sys/class/drm"), "card")
        .into_iter()
        .filter(|name| name.contains('-'))
        .collect();
    entries.sort();
    let mut connected = 0;
    for entry in &entries {
        let base = Path::new("/sys/class/drm").join(entry);
        let status = read_trimmed(&base.join("status")).unwrap_or_else(|| "unknown".to_owned());
        if status != "connected" {
            continue;
        }
        connected += 1;
        let mode = fs::read_to_string(base.join("modes"))
            .ok()
            .and_then(|text| text.lines().next().map(str::to_owned))
            .unwrap_or_else(|| "no modes".to_owned());
        report.info(entry, &format!("connected, first mode {mode}"));
    }
    report.check(
        false,
        connected > 0,
        "connected connectors",
        &format!("{connected} of {} connectors", entries.len()),
    );
}

fn input_devices(report: &mut Report, native: bool) {
    let mut events = list_names(Path::new("/sys/class/input"), "event");
    events.sort();
    let readable = events
        .iter()
        .filter(|name| {
            OpenOptions::new()
                .read(true)
                .open(Path::new("/dev/input").join(name))
                .is_ok()
        })
        .count();
    report.check(
        native,
        !events.is_empty(),
        "input event devices",
        &format!(
            "{} in /sys/class/input, {readable} readable in /dev/input (logind grants access to the active session)",
            events.len()
        ),
    );
}

fn libraries(report: &mut Report, backend: Backend) {
    let cache = ldconfig_cache();
    for library in LIBRARIES {
        let ok = cache.as_ref().is_some_and(|text| {
            text.lines()
                .any(|line| line.split_whitespace().next() == Some(library.soname))
        });
        if library.need == Need::Informational {
            report.info(
                library.soname,
                &format!(
                    "{} ({})",
                    if ok { "present" } else { "absent" },
                    library.why
                ),
            );
            continue;
        }
        let detail = match (&cache, ok) {
            (None, _) => "ldconfig -p unavailable".to_owned(),
            (Some(_), true) => format!("in loader cache ({})", library.why),
            (Some(_), false) => format!("not in loader cache ({})", library.why),
        };
        report.check(library.required(backend), ok, library.soname, &detail);
    }
}

fn ldconfig_cache() -> Option<String> {
    ["/usr/sbin/ldconfig", "ldconfig"].iter().find_map(|tool| {
        let output = Command::new(tool).arg("-p").output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    })
}

/// Runs `busctl --system` with a bounded timeout; `None` on any failure.
fn busctl(args: &[&str]) -> Option<String> {
    let output = Command::new("busctl")
        .arg("--system")
        .arg("--timeout=3")
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Last double-quoted token in a busctl reply such as `o "/org/freedesktop/login1/session/_32"`.
fn last_quoted(text: &str) -> Option<String> {
    let end = text.rfind('"')?;
    let start = text[..end].rfind('"')?;
    Some(text[start + 1..end].to_owned())
}

fn list_names(dir: &Path, prefix: &str) -> Vec<String> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(prefix))
                .collect()
        })
        .unwrap_or_default()
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}

fn find_in_path(program: &str) -> Option<PathBuf> {
    env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::{LIBRARIES, Need, last_quoted, list_names, required_sonames};
    use crate::Backend;
    use std::path::Path;

    #[test]
    fn libseat_is_never_required() {
        assert!(!required_sonames(Backend::Native).contains(&"libseat.so.1"));
        assert!(!required_sonames(Backend::Nested).contains(&"libseat.so.1"));
        let seat = LIBRARIES
            .iter()
            .find(|library| library.soname == "libseat.so.1")
            .expect("libseat is still reported");
        assert_eq!(seat.need, Need::Informational);
        assert_eq!(seat.why, "not required: logind session");
    }

    #[test]
    fn required_libraries_follow_the_selected_backend() {
        assert_eq!(
            required_sonames(Backend::Native),
            vec![
                "libEGL.so.1",
                "libGLESv2.so.2",
                "libgbm.so.1",
                "libdrm.so.2",
                "libinput.so.10",
                "libudev.so.1"
            ]
        );
        assert_eq!(
            required_sonames(Backend::Nested),
            vec!["libEGL.so.1", "libGLESv2.so.2"]
        );
    }

    #[test]
    fn last_quoted_reads_object_path_reply() {
        assert_eq!(
            last_quoted("o \"/org/freedesktop/login1/session/_32\"").as_deref(),
            Some("/org/freedesktop/login1/session/_32")
        );
        assert_eq!(
            last_quoted("s \"seat0\" \"/org/freedesktop/login1/seat/seat0\"").as_deref(),
            Some("/org/freedesktop/login1/seat/seat0")
        );
        assert_eq!(last_quoted("u 2"), None);
    }

    #[test]
    fn list_names_missing_dir_is_empty() {
        assert!(list_names(Path::new("/nonexistent/flamewm-probe"), "card").is_empty());
    }
}

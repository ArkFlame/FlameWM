use super::*;

/// KDE's own KIO opener gives Dolphin/Kate users their existing defaults and
/// Open With chooser; XDG remains the universal fallback.
#[must_use]
pub fn preferred_file_opener(
    available: impl Fn(&str) -> bool,
) -> (&'static str, &'static [&'static str]) {
    if available("kde-open6") {
        return ("kde-open6", &[]);
    }
    if available("kde-open5") {
        return ("kde-open5", &[]);
    }
    if available("kioclient6") {
        return ("kioclient6", &["exec"]);
    }
    if available("kioclient5") {
        return ("kioclient5", &["exec"]);
    }
    ("xdg-open", &[])
}

/// Window in which an identical `open_target` request is treated as a repeat.
pub(super) const REPEAT_OPEN_WINDOW: std::time::Duration = std::time::Duration::from_millis(1000);
/// Most recent `open_target` request, for the repeat guard.
pub(super) static LAST_OPEN: std::sync::Mutex<Option<(std::ffi::OsString, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// True when `target` equals the previous request and arrived within the window.
pub(super) fn is_repeat_open(
    last: Option<(&std::ffi::OsStr, std::time::Instant)>,
    target: &std::ffi::OsStr,
    now: std::time::Instant,
) -> bool {
    last.is_some_and(|(previous, at)| {
        previous == target && now.saturating_duration_since(at) < REPEAT_OPEN_WINDOW
    })
}

/// Launch a file/URI with the available desktop association provider. The
/// target is one argv entry (paths with whitespace and non-UTF8 bytes are safe).
/// Child processes are reaped by the application's existing canonical launcher.
/// An identical target repeated inside `REPEAT_OPEN_WINDOW` is dropped, so a
/// handler that re-enters the opener cannot storm the session with spawns.
pub fn open_target(target: &std::ffi::OsStr) -> FlameResult<()> {
    let now = std::time::Instant::now();
    {
        let mut last = LAST_OPEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if is_repeat_open(
            last.as_ref().map(|(t, at)| (t.as_os_str(), *at)),
            target,
            now,
        ) {
            return Ok(());
        }
        *last = Some((target.to_os_string(), now));
    }
    // Folders are FlameWM's own decision (explorer or the user's chosen entry),
    // never a KDE/XDG opener call: those ignore nested mimeapps and open Dolphin.
    let folder = Path::new(target);
    if folder.is_dir() {
        return directory_open::open_directory(folder);
    }
    let (program, prefix) = preferred_file_opener(|name| {
        let Some(path) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&path).any(|dir| {
            let binary = dir.join(name);
            binary.metadata().is_ok_and(|meta| {
                use std::os::unix::fs::PermissionsExt;
                meta.is_file() && meta.permissions().mode() & 0o111 != 0
            })
        })
    });
    let mut command = Command::new(program);
    command.args(prefix).arg(target);
    // The desktop provider still resolves and opens the target. MIME metadata
    // only labels our startup sequence; it never replaces the user's handler.
    let (name, icon, wm_class) = startup_identity_for_target(target)
        .unwrap_or_else(|| ("Opening file".into(), "document-open".into(), String::new()));
    spawn_with_startup(&mut command, &name, &icon, &wm_class, true)
}

pub(super) fn short_command_output(program: &str, args: &[&std::ffi::OsStr]) -> Option<String> {
    use std::process::Stdio;
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(30);
    loop {
        match child.try_wait().ok()? {
            Some(status) if status.success() => break,
            Some(_) => return None,
            None if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let output = child.wait_with_output().ok()?;
    std::str::from_utf8(&output.stdout)
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

pub(super) fn startup_identity_for_target(
    target: &std::ffi::OsStr,
) -> Option<(String, String, String)> {
    use std::sync::{Mutex, OnceLock};
    static DEFAULTS: OnceLock<Mutex<std::collections::BTreeMap<String, Option<String>>>> =
        OnceLock::new();
    let target_text = target.to_str();
    let mime = if let Some((scheme, _)) = target_text.and_then(|value| value.split_once("://")) {
        if scheme.is_empty()
            || !scheme
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
        {
            return None;
        }
        format!("x-scheme-handler/{scheme}")
    } else {
        short_command_output(
            "xdg-mime",
            &[
                std::ffi::OsStr::new("query"),
                std::ffi::OsStr::new("filetype"),
                target,
            ],
        )?
    };
    let defaults = DEFAULTS.get_or_init(|| Mutex::new(std::collections::BTreeMap::new()));
    let cached = { defaults.lock().ok()?.get(&mime).cloned() };
    let desktop_id = if let Some(id) = cached {
        id
    } else {
        let id = short_command_output(
            "xdg-mime",
            &[
                std::ffi::OsStr::new("query"),
                std::ffi::OsStr::new("default"),
                std::ffi::OsStr::new(&mime),
            ],
        );
        defaults.lock().ok()?.insert(mime, id.clone());
        id
    }?;
    let id = DesktopAppId::new(desktop_id);
    let mut catalog = shared_application_catalog().current();
    if catalog.find(&id).is_err() {
        // Opener-miss trigger: the default handler may be installed after the last read.
        let _ = refresh_installed_applications_if_changed();
        catalog = shared_application_catalog().current();
    }
    let entry = catalog.find(&id).ok()?;
    Some((
        entry.application.name.clone(),
        if entry.application.icon_name.is_empty() {
            entry.application.startup_wm_class.clone()
        } else {
            entry.application.icon_name.clone()
        },
        entry.application.startup_wm_class.clone(),
    ))
}

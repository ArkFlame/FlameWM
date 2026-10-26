//! Terminal resolution: one owner for which terminal emulator `FlameWM` runs.
//!
//! Order: settings override (`defaultTerminal`) when it is available, then
//! `flamewm-terminal` on PATH or next to the running binaries, then `$TERMINAL`,
//! then the Debian/common fallbacks. Fallbacks are used only when `FlameWM` Terminal
//! is missing.

use flamewm_api::{ErrorCode, FlameError, FlameResult};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const FLAMEWM_TERMINAL: &str = "flamewm-terminal";

/// Tried in order only after the settings override and `FlameWM` Terminal are unavailable.
pub const FALLBACK_TERMINALS: &[&str] = &[
    "x-terminal-emulator",
    "xdg-terminal-exec",
    "konsole",
    "kitty",
    "alacritty",
    "gnome-terminal",
    "xterm",
];

/// Settings override (`defaultTerminal`), installed by the settings owner through
/// [`set_configured_terminal`]. Empty means no override.
static CONFIGURED: Mutex<String> = Mutex::new(String::new());

/// Install the `defaultTerminal` settings value. Empty selects `FlameWM` Terminal.
pub fn set_configured_terminal(value: &str) {
    let mut configured = CONFIGURED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    value.trim().clone_into(&mut configured);
}

fn configured_terminal() -> String {
    CONFIGURED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Pure resolution. `available` answers whether a program name or absolute path can run.
#[must_use]
pub fn resolve_terminal_with(
    configured: &str,
    terminal_env: Option<&str>,
    available: impl Fn(&str) -> bool,
) -> Option<String> {
    let configured = configured.trim();
    if !configured.is_empty() && available(configured) {
        return Some(configured.to_owned());
    }
    if available(FLAMEWM_TERMINAL) {
        return Some(FLAMEWM_TERMINAL.to_owned());
    }
    let env = terminal_env
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(env) = env.filter(|value| available(value)) {
        return Some(env.to_owned());
    }
    FALLBACK_TERMINALS
        .iter()
        .find(|name| available(name))
        .map(|name| (*name).to_owned())
}

/// Whether `program` (a name searched in `path_var` and `exe_dir`, or an absolute
/// path) names an executable regular file.
#[must_use]
pub fn program_available(program: &str, path_var: Option<&OsStr>, exe_dir: Option<&Path>) -> bool {
    if program.is_empty() {
        return false;
    }
    if program.starts_with('/') {
        return is_executable_file(Path::new(program));
    }
    if program.contains('/') {
        return false;
    }
    let beside_binaries = exe_dir.map(|dir| dir.join(program));
    if beside_binaries.is_some_and(|path| is_executable_file(&path)) {
        return true;
    }
    path_var.is_some_and(|path| {
        std::env::split_paths(path).any(|dir| is_executable_file(&dir.join(program)))
    })
}

fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn current_exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// Resolve the terminal for this process from the settings override, PATH, the
/// running binaries' directory and `$TERMINAL`.
#[must_use]
pub fn resolve_terminal() -> Option<String> {
    let path_var = std::env::var_os("PATH");
    let exe_dir = current_exe_dir();
    let terminal_env = std::env::var("TERMINAL").ok();
    resolve_terminal_with(&configured_terminal(), terminal_env.as_deref(), |program| {
        program_available(program, path_var.as_deref(), exe_dir.as_deref())
    })
}

/// Terminal program name for "Open Terminal" style callers. Falls back to the
/// `FlameWM` Terminal name so a missing install produces a clear spawn error.
#[must_use]
pub fn terminal_program() -> String {
    resolve_terminal().unwrap_or_else(|| FLAMEWM_TERMINAL.to_owned())
}

/// Argv that runs `command` inside `terminal`. The execute flag depends on the
/// emulator: `kitty` and `xdg-terminal-exec` take the command directly, `gnome-terminal`
/// takes `--`, and the others take `-e`. Only `FlameWM` Terminal receives `cwd`.
#[must_use]
pub fn terminal_command(terminal: &str, command: &[String], cwd: Option<&Path>) -> Vec<String> {
    let base = Path::new(terminal)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(terminal);
    let mut argv = vec![terminal.to_owned()];
    if base == FLAMEWM_TERMINAL {
        if let Some(dir) = cwd {
            argv.push("--working-directory".to_owned());
            argv.push(dir.to_string_lossy().into_owned());
        }
    }
    match base {
        "kitty" | "xdg-terminal-exec" => {}
        "gnome-terminal" => argv.push("--".to_owned()),
        _ => argv.push("-e".to_owned()),
    }
    argv.extend(command.iter().cloned());
    argv
}

/// Wrap a `Terminal=true` desktop entry's argv so it runs inside the resolved terminal.
pub fn wrap_terminal_entry(command: &[String]) -> FlameResult<Vec<String>> {
    let terminal = resolve_terminal().ok_or_else(|| {
        FlameError::new(
            ErrorCode::NotFound,
            format!("no terminal emulator found; install {FLAMEWM_TERMINAL}"),
        )
    })?;
    Ok(terminal_command(&terminal, command, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(names: &'static [&'static str]) -> impl Fn(&str) -> bool {
        move |program| names.contains(&program)
    }

    #[test]
    fn flamewm_terminal_beats_env_and_fallbacks() {
        let available = only(&["konsole", "flamewm-terminal", "x-terminal-emulator"]);
        assert_eq!(
            resolve_terminal_with("", Some("konsole"), available).as_deref(),
            Some("flamewm-terminal")
        );
    }

    #[test]
    fn settings_override_wins_when_available() {
        let available = only(&["kitty", "flamewm-terminal"]);
        assert_eq!(
            resolve_terminal_with(" kitty ", None, available).as_deref(),
            Some("kitty")
        );
    }

    #[test]
    fn missing_override_falls_back_to_flamewm_terminal() {
        let available = only(&["flamewm-terminal", "konsole"]);
        assert_eq!(
            resolve_terminal_with("alacritty", None, available).as_deref(),
            Some("flamewm-terminal")
        );
    }

    #[test]
    fn others_are_used_only_when_flamewm_terminal_is_missing() {
        let available = only(&["konsole", "x-terminal-emulator"]);
        assert_eq!(
            resolve_terminal_with("", None, available).as_deref(),
            Some("x-terminal-emulator")
        );
        let env_only = only(&["foot", "konsole"]);
        assert_eq!(
            resolve_terminal_with("", Some("foot"), env_only).as_deref(),
            Some("foot")
        );
        assert_eq!(resolve_terminal_with("", None, only(&[])), None);
    }

    #[test]
    fn terminal_command_flags_per_emulator() {
        let cmd = vec!["htop".to_owned(), "-d".to_owned(), "5".to_owned()];
        let cwd = Path::new("/home/u/work");
        assert_eq!(
            terminal_command("/opt/flamewm-terminal", &cmd, Some(cwd)),
            [
                "/opt/flamewm-terminal",
                "--working-directory",
                "/home/u/work",
                "-e",
                "htop",
                "-d",
                "5"
            ]
        );
        assert_eq!(
            terminal_command("gnome-terminal", &cmd, None),
            ["gnome-terminal", "--", "htop", "-d", "5"]
        );
        assert_eq!(
            terminal_command("kitty", &cmd, None),
            ["kitty", "htop", "-d", "5"]
        );
        assert_eq!(
            terminal_command("xterm", &cmd, Some(cwd)),
            ["xterm", "-e", "htop", "-d", "5"]
        );
    }

    fn write_script(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, "#!/bin/sh\n").expect("write");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    #[test]
    fn program_available_uses_fake_path_and_exe_dir() {
        let root = std::env::temp_dir().join(format!("flamewm-term-test-{}", std::process::id()));
        let bin = root.join("bin");
        let exe = root.join("exe");
        std::fs::create_dir_all(&bin).expect("bin dir");
        std::fs::create_dir_all(&exe).expect("exe dir");
        let on_path = bin.join("fake-term");
        write_script(&on_path);
        let beside = exe.join(FLAMEWM_TERMINAL);
        write_script(&beside);
        let not_exec = bin.join("plain");
        std::fs::write(&not_exec, "x").expect("write");
        let path_var = std::env::join_paths([&bin]).expect("path");
        assert!(program_available("fake-term", Some(&path_var), None));
        assert!(!program_available(
            "missing-term",
            Some(&path_var),
            Some(&exe)
        ));
        assert!(!program_available("plain", Some(&path_var), None));
        assert!(program_available(FLAMEWM_TERMINAL, None, Some(&exe)));
        assert!(!program_available(FLAMEWM_TERMINAL, Some(&path_var), None));
        assert!(!program_available("bin/fake-term", Some(&path_var), None));
        assert!(program_available(
            on_path.to_str().expect("utf8"),
            None,
            None
        ));
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}

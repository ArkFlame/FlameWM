use super::*;

/// Select a desktop's native MIME opener once, without depending on `FlameWM`
/// pretending to be another desktop or modifying the user's MIME database.
#[must_use]
pub(super) fn is_brave_program(program: &str) -> bool {
    matches!(
        std::path::Path::new(program)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("brave-browser" | "brave-browser-stable" | "brave" | "brave-bin")
    )
}

pub(super) fn is_helium_program(program: &str) -> bool {
    matches!(
        Path::new(program)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("helium-browser" | "helium" | "helium-bin" | "net.imput.helium")
    )
}

pub fn launch_argv(argv: &[String], options: &ApplicationLaunchOptions) -> FlameResult<()> {
    let program = argv
        .first()
        .ok_or_else(|| FlameError::invalid("empty application argv"))?;
    let mut command = Command::new(program);
    configure_kde_theme(&mut command, program, None);
    configure_browser_secret(&mut command, program, argv)?;
    command
        .args(&argv[1..])
        .args(&options.extra_args)
        .args(&options.uris);
    let icon = std::path::Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(program);
    spawn_with_startup(&mut command, program, icon, "", true)
}

pub(super) fn launch_argv_for_app(
    argv: &[String],
    app: &DesktopApplication,
    startup_notify: Option<bool>,
) -> FlameResult<()> {
    let program = argv
        .first()
        .ok_or_else(|| FlameError::invalid("empty application argv"))?;
    let mut command = Command::new(program);
    configure_kde_theme(&mut command, program, Some(&app.id));
    configure_browser_secret(&mut command, program, argv)?;
    command.args(&argv[1..]);
    let icon = if app.icon_name.is_empty() {
        &app.startup_wm_class
    } else {
        &app.icon_name
    };
    spawn_with_startup(
        &mut command,
        &app.name,
        icon,
        &app.startup_wm_class,
        startup_notify != Some(false),
    )
}

pub(super) fn configure_kde_theme(
    command: &mut Command,
    program: &str,
    app_id: Option<&DesktopAppId>,
) {
    if is_dolphin_application(program, app_id) {
        // Match the KDE platform theme used when Dolphin is opened through KIO,
        // scoped to Dolphin so FlameWM does not start KDE session services.
        command
            .env("XDG_CURRENT_DESKTOP", "KDE")
            .env("KDE_FULL_SESSION", "true")
            .env("KDE_SESSION_VERSION", "6")
            .env("QT_QPA_PLATFORMTHEME", "KDE");
    }
}

pub(super) fn is_dolphin_application(program: &str, app_id: Option<&DesktopAppId>) -> bool {
    std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("dolphin"))
        || app_id.is_some_and(|id| id.as_str().eq_ignore_ascii_case("org.kde.dolphin.desktop"))
}

/// Preserve the command intent working directory while reporting desktop utility launches.
pub fn launch_command(program: &str, args: &[String], cwd: Option<&Path>) -> FlameResult<()> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let icon = std::path::Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(program);
    spawn_with_startup(&mut command, program, icon, "", true)
}

pub(super) fn spawn_with_startup(
    command: &mut Command,
    name: &str,
    icon: &str,
    wm_class: &str,
    enabled: bool,
) -> FlameResult<()> {
    let id = enabled.then(launch_feedback::new_id);
    let sent = id.as_deref().is_some_and(|id| {
        launch_feedback::broadcast(launch_feedback::MessageKind::New, id, name, icon, wm_class)
    });
    // Never pass a stale launch ID from the parent shell to an unrelated child.
    command.env_remove("DESKTOP_STARTUP_ID");
    if sent {
        command.env("DESKTOP_STARTUP_ID", id.as_deref().unwrap_or_default());
    }
    match spawn_reaped_with_id(command, if sent { id.clone() } else { None }) {
        Ok(_) => Ok(()),
        Err(error) => {
            if sent {
                let _ = launch_feedback::broadcast(
                    launch_feedback::MessageKind::Remove,
                    id.as_deref().unwrap_or_default(),
                    "",
                    "",
                    "",
                );
            }
            Err(error)
        }
    }
}

/// Spawns `command` and gives the child to a detached waiter thread that calls
/// `wait()`, so the child is reaped when it exits. Returns the child pid.
///
/// Why this shape: every caller drops the launched handle, and dropping a
/// `Child` does not reap it, so the WM would accumulate zombies. Returning the
/// handle does not help because callers discard it. `SIGCHLD = SIG_IGN` would
/// reap globally and disturb the WM's own children. One blocked-in-`waitpid`
/// thread per launched application exits as soon as that application exits.
/// Spawns `command` for a caller that keeps no `Child` handle, reaping it when it exits.
/// Use this for every WM-side spawn; a dropped `Child` otherwise stays a zombie until the WM exits.
pub fn spawn_reaped(command: &mut Command) -> FlameResult<u32> {
    spawn_reaped_with_id(command, None)
}

pub(super) fn spawn_reaped_with_id(
    command: &mut Command,
    startup_id: Option<String>,
) -> FlameResult<u32> {
    let mut child = command.spawn().map_err(|error| {
        FlameError::new(ErrorCode::IoFailure, format!("launch application: {error}"))
    })?;
    let pid = child.id();
    // If the OS refuses the thread, the closure (and the child handle) is dropped
    // and this child stays unreaped. The application itself did launch.
    let _ = std::thread::Builder::new()
        .name(format!("flamewm-reap-{pid}"))
        .spawn(move || {
            if let Ok(status) = child.wait()
                && !status.success()
                && let Some(id) = startup_id
            {
                let _ = launch_feedback::broadcast(
                    launch_feedback::MessageKind::Remove,
                    &id,
                    "",
                    "",
                    "",
                );
            }
        });
    Ok(pid)
}

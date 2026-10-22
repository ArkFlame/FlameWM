pub mod launch_feedback;
use flamewm_api::applications::{ApplicationLaunchOptions, DesktopApplication};
use flamewm_api::{DesktopAppId, ErrorCode, FlameError, FlameResult};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    pub id: DesktopAppId,
    pub application: DesktopApplication,
    pub path: PathBuf,
    exec: String,
    startup_notify: Option<bool>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WindowApplicationIdentity {
    pub wm_instance: String,
    pub wm_class: String,
    pub executable_basename: String,
}

impl WindowApplicationIdentity {
    #[must_use]
    pub fn new(
        wm_instance: impl Into<String>,
        wm_class: impl Into<String>,
        executable_basename: impl Into<String>,
    ) -> Self {
        Self {
            wm_instance: wm_instance.into(),
            wm_class: wm_class.into(),
            executable_basename: executable_basename.into(),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ApplicationCatalog {
    applications: Vec<DesktopEntry>,
    by_id: std::collections::BTreeMap<String, usize>,
    by_wm_class: std::collections::BTreeMap<String, usize>,
    by_stem: std::collections::BTreeMap<String, usize>,
    by_exec: std::collections::BTreeMap<String, usize>,
}

pub mod catalog {
    pub use super::{
        ApplicationCatalog, DesktopEntry, WindowApplicationIdentity, xdg_application_dirs,
    };
}

pub mod exec {
    pub use super::{expand_exec, tokenize_exec};
}

pub mod launcher {
    pub use super::launch_argv;
}

/// Display label for a desktop launcher. Unlike catalog eligibility, the visible
/// `Name` is independent of Exec/TryExec and OnlyShowIn. A shortcut whose
/// target is not currently runnable still has a meaningful name on the desktop.
/// Localized translations fall back to the unqualified Name key.
#[must_use]
pub fn desktop_file_display_name(path: &Path) -> Option<String> {
    let source = fs::read_to_string(path).ok()?;
    let mut in_entry = false;
    let mut labels = std::collections::BTreeMap::new();
    for line in source
        .lines()
        .map(|line| line.trim().trim_end_matches('\r'))
    {
        if line.starts_with('[') && line.ends_with(']') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key == "Name" || key.starts_with("Name[") && key.ends_with(']') {
                if !value.trim().is_empty() {
                    labels.insert(key.to_owned(), value.trim().to_owned());
                }
            }
        }
    }
    // XDG locale selection: LC_MESSAGES, then LANG; @modifier and territory
    // variants degrade to the language before the unconditional Name.
    let locale = std::env::var("LC_MESSAGES")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("LANG").ok())
        .unwrap_or_default();
    let locale = locale.split('.').next().unwrap_or("");
    let locale_no_modifier = locale.split('@').next().unwrap_or(locale);
    let language = locale_no_modifier
        .split('_')
        .next()
        .unwrap_or(locale_no_modifier);
    [locale, locale_no_modifier, language]
        .into_iter()
        .filter(|locale| !locale.is_empty())
        .map(|locale| format!("Name[{locale}]"))
        .find_map(|key| labels.get(&key).cloned())
        .or_else(|| labels.get("Name").cloned())
}

impl DesktopEntry {
    pub fn from_file(path: &Path) -> FlameResult<Option<Self>> {
        let file_name = path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .ok_or_else(|| FlameError::invalid("desktop entry path has no file name"))?;
        Self::from_file_with_id(path, DesktopAppId::new(file_name))
    }

    pub fn from_file_with_id(path: &Path, id: DesktopAppId) -> FlameResult<Option<Self>> {
        parse_desktop_entry(path, id, false)
    }

    #[must_use]
    pub fn id(&self) -> &DesktopAppId {
        &self.id
    }

    #[must_use]
    pub fn application(&self) -> &DesktopApplication {
        &self.application
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.application.name
    }

    #[must_use]
    pub fn icon(&self) -> &str {
        &self.application.icon_name
    }

    #[must_use]
    pub fn exec(&self) -> &str {
        &self.exec
    }

    pub fn argv(&self, options: &ApplicationLaunchOptions) -> FlameResult<Vec<String>> {
        expand_exec_with_path(&self.exec, &self.application, &self.path, options)
    }

    pub fn launch(&self, options: &ApplicationLaunchOptions) -> FlameResult<()> {
        let argv = self.argv(options)?;
        launch_argv_for_app(&argv, &self.application, self.startup_notify)
    }
}

impl ApplicationCatalog {
    pub fn discover() -> FlameResult<Self> {
        Self::from_dirs(&xdg_application_dirs())
    }

    /// MIME choosers must also see NoDisplay and other-desktop handlers.
    /// This does not expose those entries in Start or the application menu.
    pub fn discover_file_handlers() -> FlameResult<Self> {
        Self::from_dirs_with_visibility(&xdg_application_dirs(), true)
    }

    pub fn from_dirs(dirs: &[PathBuf]) -> FlameResult<Self> {
        Self::from_dirs_with_visibility(dirs, false)
    }

    fn from_dirs_with_visibility(dirs: &[PathBuf], handlers: bool) -> FlameResult<Self> {
        let mut entries = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            let mut files = Vec::new();
            collect_desktop_files(dir, dir, &mut files)?;
            files.sort();
            for path in files {
                let id = desktop_id(dir, &path);
                if !seen.insert(id.clone()) {
                    continue;
                }
                if let Some(entry) = parse_desktop_entry(&path, id, handlers)? {
                    entries.push(entry);
                }
            }
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        let mut catalog = Self {
            applications: entries,
            ..Self::default()
        };
        catalog.rebuild_indexes();
        Ok(catalog)
    }

    fn rebuild_indexes(&mut self) {
        self.by_id.clear();
        self.by_wm_class.clear();
        self.by_stem.clear();
        self.by_exec.clear();
        for (index, entry) in self.applications.iter().enumerate() {
            self.by_id
                .entry(entry.id.as_str().to_owned())
                .or_insert(index);
            let wm_class = entry.application.startup_wm_class.trim();
            if !wm_class.is_empty() {
                self.by_wm_class
                    .entry(wm_class.to_ascii_lowercase())
                    .or_insert(index);
            }
            for stem in id_stems(&entry.id) {
                self.by_stem
                    .entry(stem.to_ascii_lowercase())
                    .or_insert(index);
            }
            if let Some(base) = exec_basename(&entry.exec) {
                self.by_exec
                    .entry(base.to_ascii_lowercase())
                    .or_insert(index);
            }
        }
    }

    #[must_use]
    pub fn all(&self) -> Vec<DesktopApplication> {
        self.applications
            .iter()
            .map(|entry| entry.application.clone())
            .collect()
    }

    #[must_use]
    pub fn entries(&self) -> &[DesktopEntry] {
        &self.applications
    }

    pub fn find(&self, id: &DesktopAppId) -> FlameResult<&DesktopEntry> {
        self.by_id
            .get(id.as_str())
            .and_then(|index| self.applications.get(*index))
            .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "application id not found"))
    }

    pub fn find_by_window_identity(
        &self,
        identity: &WindowApplicationIdentity,
    ) -> Option<&DesktopEntry> {
        let candidates = [
            identity.wm_instance.as_str(),
            identity.wm_class.as_str(),
            identity.executable_basename.as_str(),
        ]
        .map(|value| (!value.trim().is_empty()).then(|| value.to_ascii_lowercase()));
        // Priorities: StartupWMClass, then desktop-file id stem (e.g. "firefox" from
        // "org.mozilla.firefox.desktop"), then Exec basename; all exact, case-insensitive.
        for index_map in [&self.by_wm_class, &self.by_stem, &self.by_exec] {
            for needle in candidates.iter().flatten() {
                if let Some(index) = index_map.get(needle) {
                    return self.applications.get(*index);
                }
            }
        }
        None
    }

    #[must_use]
    pub fn search(&self, query: &str) -> Vec<DesktopApplication> {
        let needle = query.trim().to_lowercase();
        self.applications
            .iter()
            .filter(|entry| {
                needle.is_empty() || {
                    let app = &entry.application;
                    app.id.as_str().to_lowercase().contains(&needle)
                        || app.name.to_lowercase().contains(&needle)
                        || app.generic_name.to_lowercase().contains(&needle)
                        || app.comment.to_lowercase().contains(&needle)
                        || app
                            .keywords
                            .iter()
                            .any(|v| v.to_lowercase().contains(&needle))
                        || app
                            .categories
                            .iter()
                            .any(|v| v.to_lowercase().contains(&needle))
                }
            })
            .map(|entry| entry.application.clone())
            .collect()
    }

    pub fn launch(&self, id: &DesktopAppId, options: &ApplicationLaunchOptions) -> FlameResult<()> {
        let entry = self.find(id)?;
        let argv = expand_exec_with_path(&entry.exec, &entry.application, &entry.path, options)?;
        launch_argv_for_app(&argv, &entry.application, entry.startup_notify)
    }
}

#[must_use]
pub fn xdg_application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|path| path.join(".local/share")));
    if let Some(path) = data_home {
        dirs.push(path.join("applications"));
    }
    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    for path in data_dirs.split(':').filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path).join("applications");
        if !dirs.contains(&path) {
            dirs.push(path);
        }
    }
    dirs
}

pub fn tokenize_exec(exec: &str) -> FlameResult<Vec<String>> {
    let mut result = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut chars = exec.chars().peekable();
    while let Some(ch) = chars.next() {
        match quote {
            Some('"') => match ch {
                '"' => quote = None,
                '\\' => token.push(
                    chars
                        .next()
                        .ok_or_else(|| FlameError::invalid("unterminated Exec escape"))?,
                ),
                _ => token.push(ch),
            },
            Some('\'') => match ch {
                '\'' => quote = None,
                _ => token.push(ch),
            },
            None => match ch {
                '\'' | '"' => quote = Some(ch),
                '\\' => token.push(
                    chars
                        .next()
                        .ok_or_else(|| FlameError::invalid("unterminated Exec escape"))?,
                ),
                c if c.is_whitespace() => {
                    if !token.is_empty() {
                        result.push(std::mem::take(&mut token));
                    }
                }
                _ => token.push(ch),
            },
            _ => unreachable!(),
        }
    }
    if quote.is_some() {
        return Err(FlameError::invalid("unterminated Exec quote"));
    }
    if !token.is_empty() {
        result.push(token);
    }
    if result.is_empty() {
        return Err(FlameError::invalid("empty Exec"));
    }
    Ok(result)
}

pub fn expand_exec(
    exec: &str,
    app: &DesktopApplication,
    options: &ApplicationLaunchOptions,
) -> FlameResult<Vec<String>> {
    expand_exec_with_path(exec, app, Path::new(""), options)
}

fn expand_exec_with_path(
    exec: &str,
    app: &DesktopApplication,
    desktop_path: &Path,
    options: &ApplicationLaunchOptions,
) -> FlameResult<Vec<String>> {
    let tokens = tokenize_exec(exec)?;
    let mut argv = Vec::new();
    for token in tokens {
        let mut output = String::new();
        let mut chars = token.chars();
        while let Some(ch) = chars.next() {
            if ch != '%' {
                output.push(ch);
                continue;
            }
            match chars.next() {
                Some('%') => output.push('%'),
                Some('c') => output.push_str(&app.name),
                Some('k') => output.push_str(&desktop_path.to_string_lossy()),
                Some('i') => {
                    if !app.icon_name.is_empty() {
                        argv.push("--icon".to_owned());
                        argv.push(app.icon_name.clone());
                    }
                }
                Some('f' | 'u') => {
                    if let Some(value) = options.uris.first() {
                        output.push_str(value);
                    }
                }
                Some(code @ ('F' | 'U')) => {
                    if token != format!("%{code}") {
                        return Err(FlameError::invalid(
                            "list Exec field code must be standalone",
                        ));
                    }
                    argv.extend(options.uris.iter().cloned());
                }
                // Deprecated by the XDG Desktop Entry spec: must be ignored (removed).
                Some('d' | 'D' | 'n' | 'N' | 'v' | 'm') => {}
                Some(code) => {
                    return Err(FlameError::invalid(format!(
                        "unsupported Exec field code %{code}"
                    )));
                }
                None => return Err(FlameError::invalid("trailing Exec field marker")),
            }
        }
        if !output.is_empty() {
            argv.push(output);
        }
    }
    argv.extend(options.extra_args.iter().cloned());
    Ok(argv)
}

/// Select a desktop's native MIME opener once, without depending on FlameWM
/// pretending to be another desktop or modifying the user's MIME database.
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
const REPEAT_OPEN_WINDOW: std::time::Duration = std::time::Duration::from_millis(1000);
/// Most recent `open_target` request, for the repeat guard.
static LAST_OPEN: std::sync::Mutex<Option<(std::ffi::OsString, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// True when `target` equals the previous request and arrived within the window.
fn is_repeat_open(
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
        let mut last = LAST_OPEN.lock().unwrap_or_else(|poison| poison.into_inner());
        if is_repeat_open(last.as_ref().map(|(t, at)| (t.as_os_str(), *at)), target, now) {
            return Ok(());
        }
        *last = Some((target.to_os_string(), now));
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

fn short_command_output(program: &str, args: &[&std::ffi::OsStr]) -> Option<String> {
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
                std::thread::sleep(std::time::Duration::from_millis(2))
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

fn startup_identity_for_target(target: &std::ffi::OsStr) -> Option<(String, String, String)> {
    use std::sync::{Mutex, OnceLock};
    static DEFAULTS: OnceLock<Mutex<std::collections::BTreeMap<String, Option<String>>>> =
        OnceLock::new();
    static CATALOG: OnceLock<ApplicationCatalog> = OnceLock::new();
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
    let desktop_id = match cached {
        Some(id) => id,
        None => {
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
        }
    }?;
    let catalog = CATALOG.get_or_init(|| ApplicationCatalog::discover().unwrap_or_default());
    let entry = catalog.find(&DesktopAppId::new(desktop_id)).ok()?;
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

#[must_use]
fn is_brave_program(program: &str) -> bool {
    matches!(
        std::path::Path::new(program)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("brave-browser" | "brave-browser-stable" | "brave" | "brave-bin")
    )
}

pub fn launch_argv(argv: &[String], options: &ApplicationLaunchOptions) -> FlameResult<()> {
    let program = argv
        .first()
        .ok_or_else(|| FlameError::invalid("empty application argv"))?;
    let mut command = Command::new(program);
    configure_kde_theme(&mut command, program, None);
    // The regular FlameWM application launcher can start Brave directly,
    // without going through the generated .desktop compatibility override.
    // Give ONLY Brave the KDE wallet identity; making the whole session KDE
    // would activate competing KScreen/XRandR services.
    if is_brave_program(program)
        && std::env::var("XDG_CURRENT_DESKTOP")
            .is_ok_and(|desktop| desktop.split(':').any(|part| part == "FlameWM"))
    {
        command.env("XDG_CURRENT_DESKTOP", "KDE");
    }
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

fn launch_argv_for_app(
    argv: &[String],
    app: &DesktopApplication,
    startup_notify: Option<bool>,
) -> FlameResult<()> {
    let program = argv
        .first()
        .ok_or_else(|| FlameError::invalid("empty application argv"))?;
    let mut command = Command::new(program);
    configure_kde_theme(&mut command, program, Some(&app.id));
    if is_brave_program(program)
        && std::env::var("XDG_CURRENT_DESKTOP")
            .is_ok_and(|desktop| desktop.split(':').any(|part| part == "FlameWM"))
    {
        command.env("XDG_CURRENT_DESKTOP", "KDE");
    }
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

fn configure_kde_theme(command: &mut Command, program: &str, app_id: Option<&DesktopAppId>) {
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

fn is_dolphin_application(program: &str, app_id: Option<&DesktopAppId>) -> bool {
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

fn spawn_with_startup(
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
fn spawn_reaped_with_id(command: &mut Command, startup_id: Option<String>) -> FlameResult<u32> {
    let mut child = command.spawn().map_err(|error| {
        FlameError::new(ErrorCode::IoFailure, format!("launch application: {error}"))
    })?;
    let pid = child.id();
    // If the OS refuses the thread, the closure (and the child handle) is dropped
    // and this child stays unreaped. The application itself did launch.
    let _ = std::thread::Builder::new()
        .name(format!("flamewm-reap-{pid}"))
        .spawn(move || {
            if let Ok(status) = child.wait() {
                if !status.success() {
                    if let Some(id) = startup_id {
                        let _ = launch_feedback::broadcast(
                            launch_feedback::MessageKind::Remove,
                            &id,
                            "",
                            "",
                            "",
                        );
                    }
                }
            }
        });
    Ok(pid)
}

fn collect_desktop_files(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> FlameResult<()> {
    for item in fs::read_dir(dir).map_err(|error| {
        FlameError::new(
            ErrorCode::IoFailure,
            format!("read applications directory: {error}"),
        )
    })? {
        let path = item
            .map_err(|error| FlameError::new(ErrorCode::IoFailure, error.to_string()))?
            .path();
        if path.is_dir() {
            collect_desktop_files(root, &path, files)?;
        } else if path.extension().and_then(|v| v.to_str()) == Some("desktop") {
            files.push(path);
        }
    }
    Ok(())
}

fn desktop_id(root: &Path, path: &Path) -> DesktopAppId {
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('/', "-");
    DesktopAppId::new(rel)
}

fn parse_desktop_entry(
    path: &Path,
    id: DesktopAppId,
    handlers: bool,
) -> FlameResult<Option<DesktopEntry>> {
    let text = fs::read_to_string(path).map_err(|error| {
        FlameError::new(ErrorCode::IoFailure, format!("read desktop entry: {error}"))
    })?;
    let mut values = std::collections::BTreeMap::new();
    let mut section = false;
    for line in text.lines().map(|line| line.trim_end_matches('\r')) {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1] == "Desktop Entry";
            continue;
        }
        if !section || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            values.insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    if values.get("Type").map_or("Application", String::as_str) != "Application"
        || is_true(values.get("Hidden"))
        || (!handlers && is_true(values.get("NoDisplay")))
    {
        return Ok(None);
    }
    if (!handlers && !desktop_environment_matches(&values))
        || !try_exec_is_available(values.get("TryExec"))
    {
        return Ok(None);
    }
    let Some(exec) = values.get("Exec") else {
        return Ok(None);
    };
    let argv = tokenize_exec(exec)?;
    if values
        .get("Name")
        .is_none_or(|value| value.trim().is_empty())
    {
        return Ok(None);
    }
    let app = DesktopApplication {
        id: id.clone(),
        name: values.get("Name").cloned().unwrap_or_default(),
        generic_name: values.get("GenericName").cloned().unwrap_or_default(),
        comment: values.get("Comment").cloned().unwrap_or_default(),
        startup_wm_class: values.get("StartupWMClass").cloned().unwrap_or_default(),
        argv,
        keywords: list_value(values.get("Keywords")),
        icon_name: values.get("Icon").cloned().unwrap_or_default(),
        categories: list_value(values.get("Categories")),
    };
    Ok(Some(DesktopEntry {
        id,
        application: app,
        path: path.to_owned(),
        exec: exec.clone(),
        startup_notify: values
            .get("StartupNotify")
            .map(|value| is_true(Some(value))),
    }))
}

fn id_stems(id: &DesktopAppId) -> Vec<String> {
    let full = id.as_str().strip_suffix(".desktop").unwrap_or(id.as_str());
    let mut stems = vec![full.to_owned()];
    if let Some(last) = full.rsplit('.').next() {
        if last != full {
            stems.push(last.to_owned());
        }
    }
    stems
}

fn exec_basename(exec: &str) -> Option<String> {
    let program = tokenize_exec(exec).ok()?.into_iter().next()?;
    let trimmed = program.trim().trim_matches(|c| c == '"' || c == '\'');
    let file = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let base = file.trim();
    if base.is_empty() {
        return None;
    }
    Some(base.to_owned())
}

fn is_true(value: Option<&String>) -> bool {
    value.is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
}
fn list_value(value: Option<&String>) -> Vec<String> {
    value
        .map(|v| {
            v.split(';')
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn desktop_environment_matches(values: &std::collections::BTreeMap<String, String>) -> bool {
    let current = std::env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .or_else(|| std::env::var("DESKTOP_SESSION").ok());
    let Some(current) = current else {
        return true;
    };
    let names: Vec<&str> = current
        .split(':')
        .filter(|value| !value.is_empty())
        .collect();
    let only = list_value(values.get("OnlyShowIn"));
    let not = list_value(values.get("NotShowIn"));
    (only.is_empty()
        || only
            .iter()
            .any(|value| names.iter().any(|name| name.eq_ignore_ascii_case(value))))
        && !not
            .iter()
            .any(|value| names.iter().any(|name| name.eq_ignore_ascii_case(value)))
}

fn try_exec_is_available(value: Option<&String>) -> bool {
    let Some(program) = value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return true;
    };
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file();
    }
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    paths
        .as_os_str()
        .to_string_lossy()
        .split(':')
        .filter(|directory| !directory.is_empty())
        .map(|directory| Path::new(directory).join(program))
        .any(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "flamewm-applications-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(path.join("applications/nested")).unwrap();
        fs::write(path.join("applications/nested/example.desktop"), "[Desktop Entry]\nType=Application\nName=Example Tool\nGenericName=Editor\nComment=Fixture\nKeywords=write;fixture;\nCategories=Utility;\nIcon=example\nExec=fixture %U --literal=DISPLAY=:9\n").unwrap();
        fs::write(
            path.join("applications/nested/hidden.desktop"),
            "[Desktop Entry]\nType=Application\nName=Hidden\nHidden=true\nExec=hidden\n",
        )
        .unwrap();
        path
    }

    #[test]
    fn catalog_precedence_search_and_desktop_id() {
        let root = fixture();
        let catalog = ApplicationCatalog::from_dirs(&[root.join("applications")]).unwrap();
        assert_eq!(catalog.entries().len(), 1);
        assert_eq!(catalog.entries()[0].id.as_str(), "nested-example.desktop");
        assert_eq!(catalog.search("fixture")[0].name, "Example Tool");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn strict_exec_argv_keeps_nested_display_literal() {
        let app = DesktopApplication {
            id: DesktopAppId::new("example.desktop"),
            name: "Example".into(),
            generic_name: String::new(),
            comment: String::new(),
            startup_wm_class: String::new(),
            argv: Vec::new(),
            keywords: Vec::new(),
            icon_name: "icon".into(),
            categories: Vec::new(),
        };
        let options = ApplicationLaunchOptions {
            extra_args: vec!["--extra".into()],
            uris: vec!["file:///tmp/a b".into()],
        };
        assert_eq!(
            expand_exec("program %U --literal=DISPLAY=:9", &app, &options).unwrap(),
            vec![
                "program",
                "file:///tmp/a b",
                "--literal=DISPLAY=:9",
                "--extra"
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn spawned_application_is_reaped_after_exit() {
        let pid = spawn_reaped_with_id(&mut Command::new("true"), None).unwrap();
        let proc_entry = PathBuf::from(format!("/proc/{pid}"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        // A zombie keeps /proc/<pid> alive; reaping removes it.
        while proc_entry.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "child {pid} was not reaped"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn app_with_icon(icon: &str) -> DesktopApplication {
        DesktopApplication {
            id: DesktopAppId::new("example.desktop"),
            name: "Example".into(),
            generic_name: String::new(),
            comment: String::new(),
            startup_wm_class: String::new(),
            argv: Vec::new(),
            keywords: Vec::new(),
            icon_name: icon.into(),
            categories: Vec::new(),
        }
    }

    #[test]
    fn expand_exec_ignores_deprecated_field_codes() {
        let options = ApplicationLaunchOptions {
            extra_args: Vec::new(),
            uris: vec!["file:///tmp/a".into()],
        };
        assert_eq!(
            expand_exec("prog %d %U", &app_with_icon(""), &options).unwrap(),
            vec!["prog", "file:///tmp/a"]
        );
        assert_eq!(
            expand_exec(
                "prog %d %D %n %N %v %m",
                &app_with_icon(""),
                &ApplicationLaunchOptions::default()
            )
            .unwrap(),
            vec!["prog"]
        );
    }

    #[test]
    fn expand_exec_standalone_codes_without_uris_and_percent_literal() {
        let options = ApplicationLaunchOptions::default();
        // Empty icon: %i contributes nothing; %F and %u contribute nothing without uris.
        assert_eq!(
            expand_exec("prog %F %u %i %%", &app_with_icon(""), &options).unwrap(),
            vec!["prog", "%"]
        );
        // Non-empty icon: %i pushes the --icon pair before the %% literal.
        assert_eq!(
            expand_exec("prog %F %u %i %%", &app_with_icon("example"), &options).unwrap(),
            vec!["prog", "--icon", "example", "%"]
        );
    }

    fn write_entry(root: &Path, name: &str, body: &str) -> PathBuf {
        let path = root.join(name);
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn from_file_exposes_name_icon_exec() {
        let root = fixture();
        let path = write_entry(
            &root,
            "entry.desktop",
            "[Desktop Entry]\nType=Application\nName=Entry Name\nIcon=entry-icon\nExec=myapp --flag\n",
        );
        let entry = DesktopEntry::from_file(&path).unwrap().unwrap();
        assert_eq!(entry.name(), "Entry Name");
        assert_eq!(entry.icon(), "entry-icon");
        assert_eq!(entry.exec(), "myapp --flag");
        assert_eq!(entry.application().name, "Entry Name");
        assert_eq!(entry.path(), path.as_path());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn from_file_absolute_icon_path() {
        let root = fixture();
        let path = write_entry(
            &root,
            "absolute.desktop",
            "[Desktop Entry]\nType=Application\nName=Absolute\nIcon=/usr/share/icons/absolute.png\nExec=myapp\n",
        );
        let entry = DesktopEntry::from_file(&path).unwrap().unwrap();
        assert_eq!(entry.icon(), "/usr/share/icons/absolute.png");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn from_file_icon_name() {
        let root = fixture();
        let path = write_entry(
            &root,
            "named.desktop",
            "[Desktop Entry]\nType=Application\nName=Named\nIcon=text-editor\nExec=myapp\n",
        );
        let entry = DesktopEntry::from_file(&path).unwrap().unwrap();
        assert_eq!(entry.icon(), "text-editor");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn from_file_expands_field_codes() {
        let root = fixture();
        let path = write_entry(
            &root,
            "codes.desktop",
            "[Desktop Entry]\nType=Application\nName=Field Codes\nIcon=icon\nExec=myapp %c %k %f\n",
        );
        let entry = DesktopEntry::from_file(&path).unwrap().unwrap();
        let options = ApplicationLaunchOptions {
            extra_args: Vec::new(),
            uris: vec!["file:///tmp/input".into()],
        };
        let argv = entry.argv(&options).unwrap();
        let desktop = path.to_string_lossy().into_owned();
        assert_eq!(
            argv,
            vec![
                "myapp".to_owned(),
                "Field Codes".to_owned(),
                desktop,
                "file:///tmp/input".to_owned(),
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn from_file_missing_exec_is_none() {
        let root = fixture();
        let path = write_entry(
            &root,
            "noexec.desktop",
            "[Desktop Entry]\nType=Application\nName=No Exec\nIcon=icon\n",
        );
        assert!(DesktopEntry::from_file(&path).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn from_file_hidden_is_none() {
        let root = fixture();
        let path = write_entry(
            &root,
            "hidden-entry.desktop",
            "[Desktop Entry]\nType=Application\nName=Hidden Entry\nHidden=true\nExec=myapp\n",
        );
        assert!(DesktopEntry::from_file(&path).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    fn identity_catalog() -> ApplicationCatalog {
        fn entry(id: &str, name: &str, wm_class: &str, exec: &str) -> DesktopEntry {
            DesktopEntry {
                id: DesktopAppId::new(id),
                application: DesktopApplication {
                    id: DesktopAppId::new(id),
                    name: name.into(),
                    generic_name: String::new(),
                    comment: String::new(),
                    startup_wm_class: wm_class.into(),
                    argv: Vec::new(),
                    keywords: Vec::new(),
                    icon_name: String::new(),
                    categories: Vec::new(),
                },
                path: PathBuf::from(format!("/tmp/{id}")),
                exec: exec.into(),
                startup_notify: None,
            }
        }
        let mut catalog = ApplicationCatalog {
            applications: vec![
                entry(
                    "org.kde.dolphin.desktop",
                    "Dolphin",
                    "dolphin",
                    "dolphin %U",
                ),
                entry(
                    "firefox.desktop",
                    "Firefox",
                    "firefox",
                    "/usr/lib/firefox/firefox %u",
                ),
                entry(
                    "org.prismlauncher.PrismLauncher.desktop",
                    "Prism Launcher",
                    "PrismLauncher",
                    "prismlauncher",
                ),
                entry("myeditor.desktop", "MyEditor", "", "myeditor"),
                entry("myeditor-pro.desktop", "MyEditor Pro", "", "myeditor-pro"),
            ],
            by_id: std::collections::BTreeMap::new(),
            by_wm_class: std::collections::BTreeMap::new(),
            by_stem: std::collections::BTreeMap::new(),
            by_exec: std::collections::BTreeMap::new(),
        };
        catalog.rebuild_indexes();
        catalog
    }

    #[test]
    fn dolphin_launch_sets_the_kde_platform_theme_and_other_apps_stay_untouched() {
        let mut dolphin = Command::new("dolphin");
        configure_kde_theme(&mut dolphin, "dolphin", None);
        let envs: std::collections::BTreeMap<_, _> = dolphin
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(
            envs.get("XDG_CURRENT_DESKTOP").and_then(Option::as_deref),
            Some("KDE")
        );
        assert_eq!(
            envs.get("KDE_SESSION_VERSION").and_then(Option::as_deref),
            Some("6")
        );
        assert_eq!(
            envs.get("QT_QPA_PLATFORMTHEME").and_then(Option::as_deref),
            Some("KDE")
        );
        assert!(envs.contains_key("KDE_FULL_SESSION"));

        let mut konsole = Command::new("konsole");
        configure_kde_theme(
            &mut konsole,
            "/usr/bin/konsole",
            Some(&DesktopAppId::new("konsole.desktop")),
        );
        assert_eq!(konsole.get_envs().count(), 0);
    }

    #[test]
    fn identity_matches_dolphin_by_startup_wm_class() {
        let catalog = identity_catalog();
        let found = catalog
            .find_by_window_identity(&WindowApplicationIdentity::new(
                "dolphin", "Dolphin", "dolphin",
            ))
            .unwrap();
        assert_eq!(found.id.as_str(), "org.kde.dolphin.desktop");
        assert!(is_dolphin_application("/usr/bin/dolphin", None));
        assert!(is_dolphin_application(
            "/usr/libexec/other",
            Some(&found.id)
        ));
        assert!(!is_dolphin_application(
            "/usr/bin/konsole",
            Some(&DesktopAppId::new("konsole.desktop"))
        ));
        assert_eq!(found.application.id, found.id);
        assert_eq!(
            catalog
                .all()
                .into_iter()
                .find(|application| application.name == "Dolphin")
                .unwrap()
                .id,
            found.id
        );
    }

    #[test]
    fn identity_matches_firefox_by_exec_basename() {
        let catalog = identity_catalog();
        let found = catalog
            .find_by_window_identity(&WindowApplicationIdentity::new("", "", "firefox"))
            .unwrap();
        assert_eq!(found.id.as_str(), "firefox.desktop");
    }

    #[test]
    fn identity_matches_prism_launcher_startup_wm_class() {
        let catalog = identity_catalog();
        let found = catalog
            .find_by_window_identity(&WindowApplicationIdentity::new(
                "PrismLauncher",
                "prismlauncher",
                "prismlauncher",
            ))
            .unwrap();
        assert_eq!(found.id.as_str(), "org.prismlauncher.PrismLauncher.desktop");
    }

    #[test]
    fn identity_matches_class_and_desktop_file_stem() {
        let catalog = identity_catalog();
        let by_class = catalog
            .find_by_window_identity(&WindowApplicationIdentity::new("", "DOLPHIN", ""))
            .unwrap();
        let by_stem = catalog
            .find_by_window_identity(&WindowApplicationIdentity::new("", "", "org.kde.dolphin"))
            .unwrap();
        assert_eq!(by_class.id.as_str(), "org.kde.dolphin.desktop");
        assert_eq!(by_stem.id.as_str(), "org.kde.dolphin.desktop");
        assert_eq!(by_stem.application.id, by_stem.id);
    }

    #[test]
    fn identity_matches_wm_class_instance() {
        let catalog = identity_catalog();
        let found = catalog
            .find_by_window_identity(&WindowApplicationIdentity::new("PrismLauncher", "", ""))
            .unwrap();
        assert_eq!(found.id.as_str(), "org.prismlauncher.PrismLauncher.desktop");
        assert_eq!(found.application.id, found.id);
    }

    #[test]
    fn identity_prefers_startup_wm_class_over_id_stem() {
        let catalog = identity_catalog();
        // wm_class "dolphin" must not fuzzy-match another nearby id stem.
        let found = catalog
            .find_by_window_identity(&WindowApplicationIdentity::new("", "DOLPHIN", ""))
            .unwrap();
        assert_eq!(found.id.as_str(), "org.kde.dolphin.desktop");
    }

    #[test]
    fn identity_near_collision_names_stay_exact() {
        let catalog = identity_catalog();
        let plain = catalog
            .find_by_window_identity(&WindowApplicationIdentity::new("", "", "myeditor"))
            .unwrap();
        let pro = catalog
            .find_by_window_identity(&WindowApplicationIdentity::new("", "", "myeditor-pro"))
            .unwrap();
        assert_eq!(plain.id.as_str(), "myeditor.desktop");
        assert_eq!(pro.id.as_str(), "myeditor-pro.desktop");
        // Substring must not match: "myedit" is not an exact stem/basename.
        assert!(
            catalog
                .find_by_window_identity(&WindowApplicationIdentity::new("", "", "myedit"))
                .is_none()
        );
        assert!(
            catalog
                .find_by_window_identity(&WindowApplicationIdentity::new("", "", ""))
                .is_none()
        );
    }
}

#[cfg(test)]
mod desktop_label_metadata_tests {
    use super::*;
    #[test]
    fn desktop_name_survives_an_unlaunchable_kde_only_shortcut() {
        let file = std::env::temp_dir().join(format!(
            "flamewm-name-{}-{}.desktop",
            std::process::id(),
            "kde"
        ));
        std::fs::write(&file, "[Desktop Entry]\nType=Application\nOnlyShowIn=KDE;\nName=Dolphin File Manager\nExec=does-not-exist\n").unwrap();
        assert_eq!(
            desktop_file_display_name(&file).as_deref(),
            Some("Dolphin File Manager")
        );
        std::fs::remove_file(file).unwrap();
    }
    #[test]
    fn ignores_desktop_actions_and_non_desktop_entry_names() {
        let file = std::env::temp_dir().join(format!(
            "flamewm-name-{}-{}.desktop",
            std::process::id(),
            "sections"
        ));
        std::fs::write(&file,"[Other]\nName=Wrong\n[Desktop Entry]\nName=Correct\n[Desktop Action More]\nName=Not the icon name\n").unwrap();
        assert_eq!(desktop_file_display_name(&file).as_deref(), Some("Correct"));
        std::fs::remove_file(file).unwrap();
    }
}

#[cfg(test)]
mod repeat_open_guard_tests {
    use super::*;
    use std::ffi::OsStr;
    use std::time::{Duration, Instant};

    #[test]
    fn identical_target_inside_window_is_a_repeat() {
        let start = Instant::now();
        let folder = OsStr::new("/home/u/Documents");
        let soon = start + Duration::from_millis(200);
        assert!(is_repeat_open(Some((folder, start)), folder, soon));
    }

    #[test]
    fn identical_target_after_window_or_different_target_is_allowed() {
        let start = Instant::now();
        let folder = OsStr::new("/home/u/Documents");
        let later = start + REPEAT_OPEN_WINDOW;
        assert!(!is_repeat_open(Some((folder, start)), folder, later));
        assert!(!is_repeat_open(
            Some((folder, start)),
            OsStr::new("/home/u/Music"),
            start
        ));
        assert!(!is_repeat_open(None, folder, start));
    }
}

#[cfg(test)]
mod native_mime_open_tests {
    use super::*;

    #[test]
    fn brave_launcher_detection_excludes_unrelated_browsers() {
        assert!(is_brave_program("/usr/bin/brave-browser-stable"));
        assert!(is_brave_program("brave-browser"));
        assert!(!is_brave_program("/usr/bin/chromium"));
        assert!(!is_brave_program("/opt/not-brave-browser"));
    }

    #[test]
    fn kde_default_provider_wins_without_overriding_mimeapps() {
        assert_eq!(
            preferred_file_opener(|name| name == "kioclient6").0,
            "kioclient6"
        );
        assert_eq!(
            preferred_file_opener(|name| name == "kioclient6").1,
            &["exec"]
        );
        assert_eq!(
            preferred_file_opener(|name| name == "kde-open6").0,
            "kde-open6"
        );
        assert_eq!(preferred_file_opener(|_| false).0, "xdg-open");
    }
}

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

impl DesktopEntry {
    pub fn from_file(path: &Path) -> FlameResult<Option<Self>> {
        let file_name = path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .ok_or_else(|| FlameError::invalid("desktop entry path has no file name"))?;
        Self::from_file_with_id(path, DesktopAppId::new(file_name))
    }

    pub fn from_file_with_id(path: &Path, id: DesktopAppId) -> FlameResult<Option<Self>> {
        parse_desktop_entry(path, id)
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
        launch_argv(&argv, &ApplicationLaunchOptions::default())
    }
}

impl ApplicationCatalog {
    pub fn discover() -> FlameResult<Self> {
        Self::from_dirs(&xdg_application_dirs())
    }

    pub fn from_dirs(dirs: &[PathBuf]) -> FlameResult<Self> {
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
                if let Some(entry) = parse_desktop_entry(&path, id)? {
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
        let candidates: Vec<String> = [
            identity.wm_instance.as_str(),
            identity.wm_class.as_str(),
            identity.executable_basename.as_str(),
        ]
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_ascii_lowercase())
        .collect();
        if candidates.is_empty() {
            return None;
        }
        // Priorities: StartupWMClass, then desktop-file id stem (e.g. "firefox" from
        // "org.mozilla.firefox.desktop"), then Exec basename; all exact, case-insensitive.
        for index_map in [&self.by_wm_class, &self.by_stem, &self.by_exec] {
            for needle in &candidates {
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
        launch_argv(&argv, &ApplicationLaunchOptions::default())
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

pub fn launch_argv(argv: &[String], options: &ApplicationLaunchOptions) -> FlameResult<()> {
    let program = argv
        .first()
        .ok_or_else(|| FlameError::invalid("empty application argv"))?;
    let mut command = Command::new(program);
    command
        .args(&argv[1..])
        .args(&options.extra_args)
        .args(&options.uris);
    spawn_reaped(&mut command).map(|_| ())
}

/// Spawns `command` and gives the child to a detached waiter thread that calls
/// `wait()`, so the child is reaped when it exits. Returns the child pid.
///
/// Why this shape: every caller drops the launched handle, and dropping a
/// `Child` does not reap it, so the WM would accumulate zombies. Returning the
/// handle does not help because callers discard it. `SIGCHLD = SIG_IGN` would
/// reap globally and disturb the WM's own children. One blocked-in-`waitpid`
/// thread per launched application exits as soon as that application exits.
fn spawn_reaped(command: &mut Command) -> FlameResult<u32> {
    let mut child = command.spawn().map_err(|error| {
        FlameError::new(ErrorCode::IoFailure, format!("launch application: {error}"))
    })?;
    let pid = child.id();
    // If the OS refuses the thread, the closure (and the child handle) is dropped
    // and this child stays unreaped. The application itself did launch.
    let _ = std::thread::Builder::new()
        .name(format!("flamewm-reap-{pid}"))
        .spawn(move || {
            let _ = child.wait();
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

fn parse_desktop_entry(path: &Path, id: DesktopAppId) -> FlameResult<Option<DesktopEntry>> {
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
    if values
        .get("Type")
        .map(String::as_str)
        .unwrap_or("Application")
        != "Application"
        || is_true(values.get("Hidden"))
        || is_true(values.get("NoDisplay"))
    {
        return Ok(None);
    }
    if !desktop_environment_matches(&values) || !try_exec_is_available(values.get("TryExec")) {
        return Ok(None);
    }
    let exec = match values.get("Exec") {
        Some(value) => value,
        None => return Ok(None),
    };
    let argv = tokenize_exec(exec)?;
    if values
        .get("Name")
        .map(|value| value.trim())
        .unwrap_or("")
        .is_empty()
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
    value
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
        .unwrap_or(false)
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
        let pid = spawn_reaped(&mut Command::new("true")).unwrap();
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
    fn identity_matches_dolphin_by_startup_wm_class() {
        let catalog = identity_catalog();
        let found = catalog
            .find_by_window_identity(&WindowApplicationIdentity::new(
                "dolphin", "Dolphin", "dolphin",
            ))
            .unwrap();
        assert_eq!(found.id.as_str(), "org.kde.dolphin.desktop");
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

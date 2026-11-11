use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    pub id: DesktopAppId,
    pub application: DesktopApplication,
    pub path: PathBuf,
    pub(super) exec: String,
    pub(super) startup_notify: Option<bool>,
    /// `Terminal=true`: the argv runs inside the resolved terminal.
    pub(super) terminal: bool,
}

/// Display label for a desktop launcher. Unlike catalog eligibility, the visible
/// `Name` is independent of Exec/TryExec and `OnlyShowIn`. A shortcut whose
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
        if let Some((key, value)) = line.split_once('=')
            && (key == "Name" || key.starts_with("Name[") && key.ends_with(']'))
            && !value.trim().is_empty()
        {
            labels.insert(key.to_owned(), value.trim().to_owned());
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
        let argv = expand_exec_with_path(&self.exec, &self.application, &self.path, options)?;
        if self.terminal {
            terminal::wrap_terminal_entry(&argv)
        } else {
            Ok(argv)
        }
    }

    pub fn launch(&self, options: &ApplicationLaunchOptions) -> FlameResult<()> {
        let argv = self.argv(options)?;
        launch_argv_for_app(&argv, &self.application, self.startup_notify)
    }
}

pub(super) fn collect_desktop_files(dir: &Path, files: &mut Vec<PathBuf>) -> FlameResult<()> {
    for item in fs::read_dir(dir).map_err(|error| {
        FlameError::new(
            ErrorCode::IoFailure,
            format!("read applications directory: {error}"),
        )
    })? {
        let item = match item {
            Ok(item) => item,
            Err(error) => {
                eprintln!("skip applications entry in {}: {error}", dir.display());
                continue;
            }
        };
        let path = item.path();
        let is_dir = match item.file_type() {
            Ok(kind) if !kind.is_symlink() => kind.is_dir(),
            _ => path.is_dir(),
        };
        if is_dir {
            collect_desktop_files(&path, files)?;
        } else if path.extension().and_then(|v| v.to_str()) == Some("desktop") {
            files.push(path);
        }
    }
    Ok(())
}

pub(super) fn desktop_id(root: &Path, path: &Path) -> DesktopAppId {
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('/', "-");
    DesktopAppId::new(rel)
}

pub(super) fn parse_desktop_entry(
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
        terminal: is_true(values.get("Terminal")),
        startup_notify: values
            .get("StartupNotify")
            .map(|value| is_true(Some(value))),
    }))
}

pub(super) fn id_stems(id: &DesktopAppId) -> Vec<String> {
    let full = id.as_str().strip_suffix(".desktop").unwrap_or(id.as_str());
    let mut stems = vec![full.to_owned()];
    if let Some(last) = full.rsplit('.').next()
        && last != full
    {
        stems.push(last.to_owned());
    }
    stems
}

pub(super) fn exec_basename(exec: &str) -> Option<String> {
    let program = tokenize_exec(exec).ok()?.into_iter().next()?;
    let trimmed = program.trim().trim_matches(|c| c == '"' || c == '\'');
    let file = trimmed.rsplit('/').next().unwrap_or(trimmed);
    let base = file.trim();
    if base.is_empty() {
        return None;
    }
    Some(base.to_owned())
}

pub(super) fn is_true(value: Option<&String>) -> bool {
    value.is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
}

pub(super) fn list_value(value: Option<&String>) -> Vec<String> {
    value
        .map(|v| {
            v.split(';')
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn desktop_environment_matches(
    values: &std::collections::BTreeMap<String, String>,
) -> bool {
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

pub(super) fn try_exec_is_available(value: Option<&String>) -> bool {
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

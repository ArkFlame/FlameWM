use super::*;

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
    pub(super) applications: Vec<DesktopEntry>,
    pub(super) by_id: std::collections::BTreeMap<String, usize>,
    pub(super) by_wm_class: std::collections::BTreeMap<String, usize>,
    pub(super) by_stem: std::collections::BTreeMap<String, usize>,
    pub(super) by_exec: std::collections::BTreeMap<String, usize>,
}

impl ApplicationCatalog {
    pub fn discover() -> FlameResult<Self> {
        Self::from_dirs(&xdg_application_dirs())
    }

    /// MIME choosers must also see `NoDisplay` and other-desktop handlers.
    /// This does not expose those entries in Start or the application menu.
    pub fn discover_file_handlers() -> FlameResult<Self> {
        Self::from_dirs_with_visibility(&xdg_application_dirs(), true)
    }

    pub fn from_dirs(dirs: &[PathBuf]) -> FlameResult<Self> {
        Self::from_dirs_with_visibility(dirs, false)
    }

    pub(super) fn from_dirs_with_visibility(dirs: &[PathBuf], handlers: bool) -> FlameResult<Self> {
        let mut entries = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            let mut files = Vec::new();
            collect_desktop_files(dir, &mut files)?;
            files.sort();
            for path in files {
                let id = desktop_id(dir, &path);
                if !seen.insert(id.clone()) {
                    continue;
                }
                // One unreadable or malformed launcher must not abort discovery.
                match parse_desktop_entry(&path, id, handlers) {
                    Ok(Some(entry)) => entries.push(entry),
                    Ok(None) => {}
                    Err(error) => eprintln!("skip desktop entry {}: {error}", path.display()),
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

    pub(super) fn rebuild_indexes(&mut self) {
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

    #[must_use]
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
        let argv = entry.argv(options)?;
        launch_argv_for_app(&argv, &entry.application, entry.startup_notify)
    }
}

#[must_use]
pub fn xdg_application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let data_home = flamewm_api::paths::xdg_dir("XDG_DATA_HOME", ".local/share");
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

/// Entries that differ between two catalog snapshots, keyed by desktop id.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CatalogDelta {
    pub added: Vec<DesktopAppId>,
    pub removed: Vec<DesktopAppId>,
    pub changed: Vec<DesktopAppId>,
}

impl CatalogDelta {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

impl ApplicationCatalog {
    /// Entries of `next` compared against `self` by desktop id and full entry value.
    #[must_use]
    pub fn delta_to(&self, next: &Self) -> CatalogDelta {
        let before: BTreeMap<&str, &DesktopEntry> = self
            .applications
            .iter()
            .map(|entry| (entry.id.as_str(), entry))
            .collect();
        let after: BTreeMap<&str, &DesktopEntry> = next
            .applications
            .iter()
            .map(|entry| (entry.id.as_str(), entry))
            .collect();
        let mut delta = CatalogDelta::default();
        for (id, entry) in &after {
            match before.get(id) {
                None => delta.added.push(entry.id.clone()),
                Some(old) if old != entry => delta.changed.push(entry.id.clone()),
                Some(_) => {}
            }
        }
        for (id, entry) in &before {
            if !after.contains_key(id) {
                delta.removed.push(entry.id.clone());
            }
        }
        delta
    }
}

/// Application catalog that can be re-read at runtime; readers take cheap `Arc` snapshots.
#[derive(Debug)]
pub struct SharedApplicationCatalog {
    pub(super) current: std::sync::RwLock<std::sync::Arc<ApplicationCatalog>>,
    /// Application-dir mtimes taken before the last read; `None` until one is recorded.
    pub(super) stamps: std::sync::Mutex<Option<Vec<Option<std::time::SystemTime>>>>,
}

/// Modification time of each dir in `dirs`; `None` when the dir is absent.
/// A dir's mtime changes when entries are added, removed or renamed in it.
fn dir_stamps(dirs: &[PathBuf]) -> Vec<Option<std::time::SystemTime>> {
    dirs.iter()
        .map(|dir| std::fs::metadata(dir).and_then(|meta| meta.modified()).ok())
        .collect()
}

impl SharedApplicationCatalog {
    #[must_use]
    pub fn new(catalog: ApplicationCatalog) -> Self {
        Self {
            current: std::sync::RwLock::new(std::sync::Arc::new(catalog)),
            stamps: std::sync::Mutex::new(None),
        }
    }

    #[must_use]
    pub fn current(&self) -> std::sync::Arc<ApplicationCatalog> {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Re-reads `dirs`, swaps the snapshot in, and reports what changed.
    pub fn rescan_from_dirs(&self, dirs: &[PathBuf]) -> FlameResult<CatalogDelta> {
        // Stamps are taken before the read: a change during the read forces the next refresh.
        let stamps = dir_stamps(dirs);
        let next = ApplicationCatalog::from_dirs(dirs)?;
        let mut guard = self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let delta = guard.delta_to(&next);
        *guard = std::sync::Arc::new(next);
        *self
            .stamps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(stamps);
        Ok(delta)
    }

    /// Rescans only when an application dir mtime changed since the last read.
    /// Unchanged dirs return an empty delta without reading any `.desktop` file.
    pub fn refresh_if_dirs_changed(&self, dirs: &[PathBuf]) -> FlameResult<CatalogDelta> {
        let stamps = dir_stamps(dirs);
        let seen = self
            .stamps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if seen.as_ref() == Some(&stamps) {
            return Ok(CatalogDelta::default());
        }
        self.rescan_from_dirs(dirs)
    }
}

/// Process-wide catalog shared by launch paths; refreshed by `rescan_installed_applications`.
pub fn shared_application_catalog() -> &'static SharedApplicationCatalog {
    static SHARED: std::sync::OnceLock<SharedApplicationCatalog> = std::sync::OnceLock::new();
    SHARED.get_or_init(|| {
        let dirs = xdg_application_dirs();
        let stamps = dir_stamps(&dirs);
        let shared =
            SharedApplicationCatalog::new(ApplicationCatalog::discover().unwrap_or_default());
        *shared
            .stamps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(stamps);
        shared
    })
}

/// Explicit rescan of the XDG application directories. No watcher and no logout.
pub fn rescan_installed_applications() -> FlameResult<CatalogDelta> {
    shared_application_catalog().rescan_from_dirs(&xdg_application_dirs())
}

/// Gated rescan for launch-time consumers: a no-op unless an XDG application dir changed.
pub fn refresh_installed_applications_if_changed() -> FlameResult<CatalogDelta> {
    shared_application_catalog().refresh_if_dirs_changed(&xdg_application_dirs())
}

/// Resolves `app` in `shared`; on a miss, runs the mtime-gated refresh over `dirs` and retries once.
/// A refresh that finds nothing new costs stat calls only, so a genuinely absent id still fails.
pub fn resolve_with_refresh<T>(
    shared: &SharedApplicationCatalog,
    dirs: &[PathBuf],
    app: &DesktopAppId,
    resolve: impl FnOnce(&DesktopEntry) -> FlameResult<T>,
) -> FlameResult<T> {
    let current = shared.current();
    if let Ok(entry) = current.find(app) {
        return resolve(entry);
    }
    // Refresh failure keeps the snapshot; the retry below reports the miss.
    let _ = shared.refresh_if_dirs_changed(dirs);
    let refreshed = shared.current();
    resolve(refreshed.find(app)?)
}

/// Launch-time lookup for WM launch paths: shared catalog, refresh-and-retry once on miss.
pub fn with_application_entry<T>(
    app: &DesktopAppId,
    resolve: impl FnOnce(&DesktopEntry) -> FlameResult<T>,
) -> FlameResult<T> {
    resolve_with_refresh(
        shared_application_catalog(),
        &xdg_application_dirs(),
        app,
        resolve,
    )
}

/// Launches `app` through the shared catalog with the same refresh-on-miss rule.
pub fn launch_application_resolved(
    app: &DesktopAppId,
    options: &ApplicationLaunchOptions,
) -> FlameResult<()> {
    with_application_entry(app, |entry| {
        let argv = entry.argv(options)?;
        launch_argv_for_app(&argv, &entry.application, entry.startup_notify)
    })
}

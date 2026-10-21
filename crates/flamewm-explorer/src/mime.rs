//! Native Open With choices. XDG owns associations; FlameWM owns the chooser UI.
//! Discovery and `xdg-mime` calls run on a dedicated worker, never the UI thread.
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use flamewm_api::applications::ApplicationLaunchOptions;
use flamewm_applications::{ApplicationCatalog, DesktopEntry};

pub const CHOICE_ROWS: usize = 6;

#[derive(Clone, Debug)]
pub struct AppChoice {
    pub entry: DesktopEntry,
    pub associated: bool,
    pub is_default: bool,
}

#[derive(Debug)]
pub struct OpenWithChooser {
    pub path: PathBuf,
    pub mime: String,
    pub apps: Vec<AppChoice>,
    pub show_all: bool,
    pub page: usize,
    pub selected: Option<usize>,
}

impl OpenWithChooser {
    pub fn matching_count(&self) -> usize {
        self.apps.iter().filter(|app| self.show_all || app.associated).count()
    }
    pub fn page_count(&self) -> usize {
        self.matching_count().div_ceil(CHOICE_ROWS).max(1)
    }
    pub fn page_choices(&self) -> Vec<(usize, &AppChoice)> {
        self.apps.iter().enumerate()
            .filter(|(_, app)| self.show_all || app.associated)
            .skip(self.page.saturating_mul(CHOICE_ROWS))
            .take(CHOICE_ROWS)
            .collect()
    }
    pub fn select_visible(&mut self, slot: usize) {
        let index = self.page_choices().get(slot).map(|(index, _)| *index);
        if let Some(index) = index { self.selected = Some(index); }
    }
    pub fn show_every_app(&mut self) {
        self.show_all = !self.show_all;
        self.page = 0;
        if self.selected.is_some_and(|index| !self.show_all && !self.apps[index].associated) {
            self.selected = self.apps.iter().position(|choice| choice.associated);
        }
    }
}

pub fn desktop_supports_mime(source: &str, mime: &str) -> bool {
    let mut in_desktop_entry = false;
    for line in source.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            in_desktop_entry = line == "[Desktop Entry]";
        } else if in_desktop_entry && !line.starts_with('#') {
            if let Some(types) = line.strip_prefix("MimeType=") {
                return types.split(';').any(|value| value.trim() == mime);
            }
        }
    }
    false
}

/// The MIME Apps spec distinguishes the association list from the default-app
/// lookup. Desktop-specific files only supply defaults; only generic
/// mimeapps.list files can add/remove associations.
#[derive(Debug, Default, PartialEq, Eq)]
struct AssociationChanges {
    added: Vec<String>,
    removed: Vec<String>,
}

fn parse_association_changes(text: &str, mime: &str) -> AssociationChanges {
    let mut changes = AssociationChanges::default();
    let mut group = "";
    for line in text.lines().map(|line| line.trim()) {
        if line.is_empty() || line.starts_with('#') { continue; }
        if line.starts_with('[') && line.ends_with(']') {
            group = &line[1..line.len() - 1];
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue; };
        if key.trim() != mime { continue; }
        let destination = match group {
            "Added Associations" => &mut changes.added,
            "Removed Associations" => &mut changes.removed,
            _ => continue,
        };
        destination.extend(value.split(';')
            .map(str::trim)
            .filter(|id| id.ends_with(".desktop") && !id.contains('/'))
            .map(str::to_owned));
    }
    changes
}

#[derive(Debug, Clone)]
struct AssociationLayer {
    mimeapps: PathBuf,
    applications: Option<PathBuf>,
}

fn xdg_dirs(variable: &str, fallback: &str) -> Vec<PathBuf> {
    let raw = std::env::var(variable).ok().filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_owned());
    raw.split(':').filter(|part| !part.is_empty()).map(PathBuf::from).collect()
}

/// Files appear in the exact XDG precedence order, highest-priority first.
/// Do not apply [Added/Removed Associations] from desktop-specific files.
fn association_layers() -> Vec<AssociationLayer> {
    let home = std::env::var_os("HOME").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/nonexistent"));
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty()).map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|s| !s.is_empty()).map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    let mut layers = vec![AssociationLayer {
        mimeapps: config_home.join("mimeapps.list"), applications: None,
    }];
    layers.extend(xdg_dirs("XDG_CONFIG_DIRS", "/etc/xdg").into_iter().map(|root| AssociationLayer {
        mimeapps: root.join("mimeapps.list"), applications: None,
    }));
    let data = std::iter::once(data_home)
        .chain(xdg_dirs("XDG_DATA_DIRS", "/usr/local/share:/usr/share"));
    layers.extend(data.map(|root| {
        let applications = root.join("applications");
        AssociationLayer {
            mimeapps: applications.join("mimeapps.list"), applications: Some(applications),
        }
    }));
    layers
}

/// Returns desktop IDs in association preference order. A higher-priority
/// Removed entry suppresses a lower-priority declared MIME type; a lower-
/// priority rule cannot override a higher-priority desktop file.
fn associated_ids(
    entries: &[DesktopEntry], mime: &str, layers: &[AssociationLayer],
) -> Vec<String> {
    let owners = entries.iter().map(|entry| {
        layers.iter().position(|layer| layer.applications.as_ref()
            .is_some_and(|dir| entry.path().starts_with(dir)))
    }).collect::<Vec<_>>();
    let by_id = entries.iter().enumerate()
        .map(|(index, entry)| (entry.id().as_str(), index))
        .collect::<HashMap<_, _>>();
    let mut accepted = Vec::new();
    let mut seen = HashSet::new();
    let mut removed = HashSet::new();
    let mut declared = Vec::with_capacity(entries.len());
    for entry in entries {
        let supports = fs::read_to_string(entry.path()).ok()
            .is_some_and(|text| desktop_supports_mime(&text, mime));
        declared.push(supports);
    }
    for (layer_index, layer) in layers.iter().enumerate() {
        let rules = fs::read_to_string(&layer.mimeapps).ok()
            .map(|text| parse_association_changes(&text, mime)).unwrap_or_default();
        for id in &rules.added {
            let Some(&index) = by_id.get(id.as_str()) else { continue; };
            if owners[index].is_some_and(|owner| owner < layer_index) { continue; }
            if !removed.contains(id) && seen.insert(id.clone()) {
                accepted.push(id.clone());
            }
        }
        for id in rules.removed {
            let Some(&index) = by_id.get(id.as_str()) else { continue; };
            if owners[index].is_some_and(|owner| owner < layer_index) { continue; }
            removed.insert(id);
        }
        for (index, entry) in entries.iter().enumerate() {
            if owners[index] == Some(layer_index) && declared[index] {
                let id = entry.id().as_str();
                if !removed.contains(id) && seen.insert(id.to_owned()) {
                    accepted.push(id.to_owned());
                }
            }
        }
    }
    // Unexpected application directories: keep their declared MIME handlers.
    for (index, entry) in entries.iter().enumerate() {
        if owners[index].is_none() && declared[index] {
            let id = entry.id().as_str();
            if !removed.contains(id) && seen.insert(id.to_owned()) {
                accepted.push(id.to_owned());
            }
        }
    }
    accepted
}

fn xdg_query(args: &[&std::ffi::OsStr]) -> Result<String, String> {
    let output = Command::new("xdg-mime").args(args).output()
        .map_err(|error| format!("xdg-mime unavailable: {error}"))?;
    if !output.status.success() {
        return Err(format!("xdg-mime returned {}", output.status));
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_owned())
        .map_err(|error| format!("invalid xdg-mime output: {error}"))
}

pub fn discover(path: PathBuf) -> Result<OpenWithChooser, String> {
    use std::ffi::OsStr;
    let mime = xdg_query(&[OsStr::new("query"), OsStr::new("filetype"), path.as_os_str()])?;
    if mime.is_empty() { return Err("Cannot determine file MIME type".into()); }
    let default = xdg_query(&[OsStr::new("query"), OsStr::new("default"), OsStr::new(&mime)])
        .unwrap_or_default();
    let catalog = ApplicationCatalog::discover_file_handlers()
        .map_err(|error| format!("Application catalog unavailable: {error}"))?;
    let order = associated_ids(catalog.entries(), &mime, &association_layers());
    let positions = order.into_iter().enumerate()
        .map(|(index, id)| (id, index)).collect::<HashMap<_, _>>();
    let mut apps = catalog.entries().iter().map(|entry| {
        // An explicit user-selected default should remain visible even when the
        // third-party launcher omitted MimeType= (xdg-mime only writes defaults).
        let is_default = entry.id().as_str() == default;
        let associated = is_default || positions.contains_key(entry.id().as_str());
        AppChoice {
            associated,
            is_default,
            entry: entry.clone(),
        }
    }).collect::<Vec<_>>();
    apps.sort_by(|a, b| {
        b.is_default.cmp(&a.is_default)
            .then_with(|| match (positions.get(a.entry.id().as_str()), positions.get(b.entry.id().as_str())) {
                (Some(left), Some(right)) => left.cmp(right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.entry.name().to_lowercase().cmp(&b.entry.name().to_lowercase()))
            .then_with(|| a.entry.id().as_str().cmp(b.entry.id().as_str()))
    });
    let selected = apps.iter().position(|app| app.associated);
    let show_all = selected.is_none();
    let selected = selected.or_else(|| (!apps.is_empty()).then_some(0));
    Ok(OpenWithChooser {path, mime, apps, show_all, page: 0, selected})
}

/// Convert an absolute local pathname to an RFC 8089 file URI for %u/%U.
/// The normal %f/%F case continues to receive the literal path.
pub fn local_file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut encoded = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(&mut encoded, "%{byte:02X}");
        }
    }
    encoded
}

pub fn launch(entry: &DesktopEntry, path: &Path) -> Result<(), String> {
    if !["%f", "%F", "%u", "%U"].iter().any(|code| entry.exec().contains(code)) {
        return Err("This application has no file argument in its desktop launcher".into());
    }
    // The existing shared .desktop launcher handles quoting and field codes;
    // never execute an arbitrary desktop Exec line through a shell.
    let target = if entry.exec().contains("%u") || entry.exec().contains("%U") {
        local_file_uri(path)
    } else {
        path.to_string_lossy().into_owned()
    };
    entry.launch(&ApplicationLaunchOptions {uris: vec![target], extra_args: vec![]})
        .map_err(|error| error.to_string())
}

pub fn set_default(mime: &str, desktop_id: &str) -> Result<(), String> {
    use std::ffi::OsStr;
    if mime.is_empty() || !desktop_id.ends_with(".desktop") {
        return Err("Invalid MIME default request".into());
    }
    let status = Command::new("xdg-mime")
        .args([OsStr::new("default"), OsStr::new(desktop_id), OsStr::new(mime)])
        .status().map_err(|error| format!("Cannot save MIME default: {error}"))?;
    if status.success() {Ok(())} else {Err(format!("xdg-mime default failed: {status}"))}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn desktop_mime_field_must_be_in_main_group_and_exact() {
        let source = "[Other]\nMimeType=text/plain;\n[Desktop Entry]\nName=Editor\nMimeType=text/html;text/plain;\n[Desktop Action Edit]\nMimeType=application/pdf;\n";
        assert!(desktop_supports_mime(source, "text/plain"));
        assert!(!desktop_supports_mime(source, "text/pla"));
        assert!(!desktop_supports_mime(source, "application/pdf"));
    }

    #[test]
    fn percent_escapes_spaces_and_non_utf8_bytes_in_file_uris() {
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(std::ffi::OsStr::from_bytes(b"/home/a strange/#\xff.txt"));
        assert_eq!(local_file_uri(path), "file:///home/a%20strange/%23%FF.txt");
    }

    #[test]
    fn generic_mimeapps_parses_additions_removals_without_using_defaults() {
        let rules = parse_association_changes("[Default Applications]\ntext/plain=default.desktop;\n[Added Associations]\ntext/plain=first.desktop;second.desktop;\napplication/pdf=pdf.desktop;\n[Removed Associations]\ntext/plain=blocked.desktop;\n", "text/plain");
        assert_eq!(rules.added, vec!["first.desktop", "second.desktop"]);
        assert_eq!(rules.removed, vec!["blocked.desktop"]);
    }

    #[test]
    fn higher_precedence_removal_and_addition_respect_desktop_file_priority() {
        use flamewm_api::DesktopAppId;
        let root = std::env::temp_dir().join(format!("flamewm-mime-layers-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        let config = root.join("config");
        let local = root.join("local/applications");
        let system = root.join("system/applications");
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&local).unwrap();
        fs::create_dir_all(&system).unwrap();
        fs::write(config.join("mimeapps.list"), "[Added Associations]\ntext/plain=custom.desktop;third.desktop;\n[Removed Associations]\ntext/plain=removed.desktop;\n").unwrap();
        fs::write(system.join("mimeapps.list"), "[Removed Associations]\ntext/plain=custom.desktop;local.desktop;\n").unwrap();
        let make = |dir: &Path, id: &str, supports: bool| {
            let file = dir.join(id);
            fs::write(&file, format!("[Desktop Entry]\nType=Application\nName=App\nExec=test %f\n{}", if supports {"MimeType=text/plain;\n"} else {""})).unwrap();
            DesktopEntry::from_file_with_id(&file, DesktopAppId::new(id)).unwrap().unwrap()
        };
        let entries = vec![
            make(&local,"local.desktop",true),
            make(&system,"custom.desktop",false),
            make(&system,"removed.desktop",true),
            make(&system,"third.desktop",false),
            make(&system,"normal.desktop",true),
        ];
        let layers = vec![
            AssociationLayer {mimeapps: config.join("mimeapps.list"), applications: None},
            AssociationLayer {mimeapps: local.join("mimeapps.list"), applications: Some(local)},
            AssociationLayer {mimeapps: system.join("mimeapps.list"), applications: Some(system)},
        ];
        let associated = associated_ids(&entries,"text/plain",&layers);
        assert_eq!(associated,vec!["custom.desktop","third.desktop","local.desktop","normal.desktop"]);
        fs::remove_dir_all(root).unwrap();
    }
}

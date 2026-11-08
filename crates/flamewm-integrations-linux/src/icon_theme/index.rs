//! Parsed index.theme model and section helpers.

use super::*;

#[derive(Clone, Debug, Default)]
pub struct IconThemeIndex {
    pub name: String,
    pub inherits: Vec<String>,
    pub directories: Vec<String>,
    pub dirs: HashMap<String, IconDir>,
}

impl IconThemeIndex {
    #[must_use]
    pub fn parse(name: &str, text: &str) -> Self {
        let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut current = String::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                line[1..line.len() - 1].trim().clone_into(&mut current);
                sections.entry(current.clone()).or_default();
                continue;
            }
            if current.is_empty() {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                sections
                    .entry(current.clone())
                    .or_default()
                    .insert(key.trim().to_owned(), value.trim().to_owned());
            }
        }
        let header = sections.get("Icon Theme").cloned().unwrap_or_default();
        let directories = split_list(header.get("Directories").map_or("", String::as_str));
        let inherits = split_list(header.get("Inherits").map_or("", String::as_str));
        let mut dirs: HashMap<String, _> = directories
            .iter()
            .filter_map(|dir| Some((dir.clone(), parse_dir(sections.get(dir.as_str())?))))
            .collect();
        // ScaledDirectories entries share the same section format.
        for dir in header
            .get("ScaledDirectories")
            .map(String::as_str)
            .map(split_list)
            .unwrap_or_default()
        {
            if dirs.contains_key(&dir) {
                continue;
            }
            let Some(section) = sections.get(dir.as_str()) else {
                continue;
            };
            let mut entry = parse_dir(section);
            if entry.scale == 1 {
                entry.scale = 2;
            }
            dirs.insert(dir, entry);
        }
        Self {
            name: name.to_owned(),
            inherits,
            directories,
            dirs,
        }
    }

    /// Reads only the `Inherits` list of the `[Icon Theme]` section, without
    /// building the per-directory tables of [`IconThemeIndex::parse`].
    pub(super) fn parse_inherits(name: &str, mut reader: impl std::io::BufRead) -> Option<Self> {
        let mut line = String::new();
        let mut section = String::new();
        let mut inherits = None;
        loop {
            line.clear();
            if reader.read_line(&mut line).ok()? == 0 {
                break;
            }
            let text = line.trim();
            if text.is_empty() || text.starts_with('#') || text.starts_with(';') {
                continue;
            }
            if text.starts_with('[') && text.ends_with(']') {
                text[1..text.len() - 1].trim().clone_into(&mut section);
                continue;
            }
            if section != "Icon Theme" {
                continue;
            }
            if let Some((key, value)) = text.split_once('=')
                && key.trim() == "Inherits"
            {
                inherits = Some(value.trim().to_owned());
            }
        }
        Some(Self {
            name: name.to_owned(),
            inherits: split_list(inherits.as_deref().unwrap_or("")),
            ..Self::default()
        })
    }

    #[must_use]
    pub fn load(theme_dir: &Path) -> Option<Self> {
        let name = theme_dir
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        let text = fs::read_to_string(theme_dir.join("index.theme")).ok()?;
        Some(Self::parse(&name, &text))
    }
}

/// Numeric `key` from an index.theme section, or `default` when absent or unparsable.
pub(super) fn dir_num<T: std::str::FromStr>(
    section: &HashMap<String, String>,
    key: &str,
    default: T,
) -> T {
    section
        .get(key)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

pub(super) fn parse_dir(section: &HashMap<String, String>) -> IconDir {
    let size = dir_num(section, "Size", 0);
    let scale: u32 = dir_num(section, "Scale", 1);
    let dir_type = match section.get("Type").map_or("", String::as_str) {
        "Fixed" => IconDirType::Fixed,
        "Scalable" => IconDirType::Scalable,
        _ => IconDirType::Threshold,
    };
    IconDir {
        size,
        scale: scale.max(1),
        dir_type,
        min_size: dir_num(section, "MinSize", size),
        max_size: dir_num(section, "MaxSize", size),
        threshold: dir_num(section, "Threshold", 2),
    }
}

pub(super) fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Splits `foo.png` -> `(foo, png)`; bare `foo` -> `None` (caller treats as
/// base with no explicit ext). Never yields a double extension.
pub(super) fn split_icon_filename(name: &str) -> Option<(String, String)> {
    let (base, ext) = name.rsplit_once('.')?;
    if base.is_empty() || ext.is_empty() {
        return None;
    }
    if !ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    Some((base.to_owned(), ext.to_ascii_lowercase()))
}

use super::*;

pub const MAX_BOOKMARKS: usize = 128;
const MAX_LABEL_CHARS: usize = 120;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    pub path: PathBuf,
    /// Custom label; `None` means the path's file name is shown.
    pub label: Option<String>,
}

impl Bookmark {
    #[must_use]
    pub fn display_name(&self) -> String {
        self.label.clone().unwrap_or_else(|| {
            self.path
                .file_name()
                .unwrap_or(self.path.as_os_str())
                .to_string_lossy()
                .into_owned()
        })
    }
}

/// Strips control characters and caps length. Used for labels from bookmarks and devices.
#[must_use]
pub fn sanitize_label(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Encodes an absolute path as a `file://` URI, percent-encoding everything outside the unreserved set.
#[must_use]
pub fn file_uri(path: &Path) -> String {
    let escaped: String = path
        .to_string_lossy()
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("file://{escaped}")
}

fn decode_file_uri(raw: &str) -> Option<PathBuf> {
    crate::trash::percent_decode(raw.strip_prefix("file://")?).map(PathBuf::from)
}

/// Parses a GTK `bookmarks` file. Non-`file://` and malformed lines are ignored.
#[must_use]
pub fn parse_bookmarks(text: &str) -> Vec<Bookmark> {
    text.lines()
        .filter_map(|line| {
            let (uri, label) = match line.split_once(' ') {
                Some((uri, label)) => (uri, Some(sanitize_label(label)).filter(|l| !l.is_empty())),
                None => (line, None),
            };
            let path = decode_file_uri(uri.trim())?;
            path.is_absolute().then_some(Bookmark { path, label })
        })
        .take(MAX_BOOKMARKS)
        .collect()
}

pub fn render_bookmarks(items: &[Bookmark]) -> String {
    let mut out = String::new();
    for item in items.iter().take(MAX_BOOKMARKS) {
        out.push_str(&file_uri(&item.path));
        if let Some(label) = item
            .label
            .as_deref()
            .map(sanitize_label)
            .filter(|l| !l.is_empty())
        {
            out.push(' ');
            out.push_str(&label);
        }
        out.push('\n');
    }
    out
}

/// Adds a bookmark at the end unless the path is already present or the list is full.
pub fn add(items: &mut Vec<Bookmark>, path: &Path) -> bool {
    if items.len() >= MAX_BOOKMARKS || items.iter().any(|b| b.path == path) {
        return false;
    }
    items.push(Bookmark {
        path: path.to_path_buf(),
        label: None,
    });
    true
}

pub fn remove(items: &mut Vec<Bookmark>, index: usize) -> Option<Bookmark> {
    (index < items.len()).then(|| items.remove(index))
}

pub fn rename(items: &mut [Bookmark], index: usize, label: &str) -> bool {
    let Some(item) = items.get_mut(index) else {
        return false;
    };
    let label = sanitize_label(label);
    item.label = (!label.is_empty()).then_some(label);
    true
}

/// Moves the item at `from` so that it ends up at `to` (clamped to the list bounds).
pub fn reorder(items: &mut Vec<Bookmark>, from: usize, to: usize) -> bool {
    if from >= items.len() {
        return false;
    }
    let item = items.remove(from);
    items.insert(to.min(items.len()), item);
    true
}

/// Resolves the GTK bookmarks file location (`$XDG_CONFIG_HOME/gtk-3.0/bookmarks`).
#[must_use]
pub fn bookmarks_file(config_home: &Path) -> PathBuf {
    config_home.join("gtk-3.0/bookmarks")
}

#[must_use]
pub fn load_bookmarks(file: &Path) -> Vec<Bookmark> {
    fs::read_to_string(file)
        .map(|text| parse_bookmarks(&text))
        .unwrap_or_default()
}

/// Atomically replaces the bookmarks file (write temp file, fsync, rename).
pub fn save_bookmarks(file: &Path, items: &[Bookmark]) -> io::Result<()> {
    let parent = file
        .parent()
        .ok_or_else(|| io::Error::other("bookmarks path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(".bookmarks.flamewm-tmp");
    {
        let mut out = fs::File::create(&temp)?;
        out.write_all(render_bookmarks(items).as_bytes())?;
        out.sync_all()?;
    }
    fs::rename(&temp, file)
}

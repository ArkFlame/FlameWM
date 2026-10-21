//! Sidebar places: persisted GTK-style bookmarks (add/remove/rename/reorder), mounted
//! removable devices from mountinfo + sysfs, and UDisks2 mount/unmount/eject via `udisksctl`.
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const MAX_BOOKMARKS: usize = 128;
const MAX_LABEL_CHARS: usize = 120;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bookmark {
    pub path: PathBuf,
    /// Custom label; `None` means the path's file name is shown.
    pub label: Option<String>,
}

impl Bookmark {
    pub fn display_name(&self) -> String {
        self.label.clone().unwrap_or_else(|| {
            self.path.file_name().unwrap_or(self.path.as_os_str()).to_string_lossy().into_owned()
        })
    }
}

/// Strips control characters and caps length. Used for labels from bookmarks and devices.
pub fn sanitize_label(raw: &str) -> String {
    raw.chars().filter(|c| !c.is_control()).take(MAX_LABEL_CHARS).collect::<String>().trim().to_owned()
}

/// Encodes an absolute path as a `file://` URI, percent-encoding everything outside the unreserved set.
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
        if let Some(label) = item.label.as_deref().map(sanitize_label).filter(|l| !l.is_empty()) {
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
    items.push(Bookmark { path: path.to_path_buf(), label: None });
    true
}

pub fn remove(items: &mut Vec<Bookmark>, index: usize) -> Option<Bookmark> {
    (index < items.len()).then(|| items.remove(index))
}

pub fn rename(items: &mut [Bookmark], index: usize, label: &str) -> bool {
    let Some(item) = items.get_mut(index) else { return false };
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
pub fn bookmarks_file(config_home: &Path) -> PathBuf {
    config_home.join("gtk-3.0/bookmarks")
}

pub fn load_bookmarks(file: &Path) -> Vec<Bookmark> {
    fs::read_to_string(file).map(|text| parse_bookmarks(&text)).unwrap_or_default()
}

/// Atomically replaces the bookmarks file (write temp file, fsync, rename).
pub fn save_bookmarks(file: &Path, items: &[Bookmark]) -> io::Result<()> {
    let parent = file.parent().ok_or_else(|| io::Error::other("bookmarks path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(".bookmarks.flamewm-tmp");
    {
        let mut out = fs::File::create(&temp)?;
        out.write_all(render_bookmarks(items).as_bytes())?;
        out.sync_all()?;
    }
    fs::rename(&temp, file)
}

/// A mounted filesystem under a conventional media root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountedDevice {
    pub source: String,
    pub mount_point: PathBuf,
    /// Kernel block name without `/dev/` (e.g. `sdb1`).
    pub block_name: String,
}

fn unescape_mount(text: &str) -> String {
    text.replace("\\040", " ").replace("\\011", "\t").replace("\\012", "\n").replace("\\134", "\\")
}

/// Parses `/proc/self/mountinfo` for removable-media mount points (`/media`, `/run/media`, `/mnt`).
/// Field layout: id parent maj:min root mount_point opts [optional...] - fstype source superopts.
pub fn parse_mountinfo(text: &str) -> Vec<MountedDevice> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(sep) = fields.iter().position(|f| *f == "-") else { continue };
        let (Some(mount), Some(source)) = (fields.get(4), fields.get(sep + 2)) else { continue };
        let mount = unescape_mount(mount);
        if !(mount.starts_with("/media/") || mount.starts_with("/run/media/") || mount.starts_with("/mnt/")) {
            continue;
        }
        let Some(block_name) = source.strip_prefix("/dev/").filter(|n| !n.is_empty() && !n.contains('/')) else {
            continue;
        };
        if seen.insert(mount.clone()) {
            out.push(MountedDevice {
                source: (*source).to_owned(),
                mount_point: PathBuf::from(&mount),
                block_name: block_name.to_owned(),
            });
        }
    }
    out
}

/// True when sysfs marks the block device, or its parent disk, as removable.
pub fn is_removable_in(sys_block_root: &Path, block_name: &str) -> bool {
    let flag = |dir: PathBuf| fs::read_to_string(dir.join("removable")).map(|v| v.trim() == "1").unwrap_or(false);
    let dev = sys_block_root.join(block_name);
    flag(dev.clone()) || flag(dev.join(".."))
}

pub fn is_removable(block_name: &str) -> bool {
    is_removable_in(Path::new("/sys/class/block"), block_name)
}

/// Accepts only `/dev/<name>` block paths without shell metacharacters or traversal.
pub fn valid_device_path(device: &str) -> bool {
    device.strip_prefix("/dev/").is_some_and(|name| {
        !name.is_empty() && name.len() <= 255 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

/// Runs `udisksctl` for one device operation. Returns trimmed stdout on success, a bounded stderr message on failure.
pub fn udisks_action(action: &str, device: &str) -> Result<String, String> {
    if !matches!(action, "mount" | "unmount" | "eject" | "power-off") {
        return Err(format!("Unsupported device action {action}"));
    }
    if !valid_device_path(device) {
        return Err("Refusing invalid device path".into());
    }
    let subcommand = match action {
        "eject" => "power-off",
        other => other,
    };
    let output = Command::new("udisksctl")
        .arg(subcommand)
        .arg("-b")
        .arg(device)
        .output()
        .map_err(|error| format!("udisksctl unavailable: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().chars().take(512).collect())
    } else {
        let message = String::from_utf8_lossy(&output.stderr);
        Err(sanitize_label(message.trim()).chars().take(300).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_round_trip_preserves_labels_and_rejects_non_file_uris() {
        let items = parse_bookmarks("file:///home/u/My%20Docs Work\nsftp://host/x\nfile:///tmp/a\n");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].path, PathBuf::from("/home/u/My Docs"));
        assert_eq!(items[0].display_name(), "Work");
        assert_eq!(items[1].display_name(), "a");
        let rendered = render_bookmarks(&items);
        assert_eq!(parse_bookmarks(&rendered), items);
    }

    #[test]
    fn add_remove_rename_reorder_are_bounded_and_deduplicated() {
        let mut items = Vec::new();
        assert!(add(&mut items, Path::new("/a")));
        assert!(add(&mut items, Path::new("/b")));
        assert!(add(&mut items, Path::new("/c")));
        assert!(!add(&mut items, Path::new("/a")));
        assert!(reorder(&mut items, 2, 0));
        assert_eq!(items.iter().map(|b| b.path.clone()).collect::<Vec<_>>(), vec![PathBuf::from("/c"), PathBuf::from("/a"), PathBuf::from("/b")]);
        assert!(rename(&mut items, 1, "  Alpha\u{7}  "));
        assert_eq!(items[1].label.as_deref(), Some("Alpha"));
        assert!(rename(&mut items, 1, "   "));
        assert_eq!(items[1].label, None);
        assert_eq!(remove(&mut items, 0).unwrap().path, PathBuf::from("/c"));
        assert!(remove(&mut items, 9).is_none());
        assert!(!reorder(&mut items, 9, 0));
    }

    #[test]
    fn save_and_load_bookmarks_through_tempdir() {
        let dir = std::env::temp_dir().join(format!("flamewm-places-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let file = bookmarks_file(&dir);
        let items = vec![Bookmark { path: PathBuf::from("/srv/data"), label: Some("Data".into()) }];
        save_bookmarks(&file, &items).unwrap();
        assert_eq!(load_bookmarks(&file), items);
        assert!(!dir.join("gtk-3.0/.bookmarks.flamewm-tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mountinfo_selects_media_mounts_and_unescapes_names() {
        let text = "36 25 8:17 / /run/media/u/My\\040Stick rw,nosuid shared:1 - vfat /dev/sdb1 rw\n\
                    37 25 8:1 / / rw - ext4 /dev/sda1 rw\n\
                    38 25 0:50 / /mnt/x rw - tmpfs tmpfs rw\n\
                    39 25 8:33 / /media/u/Dup rw - vfat /dev/sdc1 rw\n\
                    40 25 8:33 / /media/u/Dup rw - vfat /dev/sdc1 rw\n";
        let devices = parse_mountinfo(text);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].mount_point, PathBuf::from("/run/media/u/My Stick"));
        assert_eq!(devices[0].block_name, "sdb1");
        assert_eq!(devices[1].block_name, "sdc1");
    }

    #[test]
    fn removable_flag_is_read_from_device_or_parent_disk() {
        let root = std::env::temp_dir().join(format!("flamewm-sysblock-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sdb/sdb1")).unwrap();
        // Mirror /sys/class/block: the entry is a symlink into the parent disk's directory.
        std::os::unix::fs::symlink(root.join("sdb/sdb1"), root.join("sdb1")).unwrap();
        fs::write(root.join("sdb/removable"), "1\n").unwrap();
        assert!(is_removable_in(&root, "sdb1"));
        fs::write(root.join("sdb/removable"), "0\n").unwrap();
        assert!(!is_removable_in(&root, "sdb1"));
        assert!(!is_removable_in(&root, "missing"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn device_paths_are_validated_before_spawning() {
        assert!(valid_device_path("/dev/sdb1"));
        assert!(!valid_device_path("/dev/sdb1; rm -rf ~"));
        assert!(!valid_device_path("/dev/../etc"));
        assert!(!valid_device_path("sdb1"));
        assert!(udisks_action("format", "/dev/sdb1").is_err());
        assert!(udisks_action("mount", "/dev/../x").is_err());
    }
}

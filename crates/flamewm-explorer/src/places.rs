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

/// A network filesystem mounted in this session. `fuse.gvfsd-fuse` marks the GVFS root, whose children are shares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkMount {
    pub mount_point: PathBuf,
    pub fstype: String,
}

/// Filesystem types shown in the Network group: SMB/CIFS, NFS, sshfs, WebDAV (davfs) and the GVFS FUSE root.
pub const NETWORK_FSTYPES: [&str; 8] = ["cifs", "smb3", "nfs", "nfs4", "fuse.sshfs", "davfs", "fuse.davfs", "fuse.gvfsd-fuse"];
pub const GVFS_FSTYPE: &str = "fuse.gvfsd-fuse";

/// Network mounts from `/proc/self/mountinfo` text. Only the fstype selects a line, so pseudo or local mounts never match.
/// Field layout: id parent maj:min root mount_point opts [optional...] - fstype source superopts.
pub fn parse_network_mounts(text: &str) -> Vec<NetworkMount> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(sep) = fields.iter().position(|f| *f == "-") else { continue };
        let (Some(mount), Some(fstype)) = (fields.get(4), fields.get(sep + 1)) else { continue };
        if !NETWORK_FSTYPES.contains(fstype) {
            continue;
        }
        let mount_point = PathBuf::from(unescape_mount(mount));
        if seen.insert(mount_point.clone()) {
            out.push(NetworkMount { mount_point, fstype: (*fstype).to_owned() });
        }
    }
    out
}

/// The per-user GVFS FUSE root, `/run/user/<uid>/gvfs`. The uid is the owner of this process's `/proc/self`.
pub fn gvfs_root() -> PathBuf {
    use std::os::unix::fs::MetadataExt;
    let uid = fs::metadata("/proc/self").map(|meta| meta.uid()).unwrap_or(0);
    PathBuf::from(format!("/run/user/{uid}/gvfs"))
}

/// Share folders directly under the GVFS root, sorted by name. Each is one network location.
pub fn gvfs_shares(root: &Path) -> Vec<PathBuf> {
    let mut shares: Vec<PathBuf> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_dir())
        .collect();
    shares.sort();
    shares
}

/// Sidebar label for a GVFS share folder name such as `smb-share:server=nas,share=media` ("media on nas").
/// Falls back to the raw folder name when the name has no recognised keys.
pub fn gvfs_label(folder: &str) -> String {
    let params = folder.split_once(':').map_or("", |(_, rest)| rest);
    let value = |key: &str| params.split(',').find_map(|pair| pair.strip_prefix(key)?.strip_prefix('=').map(str::to_owned));
    let label = match (value("share"), value("server").or_else(|| value("host"))) {
        (Some(share), Some(server)) => format!("{share} on {server}"),
        (Some(share), None) => share,
        (None, Some(server)) => server,
        (None, None) => folder.to_owned(),
    };
    sanitize_label(&label)
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

/// A removable filesystem that is present but not mounted, as reported by `lsblk`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockDevice {
    /// `/dev/sdb1`
    pub device: String,
    /// Filesystem label, or the kernel name when the filesystem has none.
    pub label: String,
}

/// Minimal JSON value: just enough to read `lsblk -J` without a serialization dependency.
#[derive(Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn items(&self) -> &[Json] {
        match self {
            Json::Arr(items) => items,
            _ => &[],
        }
    }
    fn text(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    /// lsblk prints flags as booleans in recent releases and as "0"/"1" strings in older ones.
    fn flag(&self) -> bool {
        match self {
            Json::Bool(b) => *b,
            Json::Num(n) => *n != 0.0,
            Json::Str(s) => s == "1" || s == "true",
            _ => false,
        }
    }
    fn parse(text: &str) -> Option<Json> {
        let mut parser = JsonParser { bytes: text.as_bytes(), at: 0 };
        let value = parser.value(0)?;
        parser.skip_space();
        (parser.at == parser.bytes.len()).then_some(value)
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl JsonParser<'_> {
    fn skip_space(&mut self) {
        while self.bytes.get(self.at).is_some_and(|b| b.is_ascii_whitespace()) {
            self.at += 1;
        }
    }
    fn eat(&mut self, byte: u8) -> bool {
        let found = self.bytes.get(self.at) == Some(&byte);
        if found {
            self.at += 1;
        }
        found
    }
    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > 64 {
            return None;
        }
        self.skip_space();
        match *self.bytes.get(self.at)? {
            b'{' => {
                self.at += 1;
                let mut fields = Vec::new();
                self.skip_space();
                if !self.eat(b'}') {
                    loop {
                        self.skip_space();
                        let key = self.string()?;
                        self.skip_space();
                        if !self.eat(b':') {
                            return None;
                        }
                        fields.push((key, self.value(depth + 1)?));
                        self.skip_space();
                        if self.eat(b'}') {
                            break;
                        }
                        if !self.eat(b',') {
                            return None;
                        }
                    }
                }
                Some(Json::Obj(fields))
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                self.skip_space();
                if !self.eat(b']') {
                    loop {
                        items.push(self.value(depth + 1)?);
                        self.skip_space();
                        if self.eat(b']') {
                            break;
                        }
                        if !self.eat(b',') {
                            return None;
                        }
                    }
                }
                Some(Json::Arr(items))
            }
            b'"' => self.string().map(Json::Str),
            b't' => self.literal("true", Json::Bool(true)),
            b'f' => self.literal("false", Json::Bool(false)),
            b'n' => self.literal("null", Json::Null),
            _ => {
                let start = self.at;
                while self.bytes.get(self.at).is_some_and(|b| b"+-.eE0123456789".contains(b)) {
                    self.at += 1;
                }
                std::str::from_utf8(&self.bytes[start..self.at]).ok()?.parse::<f64>().ok().map(Json::Num)
            }
        }
    }
    fn literal(&mut self, word: &str, value: Json) -> Option<Json> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Some(value)
        } else {
            None
        }
    }
    fn string(&mut self) -> Option<String> {
        if !self.eat(b'"') {
            return None;
        }
        let mut out = Vec::new();
        loop {
            let byte = *self.bytes.get(self.at)?;
            self.at += 1;
            match byte {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let escaped = *self.bytes.get(self.at)?;
                    self.at += 1;
                    let decoded = match escaped {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'u' => {
                            let hex = std::str::from_utf8(self.bytes.get(self.at..self.at + 4)?).ok()?;
                            self.at += 4;
                            char::from_u32(u32::from_str_radix(hex, 16).ok()?).unwrap_or('\u{fffd}')
                        }
                        other => other as char,
                    };
                    let mut buf = [0_u8; 4];
                    out.extend_from_slice(decoded.encode_utf8(&mut buf).as_bytes());
                }
                other => out.push(other),
            }
        }
    }
}

/// Filesystems smaller than this are system partitions, not user media.
const MIN_DEVICE_BYTES: f64 = (64u64 << 20) as f64;

/// Unmounted removable filesystems from `lsblk -J` output. Partitions of a removable disk are listed one by one;
/// a removable disk without partitions is listed itself. Mounted, swap, unformatted and invalid nodes are skipped.
pub fn parse_lsblk(text: &str) -> Vec<BlockDevice> {
    let Some(root) = Json::parse(text) else { return Vec::new() };
    let mut out = Vec::new();
    for disk in root.get("blockdevices").map_or(&[][..], Json::items) {
        let removable = disk.get("rm").is_some_and(Json::flag) || disk.get("hotplug").is_some_and(Json::flag);
        if !removable {
            continue;
        }
        let partitions = disk.get("children").map_or(&[][..], Json::items);
        let nodes: Vec<&Json> = if partitions.is_empty() { vec![disk] } else { partitions.iter().collect() };
        out.extend(nodes.into_iter().filter_map(unmounted_filesystem));
    }
    out
}

fn unmounted_filesystem(node: &Json) -> Option<BlockDevice> {
    let name = node.get("name")?.text()?;
    let fstype = node.get("fstype").and_then(Json::text).unwrap_or("");
    if fstype.is_empty() || fstype == "swap" || name.starts_with("loop") || name.starts_with("ram") || name.starts_with("zram") {
        return None;
    }
    // Dolphin hides tiny system partitions; lsblk -b reports sizes in bytes.
    let size = match node.get("size") {
        Some(Json::Num(n)) => Some(*n),
        Some(Json::Str(s)) => s.parse::<f64>().ok(),
        _ => None,
    };
    if size.is_some_and(|bytes| bytes < MIN_DEVICE_BYTES) {
        return None;
    }
    // Boot partitions (EFI, Ventoy's VTOYEFI) are firmware media, not user storage.
    let label_text = node.get("label").and_then(Json::text).unwrap_or("");
    if label_text.eq_ignore_ascii_case("EFI") || label_text.eq_ignore_ascii_case("VTOYEFI") {
        return None;
    }
    let mounted = node.get("mountpoint").and_then(Json::text).is_some_and(|m| !m.is_empty())
        || node.get("mountpoints").is_some_and(|list| list.items().iter().any(|m| m.text().is_some()));
    let device = format!("/dev/{name}");
    if mounted || !valid_device_path(&device) {
        return None;
    }
    let label = node.get("label").and_then(Json::text).map(sanitize_label).filter(|l| !l.is_empty());
    // Dolphin-style fallback when the filesystem has no label.
    Some(BlockDevice { device, label: label.unwrap_or_else(|| format!("Removable Device ({name})")) })
}

/// Runs `lsblk` and returns unmounted removable filesystems. A missing or failing lsblk yields no devices.
pub fn unmounted_devices() -> Vec<BlockDevice> {
    // Dev-only hook: a saved `lsblk -J` sample stands in for hardware, so screenshots need no real device.
    if let Some(path) = std::env::var_os("FLAMEWM_EXPLORER_FAKE_DEVICES") {
        return fs::read_to_string(path).map(|text| parse_lsblk(&text)).unwrap_or_default();
    }
    let Ok(output) = Command::new("lsblk").args(["-J", "-b", "-o", "NAME,RM,HOTPLUG,FSTYPE,MOUNTPOINT,LABEL,SIZE"]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_lsblk(&String::from_utf8_lossy(&output.stdout))
}

/// Argument vector for one `udisksctl` call. Eject is a power-off of the device; no shell is involved.
pub fn udisks_args(action: &str, device: &str) -> Result<Vec<String>, String> {
    if !matches!(action, "mount" | "unmount" | "eject" | "power-off") {
        return Err(format!("Unsupported device action {action}"));
    }
    if !valid_device_path(device) {
        return Err("Refusing invalid device path".into());
    }
    let subcommand = if action == "eject" { "power-off" } else { action };
    Ok(vec![subcommand.to_owned(), "-b".to_owned(), device.to_owned()])
}

/// Runs `udisksctl` for one device operation. Returns trimmed stdout on success, a bounded stderr message on failure.
/// Callers run this on a worker thread: udisks can block while a filesystem is flushed.
pub fn udisks_action(action: &str, device: &str) -> Result<String, String> {
    let args = udisks_args(action, device)?;
    let output = Command::new("udisksctl")
        .args(&args)
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
    fn network_filter_keeps_only_network_fstypes_on_sample_lines() {
        let text = "\
36 35 0:45 / /mnt/nas rw,relatime shared:12 - cifs //nas/media rw,vers=3.0
37 35 0:46 / /home/u/share\\040dir rw,relatime shared:13 - nfs4 host:/export rw,vers=4.2
38 35 0:47 / /home/u/sshfs rw,nosuid - fuse.sshfs user@host:/srv rw,user_id=1000
39 35 0:48 / /home/u/dav rw,nosuid shared:14 - fuse.davfs https://dav.example/ rw
40 35 0:49 / /run/user/1000/gvfs rw,nosuid,nodev,relatime shared:15 - fuse.gvfsd-fuse gvfsd-fuse rw,user_id=1000
41 35 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
42 35 0:50 / /run/media/u/USB rw shared:16 - vfat /dev/sdb1 rw
43 35 0:51 / /proc rw - proc proc rw
44 35 0:45 / /mnt/nas rw shared:12 - cifs //nas/media rw
garbage line with no separator
";
        let mounts = parse_network_mounts(text);
        let found: Vec<(&str, &str)> = mounts.iter().map(|m| (m.mount_point.to_str().unwrap(), m.fstype.as_str())).collect();
        assert_eq!(
            found,
            vec![
                ("/mnt/nas", "cifs"),
                ("/home/u/share dir", "nfs4"),
                ("/home/u/sshfs", "fuse.sshfs"),
                ("/home/u/dav", "fuse.davfs"),
                ("/run/user/1000/gvfs", "fuse.gvfsd-fuse"),
            ]
        );
    }

    #[test]
    fn gvfs_folder_names_become_share_labels() {
        assert_eq!(gvfs_label("smb-share:server=nas,share=media"), "media on nas");
        assert_eq!(gvfs_label("sftp:host=example.com,user=me"), "example.com");
        assert_eq!(gvfs_label("plain"), "plain");
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
    fn udisks_commands_are_built_as_argument_vectors() {
        assert_eq!(udisks_args("mount", "/dev/sdb1").unwrap(), ["mount", "-b", "/dev/sdb1"]);
        assert_eq!(udisks_args("unmount", "/dev/sdb1").unwrap(), ["unmount", "-b", "/dev/sdb1"]);
        assert_eq!(udisks_args("eject", "/dev/sdb").unwrap(), ["power-off", "-b", "/dev/sdb"]);
        assert!(udisks_args("format", "/dev/sdb1").is_err());
        assert!(udisks_args("mount", "/dev/sdb1; rm -rf ~").is_err());
        assert!(udisks_args("mount", "sdb1").is_err());
    }

    #[test]
    fn unlabelled_removable_filesystem_gets_dolphin_style_name() {
        let json = r#"{"blockdevices":[{"name":"sdb","rm":true,"hotplug":true,"fstype":null,"mountpoint":null,"label":null,"children":[{"name":"sdb1","rm":true,"fstype":"vfat","mountpoint":null,"label":null}]}]}"#;
        let devices = parse_lsblk(json);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device, "/dev/sdb1");
        assert_eq!(devices[0].label, "Removable Device (sdb1)");
    }

    #[test]
    fn lsblk_hides_loop_and_tiny_system_partitions() {
        let sample = r#"{"blockdevices":[
            {"name":"loop0","rm":true,"hotplug":true,"children":[{"name":"loop0p1","fstype":"squashfs","mountpoint":null,"label":null,"size":104857600}]},
            {"name":"sdq","rm":true,"hotplug":true,"children":[
                {"name":"sdq1","rm":true,"fstype":"vfat","mountpoint":null,"label":"EFI","size":104857600},
                {"name":"sdq2","rm":true,"fstype":"exfat","mountpoint":null,"label":"Big","size":8000000000}]}]}"#;
        assert_eq!(parse_lsblk(sample), vec![BlockDevice { device: "/dev/sdq2".into(), label: "Big".into() }]);
    }

    #[test]
    fn lsblk_json_lists_only_unmounted_removable_filesystems() {
        let sample = r#"{"blockdevices": [
            {"name":"sda","rm":false,"hotplug":false,"fstype":null,"mountpoint":null,"label":null,
             "children":[{"name":"sda1","rm":false,"hotplug":false,"fstype":"ext4","mountpoint":"/","label":null}]},
            {"name":"sdb","rm":true,"hotplug":true,"fstype":null,"mountpoint":null,"label":null,
             "children":[
                {"name":"sdb1","rm":true,"hotplug":true,"fstype":"vfat","mountpoint":"/run/media/u/Backup","label":"Backup"},
                {"name":"sdb2","rm":true,"hotplug":true,"fstype":"vfat","mountpoint":null,"label":"Stick \u00e9\n"},
                {"name":"sdb3","rm":true,"hotplug":true,"fstype":"swap","mountpoint":null,"label":null}]},
            {"name":"sdc","rm":"1","hotplug":"1","fstype":"exfat","mountpoint":null,"label":null},
            {"name":"sdd","rm":true,"hotplug":true,"fstype":null,"mountpoint":null,"label":null},
            {"name":"sde","rm":true,"hotplug":true,"fstype":"vfat","mountpoints":[null],"label":"Camera","children":null}
        ]}"#;
        assert_eq!(
            parse_lsblk(sample),
            vec![
                BlockDevice { device: "/dev/sdb2".into(), label: "Stick é".into() },
                BlockDevice { device: "/dev/sdc".into(), label: "Removable Device (sdc)".into() },
                BlockDevice { device: "/dev/sde".into(), label: "Camera".into() },
            ]
        );
        assert!(parse_lsblk("not json").is_empty());
        assert!(parse_lsblk(r#"{"blockdevices": [{"name":"sdf","rm":true,"fstype":"vfat"}"#).is_empty());
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

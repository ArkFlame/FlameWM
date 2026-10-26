//! Sidebar places: persisted GTK-style bookmarks (add/remove/rename/reorder), mounted
//! removable devices from mountinfo + sysfs, and `UDisks2` mount/unmount/eject via `udisksctl`.
pub use flamewm_integrations_linux::removable_mounts::udisks_action;
use flamewm_integrations_linux::removable_mounts::{
    self, by_uuid_dir, is_removable_in, mounted_block_devices, unescape_mount, uuid_blocks,
    valid_device_path,
};
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

/// A mounted filesystem under a conventional media root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountedDevice {
    pub source: String,
    pub mount_point: PathBuf,
    /// Kernel block name without `/dev/` (e.g. `sdb1`).
    pub block_name: String,
}

/// Parses `/proc/self/mountinfo` for removable-media mount points (`/media`, `/run/media`, `/mnt`).
/// Field layout: id parent maj:min root `mount_point` opts [optional...] - fstype source superopts.
#[must_use]
pub fn parse_mountinfo(text: &str) -> Vec<MountedDevice> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(sep) = fields.iter().position(|f| *f == "-") else {
            continue;
        };
        let (Some(mount), Some(source)) = (fields.get(4), fields.get(sep + 2)) else {
            continue;
        };
        let mount = unescape_mount(mount);
        if !(mount.starts_with("/media/")
            || mount.starts_with("/run/media/")
            || mount.starts_with("/mnt/"))
        {
            continue;
        }
        let Some(block_name) = source
            .strip_prefix("/dev/")
            .filter(|n| !n.is_empty() && !n.contains('/'))
        else {
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

/// Filesystem types shown in the Network group: SMB/CIFS, NFS, sshfs, `WebDAV` (davfs) and the GVFS FUSE root.
pub const NETWORK_FSTYPES: [&str; 8] = [
    "cifs",
    "smb3",
    "nfs",
    "nfs4",
    "fuse.sshfs",
    "davfs",
    "fuse.davfs",
    "fuse.gvfsd-fuse",
];
pub const GVFS_FSTYPE: &str = "fuse.gvfsd-fuse";

/// Network mounts from `/proc/self/mountinfo` text. Only the fstype selects a line, so pseudo or local mounts never match.
/// Field layout: id parent maj:min root `mount_point` opts [optional...] - fstype source superopts.
#[must_use]
pub fn parse_network_mounts(text: &str) -> Vec<NetworkMount> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(sep) = fields.iter().position(|f| *f == "-") else {
            continue;
        };
        let (Some(mount), Some(fstype)) = (fields.get(4), fields.get(sep + 1)) else {
            continue;
        };
        if !NETWORK_FSTYPES.contains(fstype) {
            continue;
        }
        let mount_point = PathBuf::from(unescape_mount(mount));
        if seen.insert(mount_point.clone()) {
            out.push(NetworkMount {
                mount_point,
                fstype: (*fstype).to_owned(),
            });
        }
    }
    out
}

/// The per-user GVFS FUSE root, `/run/user/<uid>/gvfs`. The uid is the owner of this process's `/proc/self`.
#[must_use]
pub fn gvfs_root() -> PathBuf {
    use std::os::unix::fs::MetadataExt;
    let uid = fs::metadata("/proc/self")
        .map(|meta| meta.uid())
        .unwrap_or(0);
    PathBuf::from(format!("/run/user/{uid}/gvfs"))
}

/// Share folders directly under the GVFS root, sorted by name. Each is one network location.
#[must_use]
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
#[must_use]
pub fn gvfs_label(folder: &str) -> String {
    let params = folder.split_once(':').map_or("", |(_, rest)| rest);
    let value = |key: &str| {
        params
            .split(',')
            .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('=').map(str::to_owned))
    };
    let label = match (value("share"), value("server").or_else(|| value("host"))) {
        (Some(share), Some(server)) => format!("{share} on {server}"),
        (Some(share), None) => share,
        (None, Some(server)) => server,
        (None, None) => folder.to_owned(),
    };
    sanitize_label(&label)
}

/// A removable filesystem that is present but not mounted, as reported by `lsblk`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockDevice {
    /// `/dev/sdb1`
    pub device: String,
    /// Filesystem label, or the kernel name when the filesystem has none.
    pub label: String,
    /// True when `lsblk` marks the drive (or its transport) removable. An internal data partition is
    /// listed but is not removable media, so the sidebar does not present it as one.
    pub removable: bool,
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
    fn parse(text: &str) -> Option<Json> {
        let mut parser = JsonParser {
            bytes: text.as_bytes(),
            at: 0,
        };
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
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
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
                while self
                    .bytes
                    .get(self.at)
                    .is_some_and(|b| b"+-.eE0123456789".contains(b))
                {
                    self.at += 1;
                }
                std::str::from_utf8(&self.bytes[start..self.at])
                    .ok()?
                    .parse::<f64>()
                    .ok()
                    .map(Json::Num)
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
                            let hex =
                                std::str::from_utf8(self.bytes.get(self.at..self.at + 4)?).ok()?;
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

/// `lsblk` columns. Beyond the base set this asks for the identity columns that decide whether a
/// partition is user storage: `TYPE`/`TRAN`/`RM` describe the device, and `PARTTYPE`/`PARTFLAGS`
/// carry the partition-table identity (GPT type GUID or MBR type byte, plus the attribute flags) so a
/// firmware or recovery partition is rejected by what it *is*, not only by what it is labelled.
pub const LSBLK_COLUMNS: &str = "NAME,FSTYPE,MOUNTPOINT,LABEL,SIZE,TYPE,TRAN,RM,PARTTYPE,PARTFLAGS";
/// Columns every lsblk since 2.2x understands; used when the extended query is rejected.
pub const LSBLK_COLUMNS_LEGACY: &str = "NAME,FSTYPE,MOUNTPOINT,LABEL,SIZE";

/// Argument vector for an `lsblk -J` query with the given column list.
#[must_use]
pub fn lsblk_args(columns: &str) -> Vec<String> {
    vec![
        "-J".to_owned(),
        "-b".to_owned(),
        "-o".to_owned(),
        columns.to_owned(),
    ]
}

/// GPT partition type GUIDs that are firmware, recovery or a container member: never user media.
/// Compared in lowercase; `lsblk` prints them uppercase.
const SYSTEM_PART_TYPES: [&str; 11] = [
    "c12a7328-f81f-11d2-ba4b-00a0c93ec93b", // EFI System Partition
    "e3c9e316-0b5c-4db8-817d-f92df00215ae", // Microsoft Reserved (MSR)
    "de94bba4-06d1-4d40-a16a-bfd50179d6ac", // Windows Recovery Environment
    "21686148-6449-6e6f-744e-656564454649", // BIOS boot partition
    "9e1a2d38-c612-4316-aa26-8b49521e5a8b", // PowerPC PReP boot
    "d3bfe2de-3daf-11df-ba40-e3a556d89593", // Intel Fast Flash (iFFS)
    "0657fd6d-a4ab-43c4-84e5-0933c84b4f4f", // Linux swap
    "ca7d7ccb-63ed-4c53-861c-1742536059cc", // LUKS
    "e6d6d379-f507-44c2-a23c-238f2a3df928", // LVM physical volume
    "a19d880f-05fc-4d3b-a006-743f0f84911e", // Linux RAID member
    "933ac7e1-2eb4-4f13-b844-0e14e2aef915", // /home (not removable media)
];

/// The same for MBR disks, where `lsblk` prints the partition type byte as `0x..`.
const SYSTEM_PART_TYPES_MBR: [&str; 6] = [
    "0xef", // EFI System Partition
    "0x82", // Linux swap / Solaris
    "0x8e", // Linux LVM
    "0xfd", // Linux RAID autodetect
    "0x27", // Windows Recovery Environment
    "0x12", // OEM diagnostics / recovery
];

/// Labels that mark firmware or recovery media on images without a readable partition table.
const SYSTEM_PART_LABELS: [&str; 6] = [
    "efi",
    "efi system partition",
    "vtoyefi",
    "bios boot",
    "microsoft reserved",
    "system reserved",
];

/// Mount points that belong to the boot chain. A disk that holds one of them is never offered as
/// removable media, whatever its other partitions look like.
const BOOT_MOUNT_POINTS: [&str; 4] = ["/boot", "/boot/efi", "/boot/efi2", "/efi"];

/// A GPT attribute flag with this bit set marks a "legacy BIOS bootable" partition.
const GPT_BIOS_BOOT_FLAG: u64 = 0x4;
/// The MBR "active/boot" indicator as `lsblk` reports it in `PARTFLAGS`.
const MBR_BOOT_FLAG: u64 = 0x80;

/// Unmounted filesystems from `lsblk -J` output, on internal and removable disks alike. Partitions are listed one
/// by one; a disk without partitions is listed itself. Mounted, swap, encryption/RAID/LVM members, unformatted,
/// tiny nodes, firmware/recovery partitions and disks that hold the boot chain are skipped.
pub fn parse_lsblk(text: &str) -> Vec<BlockDevice> {
    let Some(root) = Json::parse(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for disk in root.get("blockdevices").map_or(&[][..], Json::items) {
        let partitions = disk.get("children").map_or(&[][..], Json::items);
        // Reject the whole disk when it carries the boot chain: /boot or the ESP on one partition
        // means the other partitions are the same system installation, not user media.
        if holds_boot_mount(disk, partitions) {
            continue;
        }
        let nodes: Vec<&Json> = if partitions.is_empty() {
            vec![disk]
        } else {
            partitions.iter().collect()
        };
        let disk_removable = node_removable(disk);
        out.extend(
            nodes
                .into_iter()
                .filter_map(|node| unmounted_filesystem(node, disk_removable)),
        );
    }
    out
}

/// True when `lsblk` marks a node removable, either as a boolean, a `"1"`/`"0"` string or a number.
fn node_removable(node: &Json) -> bool {
    match node.get("rm") {
        Some(Json::Bool(removable)) => *removable,
        Some(Json::Str(value)) => matches!(value.as_str(), "1" | "true"),
        Some(Json::Num(value)) => *value != 0.0,
        // Older lsblk without RM: a USB transport is removable media.
        _ => node_text(node, "tran").eq_ignore_ascii_case("usb"),
    }
}

/// Text value of a JSON object key, or `""` when it is absent or not a string.
fn node_text<'a>(node: &'a Json, key: &str) -> &'a str {
    node.get(key).and_then(Json::text).unwrap_or("")
}

/// Every mount point a node reports, in both the single `mountpoint` and the `mountpoints` array form.
fn mount_points(node: &Json) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    if let Some(single) = node
        .get("mountpoint")
        .and_then(Json::text)
        .filter(|m| !m.is_empty())
    {
        out.push(single);
    }
    if let Some(list) = node.get("mountpoints") {
        out.extend(
            list.items()
                .iter()
                .filter_map(Json::text)
                .filter(|m| !m.is_empty()),
        );
    }
    out
}

fn is_boot_mount(mount: &str) -> bool {
    BOOT_MOUNT_POINTS.contains(&mount)
        || mount
            .strip_prefix("/boot/")
            .is_some_and(|rest| !rest.is_empty())
}

/// True when the disk itself or any of its partitions is mounted as part of the boot chain.
fn holds_boot_mount(disk: &Json, partitions: &[Json]) -> bool {
    let own = mount_points(disk);
    let children: Vec<&str> = partitions.iter().flat_map(mount_points).collect();
    own.into_iter().chain(children).any(is_boot_mount)
}

/// Parses a `PARTFLAGS` hex value (`0x80`, `0x4`, `0x8000000000000000`) into its bits.
fn part_flags_bits(flags: &str) -> Option<u64> {
    let hex = flags
        .trim()
        .strip_prefix("0x")
        .or_else(|| flags.trim().strip_prefix("0X"))?;
    u64::from_str_radix(hex, 16).ok()
}

/// True for a partition that is firmware, recovery, swap or a container member by partition-table
/// identity: type GUID, MBR type byte, boot attribute or a reserved-type label.
fn is_system_partition(node: &Json) -> bool {
    let part_type = node_text(node, "parttype").to_ascii_lowercase();
    if !part_type.is_empty()
        && (SYSTEM_PART_TYPES.contains(&part_type.as_str())
            || SYSTEM_PART_TYPES_MBR.contains(&part_type.as_str()))
    {
        return true;
    }
    if let Some(bits) = part_flags_bits(node_text(node, "partflags")) {
        if bits & GPT_BIOS_BOOT_FLAG != 0 || bits == MBR_BOOT_FLAG {
            return true;
        }
    }
    let label = node_text(node, "label").to_ascii_lowercase();
    SYSTEM_PART_LABELS.contains(&label.as_str())
}

/// Block devices that are not one ejectable filesystem: device-mapper, MD RAID, network block
/// devices and partitions of them.
fn is_mapped_device(name: &str) -> bool {
    name.contains('/')
        || name.starts_with("dm-")
        || name.starts_with("md")
        || name.starts_with("nbd")
        || name.starts_with("drbd")
}

fn unmounted_filesystem(node: &Json, disk_removable: bool) -> Option<BlockDevice> {
    let name = node.get("name")?.text()?;
    // Device-mapper/MD nodes are assembled volumes, not a partition of a drive to offer or eject.
    if is_mapped_device(name) {
        return None;
    }
    let fstype = node.get("fstype").and_then(Json::text).unwrap_or("");
    // lsblk reports the node kind for newer versions; members of a container hold no filesystem.
    let kind = node_text(node, "type");
    if kind.starts_with("crypt")
        || kind.starts_with("lvm")
        || kind.starts_with("raid")
        || kind == "swap"
    {
        return None;
    }
    // Encrypted, RAID and LVM members hold no mountable filesystem of their own.
    let member = matches!(
        fstype,
        "swap" | "crypto_LUKS" | "LVM2_member" | "linux_raid_member"
    );
    if fstype.is_empty()
        || member
        || name.starts_with("loop")
        || name.starts_with("ram")
        || name.starts_with("zram")
    {
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
    // Firmware, recovery, swap and container partitions are identified by their partition type,
    // MBR type byte and boot/read-only flags before any label is considered.
    if is_system_partition(node) {
        return None;
    }
    let mounted = !mount_points(node).is_empty();
    let device = format!("/dev/{name}");
    if mounted || !valid_device_path(&device) {
        return None;
    }
    let label = node
        .get("label")
        .and_then(Json::text)
        .map(sanitize_label)
        .filter(|l| !l.is_empty());
    // A partition inherits the removability of the drive it lives on.
    let removable = node_removable(node) || disk_removable;
    Some(BlockDevice {
        device,
        label: label.unwrap_or_else(|| format!("Volume ({name})")),
        removable,
    })
}

/// Runs `lsblk` and returns unmounted filesystems. A missing or failing lsblk yields no devices.
/// The extended column set is retried with the legacy set when an older lsblk rejects PARTTYPE.
#[must_use]
pub fn unmounted_devices() -> Vec<BlockDevice> {
    // Dev-only hook: a saved `lsblk -J` sample stands in for hardware, so screenshots need no real device.
    if let Some(path) = std::env::var_os("FLAMEWM_EXPLORER_FAKE_DEVICES") {
        return fs::read_to_string(path)
            .map(|text| parse_lsblk(&text))
            .unwrap_or_default();
    }
    lsblk_output(LSBLK_COLUMNS)
        .or_else(|| lsblk_output(LSBLK_COLUMNS_LEGACY))
        .map(|text| parse_lsblk(&text))
        .unwrap_or_default()
}

fn lsblk_output(columns: &str) -> Option<String> {
    let output = Command::new("lsblk")
        .args(lsblk_args(columns))
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Name of the drive holding `block_name`: the parent disk of a partition, or the device itself.
#[must_use]
pub fn drive_in(sys_block_root: &Path, block_name: &str) -> String {
    let dev = sys_block_root.join(block_name);
    if !dev.join("partition").exists() {
        return block_name.to_owned();
    }
    fs::canonicalize(&dev)
        .ok()
        .and_then(|path| {
            path.parent()?
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| block_name.to_owned())
}

/// Eject a device: unmount its filesystem, then power off the drive that holds it. `run` performs one udisks action
/// on a device path. A filesystem that is already unmounted is not an error; a busy one gets a plain message.
pub fn eject_with(
    device: &str,
    drive: &str,
    run: impl FnMut(&str, &str) -> Result<String, String>,
) -> Result<String, String> {
    eject_filesystem_then_drive(device, drive, run)
}

/// Unmount the filesystem, then power off its drive.
fn eject_filesystem_then_drive(
    device: &str,
    drive: &str,
    mut run: impl FnMut(&str, &str) -> Result<String, String>,
) -> Result<String, String> {
    let explain = |error: String| {
        if error.to_lowercase().contains("busy") {
            "the device is busy: close files or programs using it, then try again".to_owned()
        } else {
            error
        }
    };
    if let Err(error) = run("unmount", device) {
        if !error.to_lowercase().contains("not mounted") {
            return Err(explain(error));
        }
    }
    run("eject", drive).map_err(explain)
}

/// Another partition of `drive` that is still mounted, as `(device, mount_point)`.
fn mounted_sibling(
    mountinfo: &str,
    sys_block_root: &Path,
    drive: &str,
    device: &str,
) -> Option<(String, PathBuf)> {
    let name = device.trim_start_matches("/dev/");
    mounted_block_devices(mountinfo)
        .into_iter()
        .find_map(|(block_name, mount_point)| {
            if block_name == name || drive_in(sys_block_root, &block_name) != drive {
                return None;
            }
            Some((format!("/dev/{block_name}"), mount_point))
        })
}

fn read_mountinfo() -> String {
    fs::read_to_string("/proc/self/mountinfo").unwrap_or_default()
}

/// Eject a device through sysfs and udisks2, for the real system. Refuses a non-removable drive
/// unless the caller says the user confirmed it.
pub fn eject_device(device: &str) -> Result<String, String> {
    eject_device_in(
        Path::new("/sys/class/block"),
        &read_mountinfo(),
        device,
        false,
        udisks_action,
    )
}

/// Eject a device the user explicitly asked to eject (sidebar eject button or menu). The click *is*
/// the confirmation for an internal drive; the mounted-sibling guard still applies.
pub fn eject_device_confirmed(device: &str) -> Result<String, String> {
    eject_device_in(
        Path::new("/sys/class/block"),
        &read_mountinfo(),
        device,
        true,
        udisks_action,
    )
}

/// Whole eject decision, with the block-device tree and the mount table injected so it is testable
/// without touching a real disk. `run` performs exactly one udisks action per call.
///
/// Refusals, in order: an invalid or device-mapper path; a drive that is not removable when the
/// caller has not confirmed; and a drive that still carries another mounted partition, because
/// powering the drive down would pull that filesystem out from under its processes.
pub fn eject_device_in(
    sys_block_root: &Path,
    mountinfo: &str,
    device: &str,
    confirmed: bool,
    mut run: impl FnMut(&str, &str) -> Result<String, String>,
) -> Result<String, String> {
    if !valid_device_path(device) {
        return Err("Refusing invalid device path".into());
    }
    let name = device.trim_start_matches("/dev/");
    if is_mapped_device(name) {
        return Err("Refusing to eject a device-mapper or RAID volume".into());
    }
    let drive = drive_in(sys_block_root, name);
    if !confirmed && !is_removable_in(sys_block_root, name) {
        return Err(
            "Refusing to eject an internal drive: confirm the eject for this device".into(),
        );
    }
    if let Some((sibling, mount_point)) = mounted_sibling(mountinfo, sys_block_root, &drive, device)
    {
        return Err(format!(
            "the drive still holds the mounted partition {sibling} ({}): unmount it first",
            mount_point.display()
        ));
    }
    eject_filesystem_then_drive(device, &format!("/dev/{drive}"), &mut run)
}

/// Records a user mount (`remember`) or unmount/eject (`!remember`) of `device` in the remembered list.
/// Reads the state file fresh: the session's remount worker saves the same file.
pub fn set_remembered(device: &str, remember: bool) {
    let block = device.trim_start_matches("/dev/");
    let Some((uuid, _)) = uuid_blocks(&by_uuid_dir())
        .into_iter()
        .find(|(_, name)| name == block)
    else {
        return;
    };
    let path = removable_mounts::state_path();
    let mut known = removable_mounts::Memory::load(&path);
    let changed = if remember {
        known.remember(&uuid)
    } else {
        known.forget(&uuid)
    };
    if changed {
        let _ = removable_mounts::save(&path, &known.known);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_integrations_linux::removable_mounts::{removable_devices, udisks_args};

    #[test]
    fn bookmark_round_trip_preserves_labels_and_rejects_non_file_uris() {
        let items =
            parse_bookmarks("file:///home/u/My%20Docs Work\nsftp://host/x\nfile:///tmp/a\n");
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
        assert_eq!(
            items.iter().map(|b| b.path.clone()).collect::<Vec<_>>(),
            vec![
                PathBuf::from("/c"),
                PathBuf::from("/a"),
                PathBuf::from("/b")
            ]
        );
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
        let items = vec![Bookmark {
            path: PathBuf::from("/srv/data"),
            label: Some("Data".into()),
        }];
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
        let found: Vec<(&str, &str)> = mounts
            .iter()
            .map(|m| (m.mount_point.to_str().unwrap(), m.fstype.as_str()))
            .collect();
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
        assert_eq!(
            gvfs_label("smb-share:server=nas,share=media"),
            "media on nas"
        );
        assert_eq!(gvfs_label("sftp:host=example.com,user=me"), "example.com");
        assert_eq!(gvfs_label("plain"), "plain");
    }

    #[test]
    fn removable_devices_reports_mount_state_and_removable_flag() {
        let pairs = vec![
            ("AAAA-1111".to_owned(), "sdb1".to_owned()),
            ("BBBB".to_owned(), "nvme0n1p6".to_owned()),
        ];
        let mounted = vec![("sdb1".to_owned(), PathBuf::from("/run/media/u/Stick"))];
        let devices = removable_devices(&pairs, &mounted, |block| block == "sdb1");
        assert_eq!(
            devices[0],
            removable_mounts::Device {
                uuid: "AAAA-1111".into(),
                block: "sdb1".into(),
                removable: true,
                mounted: true
            }
        );
        assert_eq!(
            devices[1],
            removable_mounts::Device {
                uuid: "BBBB".into(),
                block: "nvme0n1p6".into(),
                removable: false,
                mounted: false
            }
        );
    }

    #[test]
    fn uuid_blocks_is_empty_for_a_missing_directory() {
        assert!(uuid_blocks(Path::new("/nonexistent/flamewm/by-uuid")).is_empty());
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
        assert_eq!(
            devices[0].mount_point,
            PathBuf::from("/run/media/u/My Stick")
        );
        assert_eq!(devices[0].block_name, "sdb1");
        assert_eq!(devices[1].block_name, "sdc1");
    }

    #[test]
    fn removable_flag_is_read_from_device_or_parent_disk() {
        let root =
            std::env::temp_dir().join(format!("flamewm-sysblock-test-{}", std::process::id()));
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
        assert_eq!(
            udisks_args("mount", "/dev/sdb1").unwrap(),
            ["mount", "-b", "/dev/sdb1"]
        );
        assert_eq!(
            udisks_args("unmount", "/dev/sdb1").unwrap(),
            ["unmount", "-b", "/dev/sdb1"]
        );
        assert_eq!(
            udisks_args("eject", "/dev/sdb").unwrap(),
            ["power-off", "-b", "/dev/sdb"]
        );
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
        assert_eq!(devices[0].label, "Volume (sdb1)");
    }

    #[test]
    fn lsblk_hides_loop_and_tiny_system_partitions() {
        let sample = r#"{"blockdevices":[
            {"name":"loop0","rm":true,"hotplug":true,"children":[{"name":"loop0p1","fstype":"squashfs","mountpoint":null,"label":null,"size":104857600}]},
            {"name":"ram0","fstype":"ext4","mountpoint":null,"label":"RAM","size":8000000000},
            {"name":"zram0","fstype":"ext4","mountpoint":null,"label":"Swap","size":8000000000,"rm":true},
            {"name":"sdq","rm":true,"hotplug":true,"children":[
                {"name":"sdq1","rm":true,"fstype":"vfat","mountpoint":null,"label":"EFI","size":104857600},
                {"name":"sdq2","rm":true,"fstype":"exfat","mountpoint":null,"label":"Big","size":8000000000}]}]}"#;
        assert_eq!(
            parse_lsblk(sample),
            vec![BlockDevice {
                device: "/dev/sdq2".into(),
                label: "Big".into(),
                removable: true
            }]
        );
    }

    #[test]
    fn lsblk_json_lists_unmounted_filesystems_on_internal_and_removable_disks() {
        let sample = r#"{"blockdevices": [
            {"name":"sda","rm":false,"hotplug":false,"fstype":null,"mountpoint":null,"label":null,
             "children":[
                {"name":"sda1","rm":false,"hotplug":false,"fstype":"ext4","mountpoint":"/","label":null},
                {"name":"sda2","fstype":"ntfs","mountpoint":null,"label":"Windows","size":500000000000},
                {"name":"sda3","fstype":"swap","mountpoint":null,"label":null,"size":8000000000},
                {"name":"sda4","fstype":"crypto_LUKS","mountpoint":null,"label":null,"size":8000000000},
                {"name":"sda5","fstype":"ext4","mountpoint":null,"label":"Tiny","size":1000}]},
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
                BlockDevice {
                    device: "/dev/sda2".into(),
                    label: "Windows".into(),
                    removable: false
                },
                BlockDevice {
                    device: "/dev/sdb2".into(),
                    label: "Stick é".into(),
                    removable: true
                },
                BlockDevice {
                    device: "/dev/sdc".into(),
                    label: "Volume (sdc)".into(),
                    removable: true
                },
                BlockDevice {
                    device: "/dev/sde".into(),
                    label: "Camera".into(),
                    removable: true
                },
            ]
        );
        assert!(parse_lsblk("not json").is_empty());
        assert!(
            parse_lsblk(r#"{"blockdevices": [{"name":"sdf","rm":true,"fstype":"vfat"}"#).is_empty()
        );
    }

    #[test]
    fn drive_is_parent_disk_for_partitions_and_self_for_disks() {
        let root = std::env::temp_dir().join(format!("flamewm-drive-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sda/sda1")).unwrap();
        fs::write(root.join("sda/sda1/partition"), "1\n").unwrap();
        std::os::unix::fs::symlink(root.join("sda/sda1"), root.join("sda1")).unwrap();
        fs::create_dir_all(root.join("sdb")).unwrap();
        assert_eq!(drive_in(&root, "sda1"), "sda");
        assert_eq!(drive_in(&root, "sda"), "sda");
        assert_eq!(drive_in(&root, "sdb"), "sdb");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn eject_unmounts_then_powers_off_the_drive() {
        let mut calls = Vec::new();
        let result = eject_with("/dev/sdb1", "/dev/sdb", |action, device| {
            calls.push(format!("{action} {device}"));
            Ok(String::new())
        });
        assert!(result.is_ok());
        assert_eq!(calls, ["unmount /dev/sdb1", "eject /dev/sdb"]);
        assert_eq!(
            udisks_args("eject", "/dev/sdb").unwrap(),
            ["power-off", "-b", "/dev/sdb"]
        );
    }

    #[test]
    fn eject_skips_unmount_when_already_unmounted_and_reports_busy_without_power_off() {
        let mut calls = Vec::new();
        let ok = eject_with("/dev/sdb1", "/dev/sdb", |action, _| {
            calls.push(action.to_owned());
            if action == "unmount" {
                Err("Error unmounting /dev/sdb1: Not mounted".into())
            } else {
                Ok(String::new())
            }
        });
        assert!(ok.is_ok());
        assert_eq!(calls, ["unmount", "eject"]);

        let mut calls = Vec::new();
        let busy = eject_with("/dev/sdb1", "/dev/sdb", |action, _| {
            calls.push(action.to_owned());
            Err("Error unmounting /dev/sdb1: Device or resource busy".into())
        });
        assert_eq!(
            busy.unwrap_err(),
            "the device is busy: close files or programs using it, then try again"
        );
        assert_eq!(calls, ["unmount"]);
    }

    #[test]
    fn eject_device_refuses_invalid_paths_before_running_anything() {
        assert_eq!(
            eject_device("/dev/sdb1; rm -rf ~").unwrap_err(),
            "Refusing invalid device path"
        );
    }

    #[test]
    fn lsblk_query_asks_for_partition_identity_columns() {
        for column in [
            "NAME",
            "FSTYPE",
            "MOUNTPOINT",
            "LABEL",
            "SIZE",
            "TYPE",
            "TRAN",
            "RM",
            "PARTTYPE",
            "PARTFLAGS",
        ] {
            assert!(
                LSBLK_COLUMNS.split(',').any(|name| name == column),
                "missing {column}"
            );
        }
        assert_eq!(lsblk_args(LSBLK_COLUMNS), ["-J", "-b", "-o", LSBLK_COLUMNS]);
        // The fallback stays queryable by lsblk versions without PARTTYPE.
        assert!(!LSBLK_COLUMNS_LEGACY.contains("PARTTYPE"));
    }

    #[test]
    fn firmware_recovery_and_container_partitions_are_rejected_by_partition_type() {
        // Every one of these is a system partition whose *label* is empty or innocent: only the
        // GPT type GUID or the MBR type byte identifies it.
        let sample = r#"{"blockdevices":[
            {"name":"nvme0n1","type":"disk","children":[
                {"name":"nvme0n1p1","fstype":"vfat","label":"","size":536870912,"type":"part","parttype":"C12A7328-F81F-11D2-BA4B-00A0C93EC93B"},
                {"name":"nvme0n1p2","fstype":"vfat","label":"","size":134217728,"type":"part","parttype":"E3C9E316-0B5C-4DB8-817D-F92DF00215AE"},
                {"name":"nvme0n1p3","fstype":"ntfs","label":"","size":1073741824,"type":"part","parttype":"DE94BBA4-06D1-4D40-A16A-BFD50179D6AC"},
                {"name":"nvme0n1p4","fstype":"","label":"","size":2097152,"type":"part","parttype":"21686148-6449-6E6F-744E-656564454649"},
                {"name":"nvme0n1p5","fstype":"crypto_LUKS","label":"","size":8589934592,"type":"part","parttype":"CA7D7CCB-63ED-4C53-861C-1742536059CC"},
                {"name":"nvme0n1p6","fstype":"ext4","label":"Data","size":17179869184,"type":"part","parttype":"0FC63DAF-8483-4772-8E79-3D69D8477DE4"}]},
            {"name":"sdc","type":"disk","rm":true,"children":[
                {"name":"sdc1","fstype":"vfat","label":"","size":268435456,"type":"part","parttype":"0xef"},
                {"name":"sdc2","fstype":"ext4","label":"","size":268435456,"type":"part","parttype":"0xfd"},
                {"name":"sdc3","fstype":"exfat","label":"Camera","size":67108864000,"type":"part","parttype":"0x7"}]}]}"#;
        assert_eq!(
            parse_lsblk(sample),
            vec![
                BlockDevice {
                    device: "/dev/nvme0n1p6".into(),
                    label: "Data".into(),
                    removable: false
                },
                BlockDevice {
                    device: "/dev/sdc3".into(),
                    label: "Camera".into(),
                    removable: true
                },
            ]
        );
    }

    #[test]
    fn boot_attribute_flags_and_reserved_labels_are_rejected() {
        // PARTFLAGS bit 2 marks a GPT legacy-BIOS-bootable partition; 0x80 is the MBR active flag.
        let sample = r#"{"blockdevices":[
            {"name":"sda","type":"disk","children":[
                {"name":"sda1","fstype":"ext4","label":"","size":1073741824,"type":"part","partflags":"0x4"},
                {"name":"sda2","fstype":"ntfs","label":"","size":1073741824,"type":"part","partflags":"0x80"},
                {"name":"sda3","fstype":"vfat","label":"BIOS boot","size":1073741824,"type":"part"},
                {"name":"sda4","fstype":"ntfs","label":"System Reserved","size":1073741824,"type":"part"},
                {"name":"sda5","fstype":"ntfs","label":"Media","size":1073741824,"type":"part","partflags":"0x8000000000000000"},
                {"name":"sda6","fstype":"ext4","label":"Work","size":1073741824,"type":"part","parttype":"0x83"}]}]}"#;
        assert_eq!(
            parse_lsblk(sample),
            vec![
                BlockDevice {
                    device: "/dev/sda5".into(),
                    label: "Media".into(),
                    removable: false
                },
                BlockDevice {
                    device: "/dev/sda6".into(),
                    label: "Work".into(),
                    removable: false
                },
            ]
        );
        assert_eq!(
            part_flags_bits("0x8000000000000000"),
            Some(0x8000_0000_0000_0000)
        );
        assert_eq!(part_flags_bits("garbage"), None);
    }

    #[test]
    fn device_mapper_raid_and_container_nodes_are_never_listed() {
        let sample = r#"{"blockdevices":[
            {"name":"dm-0","type":"lvm","fstype":"ext4","label":"Vol","size":1073741824,"mountpoint":null},
            {"name":"md0","type":"raid1","fstype":"ext4","label":"Array","size":1073741824},
            {"name":"mapper/vg-root","type":"lvm","fstype":"ext4","label":"Root","size":1073741824},
            {"name":"sdb","type":"disk","children":[
                {"name":"sdb1","type":"part","fstype":"LVM2_member","label":"","size":1073741824},
                {"name":"sdb2","type":"part","fstype":"swap","label":"","size":1073741824},
                {"name":"sdb3","type":"part","fstype":"ext4","label":"Data","size":1073741824}]}]}"#;
        assert_eq!(
            parse_lsblk(sample),
            vec![BlockDevice {
                device: "/dev/sdb3".into(),
                label: "Data".into(),
                removable: false
            }]
        );
    }

    #[test]
    fn a_disk_holding_the_boot_chain_is_not_offered_as_media() {
        // /boot/efi is mounted on this disk, so its unmounted Windows partition is the same
        // installation rather than a removable volume to offer.
        let sample = r#"{"blockdevices":[
            {"name":"sda","type":"disk","children":[
                {"name":"sda1","type":"part","fstype":"vfat","label":"","size":536870912,"mountpoint":"/boot/efi","parttype":"C12A7328-F81F-11D2-BA4B-00A0C93EC93B"},
                {"name":"sda2","type":"part","fstype":"ntfs","label":"Windows","size":500000000000}]},
            {"name":"sdb","type":"disk","rm":true,"children":[
                {"name":"sdb1","type":"part","fstype":"exfat","label":"Stick","size":67108864000}]}]}"#;
        assert_eq!(
            parse_lsblk(sample),
            vec![BlockDevice {
                device: "/dev/sdb1".into(),
                label: "Stick".into(),
                removable: true
            }]
        );
        assert!(is_boot_mount("/boot"));
        assert!(is_boot_mount("/boot/efi"));
        assert!(is_boot_mount("/boot/grub2"));
        assert!(!is_boot_mount("/bootleg"));
        assert!(!is_boot_mount("/home"));
    }

    #[test]
    fn mountinfo_lists_every_mounted_block_device() {
        let text = "36 25 8:17 / /run/media/u/My\\040Stick rw,nosuid shared:1 - vfat /dev/sdb1 rw\n\
                    37 25 8:1 / / rw - ext4 /dev/sda1 rw\n\
                    38 25 0:50 / /mnt/nas rw - cifs //nas/media rw\n\
                    39 25 8:33 / /home/u/w rw - ext4 /dev/sdc2 rw\n\
                    40 25 8:33 / /home/u/w rw - ext4 /dev/sdc2 rw\n";
        assert_eq!(
            mounted_block_devices(text),
            vec![
                ("sdb1".to_owned(), PathBuf::from("/run/media/u/My Stick")),
                ("sda1".to_owned(), PathBuf::from("/")),
                ("sdc2".to_owned(), PathBuf::from("/home/u/w")),
            ]
        );
    }

    /// Fake `/sys/class/block` tree: `sdb` with partitions `sdb1`/`sdb2`, entry symlinks like the real one.
    fn fake_sysblock(tag: &str, removable: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("flamewm-eject-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sdb/sdb1")).unwrap();
        fs::create_dir_all(root.join("sdb/sdb2")).unwrap();
        fs::create_dir_all(root.join("nvme0n1/nvme0n1p1")).unwrap();
        fs::write(root.join("sdb/sdb1/partition"), "1\n").unwrap();
        fs::write(root.join("sdb/sdb2/partition"), "1\n").unwrap();
        fs::write(root.join("nvme0n1/nvme0n1p1/partition"), "1\n").unwrap();
        fs::write(root.join("sdb/removable"), format!("{removable}\n")).unwrap();
        fs::write(root.join("nvme0n1/removable"), "0\n").unwrap();
        std::os::unix::fs::symlink(root.join("sdb/sdb1"), root.join("sdb1")).unwrap();
        std::os::unix::fs::symlink(root.join("sdb/sdb2"), root.join("sdb2")).unwrap();
        std::os::unix::fs::symlink(root.join("nvme0n1/nvme0n1p1"), root.join("nvme0n1p1")).unwrap();
        root
    }

    #[test]
    fn eject_runs_the_full_fake_udisks_sequence_for_removable_media() {
        let root = fake_sysblock("sequence", "1");
        let mut calls = Vec::new();
        // Only the target filesystem is mounted, so the drive can be powered off after unmounting.
        let mountinfo = "36 25 8:17 / /run/media/u/Stick rw - exfat /dev/sdb1 rw\n";
        let outcome = eject_device_in(&root, mountinfo, "/dev/sdb1", false, |action, device| {
            calls.push(format!("{action} {device}"));
            Ok(String::new())
        });
        assert_eq!(outcome, Ok(String::new()));
        assert_eq!(calls, ["unmount /dev/sdb1", "eject /dev/sdb"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn eject_tolerates_an_already_unmounted_filesystem() {
        let root = fake_sysblock("unmounted", "1");
        let mut calls = Vec::new();
        let outcome = eject_device_in(&root, "", "/dev/sdb1", false, |action, device| {
            calls.push(format!("{action} {device}"));
            if action == "unmount" {
                Err("Error unmounting /dev/sdb1: Not mounted".into())
            } else {
                Ok(String::new())
            }
        });
        assert!(outcome.is_ok());
        assert_eq!(calls, ["unmount /dev/sdb1", "eject /dev/sdb"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn eject_reports_a_busy_filesystem_and_never_powers_the_drive_off() {
        let root = fake_sysblock("busy", "1");
        let mut calls = Vec::new();
        let outcome = eject_device_in(&root, "", "/dev/sdb1", false, |action, device| {
            calls.push(format!("{action} {device}"));
            Err("Error unmounting /dev/sdb1: Device or resource busy".into())
        });
        assert_eq!(
            outcome.unwrap_err(),
            "the device is busy: close files or programs using it, then try again"
        );
        assert_eq!(calls, ["unmount /dev/sdb1"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn eject_refuses_a_drive_that_still_holds_another_mounted_partition() {
        let root = fake_sysblock("sibling", "1");
        let mut calls: Vec<String> = Vec::new();
        let mountinfo = "37 25 8:18 / /run/media/u/Other rw - vfat /dev/sdb2 rw\n";
        let outcome = eject_device_in(&root, mountinfo, "/dev/sdb1", true, |action, device| {
            calls.push(format!("{action} {device}"));
            Ok(String::new())
        });
        let message = outcome.unwrap_err();
        assert!(message.contains("/dev/sdb2"), "{message}");
        assert!(message.contains("/run/media/u/Other"), "{message}");
        assert!(
            calls.is_empty(),
            "nothing must be unmounted or powered off: {calls:?}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn eject_refuses_an_internal_drive_unless_the_user_confirmed_it() {
        let root = fake_sysblock("internal", "0");
        let mut calls: Vec<String> = Vec::new();
        let outcome = eject_device_in(&root, "", "/dev/nvme0n1p1", false, |action, device| {
            calls.push(format!("{action} {device}"));
            Ok(String::new())
        });
        assert!(outcome.unwrap_err().contains("confirm"));
        assert!(calls.is_empty());
        // The sidebar/menu click is the explicit confirmation, so the same device may be ejected then.
        let confirmed = eject_device_in(&root, "", "/dev/nvme0n1p1", true, |action, device| {
            calls.push(format!("{action} {device}"));
            Ok(String::new())
        });
        assert!(confirmed.is_ok());
        assert_eq!(calls, ["unmount /dev/nvme0n1p1", "eject /dev/nvme0n1"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn eject_refuses_device_mapper_volumes_and_bogus_paths() {
        let root = fake_sysblock("mapper", "1");
        let mut calls = 0;
        for device in [
            "/dev/dm-0",
            "/dev/md0",
            "/dev/mapper/vg-root",
            "/dev/../etc/passwd",
            "sdb1",
        ] {
            let outcome = eject_device_in(&root, "", device, true, |_, _| {
                calls += 1;
                Ok(String::new())
            });
            assert!(outcome.is_err(), "{device} must be refused");
        }
        assert_eq!(calls, 0);
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

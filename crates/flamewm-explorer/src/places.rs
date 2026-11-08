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

mod bookmarks;
pub use bookmarks::*;

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
    if let Some(bits) = part_flags_bits(node_text(node, "partflags"))
        && (bits & GPT_BIOS_BOOT_FLAG != 0 || bits == MBR_BOOT_FLAG)
    {
        return true;
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
    if let Err(error) = run("unmount", device)
        && !error.to_lowercase().contains("not mounted")
    {
        return Err(explain(error));
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
mod tests;

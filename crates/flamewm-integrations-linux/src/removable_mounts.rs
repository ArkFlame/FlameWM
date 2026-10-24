//! Remembered removable filesystems: the UUIDs the user mounted, so they are mounted again when
//! present. The session's `RemovableMountWorker` runs the remount passes; the explorer shares the
//! mountinfo/sysfs/udisks2 helpers here. This module owns the state file and the pure rules.
//!
//! Rules, per present removable filesystem:
//! - mounted now: remember it;
//! - unmounted, remembered, and not seen mounted at the previous pass: mount it;
//! - unmounted, remembered, and seen mounted at the previous pass: the user unmounted it, so forget it.
//! An explicit user eject or unmount forgets the UUID before the action runs.
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::device_watch::DeviceWatch;

/// Filesystem UUIDs are short ASCII tokens; anything else in the state file is dropped.
const MAX_UUID_LEN: usize = 64;

/// One present removable filesystem as the explorer sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub uuid: String,
    /// Kernel block name without `/dev/`, for example `sdb1`.
    pub block: String,
    pub removable: bool,
    pub mounted: bool,
}

/// What one pass should do. `mount` holds block names to hand to udisks2.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub remember: Vec<String>,
    pub forget: Vec<String>,
    pub mount: Vec<String>,
}

/// Remembered UUIDs (persisted) and the UUIDs seen mounted at the previous pass (session only).
#[derive(Debug, Default)]
pub struct Memory {
    pub known: Vec<String>,
    pub seen_mounted: BTreeSet<String>,
}

/// `$XDG_STATE_HOME/flamewm/removable-mounts`, else `~/.local/state/flamewm/removable-mounts`.
pub fn state_path() -> PathBuf {
    let base = env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"));
    base.join("flamewm").join("removable-mounts")
}

fn valid_uuid(uuid: &str) -> bool {
    !uuid.is_empty() && uuid.len() <= MAX_UUID_LEN && uuid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Parses the state file: one UUID per line, invalid lines and duplicates ignored.
pub fn parse(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim) {
        if valid_uuid(line) && !out.iter().any(|known| known == line) {
            out.push(line.to_owned());
        }
    }
    out
}

pub fn load(path: &Path) -> Vec<String> {
    parse(&fs::read_to_string(path).unwrap_or_default())
}

/// Writes the list through a temporary file, so a crash never leaves a truncated state file.
pub fn save(path: &Path, known: &[String]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = String::new();
    for uuid in known.iter().filter(|uuid| valid_uuid(uuid)) {
        text.push_str(uuid);
        text.push('\n');
    }
    let temp = path.with_extension("tmp");
    let mut file = fs::File::create(&temp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temp, path)
}

impl Memory {
    pub fn load(path: &Path) -> Self {
        Self { known: load(path), seen_mounted: BTreeSet::new() }
    }

    /// Adds `uuid` to the remembered list; returns true when the list changed.
    pub fn remember(&mut self, uuid: &str) -> bool {
        if !valid_uuid(uuid) || self.known.iter().any(|known| known == uuid) {
            return false;
        }
        self.known.push(uuid.to_owned());
        true
    }

    /// Removes `uuid` from the remembered list; returns true when it was there.
    pub fn forget(&mut self, uuid: &str) -> bool {
        let before = self.known.len();
        self.known.retain(|known| known != uuid);
        self.known.len() != before
    }

    /// Applies a pass result to the remembered list and records what is mounted now.
    pub fn apply(&mut self, plan: &Plan, devices: &[Device]) -> bool {
        let mut changed = false;
        for uuid in &plan.forget {
            changed |= self.forget(uuid);
        }
        for uuid in &plan.remember {
            changed |= self.remember(uuid);
        }
        self.seen_mounted = devices.iter().filter(|d| d.removable && d.mounted).map(|d| d.uuid.clone()).collect();
        changed
    }
}

/// Decides the mounts, remembers and forgets for one pass. Pure: no udisks, no filesystem access.
pub fn plan(memory: &Memory, devices: &[Device]) -> Plan {
    let mut plan = Plan::default();
    for device in devices.iter().filter(|d| d.removable && valid_uuid(&d.uuid)) {
        let known = memory.known.iter().any(|known| *known == device.uuid);
        if device.mounted {
            if !known && !plan.remember.contains(&device.uuid) {
                plan.remember.push(device.uuid.clone());
            }
        } else if known {
            if memory.seen_mounted.contains(&device.uuid) {
                if !plan.forget.contains(&device.uuid) {
                    plan.forget.push(device.uuid.clone());
                }
            } else if !plan.mount.contains(&device.block) {
                plan.mount.push(device.block.clone());
            }
        }
    }
    plan
}

/// Removes control characters and caps the length of a udisks2 error message.
fn bounded_message(raw: &str) -> String {
    raw.chars().filter(|c| !c.is_control()).take(300).collect::<String>().trim().to_owned()
}

/// Mount-table escapes used by `/proc/self/mountinfo` mount points.
pub fn unescape_mount(text: &str) -> String {
    text.replace("\\040", " ").replace("\\011", "\t").replace("\\012", "\n").replace("\\134", "\\")
}

/// Mounted block devices from `/proc/self/mountinfo` text: `(block_name, mount_point)` for every
/// `/dev/<name>` source, whatever the mount point. Field layout: id parent maj:min root mount_point
/// opts [optional...] - fstype source superopts.
pub fn mounted_block_devices(mountinfo: &str) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for line in mountinfo.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let Some(separator) = fields.iter().position(|field| *field == "-") else { continue };
        let (Some(mount), Some(source)) = (fields.get(4), fields.get(separator + 2)) else { continue };
        let Some(block_name) = source.strip_prefix("/dev/").filter(|name| !name.is_empty() && !name.contains('/')) else {
            continue;
        };
        let mount_point = PathBuf::from(unescape_mount(mount));
        if !out.iter().any(|(name, existing)| name == block_name && *existing == mount_point) {
            out.push((block_name.to_owned(), mount_point));
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
        Err(bounded_message(&String::from_utf8_lossy(&output.stderr)))
    }
}

/// Filesystem UUID and block name pairs from a `/dev/disk/by-uuid` directory (present filesystems only).
pub fn uuid_blocks(by_uuid: &Path) -> Vec<(String, String)> {
    let Ok(entries) = fs::read_dir(by_uuid) else { return Vec::new() };
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let uuid = entry.file_name().to_string_lossy().into_owned();
            let target = fs::read_link(entry.path()).ok()?;
            let block = target.file_name()?.to_string_lossy().into_owned();
            Some((uuid, block))
        })
        .collect()
}

pub fn by_uuid_dir() -> PathBuf {
    PathBuf::from("/dev/disk/by-uuid")
}

/// Removable filesystems present now, with their mount state. Pure: the caller supplies every input.
pub fn removable_devices(
    pairs: &[(String, String)],
    mounted: &[(String, PathBuf)],
    is_removable: impl Fn(&str) -> bool,
) -> Vec<Device> {
    pairs
        .iter()
        .map(|(uuid, block)| Device {
            uuid: uuid.clone(),
            block: block.clone(),
            removable: is_removable(block),
            mounted: mounted.iter().any(|(name, _)| name == block),
        })
        .collect()
}

/// One remount pass: reloads the state file (the explorer may have forgotten a UUID since the last
/// pass), applies the plan, saves when it changed, and asks udisks2 to mount what the plan lists.
/// Blocks on udisks2, so it runs on a background thread only.
pub fn remount_pass(memory: &Mutex<Memory>) {
    let Ok(mut memory) = memory.lock() else { return };
    let path = state_path();
    memory.known = load(&path);
    let mountinfo = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    let devices = removable_devices(&uuid_blocks(&by_uuid_dir()), &mounted_block_devices(&mountinfo), is_removable);
    let plan = plan(&memory, &devices);
    if memory.apply(&plan, &devices) {
        let _ = save(&path, &memory.known);
    }
    for block in &plan.mount {
        let _ = udisks_action("mount", &format!("/dev/{block}"));
    }
}

/// The session's removable-media worker. It runs one remount pass at start, then one per settled
/// device change, so remembered drives come back without the explorer open.
pub struct RemovableMountWorker {
    _watch: Option<DeviceWatch>,
}

impl RemovableMountWorker {
    pub fn start() -> Self {
        let memory = Arc::new(Mutex::new(Memory::load(&state_path())));
        let initial = Arc::clone(&memory);
        let _ = thread::Builder::new().name("removable-mounts".into()).spawn(move || remount_pass(&initial));
        let watch = DeviceWatch::spawn(move || remount_pass(&memory)).ok();
        Self { _watch: watch }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(uuid: &str, block: &str, removable: bool, mounted: bool) -> Device {
        Device { uuid: uuid.into(), block: block.into(), removable, mounted }
    }

    fn memory(known: &[&str], seen: &[&str]) -> Memory {
        Memory {
            known: known.iter().map(|u| (*u).to_owned()).collect(),
            seen_mounted: seen.iter().map(|u| (*u).to_owned()).collect(),
        }
    }

    #[test]
    fn known_unmounted_device_is_mounted_again() {
        let plan = plan(&memory(&["abc"], &[]), &[device("abc", "sdb1", true, false)]);
        assert_eq!(plan, Plan { remember: vec![], forget: vec![], mount: vec!["sdb1".into()] });
    }

    #[test]
    fn mounted_device_is_remembered_even_when_mounted_by_someone_else() {
        let plan = plan(&memory(&[], &[]), &[device("abc", "sdb1", true, true)]);
        assert_eq!(plan.remember, vec!["abc".to_owned()]);
        assert!(plan.mount.is_empty());
    }

    #[test]
    fn unmount_while_present_is_respected_and_forgotten() {
        let plan = plan(&memory(&["abc"], &["abc"]), &[device("abc", "sdb1", true, false)]);
        assert_eq!(plan.forget, vec!["abc".to_owned()]);
        assert!(plan.mount.is_empty());
    }

    #[test]
    fn unknown_and_internal_devices_are_never_touched() {
        let devices = [device("new", "sdc1", true, false), device("int", "nvme0n1p6", false, false)];
        let plan = plan(&memory(&["int"], &[]), &devices);
        assert_eq!(plan, Plan::default());
    }

    #[test]
    fn explicit_eject_forgets_so_reinsertion_does_not_mount() {
        let mut state = memory(&["abc"], &["abc"]);
        assert!(state.forget("abc"));
        let plan = plan(&state, &[device("abc", "sdb1", true, false)]);
        assert_eq!(plan, Plan::default());
    }

    #[test]
    fn user_mount_after_eject_is_remembered_again() {
        let mut state = memory(&[], &[]);
        assert!(state.remember("abc"));
        assert_eq!(state.known, vec!["abc".to_owned()]);
        assert!(!state.remember("abc"));
    }

    #[test]
    fn apply_updates_known_and_seen_for_the_next_pass() {
        let devices = [device("abc", "sdb1", true, true), device("old", "sdc1", true, false)];
        let mut state = memory(&["old"], &[]);
        let plan = plan(&state, &devices);
        assert_eq!(plan.mount, vec!["sdc1".to_owned()]);
        assert!(state.apply(&plan, &devices));
        assert_eq!(state.known, vec!["old".to_owned(), "abc".to_owned()]);
        assert_eq!(state.seen_mounted, BTreeSet::from(["abc".to_owned()]));
    }

    #[test]
    fn invalid_uuids_are_dropped_on_parse_and_duplicates_collapse() {
        let text = "abc-1\n\nbad uuid\n../etc\nabc-1\n  def  \n";
        assert_eq!(parse(text), vec!["abc-1".to_owned(), "def".to_owned()]);
        assert!(parse(&"x".repeat(MAX_UUID_LEN + 1)).is_empty());
    }

    #[test]
    fn state_path_is_under_the_flamewm_state_dir() {
        assert!(state_path().ends_with("flamewm/removable-mounts"));
    }
}

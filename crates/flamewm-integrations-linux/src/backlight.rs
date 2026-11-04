//! Panel backlight source for the brightness media keys and the brightness OSD.
//!
//! Reading is sysfs-owned (`/sys/class/backlight`). Writing prefers
//! `org.freedesktop.login1.Session.SetBrightness`, which is the only permitted path for an
//! unprivileged session on a stock host, and falls back to a direct sysfs write for hosts
//! where the device itself is writable. Brightness is read on demand, so no D-Bus signal
//! subscription or reactor watch is opened.

use std::path::{Path, PathBuf};
use std::time::Duration;

use dbus::Message;
use flamewm_api::system::{BrightnessSnapshot, ServiceAvailability};
use flamewm_api::{ErrorCode, FlameError, FlameResult};
use flamewm_dbus_reactor::{BusKind, BusPump};

pub const BACKLIGHT_ROOT: &str = "/sys/class/backlight";
pub const LOGIND_SERVICE: &str = "org.freedesktop.login1";
pub const LOGIND_SESSION_PATH: &str = "/org/freedesktop/login1/session/auto";
pub const LOGIND_SESSION_INTERFACE: &str = "org.freedesktop.login1.Session";
const LOGIND_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BacklightDevice {
    pub name: String,
    /// `/sys/class/backlight/<name>/type`; `raw` marks a hardware-driven panel.
    pub kind: String,
    pub max_brightness: u32,
}

/// Picks the panel device: `type == "raw"` wins over any other type, else the first entry.
#[must_use]
pub fn select_device(devices: &[BacklightDevice]) -> Option<&BacklightDevice> {
    devices
        .iter()
        .find(|device| device.kind == "raw")
        .or_else(|| devices.first())
}

/// Backlight percentage from a raw sysfs value, rounded and clamped to `0..=100`.
/// A zero maximum (device without a scale) reports 0.
#[must_use]
pub fn percent_from_raw(raw: u32, max: u32) -> u8 {
    if max == 0 {
        return 0;
    }
    let max = u64::from(max);
    let raw = u64::from(raw).min(max);
    let percent = (raw * 100 + max / 2) / max;
    percent.min(100) as u8
}

/// Raw sysfs value for a percentage, rounded to the nearest step and clamped to `max`.
/// A zero maximum (device without a scale) writes 0.
#[must_use]
pub fn raw_from_percent(percent: u8, max: u32) -> u32 {
    if max == 0 {
        return 0;
    }
    let max = u64::from(max);
    let raw = (u64::from(percent.min(100)) * max + 50) / 100;
    raw.min(max) as u32
}

/// Owns the discovered panel device and the `logind` write connection.
pub struct BacklightProvider {
    pump: Option<BusPump>,
    root: PathBuf,
    device: Option<BacklightDevice>,
    snapshot: BrightnessSnapshot,
}

impl BacklightProvider {
    /// An absent backlight device, system bus, or both degrades to `Unavailable`; the desktop
    /// keeps running and the brightness keys become no-ops.
    #[must_use]
    pub fn connect() -> Self {
        Self::at(Path::new(BACKLIGHT_ROOT).to_path_buf())
    }

    /// No signal subscription exists for brightness, so there is no watch to open.
    pub fn start(&mut self) {}

    /// Nothing to release; the last observation is kept so a restart does not churn the fence.
    pub fn stop(&mut self) {}

    /// Reads the current device state. The generation advances only when the observed state
    /// (availability, percent) changes, so repeated reads stay fence-stable.
    pub fn snapshot(&mut self) -> BrightnessSnapshot {
        let observed = self.observe();
        if (observed.availability, observed.percent)
            != (self.snapshot.availability, self.snapshot.percent)
        {
            self.snapshot.generation = self.snapshot.generation.saturating_add(1);
        }
        self.snapshot.availability = observed.availability;
        self.snapshot.percent = observed.percent;
        self.snapshot
    }

    /// Writes `percent` through `logind`, falling back to sysfs; the error is returned only
    /// when both paths fail. `generation` must match the last observation.
    pub fn set_percent(&mut self, percent: u8, generation: u64) -> FlameResult<()> {
        if generation != self.snapshot().generation {
            return Err(FlameError::stale("stale backlight generation"));
        }
        let Some(device) = self.device.as_ref() else {
            return Err(FlameError::unavailable("no backlight device is present"));
        };
        let raw = raw_from_percent(percent, device.max_brightness);
        let logind_error = match self.write_logind(&device.name, raw) {
            Ok(()) => return Ok(()),
            Err(error) => error.message,
        };
        let Err(sysfs_error) = write_sysfs(&self.root, &device.name, raw) else {
            return Ok(());
        };
        Err(FlameError::unavailable(format!(
            "backlight write failed: logind: {logind_error}; sysfs: {}",
            sysfs_error.message
        )))
    }

    fn at(root: PathBuf) -> Self {
        let mut provider = Self {
            pump: BusPump::connect(BusKind::System).ok(),
            device: discover(&root),
            root,
            snapshot: BrightnessSnapshot::default(),
        };
        provider.snapshot = provider.observe();
        provider
    }

    fn observe(&self) -> BrightnessSnapshot {
        let observed = match &self.device {
            Some(device) => read_u32(&brightness_path(&self.root, &device.name)).map(|raw| {
                (
                    ServiceAvailability::Available,
                    percent_from_raw(raw, device.max_brightness),
                )
            }),
            None => None,
        };
        let (availability, percent) = observed.unwrap_or((ServiceAvailability::Unavailable, 0));
        BrightnessSnapshot {
            availability,
            generation: self.snapshot.generation,
            percent,
        }
    }

    /// `Session.SetBrightness(s interface, s device, u value)` on the auto session object.
    fn write_logind(&self, device: &str, raw: u32) -> FlameResult<()> {
        let Some(pump) = self.pump.as_ref() else {
            return Err(FlameError::unavailable("system bus is unavailable"));
        };
        let call = Message::new_method_call(
            LOGIND_SERVICE,
            LOGIND_SESSION_PATH,
            LOGIND_SESSION_INTERFACE,
            "SetBrightness",
        )
        .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?
        .append3("backlight", device.to_owned(), raw);
        pump.send_with_reply(call, LOGIND_TIMEOUT)?;
        Ok(())
    }
}

fn brightness_path(root: &Path, device: &str) -> PathBuf {
    root.join(device).join("brightness")
}

/// Discovers the panel device. Entries are sorted first because `read_dir` order is arbitrary
/// and the "first device" tie-break must be stable across runs.
fn discover(root: &Path) -> Option<BacklightDevice> {
    let mut names = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect::<Vec<_>>();
    names.sort();
    let devices = names
        .iter()
        .filter_map(|name| {
            let dir = root.join(name);
            Some(BacklightDevice {
                name: name.clone(),
                kind: read_text(&dir.join("type"))?,
                max_brightness: read_text(&dir.join("max_brightness"))?.parse().ok()?,
            })
        })
        .collect::<Vec<_>>();
    select_device(&devices).cloned()
}

fn read_text(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}

fn read_u32(path: &Path) -> Option<u32> {
    read_text(path)?.parse().ok()
}

fn write_sysfs(root: &Path, device: &str, raw: u32) -> FlameResult<()> {
    let path = brightness_path(root, device);
    std::fs::write(&path, raw.to_string()).map_err(|error| {
        FlameError::unavailable(format!("cannot write {}: {error}", path.display()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, kind: &str, max_brightness: u32) -> BacklightDevice {
        BacklightDevice {
            name: name.to_owned(),
            kind: kind.to_owned(),
            max_brightness,
        }
    }

    #[test]
    fn raw_sysfs_device_is_preferred_over_firmware_or_platform() {
        let firmware = device("acpi_video0", "firmware", 100);
        let raw = device("intel_backlight", "raw", 48_000);
        let platform = device("amdgpu_bl0", "platform", 255);
        assert_eq!(
            select_device(&[firmware.clone(), raw.clone(), platform.clone()])
                .map(|d| d.name.clone()),
            Some(raw.name)
        );
        assert_eq!(
            select_device(&[firmware, platform]).map(|d| d.name.clone()),
            Some("acpi_video0".to_owned())
        );
        assert_eq!(select_device(&[]), None);
    }

    #[test]
    fn percent_and_raw_round_trip_on_the_panel_scale() {
        assert_eq!(percent_from_raw(0, 48_000), 0);
        assert_eq!(percent_from_raw(33_600, 48_000), 70);
        assert_eq!(percent_from_raw(48_000, 48_000), 100);
        assert_eq!(raw_from_percent(0, 48_000), 0);
        assert_eq!(raw_from_percent(70, 48_000), 33_600);
        assert_eq!(raw_from_percent(100, 48_000), 48_000);
        for percent in 0..=100 {
            assert_eq!(
                percent_from_raw(raw_from_percent(percent, 48_000), 48_000),
                percent
            );
        }
    }

    #[test]
    fn percent_and_raw_are_clamped_and_a_zero_maximum_is_zero() {
        assert_eq!(percent_from_raw(99_999, 48_000), 100);
        assert_eq!(raw_from_percent(200, 48_000), 48_000);
        assert_eq!(percent_from_raw(7, 0), 0);
        assert_eq!(raw_from_percent(7, 0), 0);
        assert_eq!(percent_from_raw(1, 1), 100);
        assert_eq!(raw_from_percent(100, 1), 1);
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("flamewm-backlight-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn panel(root: &Path, name: &str, kind: &str, max: u32, raw: u32) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("device dir");
        std::fs::write(dir.join("type"), kind).expect("type");
        std::fs::write(dir.join("max_brightness"), max.to_string()).expect("max");
        std::fs::write(dir.join("brightness"), raw.to_string()).expect("brightness");
    }

    #[test]
    fn generation_advances_only_when_the_observed_percent_changes() {
        let root = scratch("generation");
        panel(&root, "intel_backlight", "raw", 48_000, 24_000);
        let mut provider = BacklightProvider::at(root.clone());
        let first = provider.snapshot();
        assert_eq!(first.availability, ServiceAvailability::Available);
        assert_eq!(first.percent, 50);
        assert_eq!(first.generation, 0);
        assert_eq!(provider.snapshot().generation, 0);

        std::fs::write(root.join("intel_backlight").join("brightness"), "36000").expect("write");
        let changed = provider.snapshot();
        assert_eq!(changed.percent, 75);
        assert_eq!(changed.generation, 1);
        assert_eq!(provider.snapshot().generation, 1);
    }

    #[test]
    fn absent_device_is_unavailable_and_rejects_a_write() {
        let root = scratch("absent");
        let mut provider = BacklightProvider::at(root);
        assert_eq!(
            provider.snapshot().availability,
            ServiceAvailability::Unavailable
        );
        let generation = provider.snapshot().generation;
        let error = provider
            .set_percent(40, generation)
            .expect_err("no device must refuse the write");
        assert_eq!(error.code, ErrorCode::Unavailable);
    }

    #[test]
    fn stale_generation_is_rejected_before_any_write() {
        let root = scratch("stale");
        panel(&root, "intel_backlight", "raw", 48_000, 24_000);
        let mut provider = BacklightProvider::at(root.clone());
        let generation = provider.snapshot().generation;
        let error = provider
            .set_percent(10, generation + 1)
            .expect_err("stale generation must be fenced");
        assert_eq!(error.code, ErrorCode::StaleRevision);
        assert_eq!(
            std::fs::read_to_string(root.join("intel_backlight").join("brightness"))
                .expect("brightness"),
            "24000"
        );
    }

    #[test]
    fn sysfs_fallback_writes_the_scaled_raw_value_when_logind_is_unavailable() {
        let root = scratch("fallback");
        panel(&root, "intel_backlight", "raw", 48_000, 24_000);
        let mut provider = BacklightProvider {
            pump: None,
            device: discover(&root),
            root: root.clone(),
            snapshot: BrightnessSnapshot::default(),
        };
        let generation = provider.snapshot().generation;
        provider.set_percent(20, generation).expect("sysfs write");
        assert_eq!(
            std::fs::read_to_string(root.join("intel_backlight").join("brightness"))
                .expect("brightness"),
            "9600"
        );
        assert_eq!(provider.snapshot().percent, 20);
    }
}

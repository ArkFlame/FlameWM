//! `UPower` `DisplayDevice` source and CPU-usage sampling for the battery menu.
//!
//! The system-bus connection is fail-soft: when `UPower` or the system bus is absent the provider
//! publishes `ServiceAvailability::Unavailable`. The sysfs/uevent fallback is not implemented.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

use dbus::arg::messageitem::MessageItem;
use dbus::{Message, MessageType};
use flamewm_api::ports::FdEvents;
use flamewm_api::system::{PowerSnapshot, PowerState, ServiceAvailability};
use flamewm_api::{FlameError, FlameResult};
use flamewm_dbus_reactor::{BusKind, BusPump, BusWatch, MessageDisposition};

pub const UPOWER_SERVICE: &str = "org.freedesktop.UPower";
pub const UPOWER_DISPLAY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
pub const UPOWER_DEVICE_INTERFACE: &str = "org.freedesktop.UPower.Device";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const PROPERTIES_CHANGED: &str = "PropertiesChanged";
const GET_ALL: &str = "GetAll";
const PROC_STAT: &str = "/proc/stat";

/// Maps `UPower` `State`: 1 charging, 2 discharging, 3 empty, 4 fully charged. Pending states (5, 6)
/// and unknown values are reported as `Unknown` rather than guessed.
#[must_use]
pub fn power_state_from_upower(value: u32) -> PowerState {
    match value {
        1 => PowerState::Charging,
        2 => PowerState::Discharging,
        3 => PowerState::Empty,
        4 => PowerState::Full,
        _ => PowerState::Unknown,
    }
}

fn is_flow_state(state: PowerState) -> bool {
    matches!(state, PowerState::Charging | PowerState::Discharging)
}

/// Battery energy flow in integer milliwatts. Only a finite, positive `EnergyRate` while charging or
/// discharging is reported; zero, negative, non-finite, or other states yield `None`.
#[must_use]
pub fn power_rate_mw(state: PowerState, watts: f64) -> Option<u32> {
    if !is_flow_state(state) || !watts.is_finite() || watts <= 0.0 {
        return None;
    }
    Some((watts * 1000.0).round() as u32)
}

/// Rounds a `UPower` percentage to 0..=100; NaN is treated as 0.
#[must_use]
pub fn clamp_percent(value: f64) -> u8 {
    if value.is_nan() {
        return 0;
    }
    // Clamped to 0..=100 before the cast, so the conversion is exact.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let percent = value.clamp(0.0, 100.0).round() as u8;
    percent
}

/// Only the aggregate display device represents the system battery. Peripheral batteries
/// (for example `hidpp_battery_*` mice) live under their own object paths and are ignored.
#[must_use]
pub fn is_system_power_source(object_path: &str) -> bool {
    object_path == UPOWER_DISPLAY_DEVICE
}

/// Cumulative `/proc/stat` CPU counters: `busy` excludes idle and iowait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTicks {
    pub busy: u64,
    pub total: u64,
}

/// Parses the aggregate `cpu` line. Fields are user, nice, system, idle, iowait, irq, softirq,
/// steal; `guest` fields are already counted in user and nice, so they are excluded.
#[must_use]
pub fn parse_cpu_ticks(line: &str) -> Option<CpuTicks> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let values = fields
        .take(8)
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if values.len() < 4 {
        return None;
    }
    let total = values
        .iter()
        .fold(0_u64, |sum, value| sum.saturating_add(*value));
    let idle = values[3].saturating_add(values.get(4).copied().unwrap_or(0));
    Some(CpuTicks {
        busy: total.saturating_sub(idle),
        total,
    })
}

/// Busy percentage between two counter reads. The first sample and any counter regression
/// (wrap or reset) yield `None` and restart the baseline; an empty interval yields `None`.
#[must_use]
pub fn cpu_percent(previous: Option<CpuTicks>, next: CpuTicks) -> Option<u8> {
    let previous = previous?;
    let busy = next.busy.checked_sub(previous.busy)?;
    let total = next.total.checked_sub(previous.total)?;
    if total == 0 {
        return None;
    }
    let percent = busy.saturating_mul(100).saturating_add(total / 2) / total;
    Some(u8::try_from(percent.min(100)).unwrap_or(100))
}

/// Holds the last counter read so each menu tick yields a delta against the previous one.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CpuSampler {
    previous: Option<CpuTicks>,
}

impl CpuSampler {
    /// Records `next` as the new baseline and returns the busy percentage since the last read.
    pub fn sample(&mut self, next: CpuTicks) -> Option<u8> {
        let percent = cpu_percent(self.previous, next);
        self.previous = Some(next);
        percent
    }

    pub fn reset(&mut self) {
        self.previous = None;
    }
}

/// Reads the aggregate CPU counters from `/proc/stat`.
#[must_use]
pub fn read_cpu_ticks() -> Option<CpuTicks> {
    let text = std::fs::read_to_string(PROC_STAT).ok()?;
    parse_cpu_ticks(text.lines().next()?)
}

enum Event {
    Reply {
        serial: u32,
        is_error: bool,
        items: Vec<MessageItem>,
    },
    Changed {
        items: Vec<MessageItem>,
    },
}

/// Fail-soft `UPower` `DisplayDevice` provider on the system bus.
pub struct PowerProvider {
    pump: Option<BusPump>,
    dormant: UnixStream,
    _peer: UnixStream,
    pending_get_all: Option<u32>,
    snapshot: PowerSnapshot,
    cpu: CpuSampler,
    menu_open: bool,
    started: bool,
}

impl PowerProvider {
    /// An unreachable system bus or absent `UPower` degrades to `Unavailable`; only socketpair
    /// creation failure is an error.
    pub fn connect() -> FlameResult<Self> {
        let (peer, dormant) = UnixStream::pair()
            .map_err(|io| FlameError::unavailable(format!("socketpair failed: {io}")))?;
        let pump = BusPump::connect(BusKind::System).ok();
        let snapshot = PowerSnapshot {
            available: if pump.is_some() {
                ServiceAvailability::Unknown
            } else {
                ServiceAvailability::Unavailable
            },
            ..PowerSnapshot::default()
        };
        Ok(Self {
            pump,
            dormant,
            _peer: peer,
            pending_get_all: None,
            snapshot,
            cpu: CpuSampler::default(),
            menu_open: false,
            started: false,
        })
    }

    /// Subscribes to `DisplayDevice` property changes and requests one full property read.
    /// Send failures degrade to `Unavailable` instead of failing the desktop.
    pub fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let Some(pump) = self.pump.as_ref() else {
            return;
        };
        let rule = format!(
            "type='signal',sender='{UPOWER_SERVICE}',interface='{PROPERTIES_INTERFACE}',path='{UPOWER_DISPLAY_DEVICE}',member='{PROPERTIES_CHANGED}'"
        );
        let add_match = Message::new_method_call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "AddMatch",
        )
        .map(|message| message.append1(rule));
        let get_all = Message::new_method_call(
            UPOWER_SERVICE,
            UPOWER_DISPLAY_DEVICE,
            PROPERTIES_INTERFACE,
            GET_ALL,
        )
        .map(|message| message.append1(UPOWER_DEVICE_INTERFACE.to_owned()));
        let sent = match (add_match, get_all) {
            (Ok(add_match), Ok(get_all)) => pump.send(add_match).and_then(|_| pump.send(get_all)),
            _ => Err(FlameError::invalid("UPower message construction failed")),
        };
        match sent {
            Ok(serial) => self.pending_get_all = Some(serial),
            Err(_) => self.mark_unavailable(),
        }
    }

    pub fn stop(&mut self) {
        self.started = false;
        self.pending_get_all = None;
        self.clear_cpu();
    }

    #[must_use]
    pub fn watch(&self) -> BusWatch {
        match &self.pump {
            Some(pump) => pump.watch(),
            None => BusWatch {
                fd: self.dormant.as_raw_fd(),
                events: FdEvents::READABLE,
            },
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> PowerSnapshot {
        self.snapshot
    }

    /// Drains one bounded reactor turn and applies `UPower` replies and property changes.
    pub fn on_ready(&mut self) -> FlameResult<usize> {
        let Some(pump) = self.pump.as_ref() else {
            return Ok(0);
        };
        let mut events = Vec::new();
        let count = pump.on_ready(|message| {
            if let Some(event) = parse_message(message) {
                events.push(event);
                return MessageDisposition::Handled;
            }
            MessageDisposition::Unhandled
        })?;
        for event in events {
            match event {
                Event::Reply {
                    serial,
                    is_error,
                    items,
                } => {
                    if self.pending_get_all != Some(serial) {
                        continue;
                    }
                    self.pending_get_all = None;
                    if is_error {
                        self.mark_unavailable();
                    } else {
                        self.apply_properties(items.first());
                    }
                }
                Event::Changed { items } => self.apply_properties(items.get(1)),
            }
        }
        Ok(count)
    }

    /// Enables or disables the 1s CPU sampling lifecycle. Closing clears the published sample.
    pub fn set_battery_menu_open(&mut self, open: bool) {
        self.menu_open = open;
        if !open {
            self.clear_cpu();
        }
    }

    fn clear_cpu(&mut self) {
        self.cpu.reset();
        self.snapshot.cpu_percent = None;
    }

    /// One CPU sample tick, called only by the reactor-owned timer while the menu is open.
    /// Returns whether the published snapshot changed.
    pub fn sample_cpu(&mut self) -> bool {
        if !self.menu_open {
            return false;
        }
        let percent = read_cpu_ticks().and_then(|ticks| self.cpu.sample(ticks));
        if self.snapshot.cpu_percent == percent {
            return false;
        }
        self.snapshot.cpu_percent = percent;
        true
    }

    fn mark_unavailable(&mut self) {
        self.snapshot = PowerSnapshot {
            available: ServiceAvailability::Unavailable,
            cpu_percent: self.snapshot.cpu_percent,
            ..PowerSnapshot::default()
        };
    }

    fn apply_properties(&mut self, item: Option<&MessageItem>) {
        let Some(MessageItem::Dict(dict)) = item else {
            return;
        };
        let mut snapshot = self.snapshot;
        let mut energy_rate = None;
        let mut state_changed = false;
        for (key, value) in dict.iter() {
            let (MessageItem::Str(key), value) = (key, unwrap_variant(value)) else {
                continue;
            };
            match (key.as_str(), value) {
                ("Percentage", MessageItem::Double(value)) => {
                    snapshot.percent = clamp_percent(*value);
                }
                ("State", MessageItem::UInt32(value)) => {
                    snapshot.state = power_state_from_upower(*value);
                    state_changed = true;
                }
                ("IsPresent", MessageItem::Bool(value)) => snapshot.present = *value,
                ("EnergyRate", MessageItem::Double(value)) => energy_rate = Some(*value),
                _ => {}
            }
        }
        if let Some(rate) = energy_rate {
            snapshot.power_rate_mw = power_rate_mw(snapshot.state, rate);
        } else if state_changed || !is_flow_state(snapshot.state) {
            snapshot.power_rate_mw = None;
        }
        snapshot.available = ServiceAvailability::Available;
        self.snapshot = snapshot;
    }
}

fn parse_message(message: &dbus::Message) -> Option<Event> {
    if message.msg_type() == MessageType::MethodReturn || message.msg_type() == MessageType::Error {
        return message.get_reply_serial().map(|serial| Event::Reply {
            serial,
            is_error: message.msg_type() == MessageType::Error,
            items: message.get_items(),
        });
    }
    if message.msg_type() != MessageType::Signal
        || message.interface().as_deref() != Some(PROPERTIES_INTERFACE)
        || message.member().as_deref() != Some(PROPERTIES_CHANGED)
        || !is_system_power_source(message.path().as_deref().unwrap_or_default())
    {
        return None;
    }
    let items = message.get_items();
    let is_device =
        matches!(items.first(), Some(MessageItem::Str(name)) if name == UPOWER_DEVICE_INTERFACE);
    is_device.then_some(Event::Changed { items })
}

fn unwrap_variant(item: &MessageItem) -> &MessageItem {
    match item {
        MessageItem::Variant(value) => unwrap_variant(value),
        item => item,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upower_state_maps_known_values_and_marks_pending_unknown() {
        assert_eq!(power_state_from_upower(1), PowerState::Charging);
        assert_eq!(power_state_from_upower(2), PowerState::Discharging);
        assert_eq!(power_state_from_upower(3), PowerState::Empty);
        assert_eq!(power_state_from_upower(4), PowerState::Full);
        assert_eq!(power_state_from_upower(5), PowerState::Unknown);
        assert_eq!(power_state_from_upower(6), PowerState::Unknown);
        assert_eq!(power_state_from_upower(99), PowerState::Unknown);
    }

    #[test]
    fn percent_is_clamped_and_rounded() {
        assert_eq!(clamp_percent(-3.0), 0);
        assert_eq!(clamp_percent(f64::NAN), 0);
        assert_eq!(clamp_percent(86.6), 87);
        assert_eq!(clamp_percent(100.4), 100);
        assert_eq!(clamp_percent(250.0), 100);
    }

    #[test]
    fn only_display_device_is_the_system_power_source() {
        assert!(is_system_power_source(UPOWER_DISPLAY_DEVICE));
        assert!(!is_system_power_source(
            "/org/freedesktop/UPower/devices/mouse_hidpp_battery_1"
        ));
        assert!(!is_system_power_source(
            "/org/freedesktop/UPower/devices/battery_BAT0"
        ));
        assert!(!is_system_power_source(""));
    }

    #[test]
    fn cpu_line_excludes_idle_and_iowait_from_busy() {
        let ticks = parse_cpu_ticks("cpu  100 0 50 800 20 5 5 0 0 0").expect("cpu line");
        assert_eq!(
            ticks,
            CpuTicks {
                busy: 160,
                total: 980
            }
        );
        assert_eq!(parse_cpu_ticks("cpu0 1 2 3 4"), None);
        assert_eq!(parse_cpu_ticks("cpu 1 2 3"), None);
        assert_eq!(parse_cpu_ticks("cpu 1 x 3 4"), None);
    }

    #[test]
    fn first_sample_is_none_and_delta_is_busy_share() {
        let first = CpuTicks {
            busy: 100,
            total: 1000,
        };
        let second = CpuTicks {
            busy: 150,
            total: 1200,
        };
        assert_eq!(cpu_percent(None, first), None);
        assert_eq!(cpu_percent(Some(first), second), Some(25));
        let mut sampler = CpuSampler::default();
        assert_eq!(sampler.sample(first), None);
        assert_eq!(sampler.sample(second), Some(25));
    }

    #[test]
    fn counter_wrap_or_empty_interval_yields_no_sample() {
        let high = CpuTicks {
            busy: 500,
            total: 2000,
        };
        let wrapped = CpuTicks {
            busy: 10,
            total: 100,
        };
        assert_eq!(cpu_percent(Some(high), wrapped), None);
        assert_eq!(cpu_percent(Some(high), high), None);
        let mut sampler = CpuSampler::default();
        let _ = sampler.sample(high);
        assert_eq!(sampler.sample(wrapped), None);
        assert_eq!(
            sampler.sample(CpuTicks {
                busy: 20,
                total: 300
            }),
            Some(5)
        );
    }

    #[test]
    fn cpu_percent_is_bounded_to_100() {
        let previous = CpuTicks { busy: 0, total: 0 };
        let next = CpuTicks {
            busy: 10,
            total: 10,
        };
        assert_eq!(cpu_percent(Some(previous), next), Some(100));
    }

    #[test]
    fn property_map_keys_are_unwrapped_from_variants() {
        let mut provider = test_provider();
        let dict = props(vec![
            ("Percentage", MessageItem::Double(86.7)),
            ("State", MessageItem::UInt32(4)),
            ("IsPresent", MessageItem::Bool(true)),
        ]);
        provider.apply_properties(Some(&dict));
        let snapshot = provider.snapshot();
        assert_eq!(snapshot.available, ServiceAvailability::Available);
        assert_eq!(snapshot.percent, 87);
        assert_eq!(snapshot.state, PowerState::Full);
        assert!(snapshot.present);
    }

    #[test]
    fn sampling_is_noop_while_battery_menu_is_closed() {
        let mut provider = PowerProvider::connect().expect("provider");
        assert_eq!(provider.snapshot().cpu_percent, None);
        assert!(!provider.sample_cpu());
        assert!(!provider.sample_cpu());
        assert_eq!(provider.snapshot().cpu_percent, None);
    }

    #[test]
    fn menu_close_clears_cpu_and_closed_menu_never_samples() {
        let mut provider = PowerProvider::connect().expect("provider");
        provider.snapshot.cpu_percent = Some(42);
        assert!(!provider.sample_cpu());
        provider.set_battery_menu_open(false);
        assert_eq!(provider.snapshot().cpu_percent, None);
    }

    fn test_provider() -> PowerProvider {
        PowerProvider {
            pump: None,
            dormant: UnixStream::pair().expect("pair").0,
            _peer: UnixStream::pair().expect("pair").0,
            pending_get_all: None,
            snapshot: PowerSnapshot::default(),
            cpu: CpuSampler::default(),
            menu_open: false,
            started: false,
        }
    }

    fn props(entries: Vec<(&str, MessageItem)>) -> MessageItem {
        MessageItem::new_dict(
            entries
                .into_iter()
                .map(|(key, value)| {
                    (
                        MessageItem::Str(key.to_owned()),
                        MessageItem::Variant(Box::new(value)),
                    )
                })
                .collect(),
        )
        .expect("dict")
    }

    fn rate_after(state: u32, rate: Option<f64>) -> Option<u32> {
        let mut provider = test_provider();
        let mut entries = vec![("State", MessageItem::UInt32(state))];
        if let Some(rate) = rate {
            entries.push(("EnergyRate", MessageItem::Double(rate)));
        }
        provider.apply_properties(Some(&props(entries)));
        provider.snapshot().power_rate_mw
    }

    const DISCHARGING: u32 = 2;
    const CHARGING: u32 = 1;

    #[test]
    fn energy_rate_absent_is_none() {
        assert_eq!(rate_after(DISCHARGING, None), None);
    }

    #[test]
    fn energy_rate_zero_is_none_never_zero_watts() {
        assert_eq!(rate_after(DISCHARGING, Some(0.0)), None);
    }

    #[test]
    fn energy_rate_discharging_converts_watts_to_milliwatts() {
        assert_eq!(rate_after(DISCHARGING, Some(12.34)), Some(12340));
    }

    #[test]
    fn energy_rate_charging_is_reported() {
        assert_eq!(rate_after(CHARGING, Some(5.5)), Some(5500));
    }

    #[test]
    fn energy_rate_non_finite_is_none() {
        assert_eq!(rate_after(DISCHARGING, Some(f64::NAN)), None);
        assert_eq!(rate_after(DISCHARGING, Some(f64::INFINITY)), None);
        assert_eq!(rate_after(DISCHARGING, Some(f64::NEG_INFINITY)), None);
    }

    #[test]
    fn energy_rate_negative_is_none() {
        assert_eq!(rate_after(DISCHARGING, Some(-3.0)), None);
    }

    #[test]
    fn energy_rate_ignored_when_full() {
        assert_eq!(rate_after(4, Some(12.0)), None);
    }

    #[test]
    fn energy_rate_ignored_when_state_unknown() {
        assert_eq!(rate_after(0, Some(12.0)), None);
    }

    #[test]
    fn energy_rate_ignored_without_battery() {
        let mut provider = test_provider();
        provider.apply_properties(Some(&props(vec![
            ("IsPresent", MessageItem::Bool(false)),
            ("State", MessageItem::UInt32(0)),
            ("EnergyRate", MessageItem::Double(0.0)),
        ])));
        assert_eq!(provider.snapshot().power_rate_mw, None);
    }

    #[test]
    fn energy_rate_follows_properties_changed_updates() {
        let mut provider = test_provider();
        provider.apply_properties(Some(&props(vec![
            ("State", MessageItem::UInt32(DISCHARGING)),
            ("EnergyRate", MessageItem::Double(12.34)),
        ])));
        assert_eq!(provider.snapshot().power_rate_mw, Some(12340));

        provider.apply_properties(Some(&props(vec![(
            "EnergyRate",
            MessageItem::Double(15.0),
        )])));
        assert_eq!(provider.snapshot().power_rate_mw, Some(15000));

        provider.apply_properties(Some(&props(vec![("State", MessageItem::UInt32(4))])));
        assert_eq!(provider.snapshot().power_rate_mw, None);
    }
}

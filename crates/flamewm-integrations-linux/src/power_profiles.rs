//! `net.hadess.PowerProfiles` facade on the system bus, with `org.freedesktop.UPower.PowerProfiles`
//! as the fallback service name.
//!
//! Event-driven: `PropertiesChanged` drives updates and `refresh` performs one `GetAll` (on open).
//! There is no polling timer. When neither service answers the snapshot stays unavailable and the
//! tile must stay hidden.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

use dbus::arg::messageitem::{MessageItem, MessageItemDict};
use dbus::{Message, MessageType};
use flamewm_api::ports::FdEvents;
use flamewm_api::{FlameError, FlameResult};
use flamewm_dbus_reactor::{BusKind, BusPump, BusWatch, MessageDisposition};

/// Service names probed in order; the first that answers `GetAll` owns the profile state.
pub const PROFILE_SERVICES: [&str; 2] = [
    "net.hadess.PowerProfiles",
    "org.freedesktop.UPower.PowerProfiles",
];
pub const PROFILE_PATH: &str = "/net/hadess/PowerProfiles";
pub const PROFILE_INTERFACE: &str = "net.hadess.PowerProfiles";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const PROPERTIES_CHANGED: &str = "PropertiesChanged";
const GET_ALL: &str = "GetAll";
const SET: &str = "Set";
const ACTIVE_PROFILE: &str = "ActiveProfile";
const PROFILES: &str = "Profiles";
const PROFILE_KEY: &str = "Profile";

/// The three wire profiles. Unknown names reported by a service are ignored, never guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerProfile {
    PowerSaver,
    Balanced,
    Performance,
}

impl PowerProfile {
    pub const ALL: [Self; 3] = [Self::PowerSaver, Self::Balanced, Self::Performance];

    /// Wire name used by both services.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            Self::PowerSaver => "power-saver",
            Self::Balanced => "balanced",
            Self::Performance => "performance",
        }
    }

    #[must_use]
    pub fn from_wire(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|profile| profile.wire() == name)
    }
}

/// Published state. `profiles` keeps service order with duplicates and unknown names removed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PowerProfilesSnapshot {
    pub available: bool,
    pub active: Option<PowerProfile>,
    pub profiles: Vec<PowerProfile>,
}

impl PowerProfilesSnapshot {
    /// The control is shown only when a service answered and advertised at least one profile.
    #[must_use]
    pub fn tile_visible(&self) -> bool {
        self.available && !self.profiles.is_empty()
    }
}

/// Unwraps nested `v` variants so payloads parse regardless of how a service wrapped them.
fn unwrap_variant(item: &MessageItem) -> &MessageItem {
    match item {
        MessageItem::Variant(value) => unwrap_variant(value),
        item => item,
    }
}

/// Reads one `{sv}` value by key, unwrapping variants.
fn dict_value<'a>(dict: &'a MessageItemDict, key: &str) -> Option<&'a MessageItem> {
    dict.iter().find_map(|(k, v)| match k {
        MessageItem::Str(name) if name == key => Some(unwrap_variant(v)),
        _ => None,
    })
}

/// Parses the `ActiveProfile` string. Unknown or non-string values yield `None`.
#[must_use]
pub fn parse_active_profile(item: &MessageItem) -> Option<PowerProfile> {
    match unwrap_variant(item) {
        MessageItem::Str(name) => PowerProfile::from_wire(name),
        _ => None,
    }
}

/// Parses the `Profiles` property (`aa{sv}`; each entry carries a `Profile` string). Malformed
/// entries and unknown names are skipped; duplicates keep their first position.
#[must_use]
pub fn parse_profiles(item: &MessageItem) -> Vec<PowerProfile> {
    let Ok(entries) = <&Vec<MessageItem>>::try_from(unwrap_variant(item)) else {
        return Vec::new();
    };
    let mut profiles = Vec::new();
    for entry in entries {
        let MessageItem::Dict(dict) = unwrap_variant(entry) else {
            continue;
        };
        let Some(MessageItem::Str(name)) = dict_value(dict, PROFILE_KEY) else {
            continue;
        };
        if let Some(profile) = PowerProfile::from_wire(name)
            && !profiles.contains(&profile)
        {
            profiles.push(profile);
        }
    }
    profiles
}

/// Serialises the `Set(interface, property, variant)` arguments for `ActiveProfile`.
#[must_use]
pub fn set_active_profile_items(profile: PowerProfile) -> [MessageItem; 3] {
    [
        MessageItem::Str(PROFILE_INTERFACE.to_owned()),
        MessageItem::Str(ACTIVE_PROFILE.to_owned()),
        MessageItem::Variant(Box::new(MessageItem::Str(profile.wire().to_owned()))),
    ]
}

/// Applies one `{sv}` property dict (from `GetAll` or `PropertiesChanged`) to the snapshot.
/// Returns whether any recognised property was present.
pub fn apply_property_dict(snapshot: &mut PowerProfilesSnapshot, dict: &MessageItemDict) -> bool {
    let mut recognised = false;
    if let Some(item) = dict_value(dict, ACTIVE_PROFILE) {
        snapshot.active = parse_active_profile(item);
        recognised = true;
    }
    if let Some(item) = dict_value(dict, PROFILES) {
        snapshot.profiles = parse_profiles(item);
        recognised = true;
    }
    recognised
}

/// Extracts the changed-properties dict from `PropertiesChanged(iface, changed{sv}, invalidated)`.
/// Only this interface's changes count.
#[must_use]
pub fn changed_properties(items: &[MessageItem]) -> Option<&MessageItemDict> {
    match items {
        [MessageItem::Str(interface), MessageItem::Dict(dict), ..]
            if interface == PROFILE_INTERFACE =>
        {
            Some(dict)
        }
        _ => None,
    }
}

/// Decides the next service to try after a failed `GetAll`. `None` means every name was absent.
#[must_use]
pub const fn next_service(index: usize) -> Option<usize> {
    if index + 1 < PROFILE_SERVICES.len() {
        Some(index + 1)
    } else {
        None
    }
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

/// Fail-soft profile facade on the system bus. Read via `refresh`; write via `set_active`.
pub struct PowerProfilesProvider {
    pump: Option<BusPump>,
    dormant: UnixStream,
    _peer: UnixStream,
    service: usize,
    pending_get_all: Option<u32>,
    pending_set: Option<u32>,
    snapshot: PowerProfilesSnapshot,
    started: bool,
}

impl PowerProfilesProvider {
    /// An unreachable system bus degrades to an unavailable snapshot; only socketpair failure errors.
    pub fn connect() -> FlameResult<Self> {
        let (peer, dormant) = UnixStream::pair()
            .map_err(|io| FlameError::unavailable(format!("socketpair failed: {io}")))?;
        Ok(Self {
            pump: BusPump::connect(BusKind::System).ok(),
            dormant,
            _peer: peer,
            service: 0,
            pending_get_all: None,
            pending_set: None,
            snapshot: PowerProfilesSnapshot::default(),
            started: false,
        })
    }

    /// Subscribes to `PropertiesChanged` for both service names and requests one read.
    pub fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let Some(pump) = self.pump.as_ref() else {
            return;
        };
        for service in PROFILE_SERVICES {
            let rule = format!(
                "type='signal',sender='{service}',interface='{PROPERTIES_INTERFACE}',path='{PROFILE_PATH}',member='{PROPERTIES_CHANGED}'"
            );
            let Ok(add_match) = Message::new_method_call(
                "org.freedesktop.DBus",
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "AddMatch",
            )
            .map(|message| message.append1(rule)) else {
                self.mark_unavailable();
                return;
            };
            if pump.send(add_match).is_err() {
                self.snapshot = PowerProfilesSnapshot::default();
                return;
            }
        }
        self.refresh();
    }

    pub fn stop(&mut self) {
        self.started = false;
        self.pending_get_all = None;
        self.pending_set = None;
    }

    /// Requests one `GetAll` from the current service. Called on start and when the menu opens.
    pub fn refresh(&mut self) {
        let Some(pump) = self.pump.as_ref() else {
            return;
        };
        let Ok(get_all) = Message::new_method_call(
            PROFILE_SERVICES[self.service],
            PROFILE_PATH,
            PROPERTIES_INTERFACE,
            GET_ALL,
        )
        .map(|message| message.append1(PROFILE_INTERFACE.to_owned())) else {
            self.snapshot = PowerProfilesSnapshot::default();
            return;
        };
        match pump.send(get_all) {
            Ok(serial) => self.pending_get_all = Some(serial),
            Err(_) => self.snapshot = PowerProfilesSnapshot::default(),
        }
    }

    /// Sets `ActiveProfile` when the service is present and advertises `profile`. The published
    /// state changes only when the service confirms it through `PropertiesChanged`.
    pub fn set_active(&mut self, profile: PowerProfile) -> FlameResult<()> {
        if !self.snapshot.tile_visible() || !self.snapshot.profiles.contains(&profile) {
            return Err(FlameError::unavailable("power profile not offered"));
        }
        let pump = self
            .pump
            .as_ref()
            .ok_or_else(|| FlameError::unavailable("system bus unavailable"))?;
        let [interface, property, value] = set_active_profile_items(profile);
        let mut set = Message::new_method_call(
            PROFILE_SERVICES[self.service],
            PROFILE_PATH,
            PROPERTIES_INTERFACE,
            SET,
        )
        .map_err(|error| FlameError::invalid(format!("Set message: {error}")))?;
        set.append_items(&[interface, property, value]);
        self.pending_set = Some(pump.send(set)?);
        Ok(())
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
    pub fn snapshot(&self) -> PowerProfilesSnapshot {
        self.snapshot.clone()
    }

    /// Drains one bounded reactor turn and applies replies and property changes.
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
                } if self.pending_get_all == Some(serial) => {
                    self.pending_get_all = None;
                    if is_error {
                        self.fall_back_or_absent();
                    } else {
                        self.apply_get_all(&items);
                    }
                }
                Event::Reply {
                    serial,
                    is_error: true,
                    ..
                } if self.pending_set == Some(serial) => {
                    self.pending_set = None;
                    self.refresh();
                }
                Event::Reply { .. } => {}
                Event::Changed { items } => {
                    if let Some(dict) = changed_properties(&items)
                        && apply_property_dict(&mut self.snapshot, dict)
                    {
                        self.snapshot.available = true;
                    }
                }
            }
        }
        Ok(count)
    }

    fn apply_get_all(&mut self, items: &[MessageItem]) {
        let Some(MessageItem::Dict(dict)) = items.first() else {
            self.mark_unavailable();
            return;
        };
        let mut snapshot = PowerProfilesSnapshot::default();
        apply_property_dict(&mut snapshot, dict);
        snapshot.available = true;
        self.snapshot = snapshot;
    }

    fn fall_back_or_absent(&mut self) {
        match next_service(self.service) {
            Some(next) => {
                self.service = next;
                self.refresh();
            }
            None => self.mark_unavailable(),
        }
    }

    fn mark_unavailable(&mut self) {
        self.snapshot = PowerProfilesSnapshot::default();
    }
}

fn parse_message(message: &Message) -> Option<Event> {
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
        || message.path().as_deref() != Some(PROFILE_PATH)
    {
        return None;
    }
    Some(Event::Changed {
        items: message.get_items(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(value: &str) -> MessageItem {
        MessageItem::Str(value.to_owned())
    }

    fn variant(item: MessageItem) -> MessageItem {
        MessageItem::Variant(Box::new(item))
    }

    fn dict(pairs: Vec<(&str, MessageItem)>) -> MessageItem {
        let pairs = pairs
            .into_iter()
            .map(|(key, value)| (s(key), variant(value)))
            .collect();
        MessageItem::new_dict(pairs).expect("dict")
    }

    fn profile_entry(name: &str) -> MessageItem {
        dict(vec![("Profile", s(name))])
    }

    fn profiles_property(names: &[&str]) -> MessageItem {
        let entries = names.iter().map(|name| profile_entry(name)).collect();
        variant(MessageItem::new_array(entries).expect("array"))
    }

    fn get_all_dict(active: &str, names: &[&str]) -> MessageItem {
        dict(vec![
            ("ActiveProfile", s(active)),
            ("Profiles", profiles_property(names)),
        ])
    }

    #[test]
    fn wire_names_round_trip_and_unknown_names_are_rejected() {
        for profile in PowerProfile::ALL {
            assert_eq!(PowerProfile::from_wire(profile.wire()), Some(profile));
        }
        assert_eq!(PowerProfile::from_wire("ultra"), None);
        assert_eq!(PowerProfile::from_wire(""), None);
    }

    #[test]
    fn active_profile_parses_string_and_ignores_other_types() {
        assert_eq!(
            parse_active_profile(&variant(s("balanced"))),
            Some(PowerProfile::Balanced)
        );
        assert_eq!(parse_active_profile(&variant(s("bogus"))), None);
        assert_eq!(parse_active_profile(&variant(MessageItem::UInt32(1))), None);
    }

    #[test]
    fn profiles_skip_unknown_malformed_and_duplicate_entries() {
        let entries = vec![
            profile_entry("performance"),
            profile_entry("nope"),
            dict(vec![("Other", s("balanced"))]),
            profile_entry("power-saver"),
            profile_entry("performance"),
        ];
        let property = variant(MessageItem::new_array(entries).expect("array"));
        assert_eq!(
            parse_profiles(&property),
            vec![PowerProfile::Performance, PowerProfile::PowerSaver]
        );
    }

    #[test]
    fn profiles_of_wrong_shape_are_empty() {
        assert!(parse_profiles(&variant(s("balanced"))).is_empty());
    }

    #[test]
    fn get_all_dict_populates_snapshot_and_tile_visibility() {
        let mut snapshot = PowerProfilesSnapshot::default();
        let MessageItem::Dict(payload) =
            get_all_dict("balanced", &["power-saver", "balanced", "performance"])
        else {
            panic!("dict");
        };
        assert!(apply_property_dict(&mut snapshot, &payload));
        snapshot.available = true;
        assert_eq!(snapshot.active, Some(PowerProfile::Balanced));
        assert_eq!(snapshot.profiles.len(), 3);
        assert!(snapshot.tile_visible());
    }

    #[test]
    fn tile_hidden_when_service_absent_or_no_profiles() {
        let absent = PowerProfilesSnapshot::default();
        assert!(!absent.tile_visible());
        let empty = PowerProfilesSnapshot {
            available: true,
            active: None,
            profiles: Vec::new(),
        };
        assert!(!empty.tile_visible());
    }

    #[test]
    fn property_changed_applies_only_this_interface_and_active_key() {
        let changed = dict(vec![("ActiveProfile", s("performance"))]);
        let items = [s(PROFILE_INTERFACE), changed];
        let payload = changed_properties(&items).expect("interface matches");
        let mut snapshot = PowerProfilesSnapshot {
            available: true,
            active: Some(PowerProfile::Balanced),
            profiles: PowerProfile::ALL.to_vec(),
        };
        assert!(apply_property_dict(&mut snapshot, payload));
        assert_eq!(snapshot.active, Some(PowerProfile::Performance));

        let other = [
            s("org.example.Other"),
            dict(vec![("ActiveProfile", s("balanced"))]),
        ];
        assert!(changed_properties(&other).is_none());
    }

    #[test]
    fn set_arguments_serialise_as_interface_property_and_string_variant() {
        let [interface, property, value] = set_active_profile_items(PowerProfile::PowerSaver);
        assert_eq!(interface, s(PROFILE_INTERFACE));
        assert_eq!(property, s("ActiveProfile"));
        assert_eq!(value, variant(s("power-saver")));
    }

    #[test]
    fn fallback_walks_to_second_name_then_reports_absent() {
        assert_eq!(next_service(0), Some(1));
        assert_eq!(next_service(1), None);
        assert_eq!(PROFILE_SERVICES[0], "net.hadess.PowerProfiles");
        assert_eq!(PROFILE_SERVICES[1], "org.freedesktop.UPower.PowerProfiles");
    }

    #[test]
    fn absent_service_provider_stays_unavailable_without_a_bus() {
        let mut provider = PowerProfilesProvider {
            pump: None,
            dormant: UnixStream::pair().expect("pair").1,
            _peer: UnixStream::pair().expect("pair").0,
            service: 0,
            pending_get_all: None,
            pending_set: None,
            snapshot: PowerProfilesSnapshot::default(),
            started: false,
        };
        provider.start();
        assert!(!provider.snapshot().tile_visible());
        assert!(provider.set_active(PowerProfile::Balanced).is_err());
    }
}

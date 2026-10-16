//! NetworkManager's system-bus source adapter.

mod actions;
mod reply;
mod rules;
#[cfg(test)]
mod tests;

use actions::*;
use reply::*;
use rules::*;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use std::ffi::CString;

use dbus::arg::messageitem::MessageItem;
use dbus::strings::ErrorName;
use dbus::{Message, MessageType};
use flamewm_api::system::{NetworkKind, NetworkSnapshot, SystemAction};
use flamewm_api::{ErrorCode, FlameError, FlameResult};
use flamewm_dbus_reactor::{BusKind, BusPump, BusWatch, MessageDisposition};
use flamewm_integrations_core::network::{
    AccessPoint, NetworkController, NetworkIntent, NetworkStatus,
};

pub const NETWORK_MANAGER_SERVICE: &str = "org.freedesktop.NetworkManager";
pub const NETWORK_MANAGER_ROOT: &str = "/org/freedesktop/NetworkManager";
pub const NETWORK_MANAGER_INTERFACE: &str = "org.freedesktop.NetworkManager";
pub const DBUS_SERVICE: &str = "org.freedesktop.DBus";
pub const DBUS_ROOT: &str = "/org/freedesktop/DBus";
pub const DBUS_INTERFACE: &str = "org.freedesktop.DBus";
pub const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";

const NAME_OWNER_CHANGED: &str = "NameOwnerChanged";
const PROPERTIES_CHANGED: &str = "PropertiesChanged";
const GET_ALL: &str = "GetAll";
const SET: &str = "Set";
const DEACTIVATE_CONNECTION: &str = "DeactivateConnection";
const ACTIVATE_CONNECTION: &str = "ActivateConnection";
const ADD_AND_ACTIVATE_CONNECTION: &str = "AddAndActivateConnection";
const GET_DEVICES: &str = "GetDevices";
const GET_ACCESS_POINTS: &str = "GetAccessPoints";
const REQUEST_SCAN: &str = "RequestScan";
const LIST_CONNECTIONS: &str = "ListConnections";
const ADD_MATCH: &str = "AddMatch";
const SECRET_AGENT_PATH: &str = "/org/freedesktop/NetworkManager/SecretAgent";
const SECRET_AGENT_INTERFACE: &str = "org.freedesktop.NetworkManager.SecretAgent";
const SECRET_AGENT_MANAGER_PATH: &str = "/org/freedesktop/NetworkManager/AgentManager";
const SECRET_AGENT_MANAGER_INTERFACE: &str = "org.freedesktop.NetworkManager.AgentManager";
const SECRET_AGENT_IDENTIFIER: &str = "com.arkflame.FlameWM";
const REGISTER_WITH_CAPABILITIES: &str = "RegisterWithCapabilities";
const GET_SECRETS: &str = "GetSecrets";
const CANCEL_GET_SECRETS: &str = "CancelGetSecrets";
const USER_CANCELED: &str = "org.freedesktop.NetworkManager.SecretAgent.Error.UserCanceled";
const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
const UNKNOWN_INTERFACE: &str = "org.freedesktop.DBus.Error.UnknownInterface";
const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";
const SAVE_SECRETS: &str = "SaveSecrets";
const DELETE_SECRETS: &str = "DeleteSecrets";
const DELETE: &str = "Delete";
const GET_SETTINGS: &str = "GetSettings";
const UPDATE: &str = "Update";
const SETTINGS_CONNECTION_INTERFACE: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const DEVICE_INTERFACE: &str = "org.freedesktop.NetworkManager.Device";
const DEVICE_STATE_CHANGED: &str = "StateChanged";
// NMDeviceState values (nm-dbus-interface.h); the attempt tracker needs only these.
const DEVICE_STATE_DISCONNECTED: u32 = 30;
const DEVICE_STATE_PREPARE: u32 = 40;
const DEVICE_STATE_SECONDARIES: u32 = 90;
const DEVICE_STATE_ACTIVATED: u32 = 100;
const DEVICE_STATE_FAILED: u32 = 120;
// NMDeviceStateReason values meaning the Wi-Fi credentials were missing or rejected.
const DEVICE_REASON_NO_SECRETS: u32 = 7;
const DEVICE_REASON_SUPPLICANT_DISCONNECT: u32 = 8;
const DEVICE_REASON_SUPPLICANT_CONFIG_FAILED: u32 = 9;
const DEVICE_REASON_SUPPLICANT_FAILED: u32 = 10;
const DEVICE_REASON_SUPPLICANT_TIMEOUT: u32 = 11;

/// Values from a NetworkManager root `PropertiesChanged` signal or `GetAll` reply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkManagerProperties {
    pub networking_enabled: Option<bool>,
    pub wireless_enabled: Option<bool>,
    pub primary_connection: Option<String>,
    pub state: Option<u32>,
    pub last_scan: Option<i64>,
    /// Device-level `ActiveConnection` (present on wireless device signals).
    pub active_connection: Option<String>,
    /// Device-level `ActiveAccessPoint` (present on wireless device signals).
    pub active_access_point: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkManagerProviderState {
    Disconnected,
    Started,
}


enum PendingRequest {
    AddMatch,
    ServiceOwner,
    RootProperties {
        generation: u64,
    },
    Devices {
        generation: u64,
    },
    DeviceProperties {
        generation: u64,
        device: String,
    },
    AccessPoints {
        generation: u64,
    },
    AccessPointProperties {
        generation: u64,
        access_point: String,
    },
    Profiles {
        generation: u64,
    },
    ProfileSettings {
        generation: u64,
        profile: String,
    },
    RegisterSecretAgent,
    Scan {
        generation: u64,
    },
    Action,
    ActivateConnection {
        attempt: u64,
    },
    AddAndActivateSecure {
        attempt: u64,
    },
    /// Read a saved profile before its psk is replaced. Holds the password in memory only.
    SavedSettings {
        attempt: u64,
        profile: String,
        device: String,
        access_point: String,
        secret: String,
    },
    UpdateSavedProfile {
        attempt: u64,
        profile: String,
        device: String,
        access_point: String,
    },
}

enum Incoming {
    Reply {
        serial: u32,
        is_error: bool,
        items: Vec<MessageItem>,
    },
    OwnerChanged {
        name: String,
        old_owner: String,
        new_owner: String,
    },
    PropertiesChanged {
        path: String,
        properties: NetworkManagerProperties,
    },
    SecretAgent(SecretAgentCall),
    DeviceStateChanged {
        path: String,
        new_state: u32,
        reason: u32,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct WifiDevice {
    active_connection: String,
    active_access_point: String,
    last_scan: Option<i64>,
}

/// Event-driven NetworkManager provider. Native replies and signals are sole state authority.
pub struct NetworkManagerProvider {
    pump: BusPump,
    controller: NetworkController,
    status: NetworkStatus,
    pending: BTreeMap<u32, PendingRequest>,
    state: NetworkManagerProviderState,
    reconcile_pending: bool,
    wifi_devices: BTreeMap<String, WifiDevice>,
    access_points: BTreeMap<String, AccessPointProperties>,
    profile_ssids: BTreeMap<String, String>,
    secret_prompt: Option<SecretPrompt>,
    secret_context: Option<SecretContext>,
    attempts: ConnectAttempts,
}

impl NetworkManagerProvider {
    pub fn connect() -> FlameResult<Self> {
        Ok(Self {
            pump: BusPump::connect(BusKind::System)?,
            controller: NetworkController::default(),
            status: NetworkStatus::default(),
            pending: BTreeMap::new(),
            state: NetworkManagerProviderState::Disconnected,
            reconcile_pending: false,
            wifi_devices: BTreeMap::new(),
            access_points: BTreeMap::new(),
            profile_ssids: BTreeMap::new(),
            secret_prompt: None,
            secret_context: None,
            attempts: ConnectAttempts::default(),
        })
    }

    pub fn start(&mut self) -> FlameResult<()> {
        if self.state == NetworkManagerProviderState::Started {
            return Ok(());
        }
        self.add_match(
            "type='signal',sender='org.freedesktop.DBus',path='/org/freedesktop/DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',arg0='org.freedesktop.NetworkManager'",
        )?;
        for interface in NM_PROPERTY_INTERFACES {
            self.add_match(&properties_changed_rule(interface))?;
        }
        self.add_match(
            "type='signal',sender='org.freedesktop.NetworkManager',interface='org.freedesktop.NetworkManager.Device',member='StateChanged'",
        )?;
        self.queue_service_owner_request()?;
        self.state = NetworkManagerProviderState::Started;
        self.reconcile_pending = true;
        Ok(())
    }

    pub fn stop(&mut self) {
        if self.state == NetworkManagerProviderState::Disconnected {
            return;
        }
        self.state = NetworkManagerProviderState::Disconnected;
        self.reconcile_pending = false;
        self.pending.clear();
        self.controller.service_vanished();
        self.status = NetworkStatus::default();
        self.wifi_devices.clear();
        self.access_points.clear();
        self.profile_ssids.clear();
        self.secret_prompt = None;
        self.secret_context = None;
        self.attempts.clear();
    }

    #[must_use]
    pub const fn state(&self) -> NetworkManagerProviderState {
        self.state
    }

    #[must_use]
    pub fn watch(&self) -> BusWatch {
        self.pump.watch()
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.controller.generation()
    }

    #[must_use]
    pub fn snapshot(&self) -> NetworkSnapshot {
        self.controller.snapshot()
    }

}

impl NetworkManagerProvider {
    /// Reissue one event-triggered root reconciliation. This is not a timer or poll operation.
    pub fn reconcile(&mut self) -> FlameResult<bool> {
        if !self.reconcile_pending {
            return Ok(false);
        }
        self.reconcile_pending = false;
        if !self.controller.is_available() {
            return Ok(false);
        }
        let generation = self.controller.generation();
        let message = get_all_message(
            NETWORK_MANAGER_SERVICE,
            NETWORK_MANAGER_ROOT,
            NETWORK_MANAGER_INTERFACE,
        )?;
        self.queue(message, PendingRequest::RootProperties { generation })?;
        self.queue_devices(generation)?;
        self.queue_profiles(generation)?;
        Ok(true)
    }

    /// Drain one reactor turn and apply only native owner/property events and their replies.
    pub fn on_ready(&mut self) -> FlameResult<usize> {
        let incoming = RefCell::new(Vec::new());
        let parse_error = RefCell::new(None);
        let count = self.pump.on_ready_with_object_path(
            SECRET_AGENT_PATH,
            |message| match parse_secret_agent_call(message) {
                Ok(call) => {
                    incoming.borrow_mut().push(Incoming::SecretAgent(call));
                    MessageDisposition::Handled
                }
                Err(error) => {
                    *parse_error.borrow_mut() = Some(error);
                    MessageDisposition::Handled
                }
            },
            |message| {
                if let Some(event) = parse_message(message) {
                    match event {
                        Ok(event) => incoming.borrow_mut().push(event),
                        Err(error) => *parse_error.borrow_mut() = Some(error),
                    }
                    return MessageDisposition::Handled;
                }
                MessageDisposition::Unhandled
            },
        )?;
        if let Some(error) = parse_error.into_inner() {
            return Err(error);
        }
        dispatch_incoming(incoming.into_inner(), |event| self.apply(event));
        let _ = self.reconcile()?;
        Ok(count)
    }
}

/// Applies every drained event in order. A failing event is logged by error text only (the
/// event payload can carry secrets, so it is never printed) and does not stop the batch.
fn dispatch_incoming(
    events: Vec<Incoming>,
    mut apply: impl FnMut(Incoming) -> FlameResult<()>,
) {
    for event in events {
        if let Err(error) = apply(event) {
            eprintln!(
                "flamewm-integrations-linux: NetworkManager event failed; remaining events still dispatched: {error}"
            );
        }
    }
}

impl NetworkManagerProvider {
    /// Derive the connected AP object path from the wireless device whose
    /// `ActiveConnection` matches the primary connection. Keyed by path, never SSID.
    fn refresh_active_access_point(&mut self) {
        let active_connection = self.status.active_path.as_str();
        self.status.snapshot.active_access_point_path = if active_connection.is_empty() {
            String::new()
        } else {
            self.wifi_devices
                .values()
                .find(|wifi| wifi.active_connection == active_connection)
                .map_or_else(String::new, |wifi| wifi.active_access_point.clone())
        };
    }

    /// Report a degraded (not absent) NetworkManager state: the shell treats
    /// `Unknown` as visible but not enabled.
    fn mark_degraded(&mut self) {
        self.reconcile_pending = false;
        self.status.snapshot.availability = flamewm_api::system::ServiceAvailability::Unknown;
        let generation = self.controller.generation();
        let _ = self
            .controller
            .inject_status(self.status.clone(), generation);
    }

    fn publish_access_points(&mut self) {
        self.refresh_active_access_point();
        let known_ssids = self.profile_ssids.values().collect::<BTreeSet<_>>();
        let mut access_points = self
            .access_points
            .iter()
            .map(|(path, properties)| AccessPoint {
                path: path.clone(),
                ssid: properties.ssid.clone(),
                strength_percent: properties.strength_percent,
                secured: properties.secured,
                known: known_ssids.contains(&properties.ssid),
            })
            .collect::<Vec<_>>();
        access_points.sort_by(|left, right| {
            right
                .strength_percent
                .cmp(&left.strength_percent)
                .then_with(|| left.ssid.cmp(&right.ssid))
                .then_with(|| left.path.cmp(&right.path))
        });
        self.status.visible_access_points = access_points;
        let active = self.status.snapshot.active_access_point_path.clone();
        let connected = (!active.is_empty())
            .then(|| {
                self.status
                    .visible_access_points
                    .iter()
                    .find(|ap| ap.path == active)
            })
            .flatten();
        self.status.snapshot.label = header_label_for(connected, !active.is_empty());
        if let Some(ap) = connected {
            self.status.snapshot.strength_percent = ap.strength_percent;
        } else {
            // Nothing connected: the strongest AP stays in the list; the header makes no claim.
            let strongest = self.status.visible_access_points.first();
            self.status.snapshot.strength_percent = strongest.map_or(0, |ap| ap.strength_percent);
        }
        let generation = self.controller.generation();
        let _ = self
            .controller
            .inject_status(self.status.clone(), generation);
    }

}


fn object_paths(items: &[MessageItem], context: &str) -> FlameResult<Vec<String>> {
    let Some(MessageItem::Array(paths)) = items.first() else {
        return Err(FlameError::invalid(context));
    };
    paths
        .clone()
        .into_vec()
        .into_iter()
        .map(|path| item_object_path(&path, context))
        .collect()
}

fn empty_dict() -> MessageItem {
    MessageItem::from_dict(std::iter::empty::<Result<(String, MessageItem), FlameError>>())
        .unwrap_or_else(|_| unreachable!())
}

fn method_call(service: &str, path: &str, interface: &str, member: &str) -> FlameResult<Message> {
    Message::new_method_call(service, path, interface, member)
        .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))
}

/// `org.freedesktop.DBus.Properties.GetAll(s interface)` on one NetworkManager object.
fn get_all_message(service: &str, path: &str, interface: &str) -> FlameResult<Message> {
    Ok(method_call(service, path, PROPERTIES_INTERFACE, GET_ALL)?.append1(interface.to_owned()))
}

fn object_path(value: String) -> FlameResult<dbus::Path<'static>> {
    dbus::Path::new(value).map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))
}

pub(crate) fn one_string(items: &[MessageItem], context: &str) -> FlameResult<String> {
    items
        .first()
        .ok_or_else(|| FlameError::invalid(context))
        .and_then(|item| item_string(item, context))
}

pub(crate) fn item_string(item: &MessageItem, context: &str) -> FlameResult<String> {
    match item {
        MessageItem::Str(value) => Ok(value.clone()),
        _ => Err(FlameError::invalid(context)),
    }
}

fn item_object_path(item: &MessageItem, context: &str) -> FlameResult<String> {
    match item {
        MessageItem::ObjectPath(value) => Ok(value.to_string()),
        _ => Err(FlameError::invalid(context)),
    }
}

pub(crate) fn item_strings(item: &MessageItem, context: &str) -> FlameResult<Vec<String>> {
    let MessageItem::Array(values) = item else {
        return Err(FlameError::invalid(context));
    };
    values
        .iter()
        .map(|value| item_string(value, context))
        .collect()
}

pub(crate) fn item_bool(item: &MessageItem, context: &str) -> FlameResult<bool> {
    match item {
        MessageItem::Bool(value) => Ok(*value),
        _ => Err(FlameError::invalid(context)),
    }
}

pub(crate) fn item_u32(item: &MessageItem, context: &str) -> FlameResult<u32> {
    match item {
        MessageItem::UInt32(value) => Ok(*value),
        _ => Err(FlameError::invalid(context)),
    }
}

fn item_i64(item: &MessageItem, context: &str) -> FlameResult<i64> {
    match item {
        MessageItem::Int64(value) => Ok(*value),
        _ => Err(FlameError::invalid(context)),
    }
}

fn item_byte(item: &MessageItem, context: &str) -> FlameResult<u8> {
    match item {
        MessageItem::Byte(value) => Ok(*value),
        _ => Err(FlameError::invalid(context)),
    }
}

fn item_bytes(item: &MessageItem, context: &str) -> FlameResult<Vec<u8>> {
    let MessageItem::Array(values) = item else {
        return Err(FlameError::invalid(context));
    };
    values
        .clone()
        .into_vec()
        .into_iter()
        .map(|value| item_byte(&value, context))
        .collect()
}

pub(crate) fn unwrap_variant(item: MessageItem) -> MessageItem {
    match item {
        MessageItem::Variant(value) => unwrap_variant(*value),
        item => item,
    }
}

/// Header label for the network snapshot: the connected SSID, or a neutral label when no AP is connected.
///
/// An active AP missing from the scan has no name in this source, so the label stays empty
/// (the shell shows "Hidden network") rather than claiming "Not connected".
fn header_label_for(connected: Option<&AccessPoint>, active_path_set: bool) -> String {
    connected.map_or_else(
        || {
            if active_path_set {
                String::new()
            } else {
                "Not connected".to_owned()
            }
        },
        |ap| ap.ssid.clone(),
    )
}

fn unsupported(message: &str) -> FlameError {
    FlameError::new(ErrorCode::Unsupported, message)
}

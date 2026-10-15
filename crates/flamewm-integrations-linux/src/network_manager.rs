//! NetworkManager's system-bus source adapter.

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
const DELETE: &str = "Delete";
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

enum SecretAgentCall {
    GetSecrets {
        setting_name: String,
        secret_key: String,
        reply: Option<Message>,
    },
    CancelGetSecrets {
        reply: Option<Message>,
    },
}

struct SecretContext {
    request_id: u64,
    setting_name: String,
    secret_key: String,
    reply: Option<Message>,
}

/// Which password request is open. A new network's password creates its profile on submit;
/// a saved profile's password is accepted only as the answer to NetworkManager's own request.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SecretPrompt {
    NewNetwork { access_point: String },
    SavedNetwork { access_point: String },
}

/// One Wi-Fi activation. Only a profile this attempt created may be deleted on failure.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectAttempt {
    id: u64,
    access_point: String,
    device: String,
    created: bool,
    profile: Option<String>,
    /// Set once the device enters an activation state. Earlier DISCONNECTED signals belong to
    /// the connection that was active before this attempt.
    activating: bool,
    /// Credentials were rejected before the add reply named the created profile.
    auth_failed: bool,
}

/// Follow-up the provider must perform for an attempt signal. The tracker only decides.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AttemptEffect {
    None,
    /// Delete `profile` when this attempt created it, then reopen the new-network prompt.
    RetryNewNetwork {
        access_point: String,
        profile: Option<String>,
    },
    /// Keep the saved profile and reopen the prompt for its access point.
    RetrySavedNetwork {
        access_point: String,
    },
}

#[derive(Debug, Default)]
struct ConnectAttempts {
    current: Option<ConnectAttempt>,
    next_id: u64,
}

impl ConnectAttempts {
    fn begin(&mut self, access_point: String, device: String, created: bool) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        self.current = Some(ConnectAttempt {
            id: self.next_id,
            access_point,
            device,
            created,
            profile: None,
            activating: false,
            auth_failed: false,
        });
        self.next_id
    }

    fn is_busy(&self) -> bool {
        self.current.is_some()
    }

    fn clear(&mut self) {
        self.current = None;
    }

    fn saved_access_point(&self) -> Option<&str> {
        self.current
            .as_ref()
            .filter(|attempt| !attempt.created)
            .map(|attempt| attempt.access_point.as_str())
    }

    /// The add reply names the profile this attempt created.
    fn on_profile_created(&mut self, id: u64, profile: String) -> AttemptEffect {
        let Some(attempt) = self.current.as_mut().filter(|attempt| attempt.id == id) else {
            return AttemptEffect::None;
        };
        attempt.profile = Some(profile);
        if attempt.auth_failed {
            self.current
                .take()
                .map_or(AttemptEffect::None, retry_effect)
        } else {
            AttemptEffect::None
        }
    }

    /// A rejected add creates nothing to delete; a rejected saved activation needs no prompt.
    fn on_request_failed(&mut self, id: u64) -> AttemptEffect {
        match self.current.take() {
            Some(attempt) if attempt.id == id => {
                if attempt.created {
                    retry_effect(attempt)
                } else {
                    AttemptEffect::None
                }
            }
            other => {
                self.current = other;
                AttemptEffect::None
            }
        }
    }

    fn on_device_state(&mut self, device: &str, state: u32, reason: u32) -> AttemptEffect {
        let activating = match self
            .current
            .as_mut()
            .filter(|attempt| attempt.device == device)
        {
            None => return AttemptEffect::None,
            Some(attempt) => {
                if (DEVICE_STATE_PREPARE..=DEVICE_STATE_SECONDARIES).contains(&state) {
                    attempt.activating = true;
                    return AttemptEffect::None;
                }
                attempt.activating
            }
        };
        match state {
            DEVICE_STATE_DISCONNECTED if !activating => AttemptEffect::None,
            DEVICE_STATE_FAILED if is_credential_reason(reason) => self.on_credentials_rejected(),
            DEVICE_STATE_DISCONNECTED | DEVICE_STATE_ACTIVATED | DEVICE_STATE_FAILED => {
                self.current = None;
                AttemptEffect::None
            }
            _ => AttemptEffect::None,
        }
    }

    fn on_credentials_rejected(&mut self) -> AttemptEffect {
        let Some(attempt) = self.current.as_mut() else {
            return AttemptEffect::None;
        };
        if attempt.created && attempt.profile.is_none() {
            attempt.auth_failed = true;
            return AttemptEffect::None;
        }
        self.current
            .take()
            .map_or(AttemptEffect::None, retry_effect)
    }
}

fn retry_effect(attempt: ConnectAttempt) -> AttemptEffect {
    if attempt.created {
        AttemptEffect::RetryNewNetwork {
            access_point: attempt.access_point,
            profile: attempt.profile,
        }
    } else {
        AttemptEffect::RetrySavedNetwork {
            access_point: attempt.access_point,
        }
    }
}

fn is_credential_reason(reason: u32) -> bool {
    matches!(
        reason,
        DEVICE_REASON_NO_SECRETS
            | DEVICE_REASON_SUPPLICANT_DISCONNECT
            | DEVICE_REASON_SUPPLICANT_CONFIG_FAILED
            | DEVICE_REASON_SUPPLICANT_FAILED
            | DEVICE_REASON_SUPPLICANT_TIMEOUT
    )
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct WifiDevice {
    active_connection: String,
    active_access_point: String,
    last_scan: Option<i64>,
}

/// Refresh work implied by one wireless device `PropertiesChanged` signal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DeviceRefresh {
    /// Re-read the device (ActiveConnection / ActiveAccessPoint); its reply re-queues the AP list.
    device: bool,
    /// Re-read the device's access-point list (ActiveAccessPoint or LastScan changed).
    access_points: bool,
}

/// Pure decision: the signal dict carries only changed properties, so a present value
/// that differs from the cached one is a change. Unchanged data requests nothing.
/// An undiscovered device (no cache) always requests a device read.
fn device_signal_refresh(
    cached: Option<&WifiDevice>,
    signal: &NetworkManagerProperties,
) -> DeviceRefresh {
    let Some(cached) = cached else {
        return DeviceRefresh {
            device: true,
            access_points: false,
        };
    };
    let active_access_point =
        signal
            .active_access_point
            .as_deref()
            .map(|path| if path == "/" { "" } else { path });
    let access_point_changed =
        active_access_point.is_some_and(|path| path != cached.active_access_point);
    let connection_changed = signal
        .active_connection
        .as_deref()
        .is_some_and(|path| path != cached.active_connection);
    let scan_changed = signal
        .last_scan
        .is_some_and(|scan| Some(scan) != cached.last_scan);
    DeviceRefresh {
        device: access_point_changed || connection_changed,
        access_points: access_point_changed || scan_changed,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AccessPointProperties {
    ssid: String,
    strength_percent: u8,
    secured: bool,
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
        self.add_match(
            "type='signal',sender='org.freedesktop.NetworkManager',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'",
        )?;
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

    /// Queue supported NetworkManager requests. Native signals remain sole snapshot authority.
    pub fn perform_action(
        &mut self,
        action: SystemAction,
        snapshot: &NetworkSnapshot,
    ) -> FlameResult<()> {
        if snapshot.generation != self.controller.generation() {
            return Err(FlameError::stale("stale NetworkManager service generation"));
        }
        match action {
            SystemAction::SetWifiEnabled(enabled) => {
                let _ = self.controller.request_set_wifi_enabled(enabled)?;
                let mut message = method_call(
                    NETWORK_MANAGER_SERVICE,
                    NETWORK_MANAGER_ROOT,
                    PROPERTIES_INTERFACE,
                    SET,
                )?;
                message.append_items(&[
                    MessageItem::Str(NETWORK_MANAGER_INTERFACE.to_owned()),
                    MessageItem::Str("WirelessEnabled".to_owned()),
                    MessageItem::Variant(Box::new(MessageItem::Bool(enabled))),
                ]);
                self.queue(message, PendingRequest::Action)
            }
            SystemAction::Disconnect => {
                let _ = self.controller.request_disconnect(snapshot.generation)?;
                if snapshot.active_path.is_empty() {
                    return Err(unsupported("no active NetworkManager connection"));
                }
                let message = method_call(
                    NETWORK_MANAGER_SERVICE,
                    NETWORK_MANAGER_ROOT,
                    NETWORK_MANAGER_INTERFACE,
                    DEACTIVATE_CONNECTION,
                )?
                .append1(
                    dbus::Path::new(snapshot.active_path.clone())
                        .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?,
                );
                self.queue(message, PendingRequest::Action)
            }
            SystemAction::ConnectKnown { access_point_path } => {
                let device = self.device_for_access_point(&access_point_path)?;
                let is_open_unknown =
                    self.access_points
                        .get(&access_point_path)
                        .map_or(false, |access_point| {
                            !access_point.secured
                                && !snapshot
                                    .access_points
                                    .iter()
                                    .any(|ap| ap.path == access_point_path && ap.known)
                        });
                if is_open_unknown {
                    let _ = self
                        .controller
                        .request_connect_open(&access_point_path, snapshot.generation)?;
                    let ssid = self
                        .access_points
                        .get(&access_point_path)
                        .map(|access_point| access_point.ssid.clone())
                        .ok_or_else(|| {
                            FlameError::new(
                                ErrorCode::NotFound,
                                "NetworkManager access point was not found",
                            )
                        })?;
                    self.queue_add_and_activate_open(ssid, device, access_point_path)
                } else {
                    let _ = self
                        .controller
                        .request_connect_known(&access_point_path, snapshot.generation)?;
                    self.connect_saved(access_point_path)
                }
            }
            SystemAction::ConnectWifi {
                access_point_path,
                generation,
            } => match self
                .controller
                .request_connect_wifi(&access_point_path, generation)?
            {
                NetworkIntent::ConnectKnown { .. } => self.connect_saved(access_point_path),
                NetworkIntent::RequestSecret { .. } => self.prompt_new_network(access_point_path),
                _ => Err(unsupported("unexpected NetworkManager Wi-Fi intent")),
            },
            SystemAction::SubmitNetworkSecret {
                request_id,
                generation,
                secret,
            } => self.submit_network_secret(request_id, generation, secret),
            SystemAction::CancelNetworkSecret {
                request_id,
                generation,
            } => {
                let _ = self
                    .controller
                    .request_cancel_secret(request_id, generation)?;
                self.secret_prompt = None;
                self.attempts.clear();
                if let Some(context) = self.secret_context.take() {
                    self.send_user_canceled(context.reply)?;
                }
                Ok(())
            }
            SystemAction::Scan => {
                let Some(_) = self.controller.request_scan()? else {
                    return Ok(());
                };
                let devices = self.wifi_devices.keys().cloned().collect::<Vec<_>>();
                if devices.is_empty() {
                    let _ = self.controller.complete_scan(self.controller.generation());
                    return Err(unsupported("NetworkManager has no wireless device"));
                }
                let controller = &mut self.controller;
                let pump = &mut self.pump;
                let pending = &mut self.pending;
                let generation = controller.generation();
                queue_scan_requests(controller, &devices, |device| {
                    let message = method_call(
                        NETWORK_MANAGER_SERVICE,
                        device,
                        "org.freedesktop.NetworkManager.Device.Wireless",
                        REQUEST_SCAN,
                    )?
                    .append1(empty_dict());
                    let serial = pump.send(message)?;
                    pending.insert(serial, PendingRequest::Scan { generation });
                    Ok(())
                })
            }
            _ => Err(unsupported("system action is not a NetworkManager action")),
        }
    }

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
        for event in incoming.into_inner() {
            self.apply(event)?;
        }
        let _ = self.reconcile()?;
        Ok(count)
    }

    fn add_match(&mut self, rule: &str) -> FlameResult<()> {
        let message = method_call(DBUS_SERVICE, DBUS_ROOT, DBUS_INTERFACE, ADD_MATCH)?
            .append1(rule.to_owned());
        self.queue(message, PendingRequest::AddMatch)
    }

    fn queue_service_owner_request(&mut self) -> FlameResult<()> {
        let message = method_call(DBUS_SERVICE, DBUS_ROOT, DBUS_INTERFACE, "GetNameOwner")?
            .append1(NETWORK_MANAGER_SERVICE.to_owned());
        self.queue(message, PendingRequest::ServiceOwner)
    }

    fn queue_register_secret_agent(&mut self) -> FlameResult<()> {
        let message = secret_agent_registration_message()?;
        self.queue(message, PendingRequest::RegisterSecretAgent)
    }

    fn queue_devices(&mut self, generation: u64) -> FlameResult<()> {
        let message = method_call(
            NETWORK_MANAGER_SERVICE,
            NETWORK_MANAGER_ROOT,
            NETWORK_MANAGER_INTERFACE,
            GET_DEVICES,
        )?;
        self.queue(message, PendingRequest::Devices { generation })
    }

    fn queue_device_properties(&mut self, generation: u64, device: String) -> FlameResult<()> {
        let message = get_all_message(
            NETWORK_MANAGER_SERVICE,
            &device,
            "org.freedesktop.NetworkManager.Device",
        )?;
        self.queue(
            message,
            PendingRequest::DeviceProperties { generation, device },
        )
    }

    fn queue_access_points(&mut self, generation: u64, device: String) -> FlameResult<()> {
        let message = method_call(
            NETWORK_MANAGER_SERVICE,
            &device,
            "org.freedesktop.NetworkManager.Device.Wireless",
            GET_ACCESS_POINTS,
        )?;
        self.queue(message, PendingRequest::AccessPoints { generation })
    }

    fn queue_access_point_properties(
        &mut self,
        generation: u64,
        access_point: String,
    ) -> FlameResult<()> {
        let message = get_all_message(
            NETWORK_MANAGER_SERVICE,
            &access_point,
            "org.freedesktop.NetworkManager.AccessPoint",
        )?;
        self.queue(
            message,
            PendingRequest::AccessPointProperties {
                generation,
                access_point,
            },
        )
    }

    fn queue_profiles(&mut self, generation: u64) -> FlameResult<()> {
        let message = method_call(
            NETWORK_MANAGER_SERVICE,
            "/org/freedesktop/NetworkManager/Settings",
            "org.freedesktop.NetworkManager.Settings",
            LIST_CONNECTIONS,
        )?;
        self.queue(message, PendingRequest::Profiles { generation })
    }

    fn queue_profile_settings(&mut self, generation: u64, profile: String) -> FlameResult<()> {
        let message = method_call(
            NETWORK_MANAGER_SERVICE,
            &profile,
            "org.freedesktop.NetworkManager.Settings.Connection",
            "GetSettings",
        )?;
        self.queue(
            message,
            PendingRequest::ProfileSettings {
                generation,
                profile,
            },
        )
    }

    fn queue_activate_connection(
        &mut self,
        attempt: u64,
        profile: String,
        device: String,
        access_point: String,
    ) -> FlameResult<()> {
        let message = method_call(
            NETWORK_MANAGER_SERVICE,
            NETWORK_MANAGER_ROOT,
            NETWORK_MANAGER_INTERFACE,
            ACTIVATE_CONNECTION,
        )?
        .append3(
            object_path(profile)?,
            object_path(device)?,
            object_path(access_point)?,
        );
        self.queue(message, PendingRequest::ActivateConnection { attempt })
    }

    fn queue_delete_profile(&mut self, profile: String) -> FlameResult<()> {
        let message = method_call(
            NETWORK_MANAGER_SERVICE,
            &profile,
            SETTINGS_CONNECTION_INTERFACE,
            DELETE,
        )?;
        self.queue(message, PendingRequest::Action)
    }

    fn queue_add_and_activate_open(
        &mut self,
        ssid: String,
        device: String,
        access_point: String,
    ) -> FlameResult<()> {
        let wireless = MessageItem::from_dict(
            vec![("ssid".to_owned(), MessageItem::from(ssid.as_bytes()))]
                .into_iter()
                .map(Ok::<_, FlameError>),
        )
        .map_err(|_| FlameError::invalid("NetworkManager open wireless settings are invalid"))?;
        let connection = MessageItem::from_dict(
            vec![
                ("id".to_owned(), MessageItem::Str(ssid)),
                (
                    "type".to_owned(),
                    MessageItem::Str("802-11-wireless".to_owned()),
                ),
            ]
            .into_iter()
            .map(Ok::<_, FlameError>),
        )
        .map_err(|_| FlameError::invalid("NetworkManager open connection settings are invalid"))?;
        let settings = MessageItem::new_dict(vec![
            (MessageItem::Str("connection".to_owned()), connection),
            (MessageItem::Str("802-11-wireless".to_owned()), wireless),
        ])
        .map_err(|_| FlameError::invalid("NetworkManager open settings are invalid"))?;
        let message = method_call(
            NETWORK_MANAGER_SERVICE,
            NETWORK_MANAGER_ROOT,
            NETWORK_MANAGER_INTERFACE,
            ADD_AND_ACTIVATE_CONNECTION,
        )?
        .append3(settings, object_path(device)?, object_path(access_point)?);
        self.queue(message, PendingRequest::Action)
    }

    fn queue_add_and_activate_secure(
        &mut self,
        attempt: u64,
        ssid: String,
        device: String,
        access_point: String,
        secret: String,
    ) -> FlameResult<()> {
        let wireless = MessageItem::from_dict(
            vec![("ssid".to_owned(), MessageItem::from(ssid.as_bytes()))]
                .into_iter()
                .map(Ok::<_, FlameError>),
        )
        .map_err(|_| FlameError::invalid("NetworkManager secure wireless settings are invalid"))?;
        // psk-flags NONE (0) stores the password in the profile, so it survives a successful connect.
        let security = MessageItem::from_dict(
            vec![
                (
                    "key-mgmt".to_owned(),
                    MessageItem::Str("wpa-psk".to_owned()),
                ),
                ("psk".to_owned(), MessageItem::Str(secret)),
                ("psk-flags".to_owned(), MessageItem::UInt32(0)),
            ]
            .into_iter()
            .map(Ok::<_, FlameError>),
        )
        .map_err(|_| {
            FlameError::invalid("NetworkManager secure wireless security settings are invalid")
        })?;
        let connection = MessageItem::from_dict(
            vec![
                ("id".to_owned(), MessageItem::Str(ssid)),
                (
                    "type".to_owned(),
                    MessageItem::Str("802-11-wireless".to_owned()),
                ),
            ]
            .into_iter()
            .map(Ok::<_, FlameError>),
        )
        .map_err(|_| {
            FlameError::invalid("NetworkManager secure connection settings are invalid")
        })?;
        let settings = MessageItem::new_dict(vec![
            (MessageItem::Str("connection".to_owned()), connection),
            (MessageItem::Str("802-11-wireless".to_owned()), wireless),
            (
                MessageItem::Str("802-11-wireless-security".to_owned()),
                security,
            ),
        ])
        .map_err(|_| FlameError::invalid("NetworkManager secure settings are invalid"))?;
        let message = method_call(
            NETWORK_MANAGER_SERVICE,
            NETWORK_MANAGER_ROOT,
            NETWORK_MANAGER_INTERFACE,
            ADD_AND_ACTIVATE_CONNECTION,
        )?
        .append3(settings, object_path(device)?, object_path(access_point)?);
        self.queue(message, PendingRequest::AddAndActivateSecure { attempt })
    }

    fn ssid_for_access_point(&self, access_point: &str) -> FlameResult<String> {
        self.access_points
            .get(access_point)
            .map(|access_point| access_point.ssid.clone())
            .ok_or_else(|| {
                FlameError::new(
                    ErrorCode::NotFound,
                    "NetworkManager access point was not found",
                )
            })
    }

    fn secret_or_attempt_pending(&self) -> bool {
        self.secret_prompt.is_some() || self.secret_context.is_some() || self.attempts.is_busy()
    }

    fn busy_error() -> FlameError {
        FlameError::new(
            ErrorCode::Busy,
            "NetworkManager connection attempt is already pending",
        )
    }

    /// Activate a saved profile. Its failure may only re-prompt; the profile is never deleted.
    fn connect_saved(&mut self, access_point: String) -> FlameResult<()> {
        if self.secret_or_attempt_pending() {
            return Err(Self::busy_error());
        }
        let device = self.device_for_access_point(&access_point)?;
        let profile = self.profile_for_access_point(&access_point)?;
        let attempt = self
            .attempts
            .begin(access_point.clone(), device.clone(), false);
        if let Err(error) = self.queue_activate_connection(attempt, profile, device, access_point) {
            self.attempts.clear();
            return Err(error);
        }
        Ok(())
    }

    /// Open the password prompt for an unknown secured network. Nothing reaches NetworkManager
    /// until the user submits, so a cancelled prompt leaves no profile behind.
    fn prompt_new_network(&mut self, access_point: String) -> FlameResult<()> {
        if self.secret_or_attempt_pending() {
            return Err(Self::busy_error());
        }
        self.device_for_access_point(&access_point)?;
        self.ssid_for_access_point(&access_point)?;
        self.reopen_prompt(SecretPrompt::NewNetwork { access_point })
    }

    fn reopen_prompt(&mut self, prompt: SecretPrompt) -> FlameResult<()> {
        self.cancel_secret_context()?;
        let access_point = match &prompt {
            SecretPrompt::NewNetwork { access_point }
            | SecretPrompt::SavedNetwork { access_point } => access_point.clone(),
        };
        let _ = self.controller.request_secret_from_agent(access_point)?;
        self.secret_prompt = Some(prompt);
        Ok(())
    }

    fn submit_network_secret(
        &mut self,
        request_id: u64,
        generation: u64,
        secret: String,
    ) -> FlameResult<()> {
        if self
            .secret_context
            .as_ref()
            .is_some_and(|context| context.request_id == request_id)
        {
            // NetworkManager is waiting on its SecretAgent request; its reply carries the password.
            let NetworkIntent::SubmitSecret { secret, .. } = self
                .controller
                .request_submit_secret(request_id, generation, secret)?
            else {
                return Err(unsupported("unexpected NetworkManager secret intent"));
            };
            self.secret_prompt = None;
            let Some(context) = self.secret_context.take() else {
                return Err(FlameError::new(
                    ErrorCode::NotFound,
                    "NetworkManager secret request was not found",
                ));
            };
            if let Some(reply) = context.reply {
                self.pump.send(secret_reply(
                    reply,
                    context.setting_name,
                    context.secret_key,
                    secret,
                )?)?;
            }
            return Ok(());
        }
        match self.secret_prompt.clone() {
            Some(SecretPrompt::NewNetwork { access_point }) => {
                self.create_secured_profile(request_id, generation, access_point, secret)
            }
            // A saved profile's password can only be returned to NetworkManager's request.
            Some(SecretPrompt::SavedNetwork { .. }) => Err(unsupported(
                "saved NetworkManager password update requires a NetworkManager secret request",
            )),
            None => Err(FlameError::new(
                ErrorCode::NotFound,
                "NetworkManager secret request was not found",
            )),
        }
    }

    /// Validate the request and lookups first so a failed lookup leaves the prompt intact.
    fn create_secured_profile(
        &mut self,
        request_id: u64,
        generation: u64,
        access_point: String,
        secret: String,
    ) -> FlameResult<()> {
        let device = self.device_for_access_point(&access_point)?;
        let ssid = self.ssid_for_access_point(&access_point)?;
        let NetworkIntent::SubmitSecret { secret, .. } = self
            .controller
            .request_submit_secret(request_id, generation, secret)?
        else {
            return Err(unsupported("unexpected NetworkManager secret intent"));
        };
        self.secret_prompt = None;
        let attempt = self
            .attempts
            .begin(access_point.clone(), device.clone(), true);
        if let Err(error) =
            self.queue_add_and_activate_secure(attempt, ssid, device, access_point, secret)
        {
            self.attempts.clear();
            return Err(error);
        }
        Ok(())
    }

    fn apply_attempt_effect(&mut self, effect: AttemptEffect) -> FlameResult<()> {
        match effect {
            AttemptEffect::None => Ok(()),
            AttemptEffect::RetryNewNetwork {
                access_point,
                profile,
            } => {
                if let Some(profile) = profile {
                    self.profile_ssids.remove(&profile);
                    self.queue_delete_profile(profile)?;
                    self.publish_access_points();
                }
                self.reopen_prompt(SecretPrompt::NewNetwork { access_point })
            }
            AttemptEffect::RetrySavedNetwork { access_point } => {
                self.reopen_prompt(SecretPrompt::SavedNetwork { access_point })
            }
        }
    }

    /// The saved-profile AP a NetworkManager GetSecrets request may be answered for.
    fn saved_secret_access_point(&self) -> Option<String> {
        match &self.secret_prompt {
            Some(SecretPrompt::SavedNetwork { access_point }) => Some(access_point.clone()),
            Some(SecretPrompt::NewNetwork { .. }) => None,
            None => self.attempts.saved_access_point().map(str::to_owned),
        }
    }

    fn clear_saved_prompt(&mut self) {
        if matches!(self.secret_prompt, Some(SecretPrompt::SavedNetwork { .. })) {
            self.secret_prompt = None;
            self.controller.cancel_secret_from_agent();
        }
    }

    fn device_for_access_point(&self, access_point: &str) -> FlameResult<String> {
        self.wifi_devices
            .keys()
            .next()
            .cloned()
            .ok_or_else(|| unsupported("NetworkManager wireless device was not found"))
            .and_then(|device| {
                if self.access_points.contains_key(access_point) {
                    Ok(device)
                } else {
                    Err(FlameError::new(
                        ErrorCode::NotFound,
                        "NetworkManager access point was not found",
                    ))
                }
            })
    }

    fn profile_for_access_point(&self, access_point: &str) -> FlameResult<String> {
        let ssid = self
            .access_points
            .get(access_point)
            .map(|access_point| &access_point.ssid)
            .ok_or_else(|| {
                FlameError::new(
                    ErrorCode::NotFound,
                    "NetworkManager access point was not found",
                )
            })?;
        self.profile_ssids
            .iter()
            .find_map(|(profile, known_ssid)| (known_ssid == ssid).then_some(profile.clone()))
            .ok_or_else(|| {
                FlameError::new(
                    ErrorCode::NotFound,
                    "saved NetworkManager connection profile was not found",
                )
            })
    }

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

    fn queue(&mut self, message: Message, request: PendingRequest) -> FlameResult<()> {
        let serial = self.pump.send(message)?;
        self.pending.insert(serial, request);
        Ok(())
    }

    fn apply(&mut self, incoming: Incoming) -> FlameResult<()> {
        match incoming {
            Incoming::Reply {
                serial,
                is_error,
                items,
            } => self.apply_reply(serial, is_error, items),
            Incoming::OwnerChanged {
                name,
                old_owner,
                new_owner,
            } => {
                if name != NETWORK_MANAGER_SERVICE {
                    return Ok(());
                }
                if new_owner.is_empty() {
                    self.cancel_secret_context()?;
                    self.controller.service_vanished();
                    self.status = NetworkStatus::default();
                    self.wifi_devices.clear();
                    self.access_points.clear();
                    self.profile_ssids.clear();
                    self.secret_prompt = None;
                    self.attempts.clear();
                    self.reconcile_pending = false;
                } else if old_owner != new_owner {
                    self.controller.service_appeared();
                    self.status = NetworkStatus {
                        snapshot: NetworkSnapshot {
                            availability: flamewm_api::system::ServiceAvailability::Available,
                            ..NetworkSnapshot::default()
                        },
                        ..NetworkStatus::default()
                    };
                    self.reconcile_pending = true;
                    self.queue_register_secret_agent()?;
                }
                Ok(())
            }
            Incoming::PropertiesChanged { path, properties } => {
                if !self.controller.is_available() {
                    return Ok(());
                }
                if path == NETWORK_MANAGER_ROOT {
                    self.apply_properties(properties);
                } else if self.wifi_devices.contains_key(&path)
                    || path.starts_with("/org/freedesktop/NetworkManager/Devices/")
                {
                    let generation = self.controller.generation();
                    let refresh = device_signal_refresh(self.wifi_devices.get(&path), &properties);
                    if let Some(device) = self.wifi_devices.get_mut(&path) {
                        if let Some(scan) = properties.last_scan {
                            device.last_scan = Some(scan);
                        }
                    }
                    if properties.last_scan.is_some() {
                        let _ = self.controller.complete_scan(generation);
                    }
                    if refresh.device {
                        // The device reply queues the AP list for wireless devices.
                        self.queue_device_properties(generation, path)?;
                    } else if refresh.access_points {
                        self.queue_access_points(generation, path)?;
                    }
                } else if self.access_points.contains_key(&path) {
                    self.queue_access_point_properties(self.controller.generation(), path)?;
                } else if path.starts_with("/org/freedesktop/NetworkManager/Settings") {
                    self.queue_profiles(self.controller.generation())?;
                }
                Ok(())
            }
            Incoming::SecretAgent(call) => self.apply_secret_agent_call(call),
            Incoming::DeviceStateChanged {
                path,
                new_state,
                reason,
            } => {
                if !self.controller.is_available() {
                    return Ok(());
                }
                if new_state == DEVICE_STATE_ACTIVATED {
                    // The activated profile must appear in the saved-profile list.
                    self.reconcile_pending = true;
                }
                let effect = self.attempts.on_device_state(&path, new_state, reason);
                self.apply_attempt_effect(effect)
            }
        }
    }

    fn apply_reply(
        &mut self,
        serial: u32,
        is_error: bool,
        items: Vec<MessageItem>,
    ) -> FlameResult<()> {
        let Some(request) = self.pending.remove(&serial) else {
            return Ok(());
        };
        if is_error {
            match request {
                // Only a NameOwnerChanged-to-empty signal means NetworkManager is absent.
                PendingRequest::ServiceOwner | PendingRequest::RootProperties { .. } => {
                    self.mark_degraded();
                    return Ok(());
                }
                PendingRequest::AddMatch => {
                    return Err(FlameError::unavailable(
                        "NetworkManager D-Bus match subscription failed",
                    ));
                }
                PendingRequest::Scan { generation } => {
                    // Clear ap_refresh_pending so the next Scan request is not blocked.
                    let _ = self.controller.complete_scan(generation);
                    return Ok(());
                }
                PendingRequest::Action | PendingRequest::RegisterSecretAgent => return Ok(()),
                PendingRequest::ActivateConnection { attempt }
                | PendingRequest::AddAndActivateSecure { attempt } => {
                    let effect = self.attempts.on_request_failed(attempt);
                    return self.apply_attempt_effect(effect);
                }
                PendingRequest::Devices { generation }
                | PendingRequest::DeviceProperties { generation, .. }
                | PendingRequest::AccessPoints { generation }
                | PendingRequest::AccessPointProperties { generation, .. }
                | PendingRequest::Profiles { generation }
                | PendingRequest::ProfileSettings { generation, .. } => {
                    if generation == self.controller.generation() {
                        self.mark_degraded();
                    }
                    return Ok(());
                }
            }
        }
        match request {
            PendingRequest::AddMatch
            | PendingRequest::Action
            | PendingRequest::ActivateConnection { .. }
            | PendingRequest::RegisterSecretAgent => Ok(()),
            PendingRequest::AddAndActivateSecure { attempt } => {
                let Some(item) = items.first() else {
                    return Err(FlameError::invalid(
                        "NetworkManager connection reply is invalid",
                    ));
                };
                let profile = item_object_path(item, "NetworkManager connection reply")?;
                let effect = self.attempts.on_profile_created(attempt, profile);
                self.apply_attempt_effect(effect)
            }
            PendingRequest::Scan { generation } => {
                let _ = self.controller.complete_scan(generation);
                Ok(())
            }
            PendingRequest::ServiceOwner => {
                let owner = one_string(&items, "NetworkManager owner reply")?;
                if !owner.is_empty() {
                    if !self.controller.is_available() {
                        self.controller.service_appeared();
                        self.status.snapshot.availability =
                            flamewm_api::system::ServiceAvailability::Available;
                    }
                    self.reconcile_pending = true;
                    self.queue_register_secret_agent()?;
                }
                Ok(())
            }
            PendingRequest::RootProperties { generation } => {
                if generation != self.controller.generation() {
                    return Ok(());
                }
                let properties = parse_property_dict(items.first())?;
                self.apply_properties(properties);
                Ok(())
            }
            PendingRequest::Devices { generation } => {
                if generation != self.controller.generation() {
                    return Ok(());
                }
                for device in object_paths(&items, "NetworkManager device reply")? {
                    self.queue_device_properties(generation, device)?;
                }
                Ok(())
            }
            PendingRequest::DeviceProperties { generation, device } => {
                if generation != self.controller.generation() {
                    return Ok(());
                }
                let properties = parse_device_properties(items.first())?;
                if properties.is_wifi {
                    let last_scan = self
                        .wifi_devices
                        .get(&device)
                        .and_then(|wifi| wifi.last_scan);
                    self.wifi_devices.insert(
                        device.clone(),
                        WifiDevice {
                            active_connection: properties.active_connection,
                            active_access_point: properties.active_access_point,
                            last_scan,
                        },
                    );
                    self.refresh_active_access_point();
                    if self
                        .wifi_devices
                        .values()
                        .any(|wifi| wifi.active_connection == self.status.active_path)
                    {
                        self.status.snapshot.kind = NetworkKind::Wireless;
                    }
                    self.queue_access_points(generation, device)?;
                }
                Ok(())
            }
            PendingRequest::AccessPoints { generation } => {
                if generation != self.controller.generation() {
                    return Ok(());
                }
                let current = object_paths(&items, "NetworkManager access point reply")?
                    .into_iter()
                    .collect::<BTreeSet<_>>();
                self.access_points.retain(|path, _| current.contains(path));
                for access_point in current {
                    self.queue_access_point_properties(generation, access_point)?;
                }
                self.publish_access_points();
                Ok(())
            }
            PendingRequest::AccessPointProperties {
                generation,
                access_point,
            } => {
                if generation != self.controller.generation() {
                    return Ok(());
                }
                self.access_points
                    .insert(access_point, parse_access_point_properties(items.first())?);
                self.publish_access_points();
                Ok(())
            }
            PendingRequest::Profiles { generation } => {
                if generation != self.controller.generation() {
                    return Ok(());
                }
                for profile in object_paths(&items, "NetworkManager profile reply")? {
                    self.queue_profile_settings(generation, profile)?;
                }
                Ok(())
            }
            PendingRequest::ProfileSettings {
                generation,
                profile,
            } => {
                if generation != self.controller.generation() {
                    return Ok(());
                }
                if let Some(ssid) = parse_profile_ssid(items.first())? {
                    self.profile_ssids.insert(profile, ssid);
                    self.publish_access_points();
                }
                Ok(())
            }
        }
    }

    fn apply_properties(&mut self, properties: NetworkManagerProperties) {
        if let Some(value) = properties.networking_enabled {
            self.status.networking_enabled = value;
            self.status.snapshot.networking_enabled = value;
        }
        if let Some(value) = properties.wireless_enabled {
            self.status.snapshot.wifi_enabled = value;
        }
        if let Some(value) = properties.primary_connection {
            self.status.active_path = if value == "/" { String::new() } else { value };
            self.refresh_active_access_point();
        }
        if let Some(state) = properties.state {
            self.status.snapshot.kind = match state {
                20 | 30 => NetworkKind::Disconnected,
                40 => NetworkKind::Connecting,
                70 => {
                    if self
                        .wifi_devices
                        .values()
                        .any(|wifi| wifi.active_connection == self.status.active_path)
                    {
                        NetworkKind::Wireless
                    } else {
                        NetworkKind::Wired
                    }
                }
                _ => self.status.snapshot.kind,
            };
        }
        self.status.snapshot.availability = flamewm_api::system::ServiceAvailability::Available;
        let generation = self.controller.generation();
        let _ = self
            .controller
            .inject_status(self.status.clone(), generation);
    }

    fn apply_secret_agent_call(&mut self, call: SecretAgentCall) -> FlameResult<()> {
        match call {
            SecretAgentCall::GetSecrets {
                setting_name,
                secret_key,
                reply,
            } => {
                self.cancel_secret_context()?;
                let Some(access_point_path) = self.saved_secret_access_point() else {
                    return self.send_user_canceled(reply);
                };
                let request = self
                    .controller
                    .request_secret_from_agent(access_point_path)?;
                self.secret_context = Some(SecretContext {
                    request_id: request.request_id,
                    setting_name,
                    secret_key,
                    reply,
                });
                Ok(())
            }
            SecretAgentCall::CancelGetSecrets { reply } => {
                self.clear_saved_prompt();
                self.cancel_secret_context()?;
                if let Some(reply) = reply {
                    self.pump.send(reply)?;
                }
                Ok(())
            }
        }
    }

    /// Clears the controller's pending request only when it belongs to the agent context, so an
    /// unrelated open new-network prompt is left intact.
    fn cancel_secret_context(&mut self) -> FlameResult<()> {
        if let Some(context) = self.secret_context.take() {
            self.controller.cancel_secret_from_agent();
            self.send_user_canceled(context.reply)?;
        }
        Ok(())
    }

    fn send_user_canceled(&self, reply: Option<Message>) -> FlameResult<()> {
        if let Some(reply) = reply {
            self.pump.send(user_canceled_reply(reply)?)?;
        }
        Ok(())
    }
}

fn parse_message(message: &Message) -> Option<Result<Incoming, FlameError>> {
    if message.msg_type() == MessageType::MethodReturn || message.msg_type() == MessageType::Error {
        return message.get_reply_serial().map(|serial| {
            Ok(Incoming::Reply {
                serial,
                is_error: message.msg_type() == MessageType::Error,
                items: message.get_items(),
            })
        });
    }
    if message.msg_type() != MessageType::Signal {
        return None;
    }
    let interface = message.interface().map(|value| value.to_string());
    let member = message.member().map(|value| value.to_string());
    let path = message.path().map(|value| value.to_string());
    if interface.as_deref() == Some(DBUS_INTERFACE)
        && member.as_deref() == Some(NAME_OWNER_CHANGED)
        && path.as_deref() == Some(DBUS_ROOT)
    {
        return Some(parse_owner_changed(message.get_items()));
    }
    if interface.as_deref() == Some(PROPERTIES_INTERFACE)
        && member.as_deref() == Some(PROPERTIES_CHANGED)
    {
        let items = message.get_items();
        return Some(parse_properties_changed(items, path.unwrap_or_default()));
    }
    if interface.as_deref() == Some(DEVICE_INTERFACE)
        && member.as_deref() == Some(DEVICE_STATE_CHANGED)
    {
        let items = message.get_items();
        return Some(parse_device_state_changed(items, path.unwrap_or_default()));
    }
    None
}

/// `StateChanged(new_state, old_state, reason)` from a NetworkManager device.
fn parse_device_state_changed(
    items: Vec<MessageItem>,
    path: String,
) -> Result<Incoming, FlameError> {
    let [new_state, _old_state, reason] = items.as_slice() else {
        return Err(FlameError::invalid(
            "NetworkManager device StateChanged arguments are invalid",
        ));
    };
    Ok(Incoming::DeviceStateChanged {
        path,
        new_state: item_u32(new_state, "NetworkManager device new state")?,
        reason: item_u32(reason, "NetworkManager device state reason")?,
    })
}

fn parse_secret_agent_call(message: &Message) -> FlameResult<SecretAgentCall> {
    if message
        .interface()
        .map(|value| value.to_string())
        .as_deref()
        != Some(SECRET_AGENT_INTERFACE)
    {
        return Err(FlameError::invalid(
            "NetworkManager SecretAgent interface is invalid",
        ));
    }
    match message.member().map(|value| value.to_string()).as_deref() {
        Some(GET_SECRETS) => {
            let items = message.get_items();
            if items.len() != 5 || !matches!(items[0], MessageItem::Dict(_)) {
                return Err(FlameError::invalid(
                    "NetworkManager SecretAgent GetSecrets arguments are invalid",
                ));
            }
            let _ = item_object_path(&items[1], "NetworkManager SecretAgent connection path")?;
            let setting_name = item_string(&items[2], "NetworkManager SecretAgent setting name")?;
            let hints = item_strings(&items[3], "NetworkManager SecretAgent hints")?;
            let _ = item_u32(&items[4], "NetworkManager SecretAgent flags")?;
            Ok(SecretAgentCall::GetSecrets {
                setting_name,
                secret_key: hints.into_iter().next().unwrap_or_else(|| "psk".to_owned()),
                reply: (!message.get_no_reply()).then(|| message.method_return()),
            })
        }
        Some(CANCEL_GET_SECRETS) => {
            if !message.get_items().is_empty() {
                return Err(FlameError::invalid(
                    "NetworkManager SecretAgent CancelGetSecrets arguments are invalid",
                ));
            }
            Ok(SecretAgentCall::CancelGetSecrets {
                reply: (!message.get_no_reply()).then(|| message.method_return()),
            })
        }
        _ => Err(FlameError::invalid(
            "NetworkManager SecretAgent method is unsupported",
        )),
    }
}

fn secret_reply(
    reply: Message,
    setting_name: String,
    secret_key: String,
    secret: String,
) -> FlameResult<Message> {
    let setting = MessageItem::from_dict(
        vec![(
            secret_key,
            MessageItem::Variant(Box::new(MessageItem::Str(secret))),
        )]
        .into_iter()
        .map(Ok::<_, FlameError>),
    )
    .map_err(|_| FlameError::invalid("NetworkManager SecretAgent secret reply is invalid"))?;
    let secrets = MessageItem::new_dict(vec![(MessageItem::Str(setting_name), setting)])
        .map_err(|_| FlameError::invalid("NetworkManager SecretAgent secret reply is invalid"))?;
    Ok(reply.append1(secrets))
}

fn user_canceled_reply(reply: Message) -> FlameResult<Message> {
    let name = ErrorName::new(USER_CANCELED)
        .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?;
    let message = CString::new("NetworkManager secret request canceled").map_err(|_| {
        FlameError::new(
            ErrorCode::InternalFailure,
            "SecretAgent cancellation message is invalid",
        )
    })?;
    Ok(reply.error(&name, &message))
}

fn parse_owner_changed(items: Vec<MessageItem>) -> Result<Incoming, FlameError> {
    if items.len() != 3 {
        return Err(FlameError::invalid(
            "NetworkManager owner event has invalid arguments",
        ));
    }
    Ok(Incoming::OwnerChanged {
        name: item_string(&items[0], "owner event name")?,
        old_owner: item_string(&items[1], "owner event old owner")?,
        new_owner: item_string(&items[2], "owner event new owner")?,
    })
}

fn parse_properties_changed(items: Vec<MessageItem>, path: String) -> Result<Incoming, FlameError> {
    if items.len() < 2 {
        return Err(FlameError::invalid(
            "NetworkManager property event has invalid arguments",
        ));
    }
    let _ = item_string(&items[0], "property interface")?;
    Ok(Incoming::PropertiesChanged {
        path,
        properties: parse_property_dict(Some(&items[1]))?,
    })
}

fn parse_property_dict(item: Option<&MessageItem>) -> FlameResult<NetworkManagerProperties> {
    let Some(MessageItem::Dict(dict)) = item else {
        return Err(FlameError::invalid(
            "NetworkManager properties are not a dictionary",
        ));
    };
    let mut properties = NetworkManagerProperties::default();
    for (key, value) in dict.clone().into_vec() {
        let key = item_string(&key, "NetworkManager property name")?;
        let value = unwrap_variant(value);
        match key.as_str() {
            "NetworkingEnabled" => properties.networking_enabled = Some(item_bool(&value, &key)?),
            "WirelessEnabled" => properties.wireless_enabled = Some(item_bool(&value, &key)?),
            "PrimaryConnection" => {
                properties.primary_connection = Some(item_object_path(&value, &key)?)
            }
            "State" => properties.state = Some(item_u32(&value, &key)?),
            "ActiveConnection" => {
                properties.active_connection = Some(item_object_path(&value, &key)?)
            }
            "ActiveAccessPoint" => {
                properties.active_access_point = Some(item_object_path(&value, &key)?)
            }
            "LastScan" => properties.last_scan = Some(item_i64(&value, &key)?),
            _ => {}
        }
    }
    Ok(properties)
}

struct DeviceProperties {
    is_wifi: bool,
    active_connection: String,
    active_access_point: String,
}

fn parse_device_properties(item: Option<&MessageItem>) -> FlameResult<DeviceProperties> {
    let Some(MessageItem::Dict(dict)) = item else {
        return Err(FlameError::invalid(
            "NetworkManager device properties are not a dictionary",
        ));
    };
    let mut is_wifi = false;
    let mut active_connection = String::new();
    let mut active_access_point = String::new();
    for (key, value) in dict.clone().into_vec() {
        let key = item_string(&key, "NetworkManager device property name")?;
        let value = unwrap_variant(value);
        match key.as_str() {
            "DeviceType" => is_wifi = item_u32(&value, &key)? == 2,
            "ActiveConnection" => active_connection = item_object_path(&value, &key)?,
            "ActiveAccessPoint" => active_access_point = item_object_path(&value, &key)?,
            _ => {}
        }
    }
    // NetworkManager reports "/" when no access point is active.
    if active_access_point == "/" {
        active_access_point.clear();
    }
    Ok(DeviceProperties {
        is_wifi,
        active_connection,
        active_access_point,
    })
}

fn parse_access_point_properties(item: Option<&MessageItem>) -> FlameResult<AccessPointProperties> {
    let Some(MessageItem::Dict(dict)) = item else {
        return Err(FlameError::invalid(
            "NetworkManager access point properties are not a dictionary",
        ));
    };
    let mut properties = AccessPointProperties::default();
    let mut flags = 0_u32;
    let mut wpa_flags = 0_u32;
    let mut rsn_flags = 0_u32;
    for (key, value) in dict.clone().into_vec() {
        let key = item_string(&key, "NetworkManager access point property name")?;
        let value = unwrap_variant(value);
        match key.as_str() {
            "Ssid" => {
                properties.ssid = String::from_utf8_lossy(&item_bytes(&value, &key)?).into_owned()
            }
            "Strength" => properties.strength_percent = item_byte(&value, &key)?,
            "Flags" => flags = item_u32(&value, &key)?,
            "WpaFlags" => wpa_flags = item_u32(&value, &key)?,
            "RsnFlags" => rsn_flags = item_u32(&value, &key)?,
            _ => {}
        }
    }
    properties.secured = flags & 1 != 0 || wpa_flags != 0 || rsn_flags != 0;
    Ok(properties)
}

fn parse_profile_ssid(item: Option<&MessageItem>) -> FlameResult<Option<String>> {
    let Some(MessageItem::Dict(sections)) = item else {
        return Err(FlameError::invalid(
            "NetworkManager profile settings are not a dictionary",
        ));
    };
    for (section, value) in sections.clone().into_vec() {
        if item_string(&section, "NetworkManager profile section")? != "802-11-wireless" {
            continue;
        }
        let MessageItem::Dict(properties) = unwrap_variant(value) else {
            return Err(FlameError::invalid(
                "NetworkManager wireless profile section is not a dictionary",
            ));
        };
        for (key, value) in properties.into_vec() {
            if item_string(&key, "NetworkManager profile property")? == "ssid" {
                return Ok(Some(
                    String::from_utf8_lossy(&item_bytes(
                        &unwrap_variant(value),
                        "NetworkManager profile ssid",
                    )?)
                    .into_owned(),
                ));
            }
        }
    }
    Ok(None)
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

fn one_string(items: &[MessageItem], context: &str) -> FlameResult<String> {
    items
        .first()
        .ok_or_else(|| FlameError::invalid(context))
        .and_then(|item| item_string(item, context))
}

fn item_string(item: &MessageItem, context: &str) -> FlameResult<String> {
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

fn item_strings(item: &MessageItem, context: &str) -> FlameResult<Vec<String>> {
    let MessageItem::Array(values) = item else {
        return Err(FlameError::invalid(context));
    };
    values
        .clone()
        .into_vec()
        .into_iter()
        .map(|value| item_string(&value, context))
        .collect()
}

fn item_bool(item: &MessageItem, context: &str) -> FlameResult<bool> {
    match item {
        MessageItem::Bool(value) => Ok(*value),
        _ => Err(FlameError::invalid(context)),
    }
}

fn item_u32(item: &MessageItem, context: &str) -> FlameResult<u32> {
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

fn unwrap_variant(item: MessageItem) -> MessageItem {
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

/// Queue one RequestScan per device. A failed queue clears the controller's pending scan so a
/// later Scan is not blocked; replies already in flight complete harmlessly.
fn queue_scan_requests<F>(
    controller: &mut NetworkController,
    devices: &[String],
    mut queue_one: F,
) -> FlameResult<()>
where
    F: FnMut(&str) -> FlameResult<()>,
{
    for device in devices {
        if let Err(error) = queue_one(device) {
            let generation = controller.generation();
            let _ = controller.complete_scan(generation);
            return Err(error);
        }
    }
    Ok(())
}

/// `RegisterWithCapabilities(s identifier, u capabilities)`. The agent object path is not an
/// argument: NetworkManager calls back on the caller's unique name at SECRET_AGENT_PATH.
fn secret_agent_registration_message() -> FlameResult<Message> {
    method_call(
        NETWORK_MANAGER_SERVICE,
        SECRET_AGENT_MANAGER_PATH,
        SECRET_AGENT_MANAGER_INTERFACE,
        REGISTER_WITH_CAPABILITIES,
    )
    .map(|message| message.append2(SECRET_AGENT_IDENTIFIER.to_owned(), 0_u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_with_capabilities_uses_identifier_and_capabilities_only() {
        let message = secret_agent_registration_message().expect("valid registration message");
        assert_eq!(
            message.member().map(|m| m.to_string()).as_deref(),
            Some(REGISTER_WITH_CAPABILITIES)
        );
        let items = message.get_items();
        let signature: String = items
            .iter()
            .map(|item| item.signature().to_string())
            .collect();
        assert_eq!(signature, "su");
        assert!(matches!(&items[0], MessageItem::Str(s) if s == SECRET_AGENT_IDENTIFIER));
        assert!(matches!(&items[1], MessageItem::UInt32(0)));
    }

    #[test]
    fn method_call_helpers_match_direct_construction() {
        let helper = get_all_message(
            NETWORK_MANAGER_SERVICE,
            NETWORK_MANAGER_ROOT,
            NETWORK_MANAGER_INTERFACE,
        )
        .expect("valid GetAll message");
        let direct = Message::new_method_call(
            NETWORK_MANAGER_SERVICE,
            NETWORK_MANAGER_ROOT,
            PROPERTIES_INTERFACE,
            GET_ALL,
        )
        .expect("valid GetAll message")
        .append1(NETWORK_MANAGER_INTERFACE.to_owned());
        assert_eq!(helper.destination(), direct.destination());
        assert_eq!(helper.path(), direct.path());
        assert_eq!(helper.interface(), direct.interface());
        assert_eq!(helper.member(), direct.member());
        assert_eq!(helper.get_items(), direct.get_items());
        assert_eq!(
            helper.member().map(|m| m.to_string()).as_deref(),
            Some(GET_ALL)
        );

        let devices = method_call(
            NETWORK_MANAGER_SERVICE,
            NETWORK_MANAGER_ROOT,
            NETWORK_MANAGER_INTERFACE,
            GET_DEVICES,
        )
        .expect("valid GetDevices message");
        assert_eq!(
            devices.member().map(|m| m.to_string()).as_deref(),
            Some(GET_DEVICES)
        );
        assert_eq!(
            devices.interface().map(|i| i.to_string()).as_deref(),
            Some(NETWORK_MANAGER_INTERFACE)
        );
        assert!(devices.get_items().is_empty());
    }

    #[test]
    fn access_point_parser_decodes_ssid_security_and_strength() {
        let values = vec![
            ("Ssid".to_owned(), MessageItem::from(&b"FlameNet"[..])),
            ("Strength".to_owned(), MessageItem::Byte(87)),
            ("Flags".to_owned(), MessageItem::UInt32(1)),
            ("WpaFlags".to_owned(), MessageItem::UInt32(0)),
            ("RsnFlags".to_owned(), MessageItem::UInt32(0)),
        ];
        let item = MessageItem::from_dict(values.into_iter().map(Ok::<_, FlameError>))
            .expect("valid D-Bus dictionary");
        assert_eq!(
            parse_access_point_properties(Some(&item)).expect("valid access point"),
            AccessPointProperties {
                ssid: "FlameNet".to_owned(),
                strength_percent: 87,
                secured: true,
            }
        );
    }

    #[test]
    fn header_label_never_claims_connection_when_disconnected() {
        assert_eq!(header_label_for(None, false), "Not connected");
        let connected = AccessPoint {
            path: "/nm/ap/1".to_owned(),
            ssid: "FlameNet".to_owned(),
            strength_percent: 80,
            secured: true,
            known: true,
        };
        assert_eq!(header_label_for(Some(&connected), true), "FlameNet");
    }

    #[test]
    fn header_label_keeps_placeholder_free_when_active_ap_absent_from_scan() {
        // Active AP path set but no matching scan entry: no name is known, so the
        // header must not claim "Not connected" (the shell shows "Hidden network").
        assert_eq!(header_label_for(None, true), "");
        assert_eq!(header_label_for(None, false), "Not connected");
    }

    #[test]
    fn profile_parser_ignores_non_wireless_profiles() {
        let connection = MessageItem::from_dict(
            vec![("id".to_owned(), MessageItem::Str("wired".to_owned()))]
                .into_iter()
                .map(Ok::<_, FlameError>),
        )
        .expect("valid profile section");
        let item = MessageItem::new_dict(vec![(
            MessageItem::Str("connection".to_owned()),
            connection,
        )])
        .expect("valid profile settings");
        assert_eq!(
            parse_profile_ssid(Some(&item)).expect("valid profile"),
            None
        );
    }

    #[test]
    fn secret_agent_parser_preserves_requested_setting_without_secret_data() {
        let mut message = Message::new_method_call(
            NETWORK_MANAGER_SERVICE,
            SECRET_AGENT_PATH,
            SECRET_AGENT_INTERFACE,
            GET_SECRETS,
        )
        .expect("valid SecretAgent call");
        // libdbus aborts on method_return() when the call serial is 0; use a
        // synthetic nonzero serial so the parser can own a real reply.
        message.set_serial(1);
        message.append_items(&[
            empty_dict(),
            MessageItem::ObjectPath(
                object_path("/org/freedesktop/NetworkManager/Settings/1".to_owned())
                    .expect("valid path"),
            ),
            MessageItem::Str("802-11-wireless-security".to_owned()),
            MessageItem::from(&["psk", "unused"][..]),
            MessageItem::UInt32(0),
        ]);
        match parse_secret_agent_call(&message).expect("valid SecretAgent call") {
            SecretAgentCall::GetSecrets {
                setting_name,
                secret_key,
                ..
            } => {
                assert_eq!(setting_name, "802-11-wireless-security");
                assert_eq!(secret_key, "psk");
            }
            SecretAgentCall::CancelGetSecrets { .. } => panic!("GetSecrets parsed as cancel"),
        }
    }

    #[test]
    fn device_signal_requests_ap_refresh_only_for_changed_ap_or_scan() {
        let cached = WifiDevice {
            active_connection: "/c/3".to_owned(),
            active_access_point: "/ap/7".to_owned(),
            last_scan: Some(10),
        };
        let signal = |ap: Option<&str>, scan: Option<i64>| NetworkManagerProperties {
            active_access_point: ap.map(str::to_owned),
            last_scan: scan,
            ..NetworkManagerProperties::default()
        };
        // New ActiveAccessPoint: device read, which queues the AP list.
        let changed_ap = device_signal_refresh(Some(&cached), &signal(Some("/ap/8"), None));
        assert!(changed_ap.device && changed_ap.access_points);
        // New LastScan only: AP list refresh without a device read.
        let changed_scan = device_signal_refresh(Some(&cached), &signal(None, Some(11)));
        assert!(!changed_scan.device && changed_scan.access_points);
        // ActiveAccessPoint cleared to "/" is a change to "no AP".
        let cleared = device_signal_refresh(Some(&cached), &signal(Some("/"), None));
        assert!(cleared.access_points);
        // Unchanged device: nothing requested.
        let unchanged = device_signal_refresh(Some(&cached), &signal(Some("/ap/7"), Some(10)));
        assert_eq!(unchanged, DeviceRefresh::default());
        // Undiscovered device: device read so the adapter learns it.
        let unknown = device_signal_refresh(None, &signal(None, Some(1)));
        assert!(unknown.device && !unknown.access_points);
    }

    #[test]
    fn failed_scan_queue_does_not_block_next_scan() {
        let mut controller = NetworkController::default();
        controller.service_appeared();
        assert!(matches!(
            controller.request_scan(),
            Ok(Some(
                flamewm_integrations_core::network::NetworkIntent::Scan
            ))
        ));
        let devices = vec!["/org/freedesktop/NetworkManager/Devices/1".to_owned()];
        let result = queue_scan_requests(&mut controller, &devices, |_| {
            Err(FlameError::unavailable("injected queue failure"))
        });
        assert!(result.is_err());
        assert!(matches!(
            controller.request_scan(),
            Ok(Some(
                flamewm_integrations_core::network::NetworkIntent::Scan
            ))
        ));
    }

    #[test]
    fn device_parser_reads_active_access_point_and_treats_slash_as_none() {
        let build = |active_ap: &str| {
            MessageItem::from_dict(
                vec![
                    ("DeviceType".to_owned(), MessageItem::UInt32(2)),
                    (
                        "ActiveConnection".to_owned(),
                        MessageItem::ObjectPath(
                            object_path(
                                "/org/freedesktop/NetworkManager/ActiveConnection/3".to_owned(),
                            )
                            .expect("valid path"),
                        ),
                    ),
                    (
                        "ActiveAccessPoint".to_owned(),
                        MessageItem::ObjectPath(
                            object_path(active_ap.to_owned()).expect("valid path"),
                        ),
                    ),
                ]
                .into_iter()
                .map(Ok::<_, FlameError>),
            )
            .expect("valid device properties")
        };
        let item = build("/org/freedesktop/NetworkManager/AccessPoint/7");
        let parsed = parse_device_properties(Some(&item)).expect("valid device");
        assert!(parsed.is_wifi);
        assert_eq!(
            parsed.active_connection,
            "/org/freedesktop/NetworkManager/ActiveConnection/3"
        );
        assert_eq!(
            parsed.active_access_point,
            "/org/freedesktop/NetworkManager/AccessPoint/7"
        );
        let none = build("/");
        assert_eq!(
            parse_device_properties(Some(&none))
                .expect("valid device")
                .active_access_point,
            ""
        );
    }

    const AP: &str = "/org/freedesktop/NetworkManager/AccessPoint/9";
    const DEV: &str = "/org/freedesktop/NetworkManager/Devices/2";
    const PROFILE: &str = "/org/freedesktop/NetworkManager/Settings/5";

    #[test]
    fn successful_new_network_attempt_keeps_its_profile() {
        let mut attempts = ConnectAttempts::default();
        let id = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
        assert_eq!(
            attempts.on_profile_created(id, PROFILE.to_owned()),
            AttemptEffect::None
        );
        assert_eq!(attempts.on_device_state(DEV, 40, 0), AttemptEffect::None);
        assert_eq!(attempts.on_device_state(DEV, 100, 0), AttemptEffect::None);
        assert!(!attempts.is_busy());
    }

    #[test]
    fn rejected_new_network_deletes_only_its_created_profile_and_reprompts() {
        let mut attempts = ConnectAttempts::default();
        let id = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
        let _ = attempts.on_profile_created(id, PROFILE.to_owned());
        assert_eq!(attempts.on_device_state(DEV, 40, 0), AttemptEffect::None);
        assert_eq!(
            attempts.on_device_state(DEV, 120, DEVICE_REASON_SUPPLICANT_DISCONNECT),
            AttemptEffect::RetryNewNetwork {
                access_point: AP.to_owned(),
                profile: Some(PROFILE.to_owned()),
            }
        );
        assert!(!attempts.is_busy());
    }

    #[test]
    fn rejection_before_profile_reply_defers_the_delete_until_the_profile_is_known() {
        let mut attempts = ConnectAttempts::default();
        let id = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
        assert_eq!(attempts.on_device_state(DEV, 40, 0), AttemptEffect::None);
        assert_eq!(
            attempts.on_device_state(DEV, 120, DEVICE_REASON_NO_SECRETS),
            AttemptEffect::None
        );
        assert!(attempts.is_busy());
        assert_eq!(
            attempts.on_profile_created(id, PROFILE.to_owned()),
            AttemptEffect::RetryNewNetwork {
                access_point: AP.to_owned(),
                profile: Some(PROFILE.to_owned()),
            }
        );
        assert!(!attempts.is_busy());
    }

    #[test]
    fn rejected_add_reprompts_without_deleting_anything() {
        let mut attempts = ConnectAttempts::default();
        let id = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
        assert_eq!(
            attempts.on_request_failed(id),
            AttemptEffect::RetryNewNetwork {
                access_point: AP.to_owned(),
                profile: None,
            }
        );
    }

    #[test]
    fn rejected_saved_profile_reprompts_and_is_never_deleted() {
        let mut attempts = ConnectAttempts::default();
        let _ = attempts.begin(AP.to_owned(), DEV.to_owned(), false);
        assert_eq!(attempts.on_device_state(DEV, 40, 0), AttemptEffect::None);
        // The effect type carries no profile, so a saved profile cannot be deleted from it.
        assert_eq!(
            attempts.on_device_state(DEV, 120, DEVICE_REASON_SUPPLICANT_CONFIG_FAILED),
            AttemptEffect::RetrySavedNetwork {
                access_point: AP.to_owned(),
            }
        );
        assert!(!attempts.is_busy());
    }

    #[test]
    fn saved_profile_activation_error_is_not_a_password_prompt() {
        let mut attempts = ConnectAttempts::default();
        let id = attempts.begin(AP.to_owned(), DEV.to_owned(), false);
        assert_eq!(attempts.on_request_failed(id), AttemptEffect::None);
        assert!(!attempts.is_busy());
    }

    #[test]
    fn non_credential_failure_clears_without_prompt_or_delete() {
        let mut attempts = ConnectAttempts::default();
        let id = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
        let _ = attempts.on_profile_created(id, PROFILE.to_owned());
        let _ = attempts.on_device_state(DEV, 40, 0);
        // NMDeviceStateReason 5 is IP_CONFIG_UNAVAILABLE, not a credential failure.
        assert_eq!(attempts.on_device_state(DEV, 120, 5), AttemptEffect::None);
        assert!(!attempts.is_busy());
    }

    #[test]
    fn previous_connection_disconnect_before_activation_is_ignored() {
        let mut attempts = ConnectAttempts::default();
        let _ = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
        assert_eq!(attempts.on_device_state(DEV, 30, 0), AttemptEffect::None);
        assert!(attempts.is_busy());
    }

    #[test]
    fn stale_profile_reply_and_other_device_signals_are_ignored() {
        let mut attempts = ConnectAttempts::default();
        let stale = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
        let current = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
        assert_ne!(stale, current);
        assert_eq!(
            attempts.on_profile_created(stale, PROFILE.to_owned()),
            AttemptEffect::None
        );
        assert_eq!(
            attempts.on_device_state("/org/freedesktop/NetworkManager/Devices/7", 120, 8),
            AttemptEffect::None
        );
        assert!(attempts.is_busy());
    }

    #[test]
    fn cancelled_attempt_clears_and_later_failure_has_no_effect() {
        let mut attempts = ConnectAttempts::default();
        let _ = attempts.begin(AP.to_owned(), DEV.to_owned(), false);
        attempts.clear();
        assert!(!attempts.is_busy());
        assert_eq!(
            attempts.on_device_state(DEV, 120, DEVICE_REASON_NO_SECRETS),
            AttemptEffect::None
        );
    }

    #[test]
    fn saved_attempt_answers_agent_requests_but_new_attempt_does_not() {
        let mut attempts = ConnectAttempts::default();
        assert_eq!(attempts.saved_access_point(), None);
        let _ = attempts.begin(AP.to_owned(), DEV.to_owned(), true);
        assert_eq!(attempts.saved_access_point(), None);
        attempts.clear();
        let _ = attempts.begin(AP.to_owned(), DEV.to_owned(), false);
        assert_eq!(attempts.saved_access_point(), Some(AP));
    }

    #[test]
    fn device_state_signal_parses_new_state_and_reason() {
        let items = vec![
            MessageItem::UInt32(120),
            MessageItem::UInt32(40),
            MessageItem::UInt32(DEVICE_REASON_SUPPLICANT_TIMEOUT),
        ];
        match parse_device_state_changed(items, DEV.to_owned()) {
            Ok(Incoming::DeviceStateChanged {
                path,
                new_state,
                reason,
            }) => assert_eq!((path.as_str(), new_state, reason), (DEV, 120, 11)),
            _ => panic!("device StateChanged parsed incorrectly"),
        }
        let short = vec![MessageItem::UInt32(120)];
        assert!(parse_device_state_changed(short, DEV.to_owned()).is_err());
    }
}

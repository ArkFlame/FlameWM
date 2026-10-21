use super::actions::*;
use super::rules::*;
use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ConnectAttempt {
    pub(super) id: u64,
    pub(super) access_point: String,
    pub(super) device: String,
    pub(super) created: bool,
    pub(super) profile: Option<String>,
    /// Set once the device enters an activation state. Earlier DISCONNECTED signals belong to
    /// the connection that was active before this attempt.
    pub(super) activating: bool,
    /// Credentials were rejected before the add reply named the created profile.
    pub(super) auth_failed: bool,
}

/// Follow-up the provider must perform for an attempt signal. The tracker only decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AttemptEffect {
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
pub(super) struct ConnectAttempts {
    pub(super) current: Option<ConnectAttempt>,
    pub(super) next_id: u64,
}

impl ConnectAttempts {
    pub(super) fn begin(&mut self, access_point: String, device: String, created: bool) -> u64 {
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

    pub(super) fn is_busy(&self) -> bool {
        self.current.is_some()
    }

    pub(super) fn clear(&mut self) {
        self.current = None;
    }

    pub(super) fn is_current(&self, id: u64) -> bool {
        self.current
            .as_ref()
            .is_some_and(|attempt| attempt.id == id)
    }

    pub(super) fn saved_access_point(&self) -> Option<&str> {
        self.current
            .as_ref()
            .filter(|attempt| !attempt.created)
            .map(|attempt| attempt.access_point.as_str())
    }

    /// The add reply names the profile this attempt created.
    pub(super) fn on_profile_created(&mut self, id: u64, profile: String) -> AttemptEffect {
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
    pub(super) fn on_request_failed(&mut self, id: u64) -> AttemptEffect {
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

    pub(super) fn on_device_state(
        &mut self,
        device: &str,
        state: u32,
        reason: u32,
    ) -> AttemptEffect {
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

    pub(super) fn on_credentials_rejected(&mut self) -> AttemptEffect {
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

pub(super) fn retry_effect(attempt: ConnectAttempt) -> AttemptEffect {
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

pub(super) fn is_credential_reason(reason: u32) -> bool {
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
pub(super) struct AccessPointProperties {
    pub(super) ssid: String,
    pub(super) strength_percent: u8,
    pub(super) secured: bool,
}

pub(super) fn parse_message(message: &Message) -> Option<Result<Incoming, FlameError>> {
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
pub(super) fn parse_device_state_changed(
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

/// Answers a SecretAgent call with a D-Bus error so NetworkManager gets a reply and the reactor
/// keeps running. Nothing secret-bearing is echoed: only the fixed error name and text.
fn secret_agent_error(message: &Message, name: &str, text: &str) -> FlameResult<SecretAgentCall> {
    let name =
        ErrorName::new(name).map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?;
    let text =
        CString::new(text).map_err(|_| FlameError::invalid("SecretAgent error text is invalid"))?;
    Ok(SecretAgentCall::SendReply {
        reply: (!message.get_no_reply()).then(|| message.error(&name, &text)),
    })
}

fn parse_get_secrets_args(items: &[MessageItem]) -> FlameResult<(String, Vec<String>)> {
    if items.len() != 5 || !matches!(items[0], MessageItem::Dict(_)) {
        return Err(FlameError::invalid(
            "NetworkManager SecretAgent GetSecrets arguments are invalid",
        ));
    }
    let _ = item_object_path(&items[1], "NetworkManager SecretAgent connection path")?;
    let setting_name = item_string(&items[2], "NetworkManager SecretAgent setting name")?;
    let hints = item_strings(&items[3], "NetworkManager SecretAgent hints")?;
    let _ = item_u32(&items[4], "NetworkManager SecretAgent flags")?;
    Ok((setting_name, hints))
}

pub(super) fn parse_secret_agent_call(message: &Message) -> FlameResult<SecretAgentCall> {
    if message
        .interface()
        .map(|value| value.to_string())
        .as_deref()
        != Some(SECRET_AGENT_INTERFACE)
    {
        return secret_agent_error(
            message,
            UNKNOWN_INTERFACE,
            "NetworkManager SecretAgent interface is unsupported",
        );
    }
    match message.member().map(|value| value.to_string()).as_deref() {
        Some(GET_SECRETS) => {
            let Ok((setting_name, hints)) = parse_get_secrets_args(&message.get_items()) else {
                return secret_agent_error(
                    message,
                    INVALID_ARGS,
                    "NetworkManager SecretAgent GetSecrets arguments are invalid",
                );
            };
            // An empty key means NetworkManager sent no hints; apply rejects that request.
            Ok(SecretAgentCall::GetSecrets {
                setting_name,
                secret_key: hints.into_iter().next().unwrap_or_default(),
                reply: (!message.get_no_reply()).then(|| message.method_return()),
            })
        }
        Some(CANCEL_GET_SECRETS) => {
            if !message.get_items().is_empty() {
                return secret_agent_error(
                    message,
                    INVALID_ARGS,
                    "NetworkManager SecretAgent CancelGetSecrets arguments are invalid",
                );
            }
            Ok(SecretAgentCall::CancelGetSecrets {
                reply: (!message.get_no_reply()).then(|| message.method_return()),
            })
        }
        Some(SAVE_SECRETS) | Some(DELETE_SECRETS) => Ok(SecretAgentCall::SendReply {
            reply: (!message.get_no_reply()).then(|| message.method_return()),
        }),
        _ => secret_agent_error(
            message,
            UNKNOWN_METHOD,
            "NetworkManager SecretAgent method is unsupported",
        ),
    }
}

pub(super) fn parse_owner_changed(items: Vec<MessageItem>) -> Result<Incoming, FlameError> {
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

pub(super) fn parse_properties_changed(
    items: Vec<MessageItem>,
    path: String,
) -> Result<Incoming, FlameError> {
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

/// Entries of a dictionary reply item; `what` names it in the not-a-dictionary error.
fn dict_entries(
    item: Option<&MessageItem>,
    what: &str,
) -> FlameResult<Vec<(MessageItem, MessageItem)>> {
    let Some(MessageItem::Dict(dict)) = item else {
        return Err(FlameError::invalid(format!("{what} are not a dictionary")));
    };
    Ok(dict.clone().into_vec())
}

pub(super) fn parse_property_dict(
    item: Option<&MessageItem>,
) -> FlameResult<NetworkManagerProperties> {
    let mut properties = NetworkManagerProperties::default();
    for (key, value) in dict_entries(item, "NetworkManager properties")? {
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

pub(super) struct DeviceProperties {
    pub(super) is_wifi: bool,
    pub(super) active_connection: String,
    pub(super) active_access_point: String,
}

pub(super) fn parse_device_properties(item: Option<&MessageItem>) -> FlameResult<DeviceProperties> {
    let mut is_wifi = false;
    let mut active_connection = String::new();
    let mut active_access_point = String::new();
    for (key, value) in dict_entries(item, "NetworkManager device properties")? {
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

pub(super) fn parse_access_point_properties(
    item: Option<&MessageItem>,
) -> FlameResult<AccessPointProperties> {
    let mut properties = AccessPointProperties::default();
    let mut flags = 0_u32;
    let mut wpa_flags = 0_u32;
    let mut rsn_flags = 0_u32;
    for (key, value) in dict_entries(item, "NetworkManager access point properties")? {
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

pub(super) fn parse_profile_ssid(item: Option<&MessageItem>) -> FlameResult<Option<String>> {
    for (section, value) in dict_entries(item, "NetworkManager profile settings")? {
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

impl NetworkManagerProvider {
    pub(super) fn apply(&mut self, incoming: Incoming) -> FlameResult<()> {
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

    pub(super) fn apply_reply(
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
                | PendingRequest::AddAndActivateSecure { attempt }
                | PendingRequest::SavedSettings { attempt, .. }
                | PendingRequest::UpdateSavedProfile { attempt, .. } => {
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
            PendingRequest::SavedSettings {
                attempt,
                profile,
                device,
                access_point,
                secret,
            } => {
                if !self.attempts.is_current(attempt) {
                    return Ok(());
                }
                let settings = items
                    .first()
                    .ok_or_else(|| {
                        FlameError::invalid("NetworkManager profile settings reply is invalid")
                    })
                    .and_then(|item| saved_profile_update_settings(item.clone(), &secret));
                let settings = self.clear_attempt_on_error(settings)?;
                let queued = self.queue_update_saved_profile(
                    attempt,
                    profile,
                    device,
                    access_point,
                    settings,
                );
                self.clear_attempt_on_error(queued)
            }
            PendingRequest::UpdateSavedProfile {
                attempt,
                profile,
                device,
                access_point,
            } => {
                if !self.attempts.is_current(attempt) {
                    return Ok(());
                }
                let queued = self.queue_activate_connection(attempt, profile, device, access_point);
                self.clear_attempt_on_error(queued)
            }
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

    pub(super) fn apply_properties(&mut self, properties: NetworkManagerProperties) {
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

    pub(super) fn apply_secret_agent_call(&mut self, call: SecretAgentCall) -> FlameResult<()> {
        match call {
            SecretAgentCall::GetSecrets {
                setting_name,
                secret_key,
                reply,
            } => {
                self.cancel_secret_context()?;
                if secret_key.is_empty() {
                    eprintln!(
                        "flamewm-integrations-linux: NetworkManager secret request for {setting_name} had no secret hints; cancelled"
                    );
                    return self.send_user_canceled(reply);
                }
                let Some(access_point_path) = self.saved_secret_access_point() else {
                    eprintln!(
                        "flamewm-integrations-linux: NetworkManager secret request for {setting_name} has no saved access point; cancelled"
                    );
                    self.send_user_canceled(reply)?;
                    return Err(FlameError::new(
                        ErrorCode::NotFound,
                        "NetworkManager secret request has no saved access point",
                    ));
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
            SecretAgentCall::SendReply { reply } => {
                if let Some(reply) = reply {
                    self.pump.send(reply)?;
                }
                Ok(())
            }
        }
    }

    /// Clears the controller's pending request only when it belongs to the agent context, so an
    /// unrelated open new-network prompt is left intact.
    pub(super) fn cancel_secret_context(&mut self) -> FlameResult<()> {
        if let Some(context) = self.secret_context.take() {
            self.controller.cancel_secret_from_agent();
            self.send_user_canceled(context.reply)?;
        }
        Ok(())
    }

    pub(super) fn send_user_canceled(&self, reply: Option<Message>) -> FlameResult<()> {
        if let Some(reply) = reply {
            self.pump.send(user_canceled_reply(reply)?)?;
        }
        Ok(())
    }
}

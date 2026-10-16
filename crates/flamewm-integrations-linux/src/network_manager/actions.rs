use super::*;
use super::reply::*;

pub(super) enum SecretAgentCall {
    GetSecrets {
        setting_name: String,
        secret_key: String,
        reply: Option<Message>,
    },
    CancelGetSecrets {
        reply: Option<Message>,
    },
    /// A reply owed to NetworkManager that changes no secret state: an empty return for
    /// SaveSecrets/DeleteSecrets (the agent stores nothing), or UnknownMethod for other members.
    SendReply {
        reply: Option<Message>,
    },
}

pub(super) struct SecretContext {
    pub(super) request_id: u64,
    pub(super) setting_name: String,
    pub(super) secret_key: String,
    pub(super) reply: Option<Message>,
}

/// Which password request is open. A new network's password creates its profile on submit;
/// a saved profile's password is accepted only as the answer to NetworkManager's own request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SecretPrompt {
    NewNetwork { access_point: String },
    SavedNetwork { access_point: String },
}

/// One Wi-Fi activation. Only a profile this attempt created may be deleted on failure.
/// Which profile write a submitted password performs: a new network creates its profile,
/// a known network's saved profile is updated in place before activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SecretSubmitPlan {
    CreateProfile { access_point: String },
    UpdateSavedProfile { access_point: String },
}

pub(super) fn secret_submit_plan(prompt: &SecretPrompt) -> SecretSubmitPlan {
    match prompt {
        SecretPrompt::NewNetwork { access_point } => SecretSubmitPlan::CreateProfile {
            access_point: access_point.clone(),
        },
        SecretPrompt::SavedNetwork { access_point } => SecretSubmitPlan::UpdateSavedProfile {
            access_point: access_point.clone(),
        },
    }
}

impl NetworkManagerProvider {
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
}

impl NetworkManagerProvider {
    pub(super) fn add_match(&mut self, rule: &str) -> FlameResult<()> {
        let message = method_call(DBUS_SERVICE, DBUS_ROOT, DBUS_INTERFACE, ADD_MATCH)?
            .append1(rule.to_owned());
        self.queue(message, PendingRequest::AddMatch)
    }

    pub(super) fn queue_service_owner_request(&mut self) -> FlameResult<()> {
        let message = method_call(DBUS_SERVICE, DBUS_ROOT, DBUS_INTERFACE, "GetNameOwner")?
            .append1(NETWORK_MANAGER_SERVICE.to_owned());
        self.queue(message, PendingRequest::ServiceOwner)
    }

    pub(super) fn queue_register_secret_agent(&mut self) -> FlameResult<()> {
        let message = secret_agent_registration_message()?;
        self.queue(message, PendingRequest::RegisterSecretAgent)
    }

    pub(super) fn queue_devices(&mut self, generation: u64) -> FlameResult<()> {
        let message = method_call(
            NETWORK_MANAGER_SERVICE,
            NETWORK_MANAGER_ROOT,
            NETWORK_MANAGER_INTERFACE,
            GET_DEVICES,
        )?;
        self.queue(message, PendingRequest::Devices { generation })
    }

    pub(super) fn queue_device_properties(&mut self, generation: u64, device: String) -> FlameResult<()> {
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

    pub(super) fn queue_access_points(&mut self, generation: u64, device: String) -> FlameResult<()> {
        let message = method_call(
            NETWORK_MANAGER_SERVICE,
            &device,
            "org.freedesktop.NetworkManager.Device.Wireless",
            GET_ACCESS_POINTS,
        )?;
        self.queue(message, PendingRequest::AccessPoints { generation })
    }

    pub(super) fn queue_access_point_properties(
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

    pub(super) fn queue_profiles(&mut self, generation: u64) -> FlameResult<()> {
        let message = method_call(
            NETWORK_MANAGER_SERVICE,
            "/org/freedesktop/NetworkManager/Settings",
            "org.freedesktop.NetworkManager.Settings",
            LIST_CONNECTIONS,
        )?;
        self.queue(message, PendingRequest::Profiles { generation })
    }

    pub(super) fn queue_profile_settings(&mut self, generation: u64, profile: String) -> FlameResult<()> {
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

    pub(super) fn queue_activate_connection(
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

    pub(super) fn queue_saved_settings(
        &mut self,
        attempt: u64,
        profile: String,
        device: String,
        access_point: String,
        secret: String,
    ) -> FlameResult<()> {
        let message = method_call(
            NETWORK_MANAGER_SERVICE,
            &profile,
            SETTINGS_CONNECTION_INTERFACE,
            GET_SETTINGS,
        )?;
        self.queue(
            message,
            PendingRequest::SavedSettings {
                attempt,
                profile,
                device,
                access_point,
                secret,
            },
        )
    }

    pub(super) fn queue_update_saved_profile(
        &mut self,
        attempt: u64,
        profile: String,
        device: String,
        access_point: String,
        settings: MessageItem,
    ) -> FlameResult<()> {
        let message = update_connection_message(&profile, settings)?;
        self.queue(
            message,
            PendingRequest::UpdateSavedProfile {
                attempt,
                profile,
                device,
                access_point,
            },
        )
    }

    pub(super) fn queue_delete_profile(&mut self, profile: String) -> FlameResult<()> {
        let message = method_call(
            NETWORK_MANAGER_SERVICE,
            &profile,
            SETTINGS_CONNECTION_INTERFACE,
            DELETE,
        )?;
        self.queue(message, PendingRequest::Action)
    }

    pub(super) fn queue_add_and_activate_open(
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

    pub(super) fn queue_add_and_activate_secure(
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

    pub(super) fn ssid_for_access_point(&self, access_point: &str) -> FlameResult<String> {
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

    pub(super) fn secret_or_attempt_pending(&self) -> bool {
        self.secret_prompt.is_some() || self.secret_context.is_some() || self.attempts.is_busy()
    }

    pub(super) fn busy_error() -> FlameError {
        FlameError::new(
            ErrorCode::Busy,
            "NetworkManager connection attempt is already pending",
        )
    }

    /// Activate a saved profile. Its failure may only re-prompt; the profile is never deleted.
    pub(super) fn connect_saved(&mut self, access_point: String) -> FlameResult<()> {
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
    pub(super) fn prompt_new_network(&mut self, access_point: String) -> FlameResult<()> {
        if self.secret_or_attempt_pending() {
            return Err(Self::busy_error());
        }
        self.device_for_access_point(&access_point)?;
        self.ssid_for_access_point(&access_point)?;
        self.reopen_prompt(SecretPrompt::NewNetwork { access_point })
    }

    pub(super) fn reopen_prompt(&mut self, prompt: SecretPrompt) -> FlameResult<()> {
        self.cancel_secret_context()?;
        let access_point = match &prompt {
            SecretPrompt::NewNetwork { access_point }
            | SecretPrompt::SavedNetwork { access_point } => access_point.clone(),
        };
        let _ = self.controller.request_secret_from_agent(access_point)?;
        self.secret_prompt = Some(prompt);
        Ok(())
    }

    pub(super) fn submit_network_secret(
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
        let Some(prompt) = self.secret_prompt.clone() else {
            return Err(FlameError::new(
                ErrorCode::NotFound,
                "NetworkManager secret request was not found",
            ));
        };
        match secret_submit_plan(&prompt) {
            SecretSubmitPlan::CreateProfile { access_point } => {
                self.create_secured_profile(request_id, generation, access_point, secret)
            }
            SecretSubmitPlan::UpdateSavedProfile { access_point } => {
                self.update_saved_profile(request_id, generation, access_point, secret)
            }
        }
    }

    /// Validate lookups first so a failed lookup leaves the prompt intact. The saved profile
    /// is read, its psk replaced, then activated; the profile itself is never deleted.
    pub(super) fn update_saved_profile(
        &mut self,
        request_id: u64,
        generation: u64,
        access_point: String,
        secret: String,
    ) -> FlameResult<()> {
        let device = self.device_for_access_point(&access_point)?;
        let profile = self.profile_for_access_point(&access_point)?;
        let NetworkIntent::SubmitSecret { secret, .. } = self
            .controller
            .request_submit_secret(request_id, generation, secret)?
        else {
            return Err(unsupported("unexpected NetworkManager secret intent"));
        };
        self.secret_prompt = None;
        let attempt = self
            .attempts
            .begin(access_point.clone(), device.clone(), false);
        if let Err(error) =
            self.queue_saved_settings(attempt, profile, device, access_point, secret)
        {
            self.attempts.clear();
            return Err(error);
        }
        Ok(())
    }

    /// Validate the request and lookups first so a failed lookup leaves the prompt intact.
    pub(super) fn create_secured_profile(
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

    pub(super) fn apply_attempt_effect(&mut self, effect: AttemptEffect) -> FlameResult<()> {
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
    pub(super) fn saved_secret_access_point(&self) -> Option<String> {
        match &self.secret_prompt {
            Some(SecretPrompt::SavedNetwork { access_point }) => Some(access_point.clone()),
            Some(SecretPrompt::NewNetwork { .. }) => None,
            None => self.attempts.saved_access_point().map(str::to_owned),
        }
    }

    pub(super) fn clear_saved_prompt(&mut self) {
        if matches!(self.secret_prompt, Some(SecretPrompt::SavedNetwork { .. })) {
            self.secret_prompt = None;
            self.controller.cancel_secret_from_agent();
        }
    }

    pub(super) fn device_for_access_point(&self, access_point: &str) -> FlameResult<String> {
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

    pub(super) fn profile_for_access_point(&self, access_point: &str) -> FlameResult<String> {
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

}

impl NetworkManagerProvider {
    pub(super) fn queue(&mut self, message: Message, request: PendingRequest) -> FlameResult<()> {
        let serial = self.pump.send(message)?;
        self.pending.insert(serial, request);
        Ok(())
    }
}


/// Full settings for `Connection.Update`: the GetSettings sections are kept, and only the
/// wireless-security `psk` is replaced with psk-flags NONE (0) so the password is stored.
pub(super) fn saved_profile_update_settings(settings: MessageItem, secret: &str) -> FlameResult<MessageItem> {
    let MessageItem::Dict(sections) = settings else {
        return Err(FlameError::invalid(
            "NetworkManager profile settings are not a dictionary",
        ));
    };
    let mut secured = false;
    let mut updated = Vec::new();
    for (section, value) in sections.into_vec() {
        let value = if item_string(&section, "NetworkManager profile section")?
            == "802-11-wireless-security"
        {
            secured = true;
            wireless_security_with_secret(unwrap_variant(value), secret)?
        } else {
            unwrap_variant(value)
        };
        updated.push((section, value));
    }
    if !secured {
        return Err(FlameError::invalid(
            "NetworkManager saved profile has no wireless security",
        ));
    }
    MessageItem::new_dict(updated)
        .map_err(|_| FlameError::invalid("NetworkManager saved profile settings are invalid"))
}

pub(super) fn wireless_security_with_secret(security: MessageItem, secret: &str) -> FlameResult<MessageItem> {
    let MessageItem::Dict(properties) = security else {
        return Err(FlameError::invalid(
            "NetworkManager wireless security section is not a dictionary",
        ));
    };
    let mut entries = Vec::new();
    for (key, value) in properties.into_vec() {
        let name = item_string(&key, "NetworkManager wireless security property")?;
        if name == "psk" || name == "psk-flags" {
            continue;
        }
        entries.push((key, MessageItem::Variant(Box::new(unwrap_variant(value)))));
    }
    entries.push((
        MessageItem::Str("psk".to_owned()),
        MessageItem::Variant(Box::new(MessageItem::Str(secret.to_owned()))),
    ));
    entries.push((
        MessageItem::Str("psk-flags".to_owned()),
        MessageItem::Variant(Box::new(MessageItem::UInt32(0))),
    ));
    MessageItem::new_dict(entries)
        .map_err(|_| FlameError::invalid("NetworkManager wireless security settings are invalid"))
}

/// `Settings.Connection.Update(a{sa{sv}})` on one saved profile.
pub(super) fn update_connection_message(profile: &str, settings: MessageItem) -> FlameResult<Message> {
    Ok(method_call(
        NETWORK_MANAGER_SERVICE,
        profile,
        SETTINGS_CONNECTION_INTERFACE,
        UPDATE,
    )?
    .append1(settings))
}

pub(super) fn secret_reply(
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

pub(super) fn user_canceled_reply(reply: Message) -> FlameResult<Message> {
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

/// Queue one RequestScan per device. A failed queue clears the controller's pending scan so a
/// later Scan is not blocked; replies already in flight complete harmlessly.
pub(super) fn queue_scan_requests<F>(
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
pub(super) fn secret_agent_registration_message() -> FlameResult<Message> {
    method_call(
        NETWORK_MANAGER_SERVICE,
        SECRET_AGENT_MANAGER_PATH,
        SECRET_AGENT_MANAGER_INTERFACE,
        REGISTER_WITH_CAPABILITIES,
    )
    .map(|message| message.append2(SECRET_AGENT_IDENTIFIER.to_owned(), 0_u32))
}

use super::*;

pub(crate) fn access_point_wire_value(point: &NetworkAccessPointSnapshot) -> WireValue {
    let mut item = WireDict::new();
    item.insert("path".to_owned(), WireValue::String(point.path.clone()));
    item.insert("ssid".to_owned(), WireValue::String(point.ssid.clone()));
    item.insert(
        "strength".to_owned(),
        WireValue::U32(u32::from(point.strength_percent)),
    );
    item.insert("secured".to_owned(), WireValue::Bool(point.secured));
    item.insert("known".to_owned(), WireValue::Bool(point.known));
    WireValue::Dict(item)
}

/// Generates a wire-name encoder (exhaustive) and a strict wire-name decoder.
macro_rules! wire_enum_codec {
    ($ty:ident, $name_fn:ident, $parse_fn:ident, $err:literal, { $($variant:ident => $wire:literal),+ $(,)? }) => {
        pub(crate) const fn $name_fn(value: $ty) -> &'static str {
            match value {
                $($ty::$variant => $wire,)+
            }
        }

        pub(crate) fn $parse_fn(value: &str) -> Result<$ty, ControlError> {
            match value {
                $($wire => Ok($ty::$variant),)+
                _ => Err(invalid($err)),
            }
        }
    };
}

wire_enum_codec!(ServiceAvailability, availability_name, parse_availability, "availability is invalid", {
    Unknown => "unknown",
    Available => "available",
    Unavailable => "unavailable",
});

wire_enum_codec!(NetworkKind, network_kind_name, parse_network_kind, "network kind is invalid", {
    Unavailable => "unavailable",
    Disconnected => "disconnected",
    Connecting => "connecting",
    Wired => "wired",
    Wireless => "wireless",
});

wire_enum_codec!(PowerState, power_state_name, parse_power_state, "power state is invalid", {
    Charging => "charging",
    Discharging => "discharging",
    Full => "full",
    Empty => "empty",
    Unknown => "unknown",
});

wire_enum_codec!(PlaybackState, playback_name, parse_playback, "playback state is invalid", {
    Playing => "playing",
    Paused => "paused",
    Stopped => "stopped",
    Unavailable => "unavailable",
});

wire_enum_codec!(AudioEndpointKind, audio_endpoint_kind_name, parse_audio_endpoint_kind, "audio endpoint kind must be sink or source", {
    Sink => "sink",
    Source => "source",
});

pub(crate) fn audio_target(
    kind: &str,
    endpoint_kind: &str,
    id: u32,
) -> Result<AudioTarget, ControlError> {
    match kind {
        "endpoint" => Ok(AudioTarget::Endpoint {
            id,
            kind: parse_audio_endpoint_kind(endpoint_kind)?,
        }),
        "stream" if endpoint_kind.is_empty() => Ok(AudioTarget::Stream { id }),
        "stream" => Err(invalid("audio stream endpointKind must be empty")),
        _ => Err(invalid("audio target kind must be endpoint or stream")),
    }
}

pub(crate) fn access_point_snapshot(
    value: &WireValue,
) -> Result<NetworkAccessPointSnapshot, ControlError> {
    let dict = required_dict(value, "accessPoint")?;
    let strength = dict_u32(dict, "strength")?;
    Ok(NetworkAccessPointSnapshot {
        path: dict_string(dict, "path")?.to_owned(),
        ssid: dict_string(dict, "ssid")?.to_owned(),
        strength_percent: u8::try_from(strength)
            .map_err(|_| invalid("access point strength exceeds u8"))?,
        secured: dict_bool(dict, "secured")?,
        known: dict_bool(dict, "known")?,
    })
}

/// Absent power keys (older peers) decode to the default snapshot.
pub(crate) fn power_snapshot(dict: &WireDict) -> Result<PowerSnapshot, ControlError> {
    if dict.get("powerAvailability").is_none() {
        return Ok(PowerSnapshot::default());
    }
    let percent = dict_u32(dict, "powerPercent")?;
    let cpu_percent = dict
        .get("powerCpuPercent")
        .map(|_| {
            u8::try_from(dict_u32(dict, "powerCpuPercent")?)
                .map_err(|_| invalid("power cpu percent exceeds u8"))
        })
        .transpose()?;
    let power_rate_mw = dict
        .get("powerRateMilliwatts")
        .map(|_| dict_u32(dict, "powerRateMilliwatts"))
        .transpose()?;
    Ok(PowerSnapshot {
        available: parse_availability(dict_string(dict, "powerAvailability")?)?,
        present: dict_bool(dict, "powerPresent")?,
        percent: u8::try_from(percent).map_err(|_| invalid("power percent exceeds u8"))?,
        state: parse_power_state(dict_string(dict, "powerState")?)?,
        cpu_percent,
        power_rate_mw,
    })
}

pub(crate) fn system_snapshot(dict: &WireDict) -> Result<SystemSnapshot, ControlError> {
    let strength = dict_u32(dict, "networkStrength")?;
    let volume = dict_u32(dict, "audioVolume")?;
    Ok(SystemSnapshot {
        revision: dict_u64(dict, "revision")?,
        network: NetworkSnapshot {
            availability: parse_availability(dict_string(dict, "networkAvailability")?)?,
            generation: dict_u64(dict, "networkGeneration")?,
            kind: parse_network_kind(dict_string(dict, "networkKind")?)?,
            label: dict_string(dict, "networkLabel")?.to_owned(),
            strength_percent: u8::try_from(strength)
                .map_err(|_| invalid("network strength exceeds u8"))?,
            wifi_enabled: dict_bool(dict, "wifiEnabled")?,
            networking_enabled: dict_bool(dict, "networkingEnabled")?,
            active_path: dict_string(dict, "networkActivePath")?.to_owned(),
            // Optional for older peers that predate this key: absent means no connected AP.
            active_access_point_path: match dict.get("networkActiveAccessPointPath") {
                Some(value) => required_string(value, "networkActiveAccessPointPath")?.to_owned(),
                None => String::new(),
            },
            access_points: dict_value(dict, "accessPoints").and_then(|value| {
                required_array(value, "accessPoints")?
                    .iter()
                    .map(access_point_snapshot)
                    .collect::<Result<Vec<_>, _>>()
            })?,
            pending_secret: if dict_bool(dict, "hasPendingNetworkSecret")? {
                Some(NetworkSecretRequestSnapshot {
                    request_id: dict_u64(dict, "pendingNetworkSecretRequestId")?,
                    access_point_path: dict_string(dict, "pendingNetworkSecretAccessPointPath")?
                        .to_owned(),
                })
            } else {
                None
            },
        },
        media: MediaSnapshot {
            availability: parse_availability(dict_string(dict, "mediaAvailability")?)?,
            generation: dict_u64(dict, "mediaGeneration")?,
            active_bus_name: dict_string(dict, "mediaBusName")?.to_owned(),
            identity: dict_string(dict, "mediaIdentity")?.to_owned(),
            title: dict_string(dict, "mediaTitle")?.to_owned(),
            artist: dict_string(dict, "mediaArtist")?.to_owned(),
            playback: parse_playback(dict_string(dict, "mediaPlayback")?)?,
            can_play: dict_bool(dict, "mediaCanPlay")?,
            can_pause: dict_bool(dict, "mediaCanPause")?,
            can_next: dict_bool(dict, "mediaCanNext")?,
            can_previous: dict_bool(dict, "mediaCanPrevious")?,
        },
        power: power_snapshot(dict)?,
        audio: AudioSnapshot {
            availability: parse_availability(dict_string(dict, "audioAvailability")?)?,
            generation: dict_u64(dict, "audioGeneration")?,
            server_generation: dict_u64(dict, "audioServerGeneration")?,
            sink_name: dict_string(dict, "audioSinkName")?.to_owned(),
            volume_percent: u8::try_from(volume).map_err(|_| invalid("audio volume exceeds u8"))?,
            muted: dict_bool(dict, "audioMuted")?,
            ..AudioSnapshot::default()
        }
        .with_items(
            dict_value(dict, "audioEndpoints")
                .and_then(|value| required_array(value, "audioEndpoints"))?
                .iter()
                .map(audio_endpoint_snapshot)
                .collect::<Result<Vec<_>, _>>()?,
            dict_value(dict, "audioStreams")
                .and_then(|value| required_array(value, "audioStreams"))?
                .iter()
                .map(audio_stream_snapshot)
                .collect::<Result<Vec<_>, _>>()?,
        ),
    })
}

pub(crate) fn audio_endpoint_wire_value(endpoint: &AudioEndpointSnapshot) -> WireValue {
    WireValue::Dict(dict_of([
        ("id", WireValue::U32(endpoint.id)),
        (
            "kind",
            WireValue::String(audio_endpoint_kind_name(endpoint.kind).to_owned()),
        ),
        ("name", WireValue::String(endpoint.name.clone())),
        (
            "description",
            WireValue::String(endpoint.description.clone()),
        ),
        ("volume", WireValue::U32(u32::from(endpoint.volume_percent))),
        ("muted", WireValue::Bool(endpoint.muted)),
        ("default", WireValue::Bool(endpoint.is_default)),
    ]))
}

pub(crate) fn audio_stream_wire_value(stream: &AudioStreamSnapshot) -> WireValue {
    let mut out = WireDict::new();
    out.insert("id".to_owned(), WireValue::U32(stream.id));
    out.insert("endpointId".to_owned(), WireValue::U32(stream.endpoint_id));
    out.insert("name".to_owned(), WireValue::String(stream.name.clone()));
    out.insert(
        "volume".to_owned(),
        WireValue::U32(u32::from(stream.volume_percent)),
    );
    out.insert("muted".to_owned(), WireValue::Bool(stream.muted));
    // Optional identity keys are omitted when unknown so older peers see the original shape.
    for (key, value) in [
        ("appName", &stream.app_name),
        ("appId", &stream.app_id),
        ("iconName", &stream.icon_name),
        ("processBinary", &stream.process_binary),
        ("mediaName", &stream.media_name),
    ] {
        if let Some(value) = value {
            out.insert(key.to_owned(), WireValue::String(value.clone()));
        }
    }
    WireValue::Dict(out)
}

pub(crate) fn audio_endpoint_snapshot(
    value: &WireValue,
) -> Result<AudioEndpointSnapshot, ControlError> {
    let dict = required_dict(value, "audio endpoint")?;
    Ok(AudioEndpointSnapshot {
        id: dict_u32(dict, "id")?,
        kind: parse_audio_endpoint_kind(dict_string(dict, "kind")?)?,
        name: dict_string(dict, "name")?.to_owned(),
        description: dict_string(dict, "description")?.to_owned(),
        volume_percent: u8::try_from(dict_u32(dict, "volume")?)
            .map_err(|_| invalid("audio endpoint volume exceeds u8"))?,
        muted: dict_bool(dict, "muted")?,
        is_default: dict_bool(dict, "default")?,
    })
}

pub(crate) fn audio_stream_snapshot(
    value: &WireValue,
) -> Result<AudioStreamSnapshot, ControlError> {
    let dict = required_dict(value, "audio stream")?;
    Ok(AudioStreamSnapshot {
        id: dict_u32(dict, "id")?,
        endpoint_id: dict_u32(dict, "endpointId")?,
        name: dict_string(dict, "name")?.to_owned(),
        volume_percent: u8::try_from(dict_u32(dict, "volume")?)
            .map_err(|_| invalid("audio stream volume exceeds u8"))?,
        muted: dict_bool(dict, "muted")?,
        app_name: optional_string(dict, "appName")?,
        app_id: optional_string(dict, "appId")?,
        icon_name: optional_string(dict, "iconName")?,
        process_binary: optional_string(dict, "processBinary")?,
        media_name: optional_string(dict, "mediaName")?,
    })
}

/// Absent key means `None`; a present key must be a string.
pub(crate) fn optional_string(dict: &WireDict, key: &str) -> Result<Option<String>, ControlError> {
    dict.get(key)
        .map(|value| required_string(value, key).map(str::to_owned))
        .transpose()
}

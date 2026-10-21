//! MPRIS session-bus source adapter.

use std::collections::BTreeMap;

use crate::network_manager::{item_bool, item_string, one_string, unsupported, unwrap_variant};
use dbus::arg::messageitem::MessageItem;
use dbus::{Message, MessageType};
use flamewm_api::system::{MediaSnapshot, PlaybackState, SystemAction};
use flamewm_api::{ErrorCode, FlameError, FlameResult};
use flamewm_dbus_reactor::{BusKind, BusPump, BusWatch, MessageDisposition};
use flamewm_integrations_core::mpris::{MediaIntent, MprisController, PlayerInfo};

pub const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
pub const MPRIS_ROOT: &str = "/org/mpris/MediaPlayer2";
pub const MPRIS_PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
pub const MPRIS_ROOT_INTERFACE: &str = "org.mpris.MediaPlayer2";
pub const DBUS_SERVICE: &str = "org.freedesktop.DBus";
pub const DBUS_ROOT: &str = "/org/freedesktop/DBus";
pub const DBUS_INTERFACE: &str = "org.freedesktop.DBus";
pub const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";

const NAME_OWNER_CHANGED: &str = "NameOwnerChanged";
const PROPERTIES_CHANGED: &str = "PropertiesChanged";
const GET_ALL: &str = "GetAll";
const ADD_MATCH: &str = "AddMatch";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MprisProviderState {
    Disconnected,
    Started,
}

enum PendingRequest {
    AddMatch,
    ListNames,
    NameOwner {
        player: String,
    },
    Properties {
        player: String,
        interface: &'static str,
        owner: String,
    },
    Action,
}

#[derive(Default)]
struct PendingPlayer {
    player: Option<PlayerInfo>,
    identity: Option<String>,
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
        owner: String,
        interface: String,
        changed: PropertyChanges,
    },
}

#[derive(Default)]
struct PropertyChanges {
    identity: Option<String>,
    playback: Option<PlaybackState>,
    track_title: Option<String>,
    artist: Option<String>,
    can_play: Option<bool>,
    can_pause: Option<bool>,
    can_next: Option<bool>,
    can_previous: Option<bool>,
}

/// Event-driven MPRIS provider. Player state is published only after both root identity and Player
/// properties have been received from the owning service.
pub struct MprisProvider {
    pump: BusPump,
    controller: MprisController,
    players: BTreeMap<String, PlayerInfo>,
    owners: BTreeMap<String, String>,
    pending_players: BTreeMap<String, PendingPlayer>,
    pending: BTreeMap<u32, PendingRequest>,
    state: MprisProviderState,
    /// A selection change consumed by an internal reconcile (`on_ready`) that the next
    /// `reconcile()` caller must still observe.
    changed_pending: bool,
}

impl MprisProvider {
    pub fn connect() -> FlameResult<Self> {
        Ok(Self {
            pump: BusPump::connect(BusKind::Session)?,
            controller: MprisController::default(),
            players: BTreeMap::new(),
            owners: BTreeMap::new(),
            pending_players: BTreeMap::new(),
            pending: BTreeMap::new(),
            state: MprisProviderState::Disconnected,
            changed_pending: false,
        })
    }

    pub fn start(&mut self) -> FlameResult<()> {
        if self.state == MprisProviderState::Started {
            return Ok(());
        }
        self.add_match(
            "type='signal',sender='org.freedesktop.DBus',path='/org/freedesktop/DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged',arg0namespace='org.mpris.MediaPlayer2'",
        )?;
        self.add_match(
            "type='signal',interface='org.freedesktop.DBus.Properties',path='/org/mpris/MediaPlayer2',member='PropertiesChanged'",
        )?;
        self.queue_list_names()?;
        self.state = MprisProviderState::Started;
        Ok(())
    }

    pub fn stop(&mut self) {
        if self.state == MprisProviderState::Disconnected {
            return;
        }
        self.state = MprisProviderState::Disconnected;
        self.pending.clear();
        self.pending_players.clear();
        self.owners.clear();
        let players = self.players.keys().cloned().collect::<Vec<_>>();
        for player in players {
            let _ = self.controller.player_vanished(&player);
        }
        self.players.clear();
    }

    #[must_use]
    pub const fn state(&self) -> MprisProviderState {
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
    pub fn snapshot(&self) -> MediaSnapshot {
        self.controller.snapshot()
    }

    /// Reconcile a coalesced property update using the core controller's deterministic active-player
    /// policy. The native event has already supplied the player values.
    pub fn reconcile(&mut self) -> bool {
        let changed = self.controller.reconcile();
        changed | std::mem::take(&mut self.changed_pending)
    }

    /// Queue a player command after validating its generation and advertised capability.
    pub fn perform_action(
        &mut self,
        action: SystemAction,
        snapshot: &MediaSnapshot,
    ) -> FlameResult<()> {
        if snapshot.generation != self.controller.generation() {
            return Err(FlameError::stale("stale MPRIS generation"));
        }
        let (intent, member) = match action {
            SystemAction::Play { bus_name } => (MediaIntent::Play { bus_name }, "Play"),
            SystemAction::Pause { bus_name } => (MediaIntent::Pause { bus_name }, "Pause"),
            SystemAction::PlayPause { bus_name } => {
                (MediaIntent::PlayPause { bus_name }, "PlayPause")
            }
            SystemAction::Next { bus_name } => (MediaIntent::Next { bus_name }, "Next"),
            SystemAction::Previous { bus_name } => (MediaIntent::Previous { bus_name }, "Previous"),
            _ => return Err(unsupported("system action is not an MPRIS action")),
        };
        let intent = self.controller.request(intent, snapshot.generation)?;
        let bus_name = match intent {
            MediaIntent::Play { bus_name }
            | MediaIntent::Pause { bus_name }
            | MediaIntent::PlayPause { bus_name }
            | MediaIntent::Next { bus_name }
            | MediaIntent::Previous { bus_name } => bus_name,
        };
        let message =
            Message::new_method_call(&bus_name, MPRIS_ROOT, MPRIS_PLAYER_INTERFACE, member)
                .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?;
        self.queue(message, PendingRequest::Action)
    }

    pub fn on_ready(&mut self) -> FlameResult<usize> {
        let mut incoming = Vec::new();
        let mut parse_error = None;
        let count = self.pump.on_ready(|message| {
            if let Some(event) = parse_message(message, &self.owners) {
                match event {
                    Ok(event) => incoming.push(event),
                    Err(error) => parse_error = Some(error),
                }
                return MessageDisposition::Handled;
            }
            MessageDisposition::Unhandled
        })?;
        if let Some(error) = parse_error {
            return Err(error);
        }
        for event in incoming {
            self.apply(event)?;
        }
        self.changed_pending |= self.controller.reconcile();
        Ok(count)
    }

    fn add_match(&mut self, rule: &str) -> FlameResult<()> {
        let message = Message::new_method_call(DBUS_SERVICE, DBUS_ROOT, DBUS_INTERFACE, ADD_MATCH)
            .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?
            .append1(rule.to_owned());
        self.queue(message, PendingRequest::AddMatch)
    }

    fn queue_list_names(&mut self) -> FlameResult<()> {
        let message =
            Message::new_method_call(DBUS_SERVICE, DBUS_ROOT, DBUS_INTERFACE, "ListNames")
                .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?;
        self.queue(message, PendingRequest::ListNames)
    }

    fn queue_name_owner(&mut self, player: String) -> FlameResult<()> {
        let message =
            Message::new_method_call(DBUS_SERVICE, DBUS_ROOT, DBUS_INTERFACE, "GetNameOwner")
                .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?
                .append1(player.clone());
        self.queue(message, PendingRequest::NameOwner { player })
    }

    fn queue_properties(&mut self, player: String, interface: &'static str) -> FlameResult<()> {
        let Some(owner) = self.owners.get(&player).cloned() else {
            return Ok(());
        };
        let message = Message::new_method_call(&player, MPRIS_ROOT, PROPERTIES_INTERFACE, GET_ALL)
            .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?
            .append1(interface.to_owned());
        self.queue(
            message,
            PendingRequest::Properties {
                player,
                interface,
                owner,
            },
        )
    }

    fn refresh_player(&mut self, player: String) -> FlameResult<()> {
        if !self.owners.contains_key(&player) {
            return Ok(());
        }
        self.pending_players
            .insert(player.clone(), PendingPlayer::default());
        self.queue_properties(player.clone(), MPRIS_ROOT_INTERFACE)?;
        self.queue_properties(player, MPRIS_PLAYER_INTERFACE)
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
            } => self.apply_owner_changed(name, old_owner, new_owner),
            Incoming::PropertiesChanged {
                owner,
                interface,
                changed,
            } => self.apply_properties_changed(owner, interface, changed),
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
                PendingRequest::AddMatch => {
                    return Err(FlameError::unavailable(
                        "MPRIS D-Bus match subscription failed",
                    ));
                }
                PendingRequest::ListNames => return Ok(()),
                PendingRequest::NameOwner { player } => {
                    self.remove_player(&player);
                    return Ok(());
                }
                PendingRequest::Properties { player, .. } => {
                    self.pending_players.remove(&player);
                    return Ok(());
                }
                PendingRequest::Action => return Ok(()),
            }
        }
        match request {
            PendingRequest::AddMatch | PendingRequest::Action => Ok(()),
            PendingRequest::ListNames => {
                for player in names_from_reply(&items)? {
                    self.queue_name_owner(player)?;
                }
                Ok(())
            }
            PendingRequest::NameOwner { player } => {
                let owner = one_string(&items, "MPRIS owner reply")?;
                if owner.is_empty() {
                    self.remove_player(&player);
                } else {
                    self.owners.insert(player.clone(), owner);
                    self.refresh_player(player)?;
                }
                Ok(())
            }
            PendingRequest::Properties {
                player,
                interface,
                owner,
            } => {
                if self.owners.get(&player) != Some(&owner) {
                    return Ok(());
                }
                let changes = parse_property_changes(items.first(), interface)?;
                self.apply_property_reply(player, interface, changes)
            }
        }
    }

    fn apply_owner_changed(
        &mut self,
        name: String,
        old_owner: String,
        new_owner: String,
    ) -> FlameResult<()> {
        if !name.starts_with(MPRIS_PREFIX) || old_owner == new_owner {
            return Ok(());
        }
        if new_owner.is_empty() {
            self.owners.remove(&name);
            self.pending_players.remove(&name);
            self.remove_player(&name);
        } else {
            self.owners.insert(name.clone(), new_owner);
            self.refresh_player(name)?;
        }
        Ok(())
    }

    fn apply_property_reply(
        &mut self,
        player: String,
        interface: &'static str,
        changes: PropertyChanges,
    ) -> FlameResult<()> {
        let pending = self
            .pending_players
            .get_mut(&player)
            .ok_or_else(|| FlameError::invalid("MPRIS property reply has no pending player"))?;
        if interface == MPRIS_ROOT_INTERFACE {
            pending.identity = changes.identity;
        } else {
            let mut info = pending.player.take().unwrap_or_else(|| PlayerInfo {
                bus_name: player.clone(),
                identity: String::new(),
                track_title: String::new(),
                artist: String::new(),
                playback: PlaybackState::Unavailable,
                can_play: false,
                can_pause: false,
                can_next: false,
                can_previous: false,
            });
            apply_changes(&mut info, changes);
            pending.player = Some(info);
        }
        if pending.identity.is_none() || pending.player.is_none() {
            return Ok(());
        }
        let identity = pending.identity.take().unwrap_or_default();
        let mut info = pending.player.take().unwrap_or_else(|| unreachable!());
        info.identity = identity;
        self.pending_players.remove(&player);
        if self.players.contains_key(&player) {
            self.players.insert(player.clone(), info.clone());
            let generation = self.controller.generation();
            let _ = self.controller.properties_changed(info, generation);
        } else {
            self.players.insert(player, info.clone());
            self.controller.player_appeared(info);
        }
        Ok(())
    }

    fn apply_properties_changed(
        &mut self,
        owner: String,
        interface: String,
        changes: PropertyChanges,
    ) -> FlameResult<()> {
        let Some(player) = self
            .owners
            .iter()
            .find_map(|(player, mapped_owner)| (mapped_owner == &owner).then_some(player.clone()))
        else {
            return Ok(());
        };
        let Some(info) = self.players.get_mut(&player) else {
            return Ok(());
        };
        if interface == MPRIS_ROOT_INTERFACE {
            if let Some(identity) = changes.identity {
                info.identity = identity;
            }
        } else if interface == MPRIS_PLAYER_INTERFACE {
            apply_changes(info, changes);
        } else {
            return Ok(());
        }
        let generation = self.controller.generation();
        let _ = self.controller.properties_changed(info.clone(), generation);
        Ok(())
    }

    fn remove_player(&mut self, player: &str) {
        self.players.remove(player);
        self.pending_players.remove(player);
        let _ = self.controller.player_vanished(player);
    }
}

fn parse_message(
    message: &Message,
    owners: &BTreeMap<String, String>,
) -> Option<Result<Incoming, FlameError>> {
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
    let interface = message.interface();
    let member = message.member();
    let path = message.path();
    if interface.as_deref() == Some(DBUS_INTERFACE)
        && member.as_deref() == Some(NAME_OWNER_CHANGED)
        && path.as_deref() == Some(DBUS_ROOT)
    {
        return Some(parse_owner_changed(message.get_items()));
    }
    if interface.as_deref() == Some(PROPERTIES_INTERFACE)
        && member.as_deref() == Some(PROPERTIES_CHANGED)
        && path.as_deref() == Some(MPRIS_ROOT)
    {
        let owner = message.sender().map(|value| value.to_string())?;
        if !owners.values().any(|mapped_owner| mapped_owner == &owner) {
            return None;
        }
        return Some(parse_properties_changed(message.get_items(), owner));
    }
    None
}

fn parse_owner_changed(items: Vec<MessageItem>) -> Result<Incoming, FlameError> {
    if items.len() != 3 {
        return Err(FlameError::invalid(
            "MPRIS owner event has invalid arguments",
        ));
    }
    Ok(Incoming::OwnerChanged {
        name: item_string(&items[0], "owner event name")?,
        old_owner: item_string(&items[1], "owner event old owner")?,
        new_owner: item_string(&items[2], "owner event new owner")?,
    })
}

fn parse_properties_changed(
    items: Vec<MessageItem>,
    owner: String,
) -> Result<Incoming, FlameError> {
    if items.len() < 2 {
        return Err(FlameError::invalid(
            "MPRIS property event has invalid arguments",
        ));
    }
    let interface = item_string(&items[0], "MPRIS property interface")?;
    let changed = parse_property_changes(Some(&items[1]), interface.as_str())?;
    Ok(Incoming::PropertiesChanged {
        owner,
        interface,
        changed,
    })
}

fn names_from_reply(items: &[MessageItem]) -> FlameResult<Vec<String>> {
    let Some(MessageItem::Array(names)) = items.first() else {
        return Err(FlameError::invalid("MPRIS ListNames reply is not an array"));
    };
    names
        .clone()
        .into_vec()
        .into_iter()
        .map(|name| item_string(&name, "MPRIS bus name"))
        .filter(|name| {
            name.as_ref()
                .map_or(true, |name| name.starts_with(MPRIS_PREFIX))
        })
        .collect()
}

fn parse_property_changes(
    item: Option<&MessageItem>,
    interface: &str,
) -> FlameResult<PropertyChanges> {
    let Some(MessageItem::Dict(dict)) = item else {
        return Err(FlameError::invalid("MPRIS properties are not a dictionary"));
    };
    let mut changes = PropertyChanges::default();
    for (key, value) in dict.clone().into_vec() {
        let key = item_string(&key, "MPRIS property name")?;
        let value = unwrap_variant(value);
        match (interface, key.as_str()) {
            (MPRIS_ROOT_INTERFACE, "Identity") => {
                changes.identity = Some(item_string(&value, "MPRIS identity")?)
            }
            (MPRIS_PLAYER_INTERFACE, "PlaybackStatus") => {
                changes.playback = Some(
                    match item_string(&value, "MPRIS playback status")?.as_str() {
                        "Playing" => PlaybackState::Playing,
                        "Paused" => PlaybackState::Paused,
                        "Stopped" => PlaybackState::Stopped,
                        _ => PlaybackState::Unavailable,
                    },
                )
            }
            (MPRIS_PLAYER_INTERFACE, "CanPlay") => {
                changes.can_play = Some(item_bool(&value, "MPRIS CanPlay")?)
            }
            (MPRIS_PLAYER_INTERFACE, "CanPause") => {
                changes.can_pause = Some(item_bool(&value, "MPRIS CanPause")?)
            }
            (MPRIS_PLAYER_INTERFACE, "CanGoNext") => {
                changes.can_next = Some(item_bool(&value, "MPRIS CanGoNext")?)
            }
            (MPRIS_PLAYER_INTERFACE, "CanGoPrevious") => {
                changes.can_previous = Some(item_bool(&value, "MPRIS CanGoPrevious")?)
            }
            (MPRIS_PLAYER_INTERFACE, "Metadata") => parse_metadata(&value, &mut changes)?,
            _ => {}
        }
    }
    Ok(changes)
}

fn parse_metadata(item: &MessageItem, changes: &mut PropertyChanges) -> FlameResult<()> {
    let MessageItem::Dict(dict) = item else {
        return Err(FlameError::invalid("MPRIS metadata is not a dictionary"));
    };
    for (key, value) in dict.clone().into_vec() {
        let key = item_string(&key, "MPRIS metadata key")?;
        let value = unwrap_variant(value);
        match key.as_str() {
            "xesam:title" => changes.track_title = Some(item_string(&value, "MPRIS title")?),
            "xesam:artist" => changes.artist = Some(artists(&value)?),
            _ => {}
        }
    }
    Ok(())
}

fn artists(item: &MessageItem) -> FlameResult<String> {
    let MessageItem::Array(values) = item else {
        return Err(FlameError::invalid("MPRIS artist metadata is not an array"));
    };
    values
        .clone()
        .into_vec()
        .into_iter()
        .map(|value| item_string(&value, "MPRIS artist value"))
        .collect::<FlameResult<Vec<_>>>()
        .map(|values| values.join(", "))
}

fn apply_changes(info: &mut PlayerInfo, changes: PropertyChanges) {
    info.playback = changes.playback.unwrap_or(info.playback);
    if let Some(value) = changes.track_title {
        info.track_title = value;
    }
    if let Some(value) = changes.artist {
        info.artist = value;
    }
    info.can_play = changes.can_play.unwrap_or(info.can_play);
    info.can_pause = changes.can_pause.unwrap_or(info.can_pause);
    info.can_next = changes.can_next.unwrap_or(info.can_next);
    info.can_previous = changes.can_previous.unwrap_or(info.can_previous);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner_changed_signal(path: &str, interface: &str, member: &str) -> Message {
        Message::new_signal(path, interface, member)
            .expect("valid signal header")
            .append3(
                "org.mpris.MediaPlayer2.test".to_owned(),
                String::new(),
                ":1.42".to_owned(),
            )
    }

    fn stage_player(provider: &mut MprisProvider, serial: u32, bus: &str, playback: &str) {
        let owner = format!(":1.{serial}");
        provider.owners.insert(bus.to_owned(), owner.clone());
        provider
            .pending_players
            .insert(bus.to_owned(), PendingPlayer::default());
        let reply = |interface: &'static str, entry: (&str, MessageItem)| {
            let (key, value) = entry;
            let dict = MessageItem::new_dict(vec![(
                MessageItem::Str(key.to_owned()),
                MessageItem::Variant(Box::new(value)),
            )])
            .expect("valid property dict");
            (interface, dict)
        };
        let replies = [
            reply(
                MPRIS_ROOT_INTERFACE,
                ("Identity", MessageItem::Str(bus.to_owned())),
            ),
            reply(
                MPRIS_PLAYER_INTERFACE,
                ("PlaybackStatus", MessageItem::Str(playback.to_owned())),
            ),
        ];
        for (offset, (interface, dict)) in replies.into_iter().enumerate() {
            let serial = serial * 10 + offset as u32;
            provider.pending.insert(
                serial,
                PendingRequest::Properties {
                    player: bus.to_owned(),
                    interface,
                    owner: owner.clone(),
                },
            );
            provider
                .apply(Incoming::Reply {
                    serial,
                    is_error: false,
                    items: vec![dict],
                })
                .expect("property reply applies");
        }
    }

    #[test]
    fn change_made_inside_on_ready_is_reported_by_next_reconcile_once() {
        // Real session bus is required for the pump; no message is sent by this test.
        let mut provider = MprisProvider::connect().expect("session bus for MPRIS pump");
        stage_player(&mut provider, 1, "org.mpris.MediaPlayer2.paused", "Paused");
        stage_player(
            &mut provider,
            2,
            "org.mpris.MediaPlayer2.playing",
            "Playing",
        );

        // Aggregate sequence: provider.on_ready() then provider.reconcile().
        provider.on_ready().expect("on_ready");
        assert!(
            provider.reconcile(),
            "selection change produced inside on_ready must be reported"
        );
        assert!(
            !provider.reconcile(),
            "change must be reported exactly once"
        );

        let snapshot = provider.snapshot();
        assert_eq!(
            snapshot.availability,
            flamewm_api::system::ServiceAvailability::Available
        );
        assert_eq!(snapshot.playback, PlaybackState::Playing);
        assert_eq!(snapshot.active_bus_name, "org.mpris.MediaPlayer2.playing");
        assert_ne!(snapshot.generation, 0);
    }

    #[test]
    fn owner_change_requires_exact_signal_header() {
        let owners = BTreeMap::new();
        let incoming = parse_message(
            &owner_changed_signal(DBUS_ROOT, DBUS_INTERFACE, NAME_OWNER_CHANGED),
            &owners,
        );
        assert!(matches!(
            incoming,
            Some(Ok(Incoming::OwnerChanged { name, new_owner, .. }))
                if name == "org.mpris.MediaPlayer2.test" && new_owner == ":1.42"
        ));

        for (path, interface, member) in [
            ("/wrong", DBUS_INTERFACE, NAME_OWNER_CHANGED),
            (DBUS_ROOT, "org.example.Other", NAME_OWNER_CHANGED),
            (DBUS_ROOT, DBUS_INTERFACE, "Other"),
        ] {
            assert!(
                parse_message(&owner_changed_signal(path, interface, member), &owners).is_none()
            );
        }
    }
}

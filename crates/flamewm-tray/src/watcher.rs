//! `org.kde.StatusNotifierWatcher` service on the session bus, driven by the caller's
//! `flamewm-dbus-reactor` pump. Owns no thread and no timer: all work happens in `on_ready`.

use std::collections::{BTreeMap, HashMap};

use flamewm_api::FlameResult;
use flamewm_dbus_reactor::{
    BusKind, BusPump, BusWatch, Message, MessageDisposition, MessageItem, MessageItemArray,
    MessageType, Signature,
};

use crate::item::{ITEM_INTERFACE, ItemKey, PropValue, parse_registration};
use crate::registry::{Registration, Registry};

pub const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
pub const WATCHER_PATH: &str = "/StatusNotifierWatcher";
pub const WATCHER_INTERFACE: &str = "org.kde.StatusNotifierWatcher";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const DBUS_SERVICE: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_INTERFACE: &str = "org.freedesktop.DBus";
const NAME_OWNER_CHANGED: &str = "NameOwnerChanged";
const PROTOCOL_VERSION: i32 = 0;
/// Outstanding property/owner requests. Replies from dead peers never arrive, so this bounds them.
const MAX_PENDING: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Props(ItemKey),
    Owner(ItemKey),
}

#[derive(Default)]
struct WatcherState {
    registry: Registry,
    pending: HashMap<u32, Pending>,
}

pub struct TrayWatcher {
    pump: BusPump,
    state: WatcherState,
}

impl TrayWatcher {
    /// Claims the watcher name and registers this process as a host. Fails if another watcher
    /// already owns the name.
    pub fn connect() -> FlameResult<Self> {
        let pump = BusPump::connect(BusKind::Session)?;
        pump.own_name(WATCHER_NAME)?;
        let host = format!("org.kde.StatusNotifierHost-{}", std::process::id());
        pump.own_name(&host)?;
        let mut state = WatcherState::default();
        // The host name is ours, so this cannot exceed capacity on an empty registry.
        let _ = state.registry.add_host(&host);
        add_match(
            &pump,
            "type='signal',sender='org.freedesktop.DBus',path='/org/freedesktop/DBus',interface='org.freedesktop.DBus',member='NameOwnerChanged'",
        )?;
        add_match(
            &pump,
            "type='signal',interface='org.kde.StatusNotifierItem'",
        )?;
        Ok(Self { pump, state })
    }

    #[must_use]
    pub fn watch(&self) -> BusWatch {
        self.pump.watch()
    }

    /// Drains pending bus traffic. Returns the number of messages visited.
    pub fn on_ready(&mut self) -> FlameResult<usize> {
        let Self { pump, state } = self;
        pump.on_ready(|message| state.handle(pump, message))
    }

    #[must_use]
    pub fn registered_items(&self) -> Vec<String> {
        self.state.registry.registered_strings()
    }
}

impl WatcherState {
    fn handle(&mut self, pump: &BusPump, message: &Message) -> MessageDisposition {
        match message.msg_type() {
            MessageType::MethodCall => self.method_call(pump, message),
            MessageType::MethodReturn => {
                self.method_return(message);
                MessageDisposition::Handled
            }
            MessageType::Signal => self.signal(pump, message),
            _ => MessageDisposition::Unhandled,
        }
    }

    fn method_call(&mut self, pump: &BusPump, message: &Message) -> MessageDisposition {
        if !message.path().is_some_and(|path| path == WATCHER_PATH) {
            return MessageDisposition::Unhandled;
        }
        let interface = message
            .interface()
            .map(|value| value.to_string())
            .unwrap_or_default();
        let member = message
            .member()
            .map(|value| value.to_string())
            .unwrap_or_default();
        match (interface.as_str(), member.as_str()) {
            (WATCHER_INTERFACE, "RegisterStatusNotifierItem") => self.register_item(pump, message),
            (WATCHER_INTERFACE, "RegisterStatusNotifierHost") => self.register_host(pump, message),
            (PROPERTIES_INTERFACE, "Get") => get_property(&self.registry, message),
            (PROPERTIES_INTERFACE, "GetAll") => get_all(&self.registry, message),
            _ => MessageDisposition::Unhandled,
        }
    }

    fn register_item(&mut self, pump: &BusPump, message: &Message) -> MessageDisposition {
        let Some(argument) = single_string(message) else {
            return MessageDisposition::Unhandled;
        };
        let sender = message.sender().map(|value| value.to_string());
        let Ok(key) = parse_registration(&argument, sender.as_deref()) else {
            return MessageDisposition::Unhandled;
        };
        match self.registry.register_item(key.clone()) {
            Ok(Registration::Added) => {
                emit(
                    pump,
                    "StatusNotifierItemRegistered",
                    Some(&key.registry_string()),
                );
                if !key.service().starts_with(':') {
                    self.request(
                        pump,
                        Pending::Owner(key.clone()),
                        |service| {
                            Message::new_method_call(
                                DBUS_SERVICE,
                                DBUS_PATH,
                                DBUS_INTERFACE,
                                "GetNameOwner",
                            )
                            .map(|call| call.append1(service.to_owned()))
                        },
                        key.service(),
                    );
                }
                self.request_props(pump, &key);
            }
            Ok(Registration::Known) => {}
            Err(_) => return MessageDisposition::Unhandled,
        }
        MessageDisposition::Reply(message.method_return())
    }

    fn register_host(&mut self, pump: &BusPump, message: &Message) -> MessageDisposition {
        let Some(argument) = single_string(message) else {
            return MessageDisposition::Unhandled;
        };
        match self.registry.add_host(&argument) {
            Ok(Registration::Added) => emit(pump, "StatusNotifierHostRegistered", None),
            Ok(Registration::Known) => {}
            Err(_) => return MessageDisposition::Unhandled,
        }
        MessageDisposition::Reply(message.method_return())
    }

    fn method_return(&mut self, message: &Message) {
        let Some(serial) = message.get_reply_serial() else {
            return;
        };
        let items = message.get_items();
        match self.pending.remove(&serial) {
            Some(Pending::Props(key)) => {
                if let [MessageItem::Dict(dict)] = items.as_slice() {
                    let props = props_from_entries(dict);
                    self.registry
                        .set_props(&key, crate::item::ItemProps::from_props(&props));
                }
            }
            Some(Pending::Owner(key)) => {
                if let [MessageItem::Str(owner)] = items.as_slice() {
                    self.registry.set_owner(&key, owner);
                }
            }
            None => {}
        }
    }

    fn signal(&mut self, pump: &BusPump, message: &Message) -> MessageDisposition {
        let interface = message
            .interface()
            .map(|value| value.to_string())
            .unwrap_or_default();
        let member = message
            .member()
            .map(|value| value.to_string())
            .unwrap_or_default();
        if interface == DBUS_INTERFACE && member == NAME_OWNER_CHANGED {
            self.name_owner_changed(pump, message);
            return MessageDisposition::Handled;
        }
        if interface == ITEM_INTERFACE && member.starts_with("New") {
            let sender = message
                .sender()
                .map(|value| value.to_string())
                .unwrap_or_default();
            let path = message
                .path()
                .map(|value| value.to_string())
                .unwrap_or_default();
            for key in self.registry.keys_for_sender(&sender, &path) {
                self.request_props(pump, &key);
            }
            return MessageDisposition::Handled;
        }
        MessageDisposition::Unhandled
    }

    fn name_owner_changed(&mut self, pump: &BusPump, message: &Message) {
        let items = message.get_items();
        let [MessageItem::Str(name), _old, MessageItem::Str(new_owner)] = items.as_slice() else {
            return;
        };
        if !new_owner.is_empty() {
            return;
        }
        for key in self.registry.remove_for_name(name) {
            emit(
                pump,
                "StatusNotifierItemUnregistered",
                Some(&key.registry_string()),
            );
        }
        if self.registry.remove_host(name) {
            emit(pump, "StatusNotifierHostUnregistered", None);
        }
        let registry = &self.registry;
        self.pending
            .retain(|_, pending| registry.get(pending.key()).is_some());
    }

    fn request_props(&mut self, pump: &BusPump, key: &ItemKey) {
        self.request(
            pump,
            Pending::Props(key.clone()),
            |service| {
                Message::new_method_call(service, key.path(), PROPERTIES_INTERFACE, "GetAll")
                    .map(|call| call.append1(ITEM_INTERFACE.to_owned()))
            },
            key.service(),
        );
    }

    fn request(
        &mut self,
        pump: &BusPump,
        pending: Pending,
        build: impl FnOnce(&str) -> Result<Message, String>,
        service: &str,
    ) {
        if self.pending.len() >= MAX_PENDING {
            return;
        }
        let Ok(call) = build(service) else {
            return;
        };
        if let Ok(serial) = pump.send(call) {
            self.pending.insert(serial, pending);
        }
    }
}

impl Pending {
    fn key(&self) -> &ItemKey {
        match self {
            Self::Props(key) | Self::Owner(key) => key,
        }
    }
}

fn single_string(message: &Message) -> Option<String> {
    match message.get_items().as_slice() {
        [MessageItem::Str(value)] => Some(value.clone()),
        _ => None,
    }
}

fn props_from_entries(entries: &[(MessageItem, MessageItem)]) -> BTreeMap<String, PropValue> {
    entries
        .iter()
        .filter_map(|(key, value)| match key {
            MessageItem::Str(name) => Some((name.clone(), prop_value(value))),
            _ => None,
        })
        .collect()
}

fn prop_value(item: &MessageItem) -> PropValue {
    match item {
        MessageItem::Variant(inner) => prop_value(inner),
        MessageItem::Str(value) => PropValue::Str(value.clone()),
        MessageItem::ObjectPath(path) => PropValue::Str(path.to_string()),
        MessageItem::Bool(value) => PropValue::Bool(*value),
        _ => PropValue::Other,
    }
}

fn watcher_property(registry: &Registry, name: &str) -> Option<MessageItem> {
    match name {
        "RegisteredStatusNotifierItems" => {
            let items: Vec<MessageItem> = registry
                .registered_strings()
                .into_iter()
                .map(MessageItem::Str)
                .collect();
            let array = MessageItemArray::new(items, Signature::new("as").ok()?).ok()?;
            Some(MessageItem::Array(array))
        }
        "IsStatusNotifierHostRegistered" => Some(MessageItem::Bool(registry.host_registered())),
        "ProtocolVersion" => Some(MessageItem::Int32(PROTOCOL_VERSION)),
        _ => None,
    }
}

fn get_property(registry: &Registry, message: &Message) -> MessageDisposition {
    let items = message.get_items();
    let [MessageItem::Str(interface), MessageItem::Str(name)] = items.as_slice() else {
        return MessageDisposition::Unhandled;
    };
    if interface != WATCHER_INTERFACE {
        return MessageDisposition::Unhandled;
    }
    let Some(value) = watcher_property(registry, name) else {
        return MessageDisposition::Unhandled;
    };
    let mut reply = message.method_return();
    reply.append_items(&[MessageItem::Variant(Box::new(value))]);
    MessageDisposition::Reply(reply)
}

fn get_all(registry: &Registry, message: &Message) -> MessageDisposition {
    let items = message.get_items();
    let [MessageItem::Str(interface)] = items.as_slice() else {
        return MessageDisposition::Unhandled;
    };
    if interface != WATCHER_INTERFACE {
        return MessageDisposition::Unhandled;
    }
    let entries: Vec<(MessageItem, MessageItem)> = [
        "RegisteredStatusNotifierItems",
        "IsStatusNotifierHostRegistered",
        "ProtocolVersion",
    ]
    .into_iter()
    .filter_map(|name| {
        let value = watcher_property(registry, name)?;
        Some((
            MessageItem::Str(name.to_owned()),
            MessageItem::Variant(Box::new(value)),
        ))
    })
    .collect();
    let Ok(dict) = MessageItem::new_dict(entries) else {
        return MessageDisposition::Unhandled;
    };
    let mut reply = message.method_return();
    reply.append_items(&[dict]);
    MessageDisposition::Reply(reply)
}

fn emit(pump: &BusPump, member: &str, argument: Option<&str>) {
    let Ok(signal) = Message::new_signal(WATCHER_PATH, WATCHER_INTERFACE, member) else {
        return;
    };
    let signal = match argument {
        Some(value) => signal.append1(value.to_owned()),
        None => signal,
    };
    let _ = pump.send(signal);
}

fn add_match(pump: &BusPump, rule: &str) -> FlameResult<()> {
    let call = Message::new_method_call(DBUS_SERVICE, DBUS_PATH, DBUS_INTERFACE, "AddMatch")
        .map_err(|error| {
            flamewm_api::FlameError::new(flamewm_api::ErrorCode::InvalidArgument, error)
        })?
        .append1(rule.to_owned());
    let reply = pump.send_with_reply(call, std::time::Duration::from_secs(2))?;
    if reply.msg_type() == MessageType::Error {
        return Err(flamewm_api::FlameError::unavailable(
            "cannot watch StatusNotifier signals",
        ));
    }
    Ok(())
}

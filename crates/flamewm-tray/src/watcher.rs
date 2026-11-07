//! `org.kde.StatusNotifierWatcher` service on the session bus, driven by the caller's
//! `flamewm-dbus-reactor` pump. Owns no thread and no timer: all work happens in `on_ready`.

use std::collections::{BTreeMap, HashMap};

use flamewm_api::FlameResult;
use flamewm_dbus_reactor::{
    BusKind, BusPump, BusWatch, Message, MessageDisposition, MessageItem, MessageItemArray,
    MessageType, Signature,
};

use crate::item::{IconPixmap, ITEM_INTERFACE, ItemKey, ItemProps, PropValue, parse_registration};
use crate::menu::{MenuNode, MenuValue};
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
/// Bounds on one `IconPixmap` property: entries per item and bytes per entry.
const MAX_PIXMAPS: usize = 8;
const MAX_PIXMAP_BYTES: usize = 256 * 256 * 4;
/// `com.canonical.dbusmenu`, the interface an item's `Menu` object path serves.
const MENU_INTERFACE: &str = "com.canonical.dbusmenu";
/// Nesting bound while decoding a `GetLayout` reply; `parse_layout` applies the display bound.
const MAX_LAYOUT_WIRE_DEPTH: usize = 32;

/// `org.kde.StatusNotifierItem` methods the tray forwards on user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemAction {
    Activate,
    SecondaryActivate,
    ContextMenu,
}

impl ItemAction {
    const fn member(self) -> &'static str {
        match self {
            Self::Activate => "Activate",
            Self::SecondaryActivate => "SecondaryActivate",
            Self::ContextMenu => "ContextMenu",
        }
    }
}

/// Outcome of one `GetLayout` call, held until the caller takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutReply {
    Menu(MenuNode),
    /// The item returned an error or a malformed layout; callers fall back to `ContextMenu`.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Props(ItemKey),
    Owner(ItemKey),
    Layout(ItemKey),
}

#[derive(Default)]
struct WatcherState {
    registry: Registry,
    pending: HashMap<u32, Pending>,
    layouts: BTreeMap<ItemKey, LayoutReply>,
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

    /// Current items in key order with their reduced properties.
    #[must_use]
    pub fn entries(&self) -> Vec<(ItemKey, ItemProps)> {
        self.state
            .registry
            .items()
            .map(|(key, item)| (key.clone(), item.props.clone()))
            .collect()
    }

    /// Changes whenever the item set or an item's props change.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.state.registry.revision()
    }

    /// Starts a dbusmenu `GetLayout` for the item's menu object. The reply arrives through
    /// `on_ready`; collect it with `take_layout`. A new request drops any earlier reply.
    pub fn request_layout(&mut self, key: &ItemKey, menu_path: &str) -> FlameResult<()> {
        let call = Message::new_method_call(key.service(), menu_path, MENU_INTERFACE, "GetLayout")
            .map_err(|error| {
                flamewm_api::FlameError::new(flamewm_api::ErrorCode::InvalidArgument, error)
            })?
            .append3(0i32, -1i32, Vec::<String>::new());
        let serial = self.pump.send(call)?;
        self.state.layouts.remove(key);
        self.state.pending.insert(serial, Pending::Layout(key.clone()));
        Ok(())
    }

    /// Takes the reply to the last `request_layout` for this item, if it has arrived.
    pub fn take_layout(&mut self, key: &ItemKey) -> Option<LayoutReply> {
        self.state.layouts.remove(key)
    }

    /// Reports a click on dbusmenu entry `id` (`Event(id, "clicked", 0, time)`).
    pub fn send_menu_event(&self, key: &ItemKey, menu_path: &str, id: i32) -> FlameResult<()> {
        let timestamp = flamewm_api::clock::unix_seconds() as u32;
        let mut call = Message::new_method_call(key.service(), menu_path, MENU_INTERFACE, "Event")
            .map_err(|error| {
                flamewm_api::FlameError::new(flamewm_api::ErrorCode::InvalidArgument, error)
            })?;
        call.append_items(&[
            MessageItem::Int32(id),
            MessageItem::Str("clicked".to_owned()),
            MessageItem::Variant(Box::new(MessageItem::Int32(0))),
            MessageItem::UInt32(timestamp),
        ]);
        self.pump.send(call).map(|_| ())
    }

    /// Fire-and-forget method call on an item at the given screen position. The reply, if any,
    /// is ignored; no thread or timer is involved.
    pub fn invoke(
        &self,
        key: &ItemKey,
        action: ItemAction,
        x: i32,
        y: i32,
    ) -> FlameResult<()> {
        let call = Message::new_method_call(
            key.service(),
            key.path(),
            ITEM_INTERFACE,
            action.member(),
        )
        .map_err(|error| flamewm_api::FlameError::new(flamewm_api::ErrorCode::InvalidArgument, error))?
        .append2(x, y);
        self.pump.send(call).map(|_| ())
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
            MessageType::Error => {
                self.method_error(message);
                MessageDisposition::Handled
            }
            MessageType::Signal => self.signal(pump, message),
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
            Some(Pending::Layout(key)) => {
                let reply = layout_reply(&items).map_or(LayoutReply::Failed, LayoutReply::Menu);
                self.layouts.insert(key, reply);
            }
            None => {}
        }
    }

    fn method_error(&mut self, message: &Message) {
        let Some(serial) = message.get_reply_serial() else {
            return;
        };
        if let Some(Pending::Layout(key)) = self.pending.remove(&serial) {
            self.layouts.insert(key, LayoutReply::Failed);
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
        self.layouts.retain(|key, _| registry.get(key).is_some());
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
            Self::Props(key) | Self::Owner(key) | Self::Layout(key) => key,
        }
    }
}

/// Decodes a `GetLayout` reply `(u revision, (ia{sv}av) root)`.
fn layout_reply(items: &[MessageItem]) -> Option<MenuNode> {
    let [MessageItem::UInt32(_), root] = items else {
        return None;
    };
    menu_node(root, 0)
}

fn menu_node(item: &MessageItem, depth: usize) -> Option<MenuNode> {
    if depth > MAX_LAYOUT_WIRE_DEPTH {
        return None;
    }
    let MessageItem::Struct(fields) = unwrap_variant(item) else {
        return None;
    };
    let [MessageItem::Int32(id), props, children] = fields.as_slice() else {
        return None;
    };
    let children = match unwrap_variant(children) {
        MessageItem::Array(array) => array
            .iter()
            .map(|child| menu_node(child, depth + 1))
            .collect::<Option<Vec<_>>>()?,
        _ => Vec::new(),
    };
    Some(MenuNode {
        id: *id,
        props: menu_props(props),
        children,
    })
}

/// Keeps the `a{sv}` properties the menu reducer reads; other value types are dropped.
fn menu_props(item: &MessageItem) -> BTreeMap<String, MenuValue> {
    let MessageItem::Dict(entries) = unwrap_variant(item) else {
        return BTreeMap::new();
    };
    entries
        .iter()
        .filter_map(|(key, value)| {
            let MessageItem::Str(name) = key else {
                return None;
            };
            let value = match unwrap_variant(value) {
                MessageItem::Str(text) => MenuValue::Str(text.clone()),
                MessageItem::Bool(flag) => MenuValue::Bool(*flag),
                MessageItem::Int32(number) => MenuValue::Int(*number),
                _ => return None,
            };
            Some((name.clone(), value))
        })
        .collect()
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
            MessageItem::Str(name) => Some((name.clone(), prop_value(name, value))),
            _ => None,
        })
        .collect()
}

fn prop_value(name: &str, item: &MessageItem) -> PropValue {
    let item = unwrap_variant(item);
    match (name, item) {
        ("ToolTip", MessageItem::Struct(fields)) => PropValue::Tooltip(match fields.get(2) {
            Some(MessageItem::Str(title)) => title.clone(),
            _ => String::new(),
        }),
        ("IconPixmap", MessageItem::Array(array)) => {
            PropValue::Pixmaps(pixmaps(array.clone().into_vec()))
        }
        (_, MessageItem::Str(value)) => PropValue::Str(value.clone()),
        (_, MessageItem::ObjectPath(path)) => PropValue::Str(path.to_string()),
        (_, MessageItem::Bool(value)) => PropValue::Bool(*value),
        _ => PropValue::Other,
    }
}

fn unwrap_variant(item: &MessageItem) -> &MessageItem {
    match item {
        MessageItem::Variant(inner) => unwrap_variant(inner),
        other => other,
    }
}

/// Decodes `a(iiay)`, dropping malformed entries (non-positive size, byte count mismatch, or
/// over the per-entry bound) and capping the entry count.
fn pixmaps(entries: Vec<MessageItem>) -> Vec<IconPixmap> {
    entries
        .into_iter()
        .filter_map(|entry| {
            let MessageItem::Struct(fields) = entry else {
                return None;
            };
            let [MessageItem::Int32(width), MessageItem::Int32(height), MessageItem::Array(bytes)] =
                fields.as_slice()
            else {
                return None;
            };
            let (width, height) = (u32::try_from(*width).ok()?, u32::try_from(*height).ok()?);
            let argb: Vec<u8> = bytes
                .clone()
                .into_vec()
                .into_iter()
                .map(|item| match item {
                    MessageItem::Byte(value) => Some(value),
                    _ => None,
                })
                .collect::<Option<_>>()?;
            let expected = (width as usize).checked_mul(height as usize)?.checked_mul(4)?;
            (width > 0 && height > 0 && expected == argb.len() && argb.len() <= MAX_PIXMAP_BYTES)
                .then_some(IconPixmap {
                    width,
                    height,
                    argb,
                })
        })
        .take(MAX_PIXMAPS)
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Array with an explicit full signature, so empty arrays are representable.
    fn array(signature: &str, items: Vec<MessageItem>) -> MessageItem {
        MessageItem::Array(
            MessageItemArray::new(items, Signature::new(signature).expect("signature"))
                .expect("array"),
        )
    }

    fn pixmap_item(width: i32, height: i32, bytes: Vec<u8>) -> MessageItem {
        MessageItem::Struct(vec![
            MessageItem::Int32(width),
            MessageItem::Int32(height),
            array("ay", bytes.into_iter().map(MessageItem::Byte).collect()),
        ])
    }

    #[test]
    fn icon_pixmap_decodes_valid_entries_and_drops_malformed_ones() {
        let valid = pixmap_item(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let short = pixmap_item(2, 2, vec![0; 8]);
        let zero = pixmap_item(0, 0, Vec::new());
        let decoded = pixmaps(vec![valid, short, zero, MessageItem::Int32(7)]);
        assert_eq!(
            decoded,
            vec![IconPixmap {
                width: 2,
                height: 1,
                argb: vec![1, 2, 3, 4, 5, 6, 7, 8],
            }]
        );
    }

    #[test]
    fn tooltip_keeps_title_and_pixmap_property_reaches_item_props() {
        let tooltip = MessageItem::Variant(Box::new(MessageItem::Struct(vec![
            MessageItem::Str("icon".into()),
            array("a(iiay)", Vec::new()),
            MessageItem::Str("Network connected".into()),
            MessageItem::Str("body".into()),
        ])));
        let pixmaps_value = MessageItem::Variant(Box::new(array(
            "a(iiay)",
            vec![pixmap_item(1, 1, vec![9, 9, 9, 9])],
        )));
        let props = props_from_entries(&[
            (MessageItem::Str("ToolTip".into()), tooltip),
            (MessageItem::Str("IconPixmap".into()), pixmaps_value),
            (
                MessageItem::Str("IconName".into()),
                MessageItem::Variant(Box::new(MessageItem::Str("network-wired".into()))),
            ),
        ]);
        let item = ItemProps::from_props(&props);
        assert_eq!(item.tooltip_title, "Network connected");
        assert_eq!(item.icon_pixmaps.len(), 1);
        assert_eq!(item.icon_name, "network-wired");
    }

    #[test]
    fn layout_reply_decodes_nested_menu_and_rejects_malformed_root() {
        let props = |pairs: Vec<(&str, MessageItem)>| {
            if pairs.is_empty() {
                return array("a{sv}", vec![]);
            }
            MessageItem::new_dict(
                pairs
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
        };
        let child = MessageItem::Variant(Box::new(MessageItem::Struct(vec![
            MessageItem::Int32(7),
            props(vec![("label", MessageItem::Str("_Quit".to_owned()))]),
            array("av", vec![]),
        ])));
        let root = MessageItem::Struct(vec![
            MessageItem::Int32(0),
            props(vec![]),
            array("av", vec![child]),
        ]);
        let node = layout_reply(&[MessageItem::UInt32(3), root]).expect("layout decodes");
        assert_eq!(node.children.len(), 1);
        assert_eq!(node.children[0].id, 7);
        assert_eq!(
            node.children[0].props.get("label"),
            Some(&MenuValue::Str("_Quit".to_owned()))
        );
        assert_eq!(layout_reply(&[MessageItem::UInt32(3)]), None);
        assert_eq!(
            layout_reply(&[MessageItem::UInt32(3), MessageItem::Int32(1)]),
            None
        );
    }
}

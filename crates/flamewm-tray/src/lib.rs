//! `StatusNotifier` tray: pure item registry, registration parsing, dbusmenu layout reduction and
//! overflow policy, plus a `StatusNotifierWatcher` service driven by `flamewm-dbus-reactor`.
//!
//! Item props include `IconName`, decoded `IconPixmap`, and the `ToolTip` title. `TrayWatcher`
//! forwards `Activate`/`SecondaryActivate`/`ContextMenu` calls, fetches dbusmenu layouts
//! (`GetLayout`) and sends menu clicks (`Event`). Not implemented here: panel projection (owned
//! by `flamewm-shell`).

pub mod item;
pub mod menu;
pub mod overflow;
pub mod registry;
mod watcher;

pub use item::{
    DEFAULT_ITEM_PATH, ITEM_INTERFACE, IconPixmap, ItemKey, ItemProps, PropValue,
    RegistrationError, Status, parse_registration,
};
pub use menu::{
    MenuEntry, MenuNode, MenuValue, TrayMenuRow, clean_label, parse_layout, top_level_rows,
};
pub use overflow::{OverflowPlan, plan_overflow};
pub use registry::{ItemState, MAX_HOSTS, MAX_ITEMS, Registration, Registry, RegistryError};
pub use watcher::{
    ItemAction, LayoutReply, TrayWatcher, WATCHER_INTERFACE, WATCHER_NAME, WATCHER_PATH,
};

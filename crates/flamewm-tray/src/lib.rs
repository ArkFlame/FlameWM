//! StatusNotifier tray: pure item registry, registration parsing, dbusmenu layout reduction and
//! overflow policy, plus a `StatusNotifierWatcher` service driven by `flamewm-dbus-reactor`.
//!
//! Item props include `IconName`, decoded `IconPixmap`, and the `ToolTip` title. `TrayWatcher`
//! forwards `Activate`/`SecondaryActivate`/`ContextMenu` calls. Not implemented here: panel
//! projection (owned by `flamewm-shell`) and dbusmenu layout fetch (`GetLayout`); the menu parser
//! is pure and tested, but no caller fetches layouts yet.

pub mod item;
pub mod menu;
pub mod overflow;
pub mod registry;
mod watcher;

pub use item::{
    DEFAULT_ITEM_PATH, ITEM_INTERFACE, IconPixmap, ItemKey, ItemProps, PropValue,
    RegistrationError, Status, parse_registration,
};
pub use menu::{MenuEntry, MenuNode, MenuValue, clean_label, parse_layout};
pub use overflow::{OverflowPlan, plan_overflow};
pub use registry::{ItemState, MAX_HOSTS, MAX_ITEMS, Registration, Registry, RegistryError};
pub use watcher::{ItemAction, TrayWatcher, WATCHER_INTERFACE, WATCHER_NAME, WATCHER_PATH};

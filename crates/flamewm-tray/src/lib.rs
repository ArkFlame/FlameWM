//! StatusNotifier tray: pure item registry, registration parsing, dbusmenu layout reduction and
//! overflow policy, plus a `StatusNotifierWatcher` service driven by `flamewm-dbus-reactor`.
//!
//! Not implemented here: panel projection (`#tray`), icon resolution/cache (`IconPixmap` is
//! ignored), and menu fetch/activation (`GetLayout`, `Activate`, `ContextMenu`). The menu parser
//! is pure and tested, but no caller fetches layouts yet.

pub mod item;
pub mod menu;
pub mod overflow;
pub mod registry;
mod watcher;

pub use item::{
    DEFAULT_ITEM_PATH, ITEM_INTERFACE, ItemKey, ItemProps, PropValue, RegistrationError, Status,
    parse_registration,
};
pub use menu::{MenuEntry, MenuNode, MenuValue, clean_label, parse_layout};
pub use overflow::{OverflowPlan, plan_overflow};
pub use registry::{ItemState, MAX_HOSTS, MAX_ITEMS, Registration, Registry, RegistryError};
pub use watcher::{TrayWatcher, WATCHER_INTERFACE, WATCHER_NAME, WATCHER_PATH};

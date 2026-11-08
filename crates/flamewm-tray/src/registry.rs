//! Bounded tray item and host registry. Pure: the watcher adapter feeds it bus facts.

use std::collections::{BTreeMap, BTreeSet};

use crate::item::{ItemKey, ItemProps};

/// Hard cap on registered items; registrations beyond it are refused, not evicted.
pub const MAX_ITEMS: usize = 64;
pub const MAX_HOSTS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemState {
    /// Unique bus owner, learned from `GetNameOwner` for well-known names.
    pub owner: Option<String>,
    pub props: ItemProps,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registration {
    Added,
    Known,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryError {
    Full,
}

#[derive(Debug, Default)]
pub struct Registry {
    items: BTreeMap<ItemKey, ItemState>,
    hosts: BTreeSet<String>,
    /// Bumped on every change a panel projection can show (item set, props).
    revision: u64,
}

impl Registry {
    /// Idempotent: re-registering a known item is `Known` and changes nothing.
    pub fn register_item(&mut self, key: ItemKey) -> Result<Registration, RegistryError> {
        if self.items.contains_key(&key) {
            return Ok(Registration::Known);
        }
        if self.items.len() >= MAX_ITEMS {
            return Err(RegistryError::Full);
        }
        self.items.insert(key, ItemState::default());
        self.revision += 1;
        Ok(Registration::Added)
    }

    pub fn set_owner(&mut self, key: &ItemKey, owner: &str) -> bool {
        match self.items.get_mut(key) {
            Some(state) => {
                state.owner = Some(owner.to_owned());
                true
            }
            None => false,
        }
    }

    pub fn set_props(&mut self, key: &ItemKey, props: ItemProps) -> bool {
        match self.items.get_mut(key) {
            Some(state) => {
                if state.props != props {
                    state.props = props;
                    self.revision += 1;
                }
                true
            }
            None => false,
        }
    }

    /// Removes every item whose service or owner is `name` (a lost bus name or connection).
    pub fn remove_for_name(&mut self, name: &str) -> Vec<ItemKey> {
        let gone: Vec<ItemKey> = self
            .items
            .iter()
            .filter(|(key, state)| key.service() == name || state.owner.as_deref() == Some(name))
            .map(|(key, _)| key.clone())
            .collect();
        for key in &gone {
            self.items.remove(key);
        }
        if !gone.is_empty() {
            self.revision += 1;
        }
        gone
    }

    /// Items whose `PropertiesChanged`-style signals arrive from `sender` on `path`.
    #[must_use]
    pub fn keys_for_sender(&self, sender: &str, path: &str) -> Vec<ItemKey> {
        self.items
            .iter()
            .filter(|(key, state)| {
                key.path() == path
                    && (key.service() == sender || state.owner.as_deref() == Some(sender))
            })
            .map(|(key, _)| key.clone())
            .collect()
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn get(&self, key: &ItemKey) -> Option<&ItemState> {
        self.items.get(key)
    }

    pub fn items(&self) -> impl Iterator<Item = (&ItemKey, &ItemState)> {
        self.items.iter()
    }

    #[must_use]
    pub fn registered_strings(&self) -> Vec<String> {
        self.items.keys().map(ItemKey::registry_string).collect()
    }

    pub fn add_host(&mut self, name: &str) -> Result<Registration, RegistryError> {
        if self.hosts.contains(name) {
            return Ok(Registration::Known);
        }
        if self.hosts.len() >= MAX_HOSTS {
            return Err(RegistryError::Full);
        }
        self.hosts.insert(name.to_owned());
        Ok(Registration::Added)
    }

    pub fn remove_host(&mut self, name: &str) -> bool {
        self.hosts.remove(name)
    }

    #[must_use]
    pub fn host_registered(&self) -> bool {
        !self.hosts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{PropValue, Status};

    fn key(service: &str) -> ItemKey {
        ItemKey::new(service, "/StatusNotifierItem").unwrap()
    }

    #[test]
    fn registration_is_idempotent() {
        let mut registry = Registry::default();
        assert_eq!(registry.register_item(key(":1.5")), Ok(Registration::Added));
        assert_eq!(registry.register_item(key(":1.5")), Ok(Registration::Known));
        assert_eq!(registry.registered_strings(), [":1.5/StatusNotifierItem"]);
    }

    #[test]
    fn revision_moves_only_on_visible_changes() {
        let mut registry = Registry::default();
        let start = registry.revision();
        registry.register_item(key(":1.7")).unwrap();
        let added = registry.revision();
        assert!(added > start);
        registry.register_item(key(":1.7")).unwrap();
        assert_eq!(registry.revision(), added, "known item changes nothing");
        let mut titled = BTreeMap::new();
        titled.insert("Title".to_owned(), PropValue::Str("Clock".into()));
        let props = ItemProps::from_props(&titled);
        registry.set_props(&key(":1.7"), props.clone());
        let after_props = registry.revision();
        assert!(after_props > added);
        registry.set_props(&key(":1.7"), props);
        assert_eq!(
            registry.revision(),
            after_props,
            "identical props change nothing"
        );
        registry.remove_for_name(":1.7");
        assert!(registry.revision() > after_props);
    }

    #[test]
    fn capacity_refuses_without_evicting() {
        let mut registry = Registry::default();
        for index in 0..MAX_ITEMS {
            registry.register_item(key(&format!(":1.{index}"))).unwrap();
        }
        assert_eq!(
            registry.register_item(key(":1.999")),
            Err(RegistryError::Full)
        );
        assert_eq!(registry.items().count(), MAX_ITEMS);
        assert_eq!(registry.register_item(key(":1.0")), Ok(Registration::Known));
    }

    #[test]
    fn removing_owner_drops_its_items_only() {
        let mut registry = Registry::default();
        registry.register_item(key(":1.5")).unwrap();
        registry.register_item(key(":1.6")).unwrap();
        let wellknown = key("org.example.Tray");
        registry.register_item(wellknown.clone()).unwrap();
        assert!(registry.set_owner(&wellknown, ":1.7"));

        assert_eq!(registry.remove_for_name(":1.7"), vec![wellknown]);
        assert_eq!(registry.remove_for_name(":1.5"), vec![key(":1.5")]);
        assert_eq!(registry.registered_strings(), [":1.6/StatusNotifierItem"]);
        assert!(registry.remove_for_name(":9.9").is_empty());
    }

    #[test]
    fn sender_matching_uses_service_or_owner_and_path() {
        let mut registry = Registry::default();
        let wellknown = key("org.example.Tray");
        registry.register_item(wellknown.clone()).unwrap();
        registry.set_owner(&wellknown, ":1.20");
        assert_eq!(
            registry.keys_for_sender(":1.20", "/StatusNotifierItem"),
            vec![wellknown.clone()]
        );
        assert_eq!(
            registry.keys_for_sender("org.example.Tray", "/StatusNotifierItem"),
            vec![wellknown]
        );
        assert!(registry.keys_for_sender(":1.20", "/Other").is_empty());
    }

    #[test]
    fn props_are_stored_for_known_items_only() {
        let mut registry = Registry::default();
        let item = key(":1.5");
        assert!(!registry.set_props(&item, ItemProps::default()));
        registry.register_item(item.clone()).unwrap();
        let mut props = std::collections::BTreeMap::new();
        props.insert("Title".to_owned(), PropValue::Str("Clock".into()));
        props.insert("Status".to_owned(), PropValue::Str("Passive".into()));
        assert!(registry.set_props(&item, ItemProps::from_props(&props)));
        let state = registry.get(&item).unwrap();
        assert_eq!(state.props.title, "Clock");
        assert_eq!(state.props.status, Status::Passive);
    }

    #[test]
    fn hosts_are_bounded_and_drive_host_registered_state() {
        let mut registry = Registry::default();
        assert!(!registry.host_registered());
        assert_eq!(registry.add_host(":1.3"), Ok(Registration::Added));
        assert_eq!(registry.add_host(":1.3"), Ok(Registration::Known));
        assert!(registry.host_registered());
        for index in 1..MAX_HOSTS {
            registry.add_host(&format!(":2.{index}")).unwrap();
        }
        assert_eq!(registry.add_host(":9.9"), Err(RegistryError::Full));
        assert!(registry.remove_host(":1.3"));
        assert!(!registry.remove_host(":1.3"));
    }
}

//! Typed configuration-store contract and an in-memory test double.
//!
//! Step 1 of the w45 remaster defines the owner boundary only. No consumer is migrated yet, so
//! behaviour and persisted bytes stay unchanged. A store owns bounded reading, parsing,
//! quarantine, defaults, atomic write and at most one user-visible notice; [`ConfigWatch`] is the
//! push path that replaces mtime polling.

use std::collections::VecDeque;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};

/// A typed snapshot that can be defaulted, copied for readers and shown in diagnostics.
pub trait ConfigValue: Default + Clone + Debug {}
impl<T: Default + Clone + Debug> ConfigValue for T {}

/// What a load returns: the usable value and at most one notice for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded<T> {
    /// The value to use; defaults when the stored file was missing or unusable.
    pub value: T,
    /// The single notice produced by a failed load, if any.
    pub notice: Option<String>,
}

/// A change already pushed by a store or an external notifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigChange {
    /// This store completed a save.
    Saved,
    /// Another writer or a native notifier changed the file.
    External,
}

/// Non-blocking drain of pushed changes: callers react from their main loop, never on a timer.
pub trait ConfigWatch {
    /// Take the next queued change, if any.
    #[must_use]
    fn next_change(&self) -> Option<ConfigChange>;
}

/// One typed owner for a config file.
pub trait TypedConfigStore {
    /// Typed value owned by this store.
    type Value: ConfigValue;
    /// Handle that receives change events pushed by this store.
    type Watch: ConfigWatch;

    /// Load the typed value; a missing or unusable file yields defaults and at most one notice.
    #[must_use]
    fn load(&self) -> Loaded<Self::Value>;

    /// Atomically persist `value`, returning one notice text on failure.
    fn save(&self, value: &Self::Value) -> Result<(), String>;

    /// Create a handle that receives changes pushed by this store.
    #[must_use]
    fn watch(&self) -> Self::Watch;
}

#[derive(Debug)]
struct MemoryState<T> {
    value: T,
    load_notice: Option<String>,
    save_failure: Option<String>,
    changes: VecDeque<ConfigChange>,
}

/// Deterministic [`TypedConfigStore`] double for tests; it touches no filesystem.
#[derive(Debug)]
pub struct MemoryConfigStore<T> {
    state: Arc<Mutex<MemoryState<T>>>,
}

impl<T: ConfigValue> Default for MemoryConfigStore<T> {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(MemoryState {
                value: T::default(),
                load_notice: None,
                save_failure: None,
                changes: VecDeque::new(),
            })),
        }
    }
}

impl<T: ConfigValue> MemoryConfigStore<T> {
    /// Queue the single notice returned by the next `load`.
    pub fn set_load_notice(&self, notice: impl Into<String>) {
        self.state.lock().expect("config store lock").load_notice = Some(notice.into());
    }

    /// Make the next `save` fail with `notice`, without changing the value or queuing a change.
    pub fn fail_next_save(&self, notice: impl Into<String>) {
        self.state.lock().expect("config store lock").save_failure = Some(notice.into());
    }

    /// Push an external change, as a native notifier would.
    pub fn notify_external(&self) {
        self.state
            .lock()
            .expect("config store lock")
            .changes
            .push_back(ConfigChange::External);
    }
}

impl<T: ConfigValue> TypedConfigStore for MemoryConfigStore<T> {
    type Value = T;
    type Watch = MemoryWatch<T>;

    fn load(&self) -> Loaded<T> {
        let mut state = self.state.lock().expect("config store lock");
        Loaded {
            value: state.value.clone(),
            notice: state.load_notice.take(),
        }
    }

    fn save(&self, value: &T) -> Result<(), String> {
        let mut state = self.state.lock().expect("config store lock");
        if let Some(notice) = state.save_failure.take() {
            return Err(notice);
        }
        state.value = value.clone();
        state.changes.push_back(ConfigChange::Saved);
        Ok(())
    }

    fn watch(&self) -> MemoryWatch<T> {
        MemoryWatch {
            state: Arc::clone(&self.state),
        }
    }
}

/// Watch handle returned by [`MemoryConfigStore::watch`].
#[derive(Debug)]
pub struct MemoryWatch<T> {
    state: Arc<Mutex<MemoryState<T>>>,
}

impl<T: ConfigValue> ConfigWatch for MemoryWatch<T> {
    fn next_change(&self) -> Option<ConfigChange> {
        self.state
            .lock()
            .expect("config store lock")
            .changes
            .pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Prefs {
        hidden: bool,
    }

    impl Default for Prefs {
        fn default() -> Self {
            Self { hidden: false }
        }
    }

    #[test]
    fn fresh_store_loads_defaults_without_notice() {
        let store = MemoryConfigStore::<Prefs>::default();
        assert_eq!(
            store.load(),
            Loaded {
                value: Prefs::default(),
                notice: None,
            }
        );
    }

    #[test]
    fn load_notice_is_delivered_once() {
        let store = MemoryConfigStore::<Prefs>::default();
        store.set_load_notice("settings too large; using defaults");
        let first = store.load();
        let second = store.load();
        assert_eq!(
            first.notice.as_deref(),
            Some("settings too large; using defaults")
        );
        assert_eq!(second.notice, None);
    }

    #[test]
    fn save_updates_value_and_pushes_one_change() {
        let store = MemoryConfigStore::<Prefs>::default();
        let watch = store.watch();
        store.save(&Prefs { hidden: true }).expect("save");
        assert_eq!(store.load().value, Prefs { hidden: true });
        assert_eq!(watch.next_change(), Some(ConfigChange::Saved));
        assert_eq!(watch.next_change(), None);
    }

    #[test]
    fn failed_save_keeps_value_and_pushes_no_change() {
        let store = MemoryConfigStore::<Prefs>::default();
        let watch = store.watch();
        store.fail_next_save("read-only home");
        assert_eq!(
            store.save(&Prefs { hidden: true }),
            Err("read-only home".to_owned())
        );
        assert_eq!(store.load().value, Prefs::default());
        assert_eq!(watch.next_change(), None);
    }

    #[test]
    fn external_change_is_pushed_without_polling() {
        let store = MemoryConfigStore::<Prefs>::default();
        let watch = store.watch();
        store.notify_external();
        assert_eq!(watch.next_change(), Some(ConfigChange::External));
        assert_eq!(watch.next_change(), None);
    }
}

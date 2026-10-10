use std::collections::{BTreeMap, BTreeSet};

use flamewm_api::{ErrorCode, FlameError, FlameResult};

pub const MAX_DISPATCH_PER_TURN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchInterest {
    pub readable: bool,
    pub writable: bool,
}

impl WatchInterest {
    #[must_use]
    pub const fn any(self) -> bool {
        self.readable || self.writable
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchRegistration {
    pub id: u64,
    pub fd: i32,
    pub interest: WatchInterest,
    pub enabled: bool,
    pub service: String,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeoutRegistration {
    pub id: u64,
    pub interval_ms: u64,
    pub enabled: bool,
    pub service: String,
    pub generation: u64,
}

/// Pure state owned by a concrete D-Bus adapter.
///
/// The actual FD/timer registration belongs to the FlameWM desktop reactor adapter. This state keeps
/// stale callback guards, bounded dispatch and unregister-before-free bookkeeping testable without
/// native bus bindings or a second event loop.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DispatcherState {
    shutdown: bool,
    next_watch_id: u64,
    next_timeout_id: u64,
    generations: BTreeMap<String, u64>,
    watches: BTreeMap<u64, WatchRegistration>,
    timeouts: BTreeMap<u64, TimeoutRegistration>,
    pending_reconcile: BTreeSet<String>,
}

impl DispatcherState {
    #[must_use]
    pub const fn is_shutdown(&self) -> bool {
        self.shutdown
    }

    #[must_use]
    pub fn generation_for(&self, service: &str) -> u64 {
        self.generations.get(service).copied().unwrap_or(0)
    }

    pub fn bump_generation(&mut self, service: &str) -> u64 {
        let next = self.generation_for(service).saturating_add(1).max(1);
        self.generations.insert(service.to_owned(), next);
        next
    }

    #[must_use]
    pub fn is_stale(&self, service: &str, captured_generation: u64) -> bool {
        self.generation_for(service) != captured_generation
    }

    pub fn add_watch(
        &mut self,
        fd: i32,
        interest: WatchInterest,
        service: &str,
    ) -> FlameResult<WatchRegistration> {
        self.ensure_running()?;
        if fd < 0 {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "D-Bus watch fd must be non-negative",
            ));
        }
        if !interest.any() {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "D-Bus watch must request read or write interest",
            ));
        }
        self.next_watch_id = self.next_watch_id.saturating_add(1).max(1);
        let registration = WatchRegistration {
            id: self.next_watch_id,
            fd,
            interest,
            enabled: true,
            service: service.to_owned(),
            generation: self.generation_for(service),
        };
        self.watches.insert(registration.id, registration.clone());
        Ok(registration)
    }

    pub fn remove_watch(&mut self, id: u64) -> Option<WatchRegistration> {
        self.watches.remove(&id)
    }

    pub fn set_watch_enabled(&mut self, id: u64, enabled: bool) -> FlameResult<()> {
        let Some(watch) = self.watches.get_mut(&id) else {
            return Err(FlameError::new(
                ErrorCode::NotFound,
                "D-Bus watch registration not found",
            ));
        };
        watch.enabled = enabled;
        Ok(())
    }

    pub fn add_timeout(
        &mut self,
        interval_ms: u64,
        service: &str,
    ) -> FlameResult<TimeoutRegistration> {
        self.ensure_running()?;
        if interval_ms == 0 {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "D-Bus timeout interval must be positive",
            ));
        }
        self.next_timeout_id = self.next_timeout_id.saturating_add(1).max(1);
        let registration = TimeoutRegistration {
            id: self.next_timeout_id,
            interval_ms,
            enabled: true,
            service: service.to_owned(),
            generation: self.generation_for(service),
        };
        self.timeouts.insert(registration.id, registration.clone());
        Ok(registration)
    }

    pub fn request_reconcile(&mut self, service: &str) {
        if !self.shutdown {
            self.pending_reconcile.insert(service.to_owned());
        }
    }

    #[must_use]
    pub const fn dispatch_budget(requested: usize) -> usize {
        if requested > MAX_DISPATCH_PER_TURN {
            MAX_DISPATCH_PER_TURN
        } else {
            requested
        }
    }

    pub fn shutdown(&mut self) {
        self.shutdown = true;
        self.pending_reconcile.clear();
        self.watches.clear();
        self.timeouts.clear();
    }

    fn ensure_running(&self) -> FlameResult<()> {
        if self.shutdown {
            Err(FlameError::new(
                ErrorCode::Unavailable,
                "D-Bus dispatcher is shut down",
            ))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_generation_invalidates_existing_watch_callback() {
        let mut state = DispatcherState::default();
        let watch = state
            .add_watch(
                4,
                WatchInterest {
                    readable: true,
                    writable: false,
                },
                "NetworkManager",
            )
            .expect("watch");
        let _ = state.bump_generation("NetworkManager");
        assert!(state.is_stale("NetworkManager", watch.generation));
    }

    #[test]
    fn dispatch_budget_is_hard_bounded() {
        assert_eq!(
            DispatcherState::dispatch_budget(10_000),
            MAX_DISPATCH_PER_TURN
        );
    }

    #[test]
    fn shutdown_unregisters_every_registration() {
        let mut state = DispatcherState::default();
        let _ = state.add_watch(
            3,
            WatchInterest {
                readable: true,
                writable: false,
            },
            "MPRIS",
        );
        let _ = state.add_timeout(250, "MPRIS");
        state.shutdown();
        assert!(state.watches.is_empty() && state.timeouts.is_empty());
    }
}

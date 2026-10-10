//! Engine-neutral domain events and RAII subscription ownership.

use crate::TaskEntryId;

pub struct Subscription {
    disconnect: Option<Box<dyn FnOnce()>>,
}

impl Subscription {
    #[must_use]
    pub fn new(disconnect: impl FnOnce() + 'static) -> Self {
        Self {
            disconnect: Some(Box::new(disconnect)),
        }
    }

    #[must_use]
    pub const fn inactive() -> Self {
        Self { disconnect: None }
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.disconnect.is_some()
    }

    pub fn reset(&mut self) {
        if let Some(disconnect) = self.disconnect.take() {
            disconnect();
        }
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.reset();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsChangedEvent {
    pub revision: u64,
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopologyChangedEvent {
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceChangedEvent {
    pub revision: u64,
    pub active_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskOrderChangedEvent {
    pub revision: u64,
    pub order: Vec<TaskEntryId>,
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    #[test]
    fn subscription_disconnects_exactly_once() {
        let count = Rc::new(Cell::new(0));
        let captured = Rc::clone(&count);
        let mut subscription = Subscription::new(move || captured.set(captured.get() + 1));
        subscription.reset();
        subscription.reset();
        assert_eq!(count.get(), 1);
    }
}

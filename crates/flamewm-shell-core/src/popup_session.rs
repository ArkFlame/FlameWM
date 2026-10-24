//! Pure ownership for the shell's single transient popup session.

use flamewm_api::OutputId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PopupKind {
    Start,
    Launcher,
    TaskContext,
    Media,
    Audio,
    Network,
    Calendar,
    Battery,
    Weather,
}

/// Exact output and snapshot provenance for one popup request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PopupTarget {
    pub output: OutputId,
    pub topology_generation: u64,
    pub panels_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PopupLease {
    pub output: OutputId,
    pub kind: PopupKind,
    pub topology_generation: u64,
    pub panels_revision: u64,
    pub sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PopupTransition {
    Close(PopupLease),
    Open(PopupLease),
    Replace {
        previous: PopupLease,
        next: PopupLease,
    },
    Keep,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PopupSession {
    active: Option<PopupLease>,
    next_sequence: u64,
}

impl Default for PopupSession {
    fn default() -> Self {
        Self {
            active: None,
            next_sequence: 1,
        }
    }
}

impl PopupSession {
    #[must_use]
    pub fn active(&self) -> Option<&PopupLease> {
        self.active.as_ref()
    }

    /// Resolve a user toggle without committing surface state.
    pub fn toggle(
        &mut self,
        kind: PopupKind,
        target: PopupTarget,
    ) -> Result<PopupTransition, SequenceExhausted> {
        if let Some(active) = self.active.as_ref()
            && active.kind == kind
            && active.output == target.output
        {
            return Ok(PopupTransition::Close(active.clone()));
        }
        self.open_transition(kind, target)
    }

    /// Resolve an idempotent open. The caller must validate the target's
    /// connected panel and measured anchor before requesting or presenting it.
    pub fn open(
        &mut self,
        kind: PopupKind,
        target: PopupTarget,
    ) -> Result<PopupTransition, SequenceExhausted> {
        if self.active.as_ref().is_some_and(|active| {
            active.kind == kind
                && active.output == target.output
                && active.topology_generation == target.topology_generation
                && active.panels_revision == target.panels_revision
        }) {
            return Ok(PopupTransition::Keep);
        }
        self.open_transition(kind, target)
    }

    /// A close from an old popup session cannot close a newer lease.
    #[must_use]
    pub fn close(&self, sequence: u64) -> PopupTransition {
        match self
            .active
            .as_ref()
            .filter(|lease| lease.sequence == sequence)
        {
            Some(lease) => PopupTransition::Close(lease.clone()),
            None => PopupTransition::Keep,
        }
    }

    /// Refresh the global snapshot provenance after confirming that this
    /// lease's output geometry is unchanged. The session sequence stays put.
    pub fn refresh_target(
        &mut self,
        output: &OutputId,
        topology_generation: u64,
        panels_revision: u64,
    ) -> bool {
        let Some(active) = self.active.as_mut().filter(|lease| &lease.output == output) else {
            return false;
        };
        active.topology_generation = topology_generation;
        active.panels_revision = panels_revision;
        true
    }

    /// Commit only after the native surface operation succeeds. Returns
    /// false when a queued transition no longer matches the active lease.
    pub fn commit(&mut self, transition: &PopupTransition) -> bool {
        match transition {
            PopupTransition::Keep => true,
            PopupTransition::Close(lease) if self.active.as_ref() == Some(lease) => {
                self.active = None;
                true
            }
            PopupTransition::Open(lease) if self.active.is_none() => {
                self.active = Some(lease.clone());
                true
            }
            PopupTransition::Replace { previous, next }
                if self.active.as_ref() == Some(previous) =>
            {
                self.active = Some(next.clone());
                true
            }
            _ => false,
        }
    }

    fn open_transition(
        &mut self,
        kind: PopupKind,
        target: PopupTarget,
    ) -> Result<PopupTransition, SequenceExhausted> {
        let lease = PopupLease {
            output: target.output,
            kind,
            topology_generation: target.topology_generation,
            panels_revision: target.panels_revision,
            sequence: self.next_sequence,
        };
        self.next_sequence = self.next_sequence.checked_add(1).ok_or(SequenceExhausted)?;
        Ok(match self.active.as_ref() {
            Some(previous) => PopupTransition::Replace {
                previous: previous.clone(),
                next: lease,
            },
            None => PopupTransition::Open(lease),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequenceExhausted;

#[cfg(test)]
mod tests {
    use super::*;

    fn target(output: &str, topology_generation: u64, panels_revision: u64) -> PopupTarget {
        PopupTarget {
            output: OutputId::new(output),
            topology_generation,
            panels_revision,
        }
    }

    #[test]
    fn start_a_to_b_replaces_in_one_transition_and_same_output_toggles_closed() {
        let mut session = PopupSession::default();
        let open_a = session.toggle(PopupKind::Start, target("A", 4, 7)).unwrap();
        assert!(matches!(open_a, PopupTransition::Open(_)));
        assert!(session.commit(&open_a));

        let replace_b = session.toggle(PopupKind::Start, target("B", 4, 7)).unwrap();
        let PopupTransition::Replace { previous, next } = &replace_b else {
            panic!("A to B must replace the popup in one transition");
        };
        assert_eq!(previous.output, OutputId::new("A"));
        assert_eq!(next.output, OutputId::new("B"));
        assert_eq!(next.sequence, 2);
        assert!(session.commit(&replace_b));

        let close_b = session.toggle(PopupKind::Start, target("B", 4, 7)).unwrap();
        assert!(matches!(close_b, PopupTransition::Close(_)));
        assert!(session.commit(&close_b));
        assert_eq!(session.active(), None);
    }

    #[test]
    fn stale_close_and_failed_replace_cannot_change_the_active_lease() {
        let mut session = PopupSession::default();
        let open = session.open(PopupKind::Network, target("A", 2, 3)).unwrap();
        assert!(session.commit(&open));
        let active = session.active().cloned().unwrap();

        assert_eq!(session.close(active.sequence + 1), PopupTransition::Keep);
        let replace = session
            .open(PopupKind::Calendar, target("B", 2, 3))
            .unwrap();
        let PopupTransition::Replace { next, .. } = &replace else {
            panic!("opening another popup kind must replace the active lease");
        };
        let next = next.clone();
        assert_eq!(
            session.active(),
            Some(&active),
            "proposal does not commit early"
        );
        assert!(session.commit(&replace));
        assert!(!session.commit(&PopupTransition::Close(active.clone())));
        assert_eq!(session.active(), Some(&next));
    }

    #[test]
    fn open_is_idempotent_only_for_the_same_target_snapshot() {
        let mut session = PopupSession::default();
        let open = session.open(PopupKind::Audio, target("A", 2, 3)).unwrap();
        assert!(session.commit(&open));
        assert_eq!(
            session.open(PopupKind::Audio, target("A", 2, 3)).unwrap(),
            PopupTransition::Keep,
        );
        assert!(matches!(
            session.open(PopupKind::Audio, target("A", 2, 4)).unwrap(),
            PopupTransition::Replace { .. }
        ));
    }

    #[test]
    fn refreshing_unaffected_target_advances_provenance_without_new_lease() {
        let mut session = PopupSession::default();
        let open = session.open(PopupKind::Start, target("A", 2, 3)).unwrap();
        assert!(session.commit(&open));
        let sequence = session.active().unwrap().sequence;
        assert!(session.refresh_target(&OutputId::new("A"), 4, 5));
        let active = session.active().unwrap();
        assert_eq!((active.topology_generation, active.panels_revision), (4, 5));
        assert_eq!(active.sequence, sequence);
        assert!(!session.refresh_target(&OutputId::new("B"), 6, 7));
    }
}

use flamewm_api::{ErrorCode, FlameError, FlameResult, ModeId, OutputId, TransactionId};

pub const DISPLAY_CONFIRM_TIMEOUT_MS: u64 = 15_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDisplayChange {
    pub transaction: TransactionId,
    pub output: OutputId,
    pub mode: ModeId,
    pub topology_generation: u64,
    pub started_ms: u64,
    pub deadline_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayDecision {
    Keep(TransactionId),
    Revert(TransactionId),
}

/// Product-level Keep/Revert transaction coordinator. Native mode capture/apply/revert stays in the
/// engine adapter so Settings process death cannot strand authoritative display state.
#[derive(Debug, Default)]
pub struct DisplayCoordinator {
    pending: Option<PendingDisplayChange>,
}

impl DisplayCoordinator {
    #[must_use]
    pub fn pending(&self) -> Option<&PendingDisplayChange> {
        self.pending.as_ref()
    }

    #[must_use]
    pub fn is_expired(&self, now_ms: u64) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| now_ms >= pending.deadline_ms)
    }

    #[must_use]
    pub fn topology_is_stale(&self, generation: u64) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| generation > pending.topology_generation.saturating_add(1))
    }

    #[must_use]
    pub fn pending_output_missing<'a, I>(&self, outputs: I) -> bool
    where
        I: IntoIterator<Item = &'a OutputId>,
    {
        let Some(pending) = self.pending.as_ref() else {
            return false;
        };
        !outputs.into_iter().any(|output| output == &pending.output)
    }

    pub fn begin(
        &mut self,
        transaction: TransactionId,
        output: OutputId,
        mode: ModeId,
        topology_generation: u64,
        now_ms: u64,
    ) -> FlameResult<&PendingDisplayChange> {
        if self.pending.is_some() {
            return Err(FlameError::new(
                ErrorCode::Busy,
                "display transaction already pending",
            ));
        }
        if !transaction.is_valid() || !mode.is_valid() || !output.is_valid() {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "invalid display transaction",
            ));
        }
        Ok(self.pending.insert(PendingDisplayChange {
            transaction,
            output,
            mode,
            topology_generation,
            started_ms: now_ms,
            deadline_ms: now_ms.saturating_add(DISPLAY_CONFIRM_TIMEOUT_MS),
        }))
    }

    pub fn keep(&mut self, transaction: TransactionId) -> FlameResult<DisplayDecision> {
        self.take_matching(transaction)
            .map(|_| DisplayDecision::Keep(transaction))
    }

    pub fn revert(&mut self, transaction: TransactionId) -> FlameResult<DisplayDecision> {
        self.take_matching(transaction)
            .map(|_| DisplayDecision::Revert(transaction))
    }

    #[must_use]
    pub fn tick(&mut self, now_ms: u64) -> Option<DisplayDecision> {
        if !self.is_expired(now_ms) {
            return None;
        }
        self.pending
            .take()
            .map(|pending| DisplayDecision::Revert(pending.transaction))
    }

    #[must_use]
    pub fn topology_changed(&mut self, generation: u64) -> Option<DisplayDecision> {
        if !self.topology_is_stale(generation) {
            return None;
        }
        self.pending
            .take()
            .map(|pending| DisplayDecision::Revert(pending.transaction))
    }

    fn take_matching(&mut self, transaction: TransactionId) -> FlameResult<PendingDisplayChange> {
        match self.pending.take() {
            Some(pending) if pending.transaction == transaction => Ok(pending),
            Some(pending) => {
                self.pending = Some(pending);
                Err(FlameError::new(
                    ErrorCode::Conflict,
                    "display transaction id mismatch",
                ))
            }
            None => Err(FlameError::new(
                ErrorCode::NotFound,
                "no pending display transaction",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_change_times_out_to_revert() {
        let mut coordinator = DisplayCoordinator::default();
        coordinator
            .begin(
                TransactionId(7),
                OutputId::new("edid:eDP-1"),
                ModeId(5),
                4,
                100,
            )
            .expect("valid transaction");
        assert_eq!(coordinator.tick(15_099), None);
        assert_eq!(
            coordinator.tick(15_100),
            Some(DisplayDecision::Revert(TransactionId(7)))
        );
    }

    fn begin_hdmi_mode_change(coordinator: &mut DisplayCoordinator) {
        coordinator
            .begin(
                TransactionId(7),
                OutputId::new("edid:HDMI-1"),
                ModeId(9),
                8,
                0,
            )
            .expect("valid transaction");
    }

    #[test]
    fn mode_apply_generation_increment_is_not_hotplug() {
        let mut coordinator = DisplayCoordinator::default();
        begin_hdmi_mode_change(&mut coordinator);
        assert_eq!(coordinator.topology_changed(9), None);
    }

    #[test]
    fn external_topology_jump_forces_revert() {
        let mut coordinator = DisplayCoordinator::default();
        begin_hdmi_mode_change(&mut coordinator);
        assert_eq!(
            coordinator.topology_changed(10),
            Some(DisplayDecision::Revert(TransactionId(7)))
        );
    }
}

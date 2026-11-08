use super::*;

impl DisplayService {
    pub fn keep<P: DisplayPort>(
        &mut self,
        port: &mut P,
        transaction: TransactionId,
        now_ms: u64,
    ) -> FlameResult<()> {
        require_transaction(&self.coordinator, transaction)?;
        if self.coordinator.is_expired(now_ms) {
            self.tick(port, now_ms)?;
            return Err(FlameError::new(
                ErrorCode::Timeout,
                "display mode confirmation deadline expired; previous mode restored",
            ));
        }
        let (product_generation, scales) = self.product_generation_and_scales();
        port.keep_mode(transaction)?;
        self.coordinator.keep(transaction)?;
        let mut fresh = port.display_snapshot()?;
        self.engine_generation = fresh.generation;
        restore_shell_scales(&mut fresh, &scales);
        fresh.generation = product_generation;
        fresh.pending = None;
        self.cached = Some(fresh);
        Ok(())
    }

    pub fn revert<P: DisplayPort>(
        &mut self,
        port: &mut P,
        transaction: TransactionId,
    ) -> FlameResult<()> {
        require_transaction(&self.coordinator, transaction)?;
        let (product_generation, scales) = self.product_generation_and_scales();
        // Retire the transaction before crossing the native boundary. If
        // RandR refuses the revert, the caller must not repeatedly dispatch
        // with an already-expired deadline.
        self.coordinator.revert(transaction)?;
        let revert_result = port.revert_mode(transaction);
        let snapshot_result = port.display_snapshot();
        let mut fresh = match snapshot_result {
            Ok(snapshot) => snapshot,
            Err(error) => {
                if let Some(cached) = self.cached.as_mut() {
                    cached.pending = None;
                }
                return Err(revert_result.err().unwrap_or(error));
            }
        };
        self.engine_generation = fresh.generation;
        restore_shell_scales(&mut fresh, &scales);
        fresh.generation = product_generation.saturating_add(1).max(1);
        fresh.pending = None;
        self.cached = Some(fresh);
        revert_result
    }

    pub fn tick<P: DisplayPort>(&mut self, port: &mut P, now_ms: u64) -> FlameResult<bool> {
        if !self.coordinator.is_expired(now_ms) {
            return Ok(false);
        }
        // Re-validate against the live topology before reverting: a disconnected output is
        // cleared (and its error surfaced) by refresh, and a topology jump is reverted there.
        if let Err(refresh_error) = self.refresh(port) {
            // Only a failed initial read leaves the expired transaction live here; the other
            // refresh failures already retired it. Never leave an expired transaction pending.
            let Some(transaction) = self.pending_id() else {
                return Err(refresh_error);
            };
            return Err(self.retire_after_read_failure(port, transaction, refresh_error));
        }
        let Some(transaction) = self.pending_id() else {
            return Ok(true);
        };
        self.revert(port, transaction)?;
        Ok(true)
    }

    /// Retires an expired transaction whose topology read failed, then makes its single rollback
    /// attempt. Returns the error to surface: a refused rollback takes precedence over the read.
    pub(super) fn retire_after_read_failure<P: DisplayPort>(
        &mut self,
        port: &mut P,
        transaction: TransactionId,
        read_error: FlameError,
    ) -> FlameError {
        // The transaction was just observed pending, so retiring it cannot fail.
        let _ = self.coordinator.revert(transaction);
        if let Some(cached) = self.cached.as_mut() {
            cached.pending = None;
        }
        let rollback = port.revert_mode(transaction);
        rollback.err().unwrap_or(read_error)
    }
}

pub(super) fn require_transaction(
    coordinator: &DisplayCoordinator,
    transaction: TransactionId,
) -> FlameResult<()> {
    match coordinator.pending() {
        Some(pending) if pending.transaction == transaction => Ok(()),
        Some(_) => Err(FlameError::new(
            ErrorCode::Conflict,
            "display transaction id mismatch",
        )),
        None => Err(FlameError::new(
            ErrorCode::NotFound,
            "no pending display transaction",
        )),
    }
}

pub(super) fn to_api_pending(pending: &crate::display::PendingDisplayChange) -> PendingModeChange {
    PendingModeChange {
        transaction: pending.transaction,
        output: pending.output.clone(),
        mode: pending.mode,
        deadline_ms: pending.deadline_ms,
    }
}

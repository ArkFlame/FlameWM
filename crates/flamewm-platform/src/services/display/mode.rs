use super::*;

impl DisplayService {
    pub fn begin_mode<P: DisplayPort>(
        &mut self,
        port: &mut P,
        output: &OutputId,
        mode: ModeId,
        expected_generation: u64,
        now_ms: u64,
    ) -> FlameResult<TransactionId> {
        if self.cached.is_none() {
            self.refresh(port)?;
        }
        // The caller's generation is checked against the snapshot it observed, before the
        // superseded transaction's revert bumps the generation.
        let observed = self.cached.as_ref().ok_or_else(|| {
            FlameError::new(ErrorCode::InternalFailure, "missing display snapshot")
        })?;
        require_generation(observed, expected_generation)?;
        // Invalid requests must not cancel an otherwise valid pending transaction.
        let selected = observed
            .outputs
            .iter()
            .find(|candidate| candidate.connected && candidate.id == *output)
            .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "display output not found"))?;
        if !selected.modes.iter().any(|candidate| candidate.id == mode) {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "mode does not belong to output",
            ));
        }
        if let Some(superseded) = self.pending_id() {
            // Restore the superseded transaction's baseline first so the new baseline is never
            // the intermediate mode. A refused revert aborts before any new apply.
            self.revert(port, superseded)?;
        }
        let cached = self.cached.as_ref().ok_or_else(|| {
            FlameError::new(ErrorCode::InternalFailure, "missing display snapshot")
        })?;
        let fresh = port.display_snapshot()?;
        if self.engine_generation != 0 && fresh.generation != self.engine_generation {
            self.refresh(port)?;
            return Err(FlameError::new(
                ErrorCode::StaleRevision,
                "display topology changed before mode apply",
            ));
        }
        let selected = fresh
            .outputs
            .iter()
            .find(|candidate| candidate.connected && candidate.id == *output)
            .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "display output not found"))?;
        if !selected.modes.iter().any(|candidate| candidate.id == mode) {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "mode does not belong to output",
            ));
        }
        if selected.current_mode == mode {
            // A distinct Conflict, not InvalidArgument: the request is well-formed, the output
            // already runs this mode, and nothing was applied.
            return Err(FlameError::new(
                ErrorCode::Conflict,
                "display mode is already active",
            ));
        }

        let product_generation = cached.generation;
        let scale_by_output = scale_map(cached);

        let transaction = port.apply_mode(output, mode)?;
        self.finish_begin(
            port,
            transaction,
            output,
            mode,
            fresh.generation,
            product_generation,
            scale_by_output,
            now_ms,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_begin<P: DisplayPort>(
        &mut self,
        port: &mut P,
        transaction: TransactionId,
        output: &OutputId,
        mode: ModeId,
        topology_generation: u64,
        product_generation: u64,
        scale_by_output: BTreeMap<OutputId, u16>,
        now_ms: u64,
    ) -> FlameResult<TransactionId> {
        if let Err(error) = self.coordinator.begin(
            transaction,
            output.clone(),
            mode,
            topology_generation,
            now_ms,
        ) {
            let _ = port.revert_mode(transaction);
            return Err(error);
        }
        let mut post = match port.display_snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                // The mode may already have changed even though reading its
                // resulting snapshot failed. Do not leave a live transaction
                // that can hold the WM at an expired zero-duration deadline.
                let _ = port.revert_mode(transaction);
                let _ = self.coordinator.revert(transaction);
                if let Ok(mut snapshot) = port.display_snapshot() {
                    snapshot.pending = None;
                    self.cached = Some(snapshot);
                } else if let Some(cached) = self.cached.as_mut() {
                    cached.pending = None;
                }
                return Err(error);
            }
        };
        self.engine_generation = post.generation;
        restore_shell_scales(&mut post, &scale_by_output);
        post.generation = product_generation.saturating_add(1).max(1);
        post.pending = self.coordinator.pending().map(to_api_pending);
        self.cached = Some(post);
        Ok(transaction)
    }
}

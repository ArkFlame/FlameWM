use super::*;

impl DisplayService {
    pub fn refresh<P: DisplayPort>(&mut self, port: &mut P) -> FlameResult<&DisplaySnapshot> {
        let mut fresh = port.display_snapshot()?;
        let raw_generation = fresh.generation;
        let pending_output_missing = self.coordinator.pending_output_missing(
            fresh
                .outputs
                .iter()
                .filter(|output| output.connected)
                .map(|output| &output.id),
        );
        // Surfaced after the cache is updated so the pending transaction is never left live.
        let mut surfaced: Option<FlameError> = None;
        if pending_output_missing {
            // The pending output vanished: there is nothing to revert against. Retire the
            // transaction without a native call and report the disconnect.
            let transaction = self.pending_transaction()?;
            self.coordinator.revert(transaction)?;
            surfaced = Some(FlameError::new(
                ErrorCode::NotFound,
                "display output disconnected; pending mode change cleared",
            ));
        } else if self.coordinator.topology_is_stale(raw_generation) {
            // Retire before crossing the native boundary so a refused revert cannot retry on
            // every refresh.
            let transaction = self.pending_transaction()?;
            self.coordinator.revert(transaction)?;
            if let Err(error) = port.revert_mode(transaction) {
                surfaced = Some(error);
            }
            fresh = port.display_snapshot()?;
        }
        let raw_generation = fresh.generation;
        let next_product_generation = match self.cached.as_ref() {
            None => raw_generation.max(1),
            Some(previous) if self.engine_generation != raw_generation => {
                previous.generation.saturating_add(1).max(1)
            }
            Some(previous) => previous.generation,
        };
        if let Some(previous) = self.cached.as_ref() {
            for output in &mut fresh.outputs {
                if let Some(old) = previous
                    .outputs
                    .iter()
                    .find(|candidate| candidate.id == output.id)
                {
                    output.shell_scale_percent = old.shell_scale_percent;
                }
            }
        }
        for output in &mut fresh.outputs {
            if let Some(scale) = self.scale_memory.get(&output.id) {
                output.shell_scale_percent = *scale;
            }
        }
        fresh.generation = next_product_generation;
        fresh.pending = self.coordinator.pending().map(to_api_pending);
        self.engine_generation = raw_generation;
        self.cached = Some(fresh);
        if let Some(error) = surfaced {
            return Err(error);
        }
        self.cached.as_ref().ok_or_else(|| {
            FlameError::new(ErrorCode::InternalFailure, "display snapshot cache failed")
        })
    }

    pub(super) fn pending_transaction(&self) -> FlameResult<TransactionId> {
        self.pending_id().ok_or_else(|| {
            FlameError::new(ErrorCode::InternalFailure, "missing display transaction")
        })
    }

    pub(super) fn pending_id(&self) -> Option<TransactionId> {
        self.coordinator
            .pending()
            .map(|pending| pending.transaction)
    }

    #[must_use]
    pub fn cached(&self) -> Option<&DisplaySnapshot> {
        self.cached.as_ref()
    }
}

pub(super) fn require_generation(snapshot: &DisplaySnapshot, expected: u64) -> FlameResult<()> {
    if snapshot.generation == expected {
        Ok(())
    } else {
        Err(FlameError::new(
            ErrorCode::StaleRevision,
            "display topology generation is stale",
        ))
    }
}

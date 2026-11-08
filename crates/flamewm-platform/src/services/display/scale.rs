use super::*;

impl DisplayService {
    pub(super) fn product_generation_and_scales(&self) -> (u64, BTreeMap<OutputId, u16>) {
        let generation = self
            .cached
            .as_ref()
            .map_or(1, |snapshot| snapshot.generation.max(1));
        let scales = self.cached.as_ref().map(scale_map).unwrap_or_default();
        (generation, scales)
    }

    pub fn set_shell_scale(
        &mut self,
        output: &OutputId,
        scale_percent: u16,
        expected_generation: u64,
    ) -> FlameResult<bool> {
        if !is_supported_scale(scale_percent) {
            return Err(FlameError::new(
                ErrorCode::InvalidArgument,
                "shell scale must be 100..=200 in 5% steps",
            ));
        }
        let snapshot = self.cached.as_mut().ok_or_else(|| {
            FlameError::new(
                ErrorCode::Unavailable,
                "display snapshot is not initialized",
            )
        })?;
        require_generation(snapshot, expected_generation)?;
        let selected = snapshot
            .outputs
            .iter_mut()
            .find(|candidate| candidate.connected && candidate.id == *output)
            .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "display output not found"))?;
        self.scale_memory.insert(output.clone(), scale_percent);
        if selected.shell_scale_percent == scale_percent {
            return Ok(false);
        }
        selected.shell_scale_percent = scale_percent;
        snapshot.generation = snapshot.generation.saturating_add(1);
        Ok(true)
    }
}

pub(super) fn scale_map(snapshot: &DisplaySnapshot) -> BTreeMap<OutputId, u16> {
    snapshot
        .outputs
        .iter()
        .map(|output| (output.id.clone(), output.shell_scale_percent))
        .collect()
}

pub(super) fn restore_shell_scales(snapshot: &mut DisplaySnapshot, scales: &BTreeMap<OutputId, u16>) {
    for output in &mut snapshot.outputs {
        if let Some(scale) = scales.get(&output.id) {
            output.shell_scale_percent = *scale;
        }
    }
}

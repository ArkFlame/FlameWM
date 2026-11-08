use super::*;

impl DisplayService {
    /// Starts a primary, rotation or layout transaction with the same generation, supersede,
    /// rollback and keep/revert rules as [`Self::begin_mode`].
    pub fn begin_topology<P: DisplayPort>(
        &mut self,
        port: &mut P,
        change: &TopologyChange,
        expected_generation: u64,
        now_ms: u64,
    ) -> FlameResult<TransactionId> {
        if self.cached.is_none() {
            self.refresh(port)?;
        }
        let observed = self.cached.as_ref().ok_or_else(|| {
            FlameError::new(ErrorCode::InternalFailure, "missing display snapshot")
        })?;
        require_generation(observed, expected_generation)?;
        // Invalid requests must not cancel an otherwise valid pending transaction.
        validate_topology(observed, change)?;
        if let Some(superseded) = self.pending_id() {
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
                "display topology changed before topology apply",
            ));
        }
        validate_topology(&fresh, change)?;
        let anchor = change
            .anchor()
            .ok_or_else(|| FlameError::new(ErrorCode::InvalidArgument, "empty topology change"))?;
        let mode = fresh
            .outputs
            .iter()
            .find(|candidate| candidate.id == *anchor)
            .map_or(ModeId(0), |candidate| candidate.current_mode);
        let product_generation = cached.generation;
        let scale_by_output = scale_map(cached);
        let transaction = port.apply_topology(change)?;
        self.finish_begin(
            port,
            transaction,
            anchor,
            mode,
            fresh.generation,
            product_generation,
            scale_by_output,
            now_ms,
        )
    }
}

/// Checks a topology change against the live snapshot. Arrangement must place every connected
/// output exactly once, with non-negative origins anchored at 0,0, no overlap, and edge contact
/// between every pair that is part of the chain.
pub(super) fn validate_topology(snapshot: &DisplaySnapshot, change: &TopologyChange) -> FlameResult<()> {
    let connected = |id: &OutputId| {
        snapshot
            .outputs
            .iter()
            .find(|candidate| candidate.connected && candidate.id == *id)
            .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "display output not found"))
    };
    match change {
        TopologyChange::SetPrimary(output) => {
            if connected(output)?.primary {
                return Err(FlameError::new(
                    ErrorCode::Conflict,
                    "display output is already primary",
                ));
            }
        }
        TopologyChange::SetOrientation { output, .. } => {
            connected(output)?;
        }
        TopologyChange::SetArrangement(placements) => {
            validate_arrangement(snapshot, placements, &connected)?;
        }
    }
    Ok(())
}

pub(super) fn validate_arrangement<'a>(
    snapshot: &'a DisplaySnapshot,
    placements: &[OutputPlacement],
    connected: &impl Fn(&OutputId) -> FlameResult<&'a OutputSnapshot>,
) -> FlameResult<()> {
    let invalid = |message: &str| FlameError::new(ErrorCode::InvalidArgument, message);
    if placements.is_empty() {
        return Err(invalid("arrangement must place at least one output"));
    }
    // Resolve every placement first so an unknown output reports NotFound, not a count mismatch.
    let outputs = placements
        .iter()
        .map(|placement| connected(&placement.output))
        .collect::<FlameResult<Vec<_>>>()?;
    let connected_count = snapshot
        .outputs
        .iter()
        .filter(|output| output.connected)
        .count();
    if placements.len() != connected_count {
        return Err(invalid(
            "arrangement must place every connected output once",
        ));
    }
    let mut rects = Vec::with_capacity(placements.len());
    for (index, (placement, output)) in placements.iter().zip(outputs).enumerate() {
        if placements[..index]
            .iter()
            .any(|earlier| earlier.output == placement.output)
        {
            return Err(invalid("arrangement lists an output twice"));
        }
        if placement.x < 0 || placement.y < 0 {
            return Err(invalid("arrangement origin must be non-negative"));
        }
        if !output.geometry.is_valid() {
            return Err(invalid("display output has no active mode"));
        }
        rects.push(Rect::new(
            placement.x,
            placement.y,
            output.geometry.width,
            output.geometry.height,
        ));
    }
    if !rects.iter().any(|rect| rect.x == 0 && rect.y == 0) {
        return Err(invalid("arrangement must anchor one output at 0,0"));
    }
    for (index, rect) in rects.iter().enumerate() {
        for other in &rects[index + 1..] {
            if rect.x < other.x + other.width
                && other.x < rect.x + rect.width
                && rect.y < other.y + other.height
                && other.y < rect.y + rect.height
            {
                return Err(invalid("arrangement outputs overlap"));
            }
        }
        if rects.len() > 1
            && !rects
                .iter()
                .any(|other| other != rect && touches(rect, other))
        {
            return Err(invalid("arrangement leaves an output disconnected"));
        }
    }
    Ok(())
}

/// Two rectangles share an edge segment (not just a corner).
pub(super) fn touches(a: &Rect, b: &Rect) -> bool {
    let x_overlap = a.x < b.x + b.width && b.x < a.x + a.width;
    let y_overlap = a.y < b.y + b.height && b.y < a.y + a.height;
    (x_overlap && (a.y + a.height == b.y || b.y + b.height == a.y))
        || (y_overlap && (a.x + a.width == b.x || b.x + b.width == a.x))
}

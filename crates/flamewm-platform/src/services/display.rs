use std::collections::BTreeMap;

use flamewm_api::display::{
    DisplaySnapshot, OutputPlacement, OutputSnapshot, PendingModeChange, TopologyChange,
};
use flamewm_api::geometry::Rect;
use flamewm_api::ports::DisplayPort;
use flamewm_api::{ErrorCode, FlameError, FlameResult, ModeId, OutputId, TransactionId};

use crate::display::DisplayCoordinator;
use crate::scale::is_supported_scale;

#[derive(Debug, Default)]
pub struct DisplayService {
    coordinator: DisplayCoordinator,
    cached: Option<DisplaySnapshot>,
    engine_generation: u64,
    /// Last shell scale set per output ID, kept across unplug so a re-plugged output keeps it.
    scale_memory: BTreeMap<OutputId, u16>,
}

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

    fn pending_transaction(&self) -> FlameResult<TransactionId> {
        self.pending_id().ok_or_else(|| {
            FlameError::new(ErrorCode::InternalFailure, "missing display transaction")
        })
    }

    fn pending_id(&self) -> Option<TransactionId> {
        self.coordinator
            .pending()
            .map(|pending| pending.transaction)
    }

    #[must_use]
    pub fn cached(&self) -> Option<&DisplaySnapshot> {
        self.cached.as_ref()
    }

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
    fn finish_begin<P: DisplayPort>(
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
    fn retire_after_read_failure<P: DisplayPort>(
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

    fn product_generation_and_scales(&self) -> (u64, BTreeMap<OutputId, u16>) {
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

/// Checks a topology change against the live snapshot. Arrangement must place every connected
/// output exactly once, with non-negative origins anchored at 0,0, no overlap, and edge contact
/// between every pair that is part of the chain.
fn validate_topology(snapshot: &DisplaySnapshot, change: &TopologyChange) -> FlameResult<()> {
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

fn validate_arrangement<'a>(
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
fn touches(a: &Rect, b: &Rect) -> bool {
    let x_overlap = a.x < b.x + b.width && b.x < a.x + a.width;
    let y_overlap = a.y < b.y + b.height && b.y < a.y + a.height;
    (x_overlap && (a.y + a.height == b.y || b.y + b.height == a.y))
        || (y_overlap && (a.x + a.width == b.x || b.x + b.width == a.x))
}

fn scale_map(snapshot: &DisplaySnapshot) -> BTreeMap<OutputId, u16> {
    snapshot
        .outputs
        .iter()
        .map(|output| (output.id.clone(), output.shell_scale_percent))
        .collect()
}

fn restore_shell_scales(snapshot: &mut DisplaySnapshot, scales: &BTreeMap<OutputId, u16>) {
    for output in &mut snapshot.outputs {
        if let Some(scale) = scales.get(&output.id) {
            output.shell_scale_percent = *scale;
        }
    }
}

fn require_generation(snapshot: &DisplaySnapshot, expected: u64) -> FlameResult<()> {
    if snapshot.generation == expected {
        Ok(())
    } else {
        Err(FlameError::new(
            ErrorCode::StaleRevision,
            "display topology generation is stale",
        ))
    }
}

fn require_transaction(
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

fn to_api_pending(pending: &crate::display::PendingDisplayChange) -> PendingModeChange {
    PendingModeChange {
        transaction: pending.transaction,
        output: pending.output.clone(),
        mode: pending.mode,
        deadline_ms: pending.deadline_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::display::{DisplayMode, OutputSnapshot};
    use flamewm_api::geometry::{Rect, Size};

    fn mode(id: u64, width: i32, height: i32, preferred: bool) -> DisplayMode {
        DisplayMode {
            id: ModeId(id),
            resolution: Size { width, height },
            refresh_millihz: 60_000,
            preferred,
        }
    }

    fn unavailable(message: &str) -> FlameError {
        FlameError::new(ErrorCode::Unavailable, message)
    }

    fn pending_of(service: &DisplayService) -> PendingModeChange {
        service
            .cached()
            .and_then(|s| s.pending.clone())
            .expect("pending")
    }

    struct TwoModePort {
        mode: ModeId,
        generation: u64,
        fail_apply: bool,
        fail_revert: bool,
    }

    impl TwoModePort {
        fn new() -> Self {
            Self {
                mode: ModeId(1),
                generation: 1,
                fail_apply: false,
                fail_revert: false,
            }
        }
    }

    impl DisplayPort for TwoModePort {
        fn display_snapshot(&self) -> FlameResult<DisplaySnapshot> {
            Ok(DisplaySnapshot {
                generation: self.generation,
                outputs: vec![OutputSnapshot {
                    id: OutputId::new("fake"),
                    connector: "fake".to_owned(),
                    edid_identity: "two-mode-fake".to_owned(),
                    connected: true,
                    primary: true,
                    geometry: Rect::new(0, 0, 1920, 1080),
                    current_mode: self.mode,
                    modes: vec![mode(1, 1920, 1080, true), mode(2, 1280, 720, false)],
                    shell_scale_percent: 100,
                }],
                pending: None,
            })
        }

        fn apply_mode(&mut self, _output: &OutputId, mode: ModeId) -> FlameResult<TransactionId> {
            if self.fail_apply {
                return Err(unavailable("fake apply failure"));
            }
            self.mode = mode;
            self.generation += 1;
            Ok(TransactionId(7))
        }

        fn keep_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
            Ok(())
        }

        fn revert_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
            if self.fail_revert {
                return Err(unavailable("fake revert failure"));
            }
            self.mode = ModeId(1);
            self.generation += 1;
            Ok(())
        }
    }

    fn begin(port: &mut TwoModePort, service: &mut DisplayService) -> FlameResult<TransactionId> {
        let generation = service.refresh(port)?.generation;
        service.begin_mode(port, &OutputId::new("fake"), ModeId(2), generation, 100)
    }

    #[test]
    fn fake_two_mode_keep_clears_pending_and_refreshes_snapshot() {
        let mut port = TwoModePort::new();
        let mut service = DisplayService::default();
        let transaction = begin(&mut port, &mut service).expect("apply mode");
        assert_eq!(
            service
                .cached()
                .and_then(|s| s.pending.as_ref())
                .map(|p| p.mode),
            Some(ModeId(2))
        );
        service
            .keep(&mut port, transaction, 100)
            .expect("keep mode");
        assert_eq!(service.cached().and_then(|s| s.pending.as_ref()), None);
        assert_eq!(
            service.cached().map(|s| s.outputs[0].current_mode),
            Some(ModeId(2))
        );
    }

    #[test]
    fn fake_two_mode_same_mode_is_a_clear_conflict_and_applies_nothing() {
        let mut port = TwoModePort::new();
        let mut service = DisplayService::default();
        let generation = service.refresh(&mut port).expect("snapshot").generation;
        let error = service
            .begin_mode(
                &mut port,
                &OutputId::new("fake"),
                ModeId(1),
                generation,
                100,
            )
            .expect_err("current mode is a no-op");
        assert_eq!(error.code, ErrorCode::Conflict);
        assert_eq!(error.message, "display mode is already active");
        assert_eq!(port.mode, ModeId(1), "nothing applied");
        assert_eq!(port.generation, generation, "no mode change happened");
        assert!(service.cached().and_then(|s| s.pending.as_ref()).is_none());
    }

    #[test]
    fn fake_two_mode_deadline_reverts_once_and_clears_pending() {
        let mut port = TwoModePort::new();
        let mut service = DisplayService::default();
        begin(&mut port, &mut service).expect("apply mode");
        assert!(!service.tick(&mut port, 15_099).expect("early tick"));
        assert!(service.tick(&mut port, 15_100).expect("deadline tick"));
        assert!(!service.tick(&mut port, 15_101).expect("subsequent tick"));
        assert_eq!(service.cached().and_then(|s| s.pending.as_ref()), None);
        assert_eq!(
            service.cached().map(|s| s.outputs[0].current_mode),
            Some(ModeId(1))
        );
    }

    #[test]
    fn fake_apply_failure_leaves_no_transaction_or_expired_deadline() {
        let mut port = TwoModePort::new();
        port.fail_apply = true;
        let mut service = DisplayService::default();
        let generation = service
            .refresh(&mut port)
            .expect("initial snapshot")
            .generation;
        assert!(service
            .begin_mode(
                &mut port,
                &OutputId::new("fake"),
                ModeId(2),
                generation,
                100
            )
            .is_err());
        assert_eq!(service.cached().and_then(|s| s.pending.as_ref()), None);
    }

    #[test]
    fn fake_revert_failure_retires_expired_deadline_without_busy_spin() {
        let mut port = TwoModePort::new();
        let mut service = DisplayService::default();
        begin(&mut port, &mut service).expect("apply mode");
        port.fail_revert = true;
        assert!(service.tick(&mut port, 15_100).is_err());
        assert_eq!(service.cached().and_then(|s| s.pending.as_ref()), None);
        assert!(!service.tick(&mut port, 15_101).expect("no retry spin"));
    }

    #[derive(Clone)]
    struct FakeOutput {
        id: OutputId,
        connected: bool,
        current: ModeId,
        modes: Vec<ModeId>,
    }

    /// Fake two-output topology with native-like revert semantics: a revert restores the previous
    /// `ModeId` on the output that the transaction changed, and only that output.
    struct TwoOutputPort {
        outputs: Vec<FakeOutput>,
        generation: u64,
        next_transaction: u64,
        pending: Option<(TransactionId, OutputId, ModeId)>,
        applied: Vec<(OutputId, ModeId)>,
        reverted: Vec<TransactionId>,
        fail_revert: bool,
        fail_snapshot: bool,
        fail_apply: bool,
    }

    impl TwoOutputPort {
        /// DP-A: 2 modes (current ModeId(1)). DP-B: 12 modes ModeId(101..=112), current 101.
        fn new() -> Self {
            Self {
                outputs: vec![
                    FakeOutput {
                        id: OutputId::new("DP-A"),
                        connected: true,
                        current: ModeId(1),
                        modes: vec![ModeId(1), ModeId(2)],
                    },
                    FakeOutput {
                        id: OutputId::new("DP-B"),
                        connected: true,
                        current: ModeId(101),
                        modes: (101..=112).map(ModeId).collect(),
                    },
                ],
                generation: 1,
                next_transaction: 0,
                pending: None,
                applied: Vec::new(),
                reverted: Vec::new(),
                fail_revert: false,
                fail_snapshot: false,
                fail_apply: false,
            }
        }

        fn output(&self, id: &str) -> &FakeOutput {
            self.outputs
                .iter()
                .find(|o| o.id.as_str() == id)
                .expect("fixture output")
        }

        fn connected_index(&self, output: &OutputId) -> FlameResult<usize> {
            self.outputs
                .iter()
                .position(|o| o.connected && &o.id == output)
                .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "fake output missing"))
        }

        fn disconnect(&mut self, id: &str) {
            if let Some(output) = self.outputs.iter_mut().find(|o| o.id.as_str() == id) {
                output.connected = false;
            }
            self.generation += 1;
        }

        fn bump_generation(&mut self, by: u64) {
            self.generation += by;
        }
    }

    impl DisplayPort for TwoOutputPort {
        fn display_snapshot(&self) -> FlameResult<DisplaySnapshot> {
            if self.fail_snapshot {
                return Err(unavailable("fake snapshot read failed"));
            }
            Ok(DisplaySnapshot {
                generation: self.generation,
                outputs: self
                    .outputs
                    .iter()
                    .filter(|output| output.connected)
                    .enumerate()
                    .map(|(index, output)| OutputSnapshot {
                        id: output.id.clone(),
                        connector: output.id.as_str().to_owned(),
                        edid_identity: format!("edid-{}", output.id.as_str()),
                        connected: true,
                        primary: index == 0,
                        geometry: Rect::new(0, 0, 1920, 1080),
                        current_mode: output.current,
                        modes: output
                            .modes
                            .iter()
                            .map(|id| mode(id.0, 1920, 1080, false))
                            .collect(),
                        shell_scale_percent: 100,
                    })
                    .collect(),
                pending: None,
            })
        }

        fn apply_mode(&mut self, output: &OutputId, mode: ModeId) -> FlameResult<TransactionId> {
            if self.fail_apply {
                return Err(FlameError::new(ErrorCode::Unavailable, "fake apply failed"));
            }
            let index = self.connected_index(output)?;
            if !self.outputs[index].modes.contains(&mode) {
                return Err(FlameError::new(
                    ErrorCode::InvalidArgument,
                    "fake mode missing",
                ));
            }
            let previous = self.outputs[index].current;
            self.outputs[index].current = mode;
            self.generation += 1;
            self.next_transaction += 1;
            let transaction = TransactionId(self.next_transaction);
            self.pending = Some((transaction, output.clone(), previous));
            self.applied.push((output.clone(), mode));
            Ok(transaction)
        }

        fn keep_mode(&mut self, transaction: TransactionId) -> FlameResult<()> {
            match self.pending.take() {
                Some((pending, _, _)) if pending == transaction => Ok(()),
                other => {
                    self.pending = other;
                    Err(FlameError::new(
                        ErrorCode::NotFound,
                        "fake transaction missing",
                    ))
                }
            }
        }

        fn revert_mode(&mut self, transaction: TransactionId) -> FlameResult<()> {
            self.reverted.push(transaction);
            if self.fail_revert {
                return Err(unavailable("fake revert refused"));
            }
            let (pending, output, previous) = self
                .pending
                .take()
                .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "fake transaction missing"))?;
            if pending != transaction {
                self.pending = Some((pending, output, previous));
                return Err(FlameError::new(
                    ErrorCode::NotFound,
                    "fake transaction missing",
                ));
            }
            let index = self.connected_index(&output)?;
            self.outputs[index].current = previous;
            self.generation += 1;
            Ok(())
        }
    }

    fn begin_on(
        port: &mut TwoOutputPort,
        service: &mut DisplayService,
        output: &str,
        mode: u64,
        now_ms: u64,
    ) -> FlameResult<TransactionId> {
        let generation = service.refresh(port)?.generation;
        service.begin_mode(
            port,
            &OutputId::new(output),
            ModeId(mode),
            generation,
            now_ms,
        )
    }

    #[test]
    fn two_output_fixture_begin_targets_exact_output_and_mode_for_every_mode() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        let generation = service.refresh(&mut port).expect("snapshot").generation;
        assert!(
            service
                .begin_mode(
                    &mut port,
                    &OutputId::new("DP-B"),
                    ModeId(101),
                    generation,
                    0
                )
                .is_err(),
            "current mode is not a change"
        );

        for mode in 102..=112u64 {
            let transaction = begin_on(&mut port, &mut service, "DP-B", mode, 0).expect("begin");
            assert_eq!(
                port.applied.last(),
                Some(&(OutputId::new("DP-B"), ModeId(mode)))
            );
            let pending = pending_of(&service);
            assert_eq!(pending.transaction, transaction);
            assert_eq!(pending.output, OutputId::new("DP-B"));
            assert_eq!(pending.mode, ModeId(mode));
            service.keep(&mut port, transaction, 0).expect("keep");
        }
        assert_eq!(port.applied.len(), 11);
        assert_eq!(port.output("DP-B").current, ModeId(112));
        assert_eq!(port.output("DP-A").current, ModeId(1));

        let transaction = begin_on(&mut port, &mut service, "DP-A", 2, 0).expect("begin on DP-A");
        assert_eq!(
            port.applied.last(),
            Some(&(OutputId::new("DP-A"), ModeId(2)))
        );
        service.keep(&mut port, transaction, 0).expect("keep DP-A");
        assert_eq!(port.output("DP-A").current, ModeId(2));
        assert_eq!(port.output("DP-B").current, ModeId(112));
    }

    #[test]
    fn fake_clock_15000ms_auto_revert_reverts_once_on_pending_output_and_changes_snapshot() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        begin_on(&mut port, &mut service, "DP-B", 112, 0).expect("begin");
        let before = service.cached().cloned().expect("cached before");

        // No UI events: only the fake clock advances.
        for now_ms in [0, 1, 5_000, 14_999] {
            assert!(!service.tick(&mut port, now_ms).expect("pre-deadline tick"));
        }
        assert!(port.reverted.is_empty());

        assert!(service.tick(&mut port, 15_000).expect("deadline tick"));
        assert_eq!(port.reverted.len(), 1, "reverted exactly once");
        assert!(!service.tick(&mut port, 15_001).expect("post-revert tick"));
        assert_eq!(port.reverted.len(), 1, "no second revert");

        let after = service.cached().expect("cached after");
        assert_eq!(after.pending, None, "pending transaction cleared");
        assert_ne!(after, &before, "snapshot change emitted");
        assert_eq!(after.generation, before.generation + 1);
        assert_eq!(
            port.output("DP-B").current,
            ModeId(101),
            "previous mode restored on DP-B"
        );
        assert_eq!(
            port.output("DP-A").current,
            ModeId(1),
            "other output untouched"
        );
    }

    #[test]
    fn late_keep_without_intervening_tick_reverts_and_rejects_confirmation() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        let transaction = begin_on(&mut port, &mut service, "DP-B", 112, 0).expect("begin");

        let error = service
            .keep(&mut port, transaction, 15_001)
            .expect_err("late confirmation must not keep the unconfirmed mode");

        assert_eq!(error.code, ErrorCode::Timeout);
        assert_eq!(
            port.reverted,
            vec![transaction],
            "one rollback at late Keep"
        );
        assert_eq!(port.output("DP-B").current, ModeId(101));
        assert_eq!(
            service
                .cached()
                .and_then(|snapshot| snapshot.pending.as_ref()),
            None
        );
        assert!(!service
            .tick(&mut port, 15_002)
            .expect("no retry after late Keep"));
        assert_eq!(port.reverted.len(), 1, "rollback is attempted exactly once");
    }

    #[test]
    fn failed_auto_revert_retires_transaction_with_bounded_attempts() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        begin_on(&mut port, &mut service, "DP-B", 112, 0).expect("begin");
        port.fail_revert = true;

        assert!(service.tick(&mut port, 15_000).is_err());
        assert_eq!(service.cached().and_then(|s| s.pending.as_ref()), None);
        for now_ms in 15_001..=15_050 {
            assert!(!service.tick(&mut port, now_ms).expect("retired, no retry"));
        }
        assert_eq!(port.reverted.len(), 1, "exactly one native revert attempt");
        assert!(service.refresh(&mut port).is_ok());
        assert_eq!(port.reverted.len(), 1);
    }

    #[test]
    fn snapshot_read_failure_at_deadline_retires_transaction_and_attempts_one_rollback() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        begin_on(&mut port, &mut service, "DP-B", 112, 0).expect("begin");
        port.fail_snapshot = true;

        assert!(
            service.tick(&mut port, 15_000).is_err(),
            "snapshot failure surfaced"
        );
        assert_eq!(
            service.cached().and_then(|s| s.pending.as_ref()),
            None,
            "expired transaction retired"
        );
        assert_eq!(
            port.reverted.len(),
            1,
            "one rollback attempt for the applied mode"
        );

        port.fail_snapshot = false;
        assert!(!service
            .tick(&mut port, 15_001)
            .expect("no retry after retirement"));
        assert_eq!(port.reverted.len(), 1, "no busy retry");
    }

    #[test]
    fn keep_clears_pending_and_never_auto_reverts() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        let transaction = begin_on(&mut port, &mut service, "DP-B", 112, 0).expect("begin");
        service.keep(&mut port, transaction, 0).expect("keep");
        assert_eq!(service.cached().and_then(|s| s.pending.as_ref()), None);
        assert!(!service.tick(&mut port, 15_000).expect("tick after keep"));
        assert!(port.reverted.is_empty());
        assert_eq!(port.output("DP-B").current, ModeId(112));
    }

    #[test]
    fn revert_before_deadline_restores_previous_mode_on_the_right_output() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        let transaction = begin_on(&mut port, &mut service, "DP-B", 112, 0).expect("begin");
        service.revert(&mut port, transaction).expect("revert");
        assert_eq!(port.reverted, vec![transaction]);
        assert_eq!(port.output("DP-B").current, ModeId(101));
        assert_eq!(port.output("DP-A").current, ModeId(1));
        assert_eq!(service.cached().and_then(|s| s.pending.as_ref()), None);
        assert!(!service.tick(&mut port, 15_000).expect("no deadline left"));
        assert_eq!(port.reverted.len(), 1);
    }

    #[test]
    fn output_disconnect_refresh_clears_pending_without_native_revert() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        begin_on(&mut port, &mut service, "DP-B", 112, 0).expect("begin");
        port.disconnect("DP-B");

        let error = service.refresh(&mut port).expect_err("disconnect surfaced");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(
            port.reverted.is_empty(),
            "no revert against the missing output"
        );
        assert_eq!(service.cached().and_then(|s| s.pending.as_ref()), None);
        assert!(!service.tick(&mut port, 15_000).expect("nothing pending"));
        assert!(port.reverted.is_empty());
    }

    #[test]
    fn output_disconnect_at_deadline_surfaces_error_without_native_revert() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        begin_on(&mut port, &mut service, "DP-B", 112, 0).expect("begin");
        port.disconnect("DP-B");

        let error = service
            .tick(&mut port, 15_000)
            .expect_err("disconnect surfaced at deadline");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(
            port.reverted.is_empty(),
            "no revert against the missing output"
        );
        assert_eq!(service.cached().and_then(|s| s.pending.as_ref()), None);
        assert!(!service
            .tick(&mut port, 15_001)
            .expect("transaction already cleared"));
    }

    #[test]
    fn topology_jump_with_refused_revert_is_retired_once() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        begin_on(&mut port, &mut service, "DP-B", 112, 0).expect("begin");
        port.bump_generation(10);
        port.fail_revert = true;

        assert!(service.refresh(&mut port).is_err());
        assert_eq!(port.reverted.len(), 1);
        assert_eq!(service.cached().and_then(|s| s.pending.as_ref()), None);
        assert!(service.refresh(&mut port).is_ok());
        assert_eq!(port.reverted.len(), 1, "no retry on the next refresh");
    }

    /// Five outputs O0..O4; each has 12 modes (index k on output i is ModeId(i*100 + 1 + k)).
    fn five_output_port() -> TwoOutputPort {
        let mut port = TwoOutputPort::new();
        port.outputs = (0..5u64)
            .map(|index| FakeOutput {
                id: OutputId::new(format!("O{index}")),
                connected: true,
                current: ModeId(index * 100 + 1),
                modes: (0..12u64).map(|k| ModeId(index * 100 + 1 + k)).collect(),
            })
            .collect();
        port
    }

    #[test]
    fn five_output_mode_index_11_on_output_index_4_applies_that_exact_mode() {
        let mut port = five_output_port();
        let mut service = DisplayService::default();
        let snapshot = service.refresh(&mut port).expect("snapshot").clone();
        assert_eq!(snapshot.outputs.len(), 5);
        let output = &snapshot.outputs[4];
        assert_eq!(output.id, OutputId::new("O4"));
        assert_eq!(output.modes.len(), 12);
        let mode = output.modes[11].id;
        assert_eq!(mode, ModeId(412));

        let transaction = service
            .begin_mode(&mut port, &output.id, mode, snapshot.generation, 0)
            .expect("begin on output index 4");
        assert_eq!(port.applied, vec![(OutputId::new("O4"), ModeId(412))]);
        let pending = pending_of(&service);
        assert_eq!(
            (pending.transaction, pending.output, pending.mode),
            (transaction, OutputId::new("O4"), ModeId(412))
        );
        service.keep(&mut port, transaction, 0).expect("keep");
        assert_eq!(port.output("O4").current, ModeId(412));
        assert_eq!(
            port.output("O0").current,
            ModeId(1),
            "other outputs untouched"
        );
    }

    #[test]
    fn five_output_stale_generation_is_rejected_before_any_native_apply() {
        let mut port = five_output_port();
        let mut service = DisplayService::default();
        let stale = service.refresh(&mut port).expect("snapshot").generation;

        // Topology moves after the caller captured its generation.
        port.bump_generation(1);
        let error = service
            .begin_mode(&mut port, &OutputId::new("O4"), ModeId(412), stale, 0)
            .expect_err("stale generation rejected");
        assert_eq!(error.code, ErrorCode::StaleRevision);
        assert!(
            port.applied.is_empty(),
            "no native apply for a stale request"
        );

        // The refreshed generation is accepted for the same request.
        let fresh = service
            .refresh(&mut port)
            .expect("fresh snapshot")
            .generation;
        assert_ne!(fresh, stale);
        assert!(service
            .begin_mode(&mut port, &OutputId::new("O4"), ModeId(412), stale, 0)
            .is_err());
        assert!(port.applied.is_empty());
        service
            .begin_mode(&mut port, &OutputId::new("O4"), ModeId(412), fresh, 0)
            .expect("fresh generation accepted");
        assert_eq!(port.applied, vec![(OutputId::new("O4"), ModeId(412))]);
    }

    fn scale_of(snapshot: &DisplaySnapshot, output: &str) -> u16 {
        snapshot
            .outputs
            .iter()
            .find(|candidate| candidate.id.as_str() == output)
            .map(|candidate| candidate.shell_scale_percent)
            .expect("output present in snapshot")
    }

    #[test]
    fn five_percent_scale_does_not_round_and_rejects_stale_generation() {
        let mut port = five_output_port();
        let mut service = DisplayService::default();
        let generation = service.refresh(&mut port).expect("initial").generation;
        let id = OutputId::new("O3");
        assert!(service.set_shell_scale(&id, 135, generation).expect("135%"));
        let next = service.cached().expect("cached").generation;
        assert_eq!(scale_of(service.cached().unwrap(), "O3"), 135);
        assert_eq!(
            service
                .set_shell_scale(&id, 195, generation)
                .expect_err("stale")
                .code,
            ErrorCode::StaleRevision
        );
        assert!(service.set_shell_scale(&id, 195, next).expect("195%"));
        let current = service.cached().expect("cached").generation;
        for invalid in [99, 101, 106, 133, 201] {
            assert_eq!(
                service
                    .set_shell_scale(&id, invalid, current)
                    .expect_err("invalid")
                    .code,
                ErrorCode::InvalidArgument
            );
        }
        assert_eq!(
            scale_of(service.refresh(&mut port).expect("rescan"), "O3"),
            195
        );
    }

    #[test]
    fn per_output_scales_survive_remove_readd_and_primary_change_independently() {
        let mut port = five_output_port();
        let mut service = DisplayService::default();
        let scales = [
            ("O0", 100),
            ("O1", 125),
            ("O2", 150),
            ("O3", 175),
            ("O4", 200),
        ];
        for (output, scale) in scales {
            let generation = service.refresh(&mut port).expect("snapshot").generation;
            service
                .set_shell_scale(&OutputId::new(output), scale, generation)
                .expect("supported scale applies");
        }

        // Unplug O1, then re-plug it: its scale must come back unchanged and the others stay put.
        port.disconnect("O1");
        service.refresh(&mut port).expect("snapshot after unplug");
        port.outputs[1].connected = true;
        port.bump_generation(1);
        let after_readd = service.refresh(&mut port).expect("snapshot after re-add");
        assert_eq!(scale_of(after_readd, "O1"), 125);
        for (output, scale) in scales {
            assert_eq!(
                scale_of(after_readd, output),
                scale,
                "{output} after re-add"
            );
        }

        // Unplug the primary O0: O1 becomes primary but keeps its own scale; O0 keeps its scale on re-add.
        port.disconnect("O0");
        let after_primary_loss = service
            .refresh(&mut port)
            .expect("snapshot after primary loss");
        assert_eq!(
            after_primary_loss
                .outputs
                .iter()
                .find(|o| o.primary)
                .map(|o| o.id.as_str()),
            Some("O1")
        );
        assert_eq!(scale_of(after_primary_loss, "O1"), 125);
        port.outputs[0].connected = true;
        port.bump_generation(1);
        let restored = service
            .refresh(&mut port)
            .expect("snapshot after primary re-add");
        assert_eq!(scale_of(restored, "O0"), 100);
        assert_eq!(scale_of(restored, "O1"), 125);
    }

    #[test]
    fn supersede_keeps_only_latest_transaction_and_rejects_old_keep() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        let first = begin_on(&mut port, &mut service, "DP-B", 102, 0).expect("A to B");
        let second = begin_on(&mut port, &mut service, "DP-B", 103, 100).expect("B to C");
        assert_ne!(first, second);
        let pending = pending_of(&service);
        assert_eq!(pending.transaction, second);
        assert_eq!(pending.mode, ModeId(103));
        assert_eq!(
            service
                .keep(&mut port, first, 100)
                .expect_err("old keep")
                .code,
            ErrorCode::Conflict
        );
        service.keep(&mut port, second, 100).expect("latest keep");
        assert_eq!(port.output("DP-B").current, ModeId(103));
    }

    #[test]
    fn supersede_cancel_rolls_back_to_original_baseline_not_intermediate() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        begin_on(&mut port, &mut service, "DP-B", 102, 0).expect("A to B");
        let latest = begin_on(&mut port, &mut service, "DP-B", 103, 0).expect("B to C");
        service.revert(&mut port, latest).expect("cancel");
        assert_eq!(port.output("DP-B").current, ModeId(101));
        assert_eq!(service.cached().and_then(|s| s.pending.as_ref()), None);
    }

    #[test]
    fn supersede_sets_fresh_15000ms_deadline() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        begin_on(&mut port, &mut service, "DP-B", 102, 0).expect("A to B");
        begin_on(&mut port, &mut service, "DP-B", 103, 10_000).expect("B to C");
        let pending = pending_of(&service);
        assert_eq!(pending.deadline_ms, 25_000);
        assert!(!service
            .tick(&mut port, 24_999)
            .expect("before fresh deadline"));
        assert!(service.tick(&mut port, 25_000).expect("fresh deadline"));
        assert_eq!(port.output("DP-B").current, ModeId(101));
    }

    #[test]
    fn supersede_failed_apply_leaves_no_orphan_pending_and_original_baseline() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        begin_on(&mut port, &mut service, "DP-B", 102, 0).expect("A to B");
        port.fail_apply = true;
        assert!(begin_on(&mut port, &mut service, "DP-B", 103, 0).is_err());
        assert_eq!(service.cached().and_then(|s| s.pending.as_ref()), None);
        assert_eq!(port.output("DP-B").current, ModeId(101));
        assert!(!service.tick(&mut port, 15_000).expect("no orphan deadline"));
    }

    #[test]
    fn supersede_refused_revert_blocks_apply() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        let first = begin_on(&mut port, &mut service, "DP-B", 102, 0).expect("A to B");
        port.fail_revert = true;
        assert!(begin_on(&mut port, &mut service, "DP-B", 103, 0).is_err());
        assert_eq!(port.applied.len(), 1);
        assert_eq!(service.cached().and_then(|s| s.pending.as_ref()), None);
        assert_eq!(
            service.keep(&mut port, first, 0).expect_err("retired").code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn invalid_superseding_request_preserves_pending_transaction() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        let transaction = begin_on(&mut port, &mut service, "DP-B", 102, 0).unwrap();
        let generation = service.cached().unwrap().generation;
        for (output, mode, code) in [
            ("missing", 2, ErrorCode::NotFound),
            ("DP-A", 102, ErrorCode::InvalidArgument),
        ] {
            assert_eq!(
                service
                    .begin_mode(
                        &mut port,
                        &OutputId::new(output),
                        ModeId(mode),
                        generation,
                        1
                    )
                    .unwrap_err()
                    .code,
                code
            );
            assert_eq!(pending_of(&service).transaction, transaction);
            assert!(port.reverted.is_empty());
            assert_eq!(port.output("DP-B").current, ModeId(102));
        }
        service.revert(&mut port, transaction).unwrap();
        assert_eq!(port.output("DP-B").current, ModeId(101));
    }

    #[test]
    fn supersede_other_output_reverts_pending_output_before_apply() {
        let mut port = TwoOutputPort::new();
        let mut service = DisplayService::default();
        begin_on(&mut port, &mut service, "DP-B", 102, 0).expect("DP-B");
        let latest = begin_on(&mut port, &mut service, "DP-A", 2, 0).expect("DP-A");
        assert_eq!(port.output("DP-B").current, ModeId(101));
        assert_eq!(port.output("DP-A").current, ModeId(2));
        let pending = pending_of(&service);
        assert_eq!(pending.output, OutputId::new("DP-A"));
        service.revert(&mut port, latest).expect("cancel");
        assert_eq!(port.output("DP-A").current, ModeId(1));
    }
}

#[cfg(test)]
mod topology_tests {
    use super::*;
    use flamewm_api::display::{DisplayMode, OutputSnapshot, Rotation};
    use flamewm_api::geometry::{Rect, Size};

    fn output(id: &str, x: i32, primary: bool) -> OutputSnapshot {
        OutputSnapshot {
            id: OutputId::new(id),
            connector: id.to_owned(),
            edid_identity: id.to_owned(),
            connected: true,
            primary,
            geometry: Rect::new(x, 0, 1920, 1080),
            current_mode: ModeId(101),
            modes: vec![DisplayMode {
                id: ModeId(101),
                resolution: Size {
                    width: 1920,
                    height: 1080,
                },
                refresh_millihz: 60_000,
                preferred: true,
            }],
            shell_scale_percent: 100,
        }
    }

    /// Two side-by-side 1920x1080 outputs. `fail_post_snapshot` makes the snapshot read that
    /// follows an apply fail, which must roll the applied topology back.
    struct TopologyPort {
        outputs: Vec<OutputSnapshot>,
        generation: u64,
        baseline: Option<Vec<OutputSnapshot>>,
        applied: Vec<TopologyChange>,
        reverted: Vec<TransactionId>,
        fail_apply: bool,
        fail_post_snapshot: bool,
        fail_revert: bool,
    }

    impl TopologyPort {
        fn new() -> Self {
            Self {
                outputs: vec![output("DP-A", 0, true), output("DP-B", 1920, false)],
                generation: 1,
                baseline: None,
                applied: Vec::new(),
                reverted: Vec::new(),
                fail_apply: false,
                fail_post_snapshot: false,
                fail_revert: false,
            }
        }
    }

    impl DisplayPort for TopologyPort {
        fn display_snapshot(&self) -> FlameResult<DisplaySnapshot> {
            if self.fail_post_snapshot && !self.applied.is_empty() && self.baseline.is_some() {
                return Err(FlameError::new(
                    ErrorCode::Unavailable,
                    "fake snapshot failure",
                ));
            }
            Ok(DisplaySnapshot {
                generation: self.generation,
                outputs: self.outputs.clone(),
                pending: None,
            })
        }

        fn apply_mode(&mut self, _output: &OutputId, _mode: ModeId) -> FlameResult<TransactionId> {
            Err(FlameError::new(
                ErrorCode::Unsupported,
                "mode not used here",
            ))
        }

        fn apply_topology(&mut self, change: &TopologyChange) -> FlameResult<TransactionId> {
            if self.fail_apply {
                return Err(FlameError::new(
                    ErrorCode::Unavailable,
                    "fake apply failure",
                ));
            }
            self.baseline = Some(self.outputs.clone());
            match change {
                TopologyChange::SetPrimary(target) => {
                    for candidate in &mut self.outputs {
                        candidate.primary = candidate.id == *target;
                    }
                }
                TopologyChange::SetOrientation { .. } => {}
                TopologyChange::SetArrangement(placements) => {
                    for placement in placements {
                        if let Some(candidate) = self
                            .outputs
                            .iter_mut()
                            .find(|candidate| candidate.id == placement.output)
                        {
                            candidate.geometry.x = placement.x;
                            candidate.geometry.y = placement.y;
                        }
                    }
                }
            }
            self.applied.push(change.clone());
            self.generation += 1;
            Ok(TransactionId(9))
        }

        fn keep_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
            self.baseline = None;
            Ok(())
        }

        fn revert_mode(&mut self, transaction: TransactionId) -> FlameResult<()> {
            self.reverted.push(transaction);
            if self.fail_revert {
                return Err(FlameError::new(
                    ErrorCode::EngineRejected,
                    "fake revert failure",
                ));
            }
            if let Some(baseline) = self.baseline.take() {
                self.outputs = baseline;
            }
            self.generation += 1;
            Ok(())
        }
    }

    fn primary_b() -> TopologyChange {
        TopologyChange::SetPrimary(OutputId::new("DP-B"))
    }

    fn place(id: &str, x: i32, y: i32) -> OutputPlacement {
        OutputPlacement {
            output: OutputId::new(id),
            x,
            y,
        }
    }

    fn begin(
        port: &mut TopologyPort,
        service: &mut DisplayService,
        change: &TopologyChange,
    ) -> FlameResult<TransactionId> {
        let generation = service.refresh(port)?.generation;
        service.begin_topology(port, change, generation, 100)
    }

    #[test]
    fn set_primary_applies_and_keep_clears_pending() {
        let mut port = TopologyPort::new();
        let mut service = DisplayService::default();
        let transaction = begin(&mut port, &mut service, &primary_b()).expect("begin primary");
        assert_eq!(port.applied, vec![primary_b()]);
        let pending = service
            .cached()
            .and_then(|s| s.pending.clone())
            .expect("pending");
        assert_eq!(pending.output, OutputId::new("DP-B"));
        service.keep(&mut port, transaction, 100).expect("keep");
        assert!(service.cached().and_then(|s| s.pending.as_ref()).is_none());
        assert!(service.cached().is_some_and(|s| s.outputs[1].primary));
    }

    #[test]
    fn set_primary_on_current_primary_is_conflict_and_applies_nothing() {
        let mut port = TopologyPort::new();
        let mut service = DisplayService::default();
        let error = begin(
            &mut port,
            &mut service,
            &TopologyChange::SetPrimary(OutputId::new("DP-A")),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Conflict);
        assert!(port.applied.is_empty());
    }

    #[test]
    fn unknown_output_is_not_found_for_every_request_kind() {
        let mut port = TopologyPort::new();
        let mut service = DisplayService::default();
        for change in [
            TopologyChange::SetPrimary(OutputId::new("HDMI-9")),
            TopologyChange::SetOrientation {
                output: OutputId::new("HDMI-9"),
                rotation: Rotation::Left,
            },
            TopologyChange::SetArrangement(vec![place("HDMI-9", 0, 0)]),
        ] {
            assert_eq!(
                begin(&mut port, &mut service, &change).unwrap_err().code,
                ErrorCode::NotFound
            );
        }
        assert!(port.applied.is_empty());
    }

    #[test]
    fn orientation_applies_through_transaction_and_reverts() {
        let mut port = TopologyPort::new();
        let mut service = DisplayService::default();
        let change = TopologyChange::SetOrientation {
            output: OutputId::new("DP-B"),
            rotation: Rotation::Inverted,
        };
        let transaction = begin(&mut port, &mut service, &change).expect("orientation");
        assert_eq!(port.applied, vec![change]);
        service.revert(&mut port, transaction).expect("revert");
        assert_eq!(port.reverted, vec![transaction]);
    }

    #[test]
    fn arrangement_rejects_bad_layouts_without_applying() {
        let cases = [
            (vec![place("DP-A", 0, 0)], "missing output"),
            (
                vec![place("DP-A", 0, 0), place("DP-A", 1920, 0)],
                "duplicate output",
            ),
            (
                vec![place("DP-A", -10, 0), place("DP-B", 1910, 0)],
                "negative origin",
            ),
            (
                vec![place("DP-A", 100, 0), place("DP-B", 1920, 0)],
                "no 0,0 anchor",
            ),
            (vec![place("DP-A", 0, 0), place("DP-B", 1000, 0)], "overlap"),
            (
                vec![place("DP-A", 0, 0), place("DP-B", 1920, 2000)],
                "no edge contact",
            ),
        ];
        for (placements, label) in cases {
            let mut port = TopologyPort::new();
            let mut service = DisplayService::default();
            let error = begin(
                &mut port,
                &mut service,
                &TopologyChange::SetArrangement(placements),
            )
            .unwrap_err();
            assert!(
                matches!(error.code, ErrorCode::InvalidArgument | ErrorCode::NotFound),
                "{label}: {:?}",
                error.code
            );
            assert!(port.applied.is_empty(), "{label} applied");
        }
    }

    #[test]
    fn valid_arrangement_moves_outputs_and_reverts_on_rollback() {
        let mut port = TopologyPort::new();
        let mut service = DisplayService::default();
        let change =
            TopologyChange::SetArrangement(vec![place("DP-A", 0, 0), place("DP-B", 0, 1080)]);
        let transaction = begin(&mut port, &mut service, &change).expect("arrangement");
        assert_eq!(port.outputs[1].geometry.y, 1080);
        service.revert(&mut port, transaction).expect("revert");
        assert_eq!(port.outputs[1].geometry.x, 1920);
        assert_eq!(port.outputs[1].geometry.y, 0);
    }

    #[test]
    fn failed_apply_leaves_no_pending_transaction() {
        let mut port = TopologyPort::new();
        port.fail_apply = true;
        let mut service = DisplayService::default();
        assert!(begin(&mut port, &mut service, &primary_b()).is_err());
        assert!(service.cached().and_then(|s| s.pending.as_ref()).is_none());
    }

    #[test]
    fn failed_post_apply_snapshot_rolls_topology_back() {
        let mut port = TopologyPort::new();
        port.fail_post_snapshot = true;
        let mut service = DisplayService::default();
        assert!(begin(&mut port, &mut service, &primary_b()).is_err());
        assert_eq!(port.reverted.len(), 1, "applied topology must be reverted");
        assert!(port.outputs[0].primary, "baseline primary restored");
        assert!(service.cached().and_then(|s| s.pending.as_ref()).is_none());
    }

    #[test]
    fn stale_generation_is_rejected_before_apply() {
        let mut port = TopologyPort::new();
        let mut service = DisplayService::default();
        let generation = service.refresh(&mut port).expect("snapshot").generation;
        port.generation += 5;
        let error = service
            .begin_topology(&mut port, &primary_b(), generation, 100)
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::StaleRevision);
        assert!(port.applied.is_empty());
    }
}

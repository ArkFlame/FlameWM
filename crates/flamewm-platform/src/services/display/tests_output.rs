use super::tests_mode::{mode, pending_of, unavailable};
use super::*;

#[derive(Clone)]
pub(super) struct FakeOutput {
    pub(super) id: OutputId,
    pub(super) connected: bool,
    pub(super) current: ModeId,
    pub(super) modes: Vec<ModeId>,
}

/// Fake two-output topology with native-like revert semantics: a revert restores the previous
/// `ModeId` on the output that the transaction changed, and only that output.
pub(super) struct TwoOutputPort {
    pub(super) outputs: Vec<FakeOutput>,
    pub(super) generation: u64,
    pub(super) next_transaction: u64,
    pub(super) pending: Option<(TransactionId, OutputId, ModeId)>,
    pub(super) applied: Vec<(OutputId, ModeId)>,
    pub(super) reverted: Vec<TransactionId>,
    pub(super) fail_revert: bool,
    pub(super) fail_snapshot: bool,
    pub(super) fail_apply: bool,
}

impl TwoOutputPort {
    /// DP-A: 2 modes (current ModeId(1)). DP-B: 12 modes ModeId(101..=112), current 101.
    pub(super) fn new() -> Self {
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

    pub(super) fn output(&self, id: &str) -> &FakeOutput {
        self.outputs
            .iter()
            .find(|o| o.id.as_str() == id)
            .expect("fixture output")
    }

    pub(super) fn connected_index(&self, output: &OutputId) -> FlameResult<usize> {
        self.outputs
            .iter()
            .position(|o| o.connected && &o.id == output)
            .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "fake output missing"))
    }

    pub(super) fn disconnect(&mut self, id: &str) {
        if let Some(output) = self.outputs.iter_mut().find(|o| o.id.as_str() == id) {
            output.connected = false;
        }
        self.generation += 1;
    }

    pub(super) fn bump_generation(&mut self, by: u64) {
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

pub(super) fn begin_on(
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
    assert!(
        !service
            .tick(&mut port, 15_002)
            .expect("no retry after late Keep")
    );
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
    assert!(
        !service
            .tick(&mut port, 15_001)
            .expect("no retry after retirement")
    );
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
    assert!(
        !service
            .tick(&mut port, 15_001)
            .expect("transaction already cleared")
    );
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

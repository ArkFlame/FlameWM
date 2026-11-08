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
fn set_primary_revert_restores_the_prior_primary() {
    let mut port = TopologyPort::new();
    let mut service = DisplayService::default();
    let transaction = begin(&mut port, &mut service, &primary_b()).expect("begin primary");
    assert!(service.cached().is_some_and(|s| s.outputs[1].primary));
    service.revert(&mut port, transaction).expect("revert");
    let outputs = &service.cached().expect("snapshot").outputs;
    assert!(outputs[0].primary, "DP-A must be primary again");
    assert!(!outputs[1].primary);
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

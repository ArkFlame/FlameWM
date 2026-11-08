use super::tests_mode::pending_of;
use super::tests_output::{FakeOutput, TwoOutputPort, begin_on};
use super::*;

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
    assert!(
        service
            .begin_mode(&mut port, &OutputId::new("O4"), ModeId(412), stale, 0)
            .is_err()
    );
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
    assert!(
        !service
            .tick(&mut port, 24_999)
            .expect("before fresh deadline")
    );
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

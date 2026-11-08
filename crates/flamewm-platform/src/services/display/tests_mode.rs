use super::*;
use flamewm_api::display::DisplayMode;
use flamewm_api::geometry::{Rect, Size};

pub(super) fn mode(id: u64, width: i32, height: i32, preferred: bool) -> DisplayMode {
    DisplayMode {
        id: ModeId(id),
        resolution: Size { width, height },
        refresh_millihz: 60_000,
        preferred,
    }
}

pub(super) fn unavailable(message: &str) -> FlameError {
    FlameError::new(ErrorCode::Unavailable, message)
}

pub(super) fn pending_of(service: &DisplayService) -> PendingModeChange {
    service
        .cached()
        .and_then(|s| s.pending.clone())
        .expect("pending")
}

pub(super) struct TwoModePort {
    pub(super) mode: ModeId,
    pub(super) generation: u64,
    pub(super) fail_apply: bool,
    pub(super) fail_revert: bool,
}

impl TwoModePort {
    pub(super) fn new() -> Self {
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

pub(super) fn begin(
    port: &mut TwoModePort,
    service: &mut DisplayService,
) -> FlameResult<TransactionId> {
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
    assert!(
        service
            .begin_mode(
                &mut port,
                &OutputId::new("fake"),
                ModeId(2),
                generation,
                100
            )
            .is_err()
    );
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

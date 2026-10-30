use std::collections::BTreeMap;

use flamewm_api::ports::ShortcutPort;
use flamewm_api::shortcuts::{KeyBinding, ShortcutSnapshot};
use flamewm_api::{ErrorCode, FlameError, FlameResult};

use crate::shortcuts::{ASSISTANT_CHORDS, ShortcutRegistry};

/// Builds the shortcut set the WM must grab at startup: the default registry with every persisted
/// hotkey (`ProductSettings.hotkeys`, action id -> raw binding text) overlaid. Empty or `Escape`
/// means unassigned. The whole set is validated at once, so a conflict is an error, not a partial set.
/// The assistant chords are grabbed only while `assistant_enabled`; disabled, they are unassigned even
/// when a hotkey was persisted for them.
pub fn effective_registry(
    persisted: &BTreeMap<String, String>,
    assistant_enabled: bool,
) -> FlameResult<ShortcutRegistry> {
    let mut desired = ShortcutRegistry::default().bindings().clone();
    for (action, binding) in persisted {
        desired.insert(action.clone(), KeyBinding::assigned(binding.clone()));
    }
    for (action, chord) in ASSISTANT_CHORDS {
        if !assistant_enabled {
            desired.insert(action.to_owned(), KeyBinding::unassigned());
        } else if !persisted.contains_key(action) {
            desired.insert(action.to_owned(), KeyBinding::assigned(chord));
        }
    }
    let mut registry = ShortcutRegistry::default();
    registry.replace_all(&desired)?;
    Ok(registry)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureLease {
    id: u64,
    action: String,
}

/// Owns the Settings shortcut registry, its revision, and the in-memory capture lease.
///
/// While a capture lease is held the WM grab set is empty, so Settings receives raw keys and the
/// WM neither fires shortcuts nor toggles Start from a bare Super tap. Leases are not persisted: a
/// WM restart starts with no lease and grabs the effective registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutService {
    registry: ShortcutRegistry,
    revision: u64,
    capture: Option<CaptureLease>,
    next_lease: u64,
}

impl Default for ShortcutService {
    fn default() -> Self {
        Self {
            registry: ShortcutRegistry::default(),
            revision: 1,
            capture: None,
            next_lease: 1,
        }
    }
}

impl ShortcutService {
    #[must_use]
    pub fn snapshot(&self) -> ShortcutSnapshot {
        ShortcutSnapshot {
            revision: self.revision,
            bindings: self.registry.bindings().clone(),
        }
    }

    pub fn set_binding<P: ShortcutPort>(
        &mut self,
        port: &mut P,
        action: &str,
        binding: &str,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.mutate(port, expected_revision, |registry| {
            registry.set(action, binding)
        })
    }

    pub fn clear_binding<P: ShortcutPort>(
        &mut self,
        port: &mut P,
        action: &str,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.mutate(port, expected_revision, |registry| registry.clear(action))
    }

    pub fn reset_binding<P: ShortcutPort>(
        &mut self,
        port: &mut P,
        action: &str,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.mutate(port, expected_revision, |registry| {
            registry.reset_one(action)
        })
    }

    pub fn apply<P: ShortcutPort>(
        &mut self,
        port: &mut P,
        bindings: &BTreeMap<String, KeyBinding>,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.mutate(port, expected_revision, |registry| {
            registry.replace_all(bindings)
        })
    }

    /// Refuses during capture or on a stale revision, applies `edit` to a candidate copy, then commits it.
    fn mutate<P: ShortcutPort>(
        &mut self,
        port: &mut P,
        expected_revision: u64,
        edit: impl FnOnce(&mut ShortcutRegistry) -> FlameResult<()>,
    ) -> FlameResult<()> {
        self.require_idle()?;
        self.require_revision(expected_revision)?;
        let mut candidate = self.registry.clone();
        edit(&mut candidate)?;
        if candidate == self.registry {
            return Ok(());
        }
        Self::stage_and_commit(port, candidate.bindings())?;
        self.registry = candidate;
        self.revision = self.revision.saturating_add(1);
        Ok(())
    }

    /// Startup grab of the effective set through the same prepare/commit port path as `apply`.
    /// No equality short-circuit: the registry starts at defaults, so the first grab must reach the port
    /// even when the effective set equals the defaults.
    pub fn commit_startup<P: ShortcutPort>(
        &mut self,
        port: &mut P,
        registry: ShortcutRegistry,
    ) -> FlameResult<()> {
        Self::stage_and_commit(port, registry.bindings())?;
        if registry != self.registry {
            self.revision = self.revision.saturating_add(1);
        }
        self.registry = registry;
        Ok(())
    }

    /// Suspends every grab for a Settings capture. The grab set becomes empty; the registry and
    /// revision are unchanged. Refused with `Busy` while another lease is held.
    pub fn begin_capture<P: ShortcutPort>(
        &mut self,
        port: &mut P,
        action: &str,
        expected_revision: u64,
    ) -> FlameResult<u64> {
        self.require_revision(expected_revision)?;
        if self.registry.binding(action).is_none() {
            return Err(FlameError::new(
                ErrorCode::NotFound,
                "shortcut action is unknown",
            ));
        }
        // A held lease (stale after a Settings crash, or a re-armed row) is superseded: the grab
        // set is already empty, so only the first lease needs to suspend the grabs.
        if self.capture.is_none() {
            Self::stage_and_commit(port, &BTreeMap::new())?;
        }
        let id = self.next_lease;
        self.next_lease = self.next_lease.saturating_add(1);
        self.capture = Some(CaptureLease {
            id,
            action: action.to_owned(),
        });
        Ok(id)
    }

    /// Ends the lease `lease` and restores the grab set from the registry. A restore failure keeps
    /// the lease held so the owner can retry.
    pub fn end_capture<P: ShortcutPort>(&mut self, port: &mut P, lease: u64) -> FlameResult<()> {
        let held = self.capture.as_ref().map(|active| active.id);
        match held {
            Some(id) if id == lease => self.release_capture(port),
            Some(_) => Err(FlameError::new(
                ErrorCode::Conflict,
                "hotkey capture lease does not match the active lease",
            )),
            None => Err(FlameError::new(
                ErrorCode::NotFound,
                "no hotkey capture is active",
            )),
        }
    }

    /// Ends whatever lease is held and restores the grab set. Used by teardown and owner-loss paths
    /// that do not carry a lease id. A no-op when no lease is held.
    pub fn release_capture<P: ShortcutPort>(&mut self, port: &mut P) -> FlameResult<()> {
        if self.capture.is_none() {
            return Ok(());
        }
        Self::stage_and_commit(port, self.registry.bindings())?;
        self.capture = None;
        Ok(())
    }

    #[must_use]
    pub fn capture_active(&self) -> bool {
        self.capture.is_some()
    }

    /// Registry and revision before a mutation, used by `restore` when the mutation's persistence fails.
    #[must_use]
    pub fn checkpoint(&self) -> (ShortcutRegistry, u64) {
        (self.registry.clone(), self.revision)
    }

    /// Re-grabs a `checkpoint` set and restores its registry and revision. Refused without state change
    /// if the port cannot commit the checkpoint's grabs.
    pub fn restore<P: ShortcutPort>(
        &mut self,
        port: &mut P,
        checkpoint: (ShortcutRegistry, u64),
    ) -> FlameResult<()> {
        Self::stage_and_commit(port, checkpoint.0.bindings())?;
        self.registry = checkpoint.0;
        self.revision = checkpoint.1;
        Ok(())
    }

    fn require_idle(&self) -> FlameResult<()> {
        match &self.capture {
            None => Ok(()),
            Some(_) => Err(FlameError::new(
                ErrorCode::Busy,
                "shortcut changes are refused while hotkey capture is active",
            )),
        }
    }

    /// Stages `desired` on the port and commits it. A failure at either step rolls the port back and
    /// returns the original error; the caller's registry, revision, and lease are left untouched.
    fn stage_and_commit<P: ShortcutPort>(
        port: &mut P,
        desired: &BTreeMap<String, KeyBinding>,
    ) -> FlameResult<()> {
        if let Err(error) = port.prepare_shortcuts(desired) {
            port.rollback_shortcuts();
            return Err(error);
        }
        if let Err(error) = port.commit_shortcuts() {
            port.rollback_shortcuts();
            return Err(error);
        }
        Ok(())
    }

    fn require_revision(&self, expected: u64) -> FlameResult<()> {
        if expected == self.revision {
            Ok(())
        } else {
            Err(FlameError::new(
                ErrorCode::StaleRevision,
                "shortcut revision is stale",
            ))
        }
    }
}

#[cfg(test)]
mod effective_registry_tests {
    use flamewm_api::shortcuts::{ACTION_TOGGLE_START_MENU, ACTION_WINDOW_MAXIMIZE};

    use super::*;

    #[test]
    fn persisted_binding_overlays_only_its_own_action() {
        let persisted =
            BTreeMap::from([(ACTION_WINDOW_MAXIMIZE.to_owned(), "Ctrl+Super+M".to_owned())]);
        let effective =
            effective_registry(&persisted, false).expect("single custom binding is valid");
        let defaults = ShortcutRegistry::default();

        assert_eq!(
            effective.binding(ACTION_WINDOW_MAXIMIZE),
            Some(&KeyBinding::assigned("Ctrl+Super+M"))
        );
        for action in crate::shortcuts::ACTIONS {
            if action != ACTION_WINDOW_MAXIMIZE {
                assert_eq!(
                    effective.binding(action),
                    defaults.binding(action),
                    "{action}"
                );
            }
        }
    }

    #[test]
    fn empty_persisted_binding_unassigns_its_action() {
        let persisted = BTreeMap::from([(ACTION_TOGGLE_START_MENU.to_owned(), String::new())]);
        let effective = effective_registry(&persisted, false).expect("empty means unassigned");
        assert_eq!(
            effective.binding(ACTION_TOGGLE_START_MENU),
            Some(&KeyBinding::unassigned())
        );
    }

    #[test]
    fn empty_persisted_map_equals_defaults() {
        let effective = effective_registry(&BTreeMap::new(), false).expect("defaults are valid");
        assert_eq!(effective, ShortcutRegistry::default());
    }

    #[test]
    fn assistant_chords_are_unassigned_by_default_and_grabbed_only_when_enabled() {
        use crate::shortcuts::{ACTION_OPEN_ASSISTANT_CTRL_ALT, ACTION_OPEN_ASSISTANT_SUPER};

        assert_eq!(
            ShortcutRegistry::default().binding(ACTION_OPEN_ASSISTANT_SUPER),
            Some(&KeyBinding::unassigned())
        );
        let disabled = effective_registry(&BTreeMap::new(), false).expect("valid");
        assert_eq!(
            disabled.binding(ACTION_OPEN_ASSISTANT_SUPER),
            Some(&KeyBinding::unassigned())
        );

        let enabled = effective_registry(&BTreeMap::new(), true).expect("chords do not conflict");
        assert_eq!(
            enabled.binding(ACTION_OPEN_ASSISTANT_SUPER),
            Some(&KeyBinding::assigned("Super+C"))
        );
        assert_eq!(
            enabled.binding(ACTION_OPEN_ASSISTANT_CTRL_ALT),
            Some(&KeyBinding::assigned("Ctrl+Alt+C"))
        );
    }

    #[test]
    fn disabled_assistant_drops_persisted_chord_and_enabled_keeps_user_override() {
        use crate::shortcuts::ACTION_OPEN_ASSISTANT_SUPER;

        let persisted = BTreeMap::from([(
            ACTION_OPEN_ASSISTANT_SUPER.to_owned(),
            "Super+Shift+C".to_owned(),
        )]);
        let disabled = effective_registry(&persisted, false).expect("valid");
        assert_eq!(
            disabled.binding(ACTION_OPEN_ASSISTANT_SUPER),
            Some(&KeyBinding::unassigned())
        );
        let enabled = effective_registry(&persisted, true).expect("valid");
        assert_eq!(
            enabled.binding(ACTION_OPEN_ASSISTANT_SUPER),
            Some(&KeyBinding::assigned("Super+Shift+C"))
        );
    }

    #[test]
    fn assistant_chords_do_not_collide_with_existing_default_chords() {
        use crate::shortcuts::ASSISTANT_CHORDS;

        let defaults = ShortcutRegistry::default();
        for (action, chord) in ASSISTANT_CHORDS {
            assert_eq!(
                defaults.conflict_for(action, &crate::shortcuts::normalize(chord)),
                None,
                "{chord}"
            );
        }
    }
}

#[cfg(test)]
mod capture_lease_tests {
    use flamewm_api::shortcuts::{ACTION_TOGGLE_START_MENU, ACTION_WINDOW_MAXIMIZE};

    use super::*;

    /// Records the live grab set. `prepare` stages, `commit` makes the staged set live.
    #[derive(Default)]
    struct FakeShortcutPort {
        staged: Option<BTreeMap<String, KeyBinding>>,
        live: BTreeMap<String, KeyBinding>,
        fail_commits: usize,
    }

    impl ShortcutPort for FakeShortcutPort {
        fn prepare_shortcuts(&mut self, desired: &BTreeMap<String, KeyBinding>) -> FlameResult<()> {
            self.staged = Some(desired.clone());
            Ok(())
        }
        fn commit_shortcuts(&mut self) -> FlameResult<()> {
            if self.fail_commits > 0 {
                self.fail_commits -= 1;
                return Err(FlameError::new(ErrorCode::Unavailable, "grab refused"));
            }
            let staged = self
                .staged
                .take()
                .ok_or_else(|| FlameError::new(ErrorCode::InvalidArgument, "nothing staged"))?;
            self.live = staged;
            Ok(())
        }
        fn rollback_shortcuts(&mut self) {
            self.staged = None;
        }
    }

    fn defaults() -> BTreeMap<String, KeyBinding> {
        ShortcutRegistry::default().bindings().clone()
    }

    fn started() -> (ShortcutService, FakeShortcutPort) {
        let mut port = FakeShortcutPort::default();
        let mut service = ShortcutService::default();
        service
            .commit_startup(&mut port, ShortcutRegistry::default())
            .expect("startup grab");
        assert_eq!(port.live, defaults());
        (service, port)
    }

    fn revision(service: &ShortcutService) -> u64 {
        service.snapshot().revision
    }

    #[test]
    fn begin_capture_suspends_every_grab_including_bare_super() {
        let (mut service, mut port) = started();
        let lease = service
            .begin_capture(&mut port, ACTION_TOGGLE_START_MENU, revision(&service))
            .expect("first lease");
        assert_eq!(lease, 1);
        assert!(service.capture_active());
        assert!(port.live.is_empty(), "no WM grab may survive a capture");
    }

    #[test]
    fn second_begin_supersedes_the_first_lease_and_keeps_suspension() {
        let (mut service, mut port) = started();
        let first = service
            .begin_capture(&mut port, ACTION_TOGGLE_START_MENU, revision(&service))
            .expect("first lease");
        let second = service
            .begin_capture(&mut port, ACTION_WINDOW_MAXIMIZE, revision(&service))
            .expect("second lease supersedes the first");
        assert!(port.live.is_empty());
        assert_eq!(
            service.end_capture(&mut port, first).unwrap_err().code,
            ErrorCode::Conflict
        );
        service
            .end_capture(&mut port, second)
            .expect("new lease ends");
    }

    #[test]
    fn end_capture_restores_the_registry_grab_set() {
        let (mut service, mut port) = started();
        let lease = service
            .begin_capture(&mut port, ACTION_TOGGLE_START_MENU, revision(&service))
            .expect("lease");
        service.end_capture(&mut port, lease).expect("end");
        assert!(!service.capture_active());
        assert_eq!(port.live, defaults());
        assert_eq!(
            port.live.get(ACTION_TOGGLE_START_MENU),
            Some(&KeyBinding::assigned("Super_L")),
            "Super-to-Start works again outside capture"
        );
    }

    #[test]
    fn disconnect_owner_loss_restores_through_the_lease_id() {
        let (mut service, mut port) = started();
        let lease = service
            .begin_capture(&mut port, ACTION_TOGGLE_START_MENU, revision(&service))
            .expect("lease");
        // Settings disconnect is observed by the bus adapter and ends the lease it owned.
        service
            .end_capture(&mut port, lease)
            .expect("owner loss end");
        assert_eq!(port.live, defaults());
    }

    #[test]
    fn unmanage_teardown_release_restores_without_a_lease_id() {
        let (mut service, mut port) = started();
        service
            .begin_capture(&mut port, ACTION_TOGGLE_START_MENU, revision(&service))
            .expect("lease");
        service
            .release_capture(&mut port)
            .expect("teardown release");
        assert!(!service.capture_active());
        assert_eq!(port.live, defaults());
        service
            .release_capture(&mut port)
            .expect("release with no lease is a no-op");
    }

    #[test]
    fn end_with_unknown_lease_is_refused_and_keeps_suspension() {
        let (mut service, mut port) = started();
        service
            .begin_capture(&mut port, ACTION_TOGGLE_START_MENU, revision(&service))
            .expect("lease");
        let error = service
            .end_capture(&mut port, 999)
            .expect_err("wrong lease");
        assert_eq!(error.code, ErrorCode::Conflict);
        assert!(service.capture_active());
        assert!(port.live.is_empty());
    }

    #[test]
    fn end_without_any_lease_is_not_found() {
        let (mut service, mut port) = started();
        let error = service.end_capture(&mut port, 1).expect_err("no lease");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(port.live, defaults());
    }

    #[test]
    fn shortcut_mutations_are_refused_while_capture_is_active() {
        let (mut service, mut port) = started();
        let before = service.snapshot();
        service
            .begin_capture(&mut port, ACTION_TOGGLE_START_MENU, revision(&service))
            .expect("lease");
        let error = service
            .set_binding(
                &mut port,
                ACTION_WINDOW_MAXIMIZE,
                "Ctrl+Super+M",
                before.revision,
            )
            .expect_err("mutation during capture");
        assert_eq!(error.code, ErrorCode::Busy);
        assert_eq!(service.snapshot(), before);
        assert!(port.live.is_empty());
    }

    #[test]
    fn unknown_action_is_refused_without_suspending_grabs() {
        let (mut service, mut port) = started();
        let error = service
            .begin_capture(&mut port, "no.such.action", revision(&service))
            .expect_err("unknown action");
        assert_eq!(error.code, ErrorCode::NotFound);
        assert!(!service.capture_active());
        assert_eq!(port.live, defaults());
    }

    #[test]
    fn stale_revision_is_refused_without_suspending_grabs() {
        let (mut service, mut port) = started();
        let error = service
            .begin_capture(&mut port, ACTION_TOGGLE_START_MENU, 0)
            .expect_err("stale");
        assert_eq!(error.code, ErrorCode::StaleRevision);
        assert_eq!(port.live, defaults());
    }

    #[test]
    fn failed_restore_keeps_the_lease_so_the_owner_can_retry() {
        let (mut service, mut port) = started();
        let lease = service
            .begin_capture(&mut port, ACTION_TOGGLE_START_MENU, revision(&service))
            .expect("lease");
        port.fail_commits = 1;
        assert!(service.end_capture(&mut port, lease).is_err());
        assert!(service.capture_active(), "lease kept after failed restore");
        service
            .end_capture(&mut port, lease)
            .expect("retry restores");
        assert_eq!(port.live, defaults());
        assert!(!service.capture_active());
    }
}

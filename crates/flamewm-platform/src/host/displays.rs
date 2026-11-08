use super::*;

use flamewm_api::display::TopologyChange;

impl<E: EnginePorts> PlatformHost<E> {
    pub fn display_snapshot(&mut self) -> FlameResult<DisplaySnapshot> {
        Ok(self.displays.refresh(&mut self.engine)?.clone())
    }

    /// Reconcile per-output panel surfaces/reservations after a display topology refresh.
    pub fn resync_panels_for_displays(&mut self, displays: &DisplaySnapshot) -> FlameResult<bool> {
        let previous_revision = self.panels.revision();
        self.panels.sync_outputs(displays);
        // A detached and reattached output creates a fresh OutputPanel. Reapply
        // persisted presentation policy, otherwise the panel silently reverts
        // to bottom/44px/classic after RandR hotplug or monitor rearrangement.
        self.seed_persisted_panel_edges();
        self.panels
            .reconcile_taskbar_height(self.settings.current().effective().taskbar_height);
        self.panels
            .reconcile_taskbar_frame(TaskbarFrame::from_settings(self.settings.current()));
        self.panels
            .reconcile_taskbar_enabled(self.settings.current().effective().taskbar_enabled);
        Ok(self.panels.revision() != previous_revision)
    }

    pub fn begin_display_mode(
        &mut self,
        output: &OutputId,
        mode: ModeId,
        expected_generation: u64,
        now_ms: u64,
    ) -> FlameResult<TransactionId> {
        self.displays
            .begin_mode(&mut self.engine, output, mode, expected_generation, now_ms)
    }

    /// Primary, rotation or layout change; keep/revert use the mode transaction calls.
    pub fn begin_display_topology(
        &mut self,
        change: &TopologyChange,
        expected_generation: u64,
        now_ms: u64,
    ) -> FlameResult<TransactionId> {
        self.displays
            .begin_topology(&mut self.engine, change, expected_generation, now_ms)
    }

    pub fn keep_display_mode(
        &mut self,
        transaction: TransactionId,
        now_ms: u64,
    ) -> FlameResult<()> {
        self.displays.keep(&mut self.engine, transaction, now_ms)
    }

    pub fn revert_display_mode(&mut self, transaction: TransactionId) -> FlameResult<()> {
        self.displays.revert(&mut self.engine, transaction)
    }

    pub fn tick_display_transactions(&mut self, now_ms: u64) -> FlameResult<bool> {
        self.displays.tick(&mut self.engine, now_ms)
    }

    /// Revert deadline of the pending display mode transaction, if any (cached snapshot).
    #[must_use]
    pub fn display_transaction_deadline_ms(&self) -> Option<u64> {
        self.displays
            .cached()
            .and_then(|snapshot| snapshot.pending.as_ref())
            .map(|pending| pending.deadline_ms)
    }

    pub fn set_shell_scale(
        &mut self,
        output: &OutputId,
        percent: u16,
        expected_generation: u64,
    ) -> FlameResult<bool> {
        let previous = self
            .displays
            .cached()
            .and_then(|snapshot| {
                snapshot
                    .outputs
                    .iter()
                    .find(|candidate| candidate.connected && candidate.id == *output)
            })
            .map(|candidate| candidate.shell_scale_percent);
        let changed = self
            .displays
            .set_shell_scale(output, percent, expected_generation)?;
        if changed {
            // Persist before the panel resync so a failed write leaves no runtime-only scale behind.
            let index = self
                .displays
                .cached()
                .and_then(|snapshot| {
                    snapshot
                        .outputs
                        .iter()
                        .position(|candidate| candidate.id == *output)
                })
                .unwrap_or(0);
            if let Err(error) = self.persist_output_scale(output, index, percent) {
                if let Some(previous) = previous {
                    let generation = self.cached_generation();
                    if let Err(restore_error) =
                        self.displays.set_shell_scale(output, previous, generation)
                    {
                        eprintln!(
                            "flamewm: shell scale rollback failed after persistence error: {}",
                            restore_error.message
                        );
                    }
                }
                return Err(error);
            }
            // Panel thickness is derived from the output scale; resync it the same way
            // apply_settings does so the panel geometry follows the new display scale.
            if let Some(displays) = self.displays.cached().cloned() {
                self.panels.sync_outputs(&displays);
            }
        }
        Ok(changed)
    }

    /// Writes the scale under the keys the display backend reads (`DisplayPort::scale_keys`). A
    /// failed write restores earlier keys, so the caller can roll back the runtime scale.
    fn persist_output_scale(
        &mut self,
        output: &OutputId,
        index: usize,
        percent: u16,
    ) -> FlameResult<()> {
        let keys = self.engine.scale_keys(output, index);
        let prior = self.settings.current().per_output_scale.clone();
        let settings = &mut self.settings;
        persist_scale_keys(
            &keys,
            percent,
            |key| prior.get(key).copied(),
            |key, value| settings.persist_scale(key, value),
        )
    }

    /// Last display snapshot already cached by the platform, without refreshing from the engine.
    #[must_use]
    pub fn cached_display_snapshot(&self) -> Option<DisplaySnapshot> {
        self.displays.cached().cloned()
    }
}

/// Writes `percent` under every key in order. A failed write restores the keys already written:
/// to their prior value, or deleted when they had none. The error is returned, so a partial
/// persist never survives.
fn persist_scale_keys(
    keys: &[String],
    percent: u16,
    prior: impl Fn(&str) -> Option<u16>,
    mut write: impl FnMut(&str, Option<u16>) -> FlameResult<()>,
) -> FlameResult<()> {
    let mut written: Vec<(&String, Option<u16>)> = Vec::with_capacity(keys.len());
    for key in keys {
        if let Err(error) = write(key, Some(percent)) {
            for (done, previous) in written.iter().rev() {
                if let Err(restore) = write(done, *previous) {
                    eprintln!(
                        "flamewm: scale rollback for {done} failed: {}",
                        restore.message
                    );
                }
            }
            return Err(error);
        }
        written.push((key, prior(key)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn keys(index: usize) -> Vec<String> {
        flamewm_api::ports::wayland_scale_keys(index)
    }

    #[test]
    fn first_output_persists_only_wayland_zero() {
        assert_eq!(keys(0), vec!["wayland-0".to_owned()]);
    }

    #[test]
    fn other_outputs_persist_their_index_key_and_the_wm_key() {
        assert_eq!(
            keys(2),
            vec!["wayland-2".to_owned(), "wayland-0".to_owned()]
        );
    }

    #[test]
    fn successful_persist_writes_every_key() {
        let mut store = BTreeMap::new();
        persist_scale_keys(
            &keys(2),
            150,
            |_| None,
            |key, value| {
                store.insert(key.to_owned(), value.expect("set"));
                Ok(())
            },
        )
        .expect("persist");
        assert_eq!(store.get("wayland-2"), Some(&150));
        assert_eq!(store.get("wayland-0"), Some(&150));
    }

    #[test]
    fn failed_second_key_deletes_a_key_that_had_no_prior_value() {
        let mut store: BTreeMap<String, u16> = BTreeMap::new();
        let result = persist_scale_keys(
            &keys(2),
            150,
            |_| None,
            |key, value| {
                if key == "wayland-0" {
                    return Err(FlameError::new(ErrorCode::Unavailable, "disk full"));
                }
                match value {
                    Some(value) => store.insert(key.to_owned(), value),
                    None => store.remove(key),
                };
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err().code, ErrorCode::Unavailable);
        assert!(!store.contains_key("wayland-2"), "new key deleted on rollback");
        assert!(store.is_empty());
    }

    #[test]
    fn failed_second_key_restores_first_key_and_fails_the_transaction() {
        let mut store: BTreeMap<String, u16> = BTreeMap::new();
        store.insert("wayland-2".to_owned(), 100);
        store.insert("wayland-0".to_owned(), 100);
        let prior = store.clone();
        let result = persist_scale_keys(
            &keys(2),
            150,
            |key| prior.get(key).copied(),
            |key, value| {
                if key == "wayland-0" {
                    return Err(FlameError::new(ErrorCode::Unavailable, "disk full"));
                }
                match value {
                    Some(value) => store.insert(key.to_owned(), value),
                    None => store.remove(key),
                };
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err().code, ErrorCode::Unavailable);
        assert_eq!(store.get("wayland-2"), Some(&100), "first key rolled back");
        assert_eq!(store.get("wayland-0"), Some(&100));
    }
}

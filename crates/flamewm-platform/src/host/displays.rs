use super::*;

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
            .reconcile_taskbar_height(self.settings.current().taskbar_height);
        self.panels
            .reconcile_taskbar_frame(TaskbarFrame::from_settings(self.settings.current()));
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
            if let Err(error) = self.settings.persist_scale(output.as_str(), percent) {
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

    /// Last display snapshot already cached by the platform, without refreshing from the engine.
    #[must_use]
    pub fn cached_display_snapshot(&self) -> Option<DisplaySnapshot> {
        self.displays.cached().cloned()
    }
}

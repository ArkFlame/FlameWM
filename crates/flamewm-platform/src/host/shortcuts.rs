use super::*;

impl<E: EnginePorts> PlatformHost<E> {
    #[must_use]
    pub fn shortcut_snapshot(&self) -> ShortcutSnapshot {
        self.shortcuts.snapshot()
    }

    pub fn set_shortcut(
        &mut self,
        action: &str,
        binding: &str,
        expected_revision: u64,
    ) -> FlameResult<()> {
        let checkpoint = self.shortcuts.checkpoint();
        self.shortcuts
            .set_binding(&mut self.engine, action, binding, expected_revision)?;
        let persisted = self
            .shortcuts
            .snapshot()
            .bindings
            .get(action)
            .and_then(|entry| entry.0.clone())
            .unwrap_or_default();
        self.persist_shortcut(checkpoint, action, Some(&persisted))
    }

    pub fn clear_shortcut(&mut self, action: &str, expected_revision: u64) -> FlameResult<()> {
        let checkpoint = self.shortcuts.checkpoint();
        self.shortcuts
            .clear_binding(&mut self.engine, action, expected_revision)?;
        self.persist_shortcut(checkpoint, action, Some(""))
    }

    pub fn reset_shortcut(&mut self, action: &str, expected_revision: u64) -> FlameResult<()> {
        let checkpoint = self.shortcuts.checkpoint();
        self.shortcuts
            .reset_binding(&mut self.engine, action, expected_revision)?;
        self.persist_shortcut(checkpoint, action, None)
    }

    /// Persists one hotkey entry after its grabs committed. If persistence fails, the checkpoint
    /// registry and grabs are restored so the WM and the settings file never diverge.
    fn persist_shortcut(
        &mut self,
        checkpoint: (ShortcutRegistry, u64),
        action: &str,
        binding: Option<&str>,
    ) -> FlameResult<()> {
        let Err(error) = self.settings.persist_hotkey(action, binding) else {
            return Ok(());
        };
        if let Err(restore_error) = self.shortcuts.restore(&mut self.engine, checkpoint) {
            eprintln!("flamewm: hotkey rollback failed after persistence error: {restore_error}");
        }
        Err(error)
    }

    pub fn apply_shortcuts(
        &mut self,
        bindings: &std::collections::BTreeMap<String, KeyBinding>,
        expected_revision: u64,
    ) -> FlameResult<()> {
        self.shortcuts
            .apply(&mut self.engine, bindings, expected_revision)
    }

    /// Suspends every shortcut grab for a Settings capture lease. See `ShortcutService::begin_capture`.
    pub fn begin_hotkey_capture(
        &mut self,
        action: &str,
        expected_revision: u64,
    ) -> FlameResult<u64> {
        self.shortcuts
            .begin_capture(&mut self.engine, action, expected_revision)
    }

    pub fn end_hotkey_capture(&mut self, lease: u64) -> FlameResult<()> {
        self.shortcuts.end_capture(&mut self.engine, lease)
    }

    /// Grabs the effective shortcut set (persisted hotkeys over defaults) after `start()` loaded
    /// settings. A persisted set that fails validation logs once and grabs the defaults instead.
    pub fn commit_startup_shortcuts(&mut self) -> FlameResult<()> {
        let current = self.settings.current();
        let registry = match effective_registry(&current.hotkeys, current.assistant_enabled) {
            Ok(registry) => registry,
            Err(error) => {
                eprintln!("flamewm: persisted hotkeys rejected, grabbing defaults: {error}");
                ShortcutRegistry::default()
            }
        };
        self.shortcuts.commit_startup(&mut self.engine, registry)
    }
}

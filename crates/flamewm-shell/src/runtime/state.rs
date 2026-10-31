use super::*;

impl ShellRuntime {
    #[must_use]
    pub fn new(snapshot: ShellSnapshot) -> Self {
        Self {
            snapshot: Rc::new(snapshot),
            selected_panel_output: None,
            controls: VecDeque::new(),
            dirty: ShellDirty::default(),
            switcher: None,
        }
    }

    #[must_use]
    pub fn dirty(&self) -> ShellDirty {
        self.dirty
    }

    /// Record a delivered `ControlSignal` by applying snapshot payloads
    /// directly. No fetch happens here and legacy revision-only signals
    /// (`WindowsChanged` / `WorkspacesChanged` / `PanelsChanged` /
    /// `SystemChanged`) are ignored for normal refresh; the shell loop
    /// projects from the already-applied snapshot once per turn.
    pub fn mark_signal(&mut self, signal: &ControlSignal) {
        match signal {
            ControlSignal::WindowsSnapshotChanged { windows, .. } => {
                let _s = crate::runtime::shell_span("shell.signal_to_visual.windows").start();
                if *windows != self.snapshot.windows {
                    Rc::make_mut(&mut self.snapshot).windows.clone_from(windows);
                    self.dirty.windows = true;
                }
            }
            ControlSignal::WorkspacesSnapshotChanged { snapshot } => {
                let _s = crate::runtime::shell_span("shell.signal_to_visual.workspaces").start();
                if self.snapshot.workspaces.as_ref() != Some(snapshot) {
                    Rc::make_mut(&mut self.snapshot).workspaces = Some(snapshot.clone());
                    self.dirty.workspaces = true;
                }
            }
            ControlSignal::PanelsSnapshotChanged { snapshot } => {
                if *snapshot != self.snapshot.panels {
                    Rc::make_mut(&mut self.snapshot).panels = snapshot.clone();
                    self.dirty.panels = true;
                }
            }
            ControlSignal::DisplaysSnapshotChanged { snapshot } => {
                // Output shell scale lives only in the display snapshot. Refreshing it here lets
                // resync_panel_geometry see the scale drift; panels are marked dirty so the
                // frame loop runs the panel projection path this turn.
                if *snapshot != self.snapshot.displays {
                    Rc::make_mut(&mut self.snapshot).displays = snapshot.clone();
                    self.dirty.panels = true;
                }
            }
            ControlSignal::SystemSnapshotChanged { snapshot } => {
                let _s = crate::runtime::shell_span("shell.signal_to_visual.system").start();
                if snapshot != &self.snapshot.system {
                    Rc::make_mut(&mut self.snapshot).system = snapshot.clone();
                    self.dirty.system = true;
                }
            }
            ControlSignal::SystemChanged { .. } => {}
            // Same path as the `start.toggle` dispatch: one queued ToggleStart control.
            ControlSignal::StartMenuToggleRequested => {
                self.controls.push_back(ShellControl::ToggleStart {
                    output: None,
                    interaction: None,
                });
            }
            // Same shape as the Start toggle: one queued control per request.
            ControlSignal::LauncherToggleRequested => {
                self.controls.push_back(ShellControl::ToggleLauncher {
                    output: None,
                    interaction: None,
                });
            }
            // Latest snapshot wins; the overlay renders it and never decides the switch.
            ControlSignal::WindowSwitcherChanged {
                open,
                immediate,
                selected,
                entries,
            } => {
                self.switcher = Some(crate::switcher::SwitcherFrame {
                    open: *open,
                    immediate: *immediate,
                    selected: usize::try_from(*selected).unwrap_or(0),
                    entries: entries.clone(),
                });
            }
            _ => {}
        }
    }

    /// Reconcile already-applied snapshot domains into a change set without
    /// any fetch. Every steady-state domain is supplied by a snapshot signal;
    /// this method only consumes dirty flags and never owns a control client.
    pub fn reconcile_dirty(&mut self) -> ChangedDomains {
        let take = |flag: &mut bool, kind: DomainKind| {
            let set = std::mem::take(flag);
            if set {
                domain_reconcile_counter(kind).increment();
            }
            set
        };
        let changed = ChangedDomains {
            windows: take(&mut self.dirty.windows, DomainKind::Windows),
            workspaces: take(&mut self.dirty.workspaces, DomainKind::Workspaces),
            panels: take(&mut self.dirty.panels, DomainKind::Panels),
            system: take(&mut self.dirty.system, DomainKind::System),
            applications: false,
        };
        // No application signal is owned by this reconciliation path; keep
        // the flag consumed if a producer marks it before a future snapshot
        // contract is added.
        self.dirty.applications = false;
        changed
    }

    /// Compatibility name for callers that only consume applied snapshots.
    pub fn reconcile_applied(&mut self) -> ChangedDomains {
        self.reconcile_dirty()
    }

    #[must_use]
    pub fn snapshot(&self) -> &ShellSnapshot {
        &self.snapshot
    }

    /// Cheap shared handle to the current snapshot (refcount bump, no copy).
    /// Writers use `Rc::make_mut`, so a held handle keeps its original value.
    #[must_use]
    pub fn snapshot_rc(&self) -> Rc<ShellSnapshot> {
        Rc::clone(&self.snapshot)
    }

    /// Select the panel output that originated the current taskbar input.
    /// Stale outputs are ignored; callers must resolve global actions separately.
    pub fn select_panel_output(&mut self, output: flamewm_api::OutputId) {
        if self
            .snapshot
            .panels
            .panels
            .iter()
            .any(|panel| panel.output == output)
        {
            self.selected_panel_output = Some(output);
        }
    }

    pub(super) fn selected_output(&self) -> Option<flamewm_api::OutputId> {
        self.shown_panel_output().cloned()
    }

    /// Keyboard Start fallback when no root pointer output is available.
    #[must_use]
    pub fn focused_panel_output(&self) -> Option<flamewm_api::OutputId> {
        self.snapshot
            .windows
            .iter()
            .find(|window| window.focused)
            .map(|window| window.output.clone())
            .filter(|output| {
                self.snapshot
                    .panels
                    .panels
                    .iter()
                    .any(|panel| &panel.output == output)
            })
    }
}

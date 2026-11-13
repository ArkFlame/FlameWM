//! `PlatformHost` port implementations for `WaylandDesktop`.

use super::*;

impl WaylandDesktop {
    pub(super) fn window(&self, window: WindowRef) -> FlameResult<WindowSnapshot> {
        self.mirror
            .borrow()
            .windows
            .iter()
            .find(|candidate| candidate.reference.id == window.id)
            .cloned()
            .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "window is not managed"))
    }

    pub(super) fn queue(&self, command: Command) -> FlameResult<()> {
        self.commands.borrow_mut().push(command);
        Ok(())
    }

    pub(super) fn output_id() -> OutputId {
        OutputId::new(OUTPUT_NAME)
    }
}

impl WindowPort for WaylandDesktop {
    fn get(&self, window: WindowRef) -> FlameResult<WindowSnapshot> {
        self.window(window)
    }

    fn snapshot(&self) -> FlameResult<Vec<WindowSnapshot>> {
        Ok(self.mirror.borrow().windows.clone())
    }

    fn activate(&mut self, window: WindowRef) -> FlameResult<()> {
        self.window(window)?;
        self.queue(Command::Activate(window.id))
    }

    fn minimize(&mut self, window: WindowRef) -> FlameResult<()> {
        self.window(window)?;
        self.queue(Command::Minimize(window.id))
    }

    fn maximize(&mut self, window: WindowRef) -> FlameResult<()> {
        self.window(window)?;
        self.queue(Command::Maximize(window.id))
    }

    fn restore(&mut self, window: WindowRef) -> FlameResult<()> {
        self.window(window)?;
        self.queue(Command::Restore(window.id))
    }

    fn close(&mut self, window: WindowRef) -> FlameResult<()> {
        self.window(window)?;
        self.queue(Command::Close(window.id))
    }

    fn set_outer_geometry(&mut self, window: WindowRef, geometry: Rect) -> FlameResult<()> {
        // Mirror X11: reject an invalid rect before touching the model, with the same message.
        // `is_valid` already requires a positive width and height.
        if !geometry.is_valid() {
            return Err(FlameError::invalid("window geometry is invalid"));
        }
        self.window(window)?;
        self.queue(Command::SetGeometry(
            window.id,
            crate::model::Rect::new(geometry.x, geometry.y, geometry.width, geometry.height),
        ))
    }

    fn work_area(&self, _window: WindowRef) -> FlameResult<Rect> {
        Ok(self.mirror.borrow().work_area)
    }

    fn output(&self, window: WindowRef) -> FlameResult<OutputId> {
        self.window(window).map(|snapshot| snapshot.output)
    }
}

impl WorkspacePort for WaylandDesktop {
    fn workspace_snapshot(&self) -> FlameResult<WorkspaceSnapshot> {
        let mirror = self.mirror.borrow();
        Ok(WorkspaceSnapshot {
            revision: mirror.workspace_revision,
            count: mirror.workspace_count,
            active_index: mirror.active_workspace,
            last_index: None,
            names: (1..=mirror.workspace_count)
                .map(|index| index.to_string())
                .collect(),
            occupancy: mirror.workspace_occupancy(),
        })
    }

    fn activate_workspace(&mut self, index: usize, _expected_revision: u64) -> FlameResult<()> {
        self.queue(Command::SwitchWorkspace(index))
    }

    fn move_window_to_workspace(&mut self, window: WindowRef, target: usize) -> FlameResult<()> {
        self.window(window)?;
        self.queue(Command::MoveToWorkspace(window.id, target))
    }

    fn switch_workspace_with_window(
        &mut self,
        window: WindowRef,
        target: usize,
        expected_revision: u64,
    ) -> FlameResult<()> {
        // One command, so the window and the active workspace never disagree: both mutations
        // happen in the same turn and nothing is rendered between them.
        self.window(window)?;
        let _ = expected_revision;
        self.queue(Command::SwitchWithWindow {
            id: window.id,
            target,
        })
    }

    fn apply_workspace_plan(&mut self, plan: &WorkspacePlan) -> FlameResult<()> {
        plan.validate()?;
        if matches!(plan.kind, WorkspacePlanKind::Activate) {
            return self.activate_workspace(plan.new_active, plan.expected_revision);
        }
        self.queue(Command::ApplyWorkspacePlan(plan.clone()))
    }
}

impl DisplayPort for WaylandDesktop {
    fn display_snapshot(&self) -> FlameResult<DisplaySnapshot> {
        let mirror = self.mirror.borrow();
        let mode = ModeId(1);
        let size = Size::new(mirror.output.width, mirror.output.height);
        Ok(DisplaySnapshot {
            generation: 1,
            outputs: vec![OutputSnapshot {
                id: Self::output_id(),
                connector: OUTPUT_NAME.to_owned(),
                edid_identity: String::new(),
                connected: true,
                primary: true,
                geometry: mirror.output,
                current_mode: mode,
                modes: vec![DisplayMode {
                    id: mode,
                    resolution: size,
                    refresh_millihz: 60_000,
                    preferred: true,
                }],
                shell_scale_percent: nested_output_scale_percent(),
                rotation: flamewm_api::display::Rotation::Normal,
            }],
            pending: None,
        })
    }

    fn apply_mode(&mut self, _output: &OutputId, _mode: ModeId) -> FlameResult<TransactionId> {
        Err(FlameError::unsupported(
            "nested compositor output mode is fixed by the host window",
        ))
    }

    fn keep_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
        Err(FlameError::unsupported(
            "nested compositor output mode is fixed by the host window",
        ))
    }

    fn revert_mode(&mut self, _transaction: TransactionId) -> FlameResult<()> {
        Err(FlameError::unsupported(
            "nested compositor output mode is fixed by the host window",
        ))
    }

    fn scale_keys(&self, _output: &OutputId, index: usize) -> Vec<String> {
        flamewm_api::ports::wayland_scale_keys(index)
    }
}

impl ShortcutPort for WaylandDesktop {
    fn prepare_shortcuts(&mut self, desired: &BTreeMap<String, KeyBinding>) -> FlameResult<()> {
        // Stage only: a prepare that is never committed must leave the live table alone.
        *self.staged_shortcuts.borrow_mut() = Some(desired.clone());
        Ok(())
    }

    fn commit_shortcuts(&mut self) -> FlameResult<()> {
        // The live table is rebuilt from the staged bindings on the next loop turn, because the
        // engine holds `State` and this port does not. Applying takes effect immediately after
        // that turn; nothing is silently dropped.
        let staged = self.staged_shortcuts.borrow_mut().take();
        match staged {
            Some(bindings) => self.queue(Command::SetShortcuts(bindings)),
            None => Err(FlameError::new(
                ErrorCode::EngineRejected,
                "commit_shortcuts without a prepared set",
            )),
        }
    }

    fn rollback_shortcuts(&mut self) {
        *self.staged_shortcuts.borrow_mut() = None;
    }
}

impl WorkAreaPort for WaylandDesktop {
    fn base_work_areas(&self) -> FlameResult<Vec<(OutputId, Rect)>> {
        let mirror = self.mirror.borrow();
        Ok(vec![(Self::output_id(), mirror.output)])
    }

    /// Panels reserve space through layer-shell and `_NET_WM_STRUT*`, which the compositor already
    /// folds into the work area. The shell's own reservations are kept so they can be reported back.
    fn apply_flame_reservations(&mut self, _reservations: &[(OutputId, Rect)]) -> FlameResult<()> {
        Ok(())
    }

    fn request_recompute(&mut self) {}
}

impl MainLoopPort for WaylandDesktop {
    fn add_poll(
        &mut self,
        _fd: i32,
        _events: FdEvents,
        _callback: FdCallback,
    ) -> FlameResult<FdHandle> {
        Err(FlameError::unsupported(
            "extra descriptors are driven by the compositor loop, not the engine",
        ))
    }

    fn remove_poll(&mut self, _handle: FdHandle) {}

    fn add_timer(
        &mut self,
        _delay_ms: u64,
        _callback: TimerCallback,
        _repeat: bool,
    ) -> FlameResult<TimerHandle> {
        Err(FlameError::unsupported(
            "engine timers are driven by the compositor loop, not the engine",
        ))
    }

    fn remove_timer(&mut self, _handle: TimerHandle) {}

    fn defer(&mut self, _callback: TimerCallback) -> FlameResult<TimerHandle> {
        Err(FlameError::unsupported(
            "engine deferred callbacks are not available on the Wayland compositor",
        ))
    }
}

impl InputPort for WaylandDesktop {
    fn root_pointer(&self) -> FlameResult<PointerPosition> {
        let mirror = self.mirror.borrow();
        Ok(PointerPosition {
            root: flamewm_api::Point {
                x: mirror.pointer.0,
                y: mirror.pointer.1,
            },
            output: Some(Self::output_id()),
        })
    }
}

impl ApplicationPort for WaylandDesktop {
    fn launch(
        &mut self,
        app: &DesktopAppId,
        options: &ApplicationLaunchOptions,
    ) -> FlameResult<()> {
        // Shared catalog with refresh-on-miss: an app installed after WM start still launches.
        let argv = flamewm_applications::with_application_entry(app, |entry| entry.argv(options))?;
        self.queue(Command::Spawn(argv))
    }

    fn launch_uri(&mut self, uri: &str) -> FlameResult<()> {
        self.queue(Command::Spawn(vec!["xdg-open".to_owned(), uri.to_owned()]))
    }
}

impl SessionPort for WaylandDesktop {
    fn session_capabilities(&self) -> SessionCapabilities {
        let power = power_program().is_some();
        SessionCapabilities {
            // The bundled lock screen is an X11 grab, which cannot secure native Wayland clients.
            // Not advertised until a compositor-owned session lock exists.
            lock: false,
            logout: logout_supported(),
            suspend: power,
            reboot: power,
            shutdown: power,
        }
    }

    fn perform_session_action(&mut self, action: SessionAction) -> FlameResult<()> {
        match action {
            SessionAction::Logout if logout_supported() => self.queue(Command::Logout),
            SessionAction::Logout => Err(FlameError::new(
                ErrorCode::Unavailable,
                "logout requires a managed FlameWM session",
            )),
            SessionAction::Suspend | SessionAction::Reboot | SessionAction::Shutdown => {
                let program = power_program().ok_or_else(|| {
                    FlameError::new(ErrorCode::Unavailable, "systemctl is not installed")
                })?;
                let verb = match action {
                    SessionAction::Suspend => "suspend",
                    SessionAction::Reboot => "reboot",
                    _ => "poweroff",
                };
                // Never block the single-threaded compositor while logind authorizes the action:
                // a successful spawn reports dispatch, not completion of the power transition.
                std::process::Command::new(program)
                    .arg(verb)
                    .spawn()
                    .map(|_| ())
                    .map_err(|error| {
                        FlameError::new(
                            ErrorCode::IoFailure,
                            format!("session action launch failed: {error}"),
                        )
                    })
            }
            SessionAction::Lock
            | SessionAction::LockPreview
            | SessionAction::ScreensaverPreview => Err(FlameError::unsupported(
                "the Wayland compositor has no session lock or screensaver yet",
            )),
        }
    }
}

impl BackgroundPort for WaylandDesktop {
    fn project_background(&mut self, _state: &BackgroundState) -> FlameResult<()> {
        Err(FlameError::unsupported(
            "the Wayland compositor does not project wallpapers yet",
        ))
    }

    fn reload_background(&mut self) -> FlameResult<()> {
        Err(FlameError::unsupported(
            "the Wayland compositor does not project wallpapers yet",
        ))
    }
}

impl TrayPort for WaylandDesktop {
    fn set_tray_owner(&mut self, _output: &OutputId) -> FlameResult<()> {
        Err(FlameError::unsupported(
            "the Wayland compositor does not implement system tray ownership yet",
        ))
    }

    fn clear_tray_owner(&mut self) -> FlameResult<()> {
        Err(FlameError::unsupported(
            "the Wayland compositor does not implement system tray ownership yet",
        ))
    }

    fn tray_owner(&self) -> FlameResult<Option<OutputId>> {
        Ok(None)
    }
}

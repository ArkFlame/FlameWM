use super::*;

impl WindowPort for X11Desktop {
    fn get(&self, reference: WindowRef) -> FlameResult<flamewm_api::window::WindowSnapshot> {
        let window = self.check_window(reference)?;
        let tree = self
            .conn
            .query_tree(window)
            .map_err(FlameError::io)?
            .reply()
            .map_err(FlameError::io)?;
        let atoms = self.window_atoms();
        let reads = self.send_window_reads(window, tree.parent, &atoms);
        let frame = self.frame(&atoms)?;
        Ok(self.finish_window(reads, &atoms, &frame, reference)?.0)
    }
    fn snapshot(&self) -> FlameResult<Vec<flamewm_api::window::WindowSnapshot>> {
        let tree = self.managed_tree()?;
        let atoms = self.window_atoms();
        let reads: Vec<WindowReads<'_>> = tree
            .iter()
            .map(|&(window, parent)| self.send_window_reads(window, parent, &atoms))
            .collect();
        // A failed shared read drops every window, as the per-window `get` did.
        let Ok(frame) = self.frame(&atoms) else {
            return Ok(Vec::new());
        };
        Ok(reads
            .into_iter()
            .filter_map(|reads| {
                let reference = WindowRef::new(u64::from(reads.window), 0);
                self.finish_window(reads, &atoms, &frame, reference)
                    .ok()
                    .filter(|(_, named)| *named)
                    .map(|(window, _)| window)
            })
            .collect())
    }
    fn activate(&mut self, w: WindowRef) -> FlameResult<()> {
        self.client_message(
            self.check_window(w)?,
            "_NET_ACTIVE_WINDOW",
            [2, CURRENT_TIME, 0, 0, 0],
        )
    }
    fn minimize(&mut self, w: WindowRef) -> FlameResult<()> {
        let window = self.check_window(w)?;
        let (target, name, data) = Self::minimize_message(window);
        self.client_message(target, name, data)
    }
    fn maximize(&mut self, w: WindowRef) -> FlameResult<()> {
        let window = self.check_window(w)?;
        self.send_state(
            window,
            self.atom("_NET_WM_STATE_MAXIMIZED_VERT"),
            FeatureAction::Add,
        )?;
        self.send_state(
            window,
            self.atom("_NET_WM_STATE_MAXIMIZED_HORZ"),
            FeatureAction::Add,
        )
    }
    fn restore(&mut self, w: WindowRef) -> FlameResult<()> {
        let window = self.check_window(w)?;
        self.send_state(
            window,
            self.atom("_NET_WM_STATE_MAXIMIZED_VERT"),
            FeatureAction::Remove,
        )?;
        self.send_state(
            window,
            self.atom("_NET_WM_STATE_MAXIMIZED_HORZ"),
            FeatureAction::Remove,
        )
    }
    fn close(&mut self, w: WindowRef) -> FlameResult<()> {
        self.client_message(
            self.check_window(w)?,
            "_NET_CLOSE_WINDOW",
            [CURRENT_TIME, 2, 0, 0, 0],
        )
    }
    fn set_outer_geometry(&mut self, w: WindowRef, geometry: Rect) -> FlameResult<()> {
        let geometry = shared::window_geometry_aux(geometry)?;
        self.conn
            .configure_window(self.check_window(w)?, &geometry)
            .map_err(FlameError::io)?
            .check()
            .map_err(FlameError::io)?;
        self.flush()
    }
    fn work_area(&self, _w: WindowRef) -> FlameResult<Rect> {
        self.root_work_area()
    }
    fn output(&self, w: WindowRef) -> FlameResult<OutputId> {
        Ok(self.get(w)?.output)
    }
    fn feature_snapshot(&self, w: WindowRef) -> FlameResult<WindowFeatureSnapshot> {
        let states = self.property32(self.check_window(w)?, "_NET_WM_STATE")?;
        Ok(shared::feature_snapshot(&states, |name| self.atom(name)))
    }
    fn apply_window_feature(
        &mut self,
        w: WindowRef,
        feature: WindowFeature,
        action: FeatureAction,
    ) -> FlameResult<()> {
        let atom = match feature {
            WindowFeature::Sticky => "_NET_WM_STATE_STICKY",
            WindowFeature::Modal => "_NET_WM_STATE_MODAL",
            WindowFeature::Shaded => "_NET_WM_STATE_SHADED",
            WindowFeature::SkipTaskbar => "_NET_WM_STATE_SKIP_TASKBAR",
            WindowFeature::SkipPager => "_NET_WM_STATE_SKIP_PAGER",
            WindowFeature::Above => "_NET_WM_STATE_ABOVE",
            WindowFeature::Below => "_NET_WM_STATE_BELOW",
            WindowFeature::DemandsAttention => "_NET_WM_STATE_DEMANDS_ATTENTION",
        };
        self.send_state(self.check_window(w)?, self.atom(atom), action)
    }
}

impl WorkspacePort for X11Desktop {
    fn workspace_snapshot(&self) -> FlameResult<WorkspaceSnapshot> {
        let count = self
            .property32(self.root, "_NET_NUMBER_OF_DESKTOPS")?
            .first()
            .copied()
            .unwrap_or(1) as usize;
        let active_index = self
            .property32(self.root, "_NET_CURRENT_DESKTOP")?
            .first()
            .copied()
            .unwrap_or(0) as usize;
        let bytes = self.property_bytes(self.root, "_NET_DESKTOP_NAMES")?;
        Ok(shared::workspace_snapshot(count, active_index, &bytes, 0))
    }
    fn activate_workspace(&mut self, index: usize, expected_revision: u64) -> FlameResult<()> {
        let current = self.workspace_snapshot()?;
        if expected_revision != current.revision {
            return Err(FlameError::stale("workspace revision changed"));
        }
        if index >= current.count {
            return Err(FlameError::invalid("workspace index is outside topology"));
        }
        self.client_message(
            self.root,
            "_NET_CURRENT_DESKTOP",
            [index as u32, CURRENT_TIME, 0, 0, 0],
        )
    }
    fn move_window_to_workspace(&mut self, w: WindowRef, target: usize) -> FlameResult<()> {
        let current = self.workspace_snapshot()?;
        if target >= current.count {
            return Err(FlameError::invalid("workspace index is outside topology"));
        }
        self.client_message(
            self.check_window(w)?,
            "_NET_WM_DESKTOP",
            [target as u32, CURRENT_TIME, 0, 0, 0],
        )
    }
    fn switch_workspace_with_window(
        &mut self,
        _w: WindowRef,
        _target: usize,
        _revision: u64,
    ) -> FlameResult<()> {
        Err(FlameError::unsupported(
            "X11 EWMH has no atomic workspace-and-window transaction",
        ))
    }
    fn apply_workspace_plan(&mut self, plan: &WorkspacePlan) -> FlameResult<()> {
        plan.validate()?;
        if matches!(plan.kind, WorkspacePlanKind::Activate) {
            return self.activate_workspace(plan.new_active, plan.expected_revision);
        }
        apply_workspace_plan_to_root(&self.conn, self.root, plan)
    }
}

impl InputPort for X11Desktop {
    fn root_pointer(&self) -> FlameResult<PointerPosition> {
        shared::root_pointer(&self.conn, self.root, || self.display_snapshot())
    }
}

impl ApplicationPort for X11Desktop {
    fn launch(
        &mut self,
        app: &DesktopAppId,
        options: &ApplicationLaunchOptions,
    ) -> FlameResult<()> {
        let catalog = self.application_catalog()?;
        catalog.launch(app, options)
    }
    fn launch_uri(&mut self, uri: &str) -> FlameResult<()> {
        flamewm_applications::open_target(std::ffi::OsStr::new(uri))
    }
}

impl SessionPort for X11Desktop {
    fn session_capabilities(&self) -> SessionCapabilities {
        SessionCapabilities {
            lock: Path::new("/usr/bin/loginctl").exists(),
            logout: session_logout_supported(),
            suspend: Path::new("/usr/bin/systemctl").exists(),
            reboot: Path::new("/usr/bin/systemctl").exists(),
            shutdown: Path::new("/usr/bin/systemctl").exists(),
        }
    }
    fn perform_session_action(&mut self, action: SessionAction) -> FlameResult<()> {
        let (program, args): (&str, &[&str]) = match action {
            SessionAction::Lock => return start_lock_screen(),
            SessionAction::LockPreview => return start_lock_preview(),
            SessionAction::ScreensaverPreview => return start_screensaver_preview(),
            SessionAction::Logout => {
                if !session_logout_supported() {
                    return Err(FlameError::new(
                        ErrorCode::Unavailable,
                        "logout requires a managed FlameWM session",
                    ));
                }
                request_session_logout();
                return Ok(());
            }
            SessionAction::Suspend => ("systemctl", &["suspend"]),
            SessionAction::Reboot => ("systemctl", &["reboot"]),
            SessionAction::Shutdown => ("systemctl", &["poweroff"]),
        };
        Command::new(program)
            .args(args)
            .status()
            .map_err(FlameError::io)?
            .success()
            .then_some(())
            .ok_or_else(|| {
                FlameError::new(ErrorCode::PermissionDenied, "session action command failed")
            })
    }
}

impl MainLoopPort for X11Desktop {
    fn add_poll(
        &mut self,
        _fd: i32,
        _events: FdEvents,
        _callback: FdCallback,
    ) -> FlameResult<FdHandle> {
        Err(FlameError::unsupported("desktop host owns X11 event loop"))
    }
    fn remove_poll(&mut self, _handle: FdHandle) {}
    fn add_timer(
        &mut self,
        _delay_ms: u64,
        _callback: TimerCallback,
        _repeat: bool,
    ) -> FlameResult<TimerHandle> {
        Err(FlameError::unsupported("desktop host owns timers"))
    }
    fn remove_timer(&mut self, _handle: TimerHandle) {}
    fn defer(&mut self, _callback: TimerCallback) -> FlameResult<TimerHandle> {
        Err(FlameError::unsupported(
            "desktop host owns deferred callbacks",
        ))
    }
}
impl BackgroundPort for X11Desktop {
    fn project_background(&mut self, _state: &BackgroundState) -> FlameResult<()> {
        Err(FlameError::unsupported(
            "X11 desktop adapter does not own wallpaper rendering",
        ))
    }
    fn reload_background(&mut self) -> FlameResult<()> {
        Err(FlameError::unsupported(
            "X11 desktop adapter does not own wallpaper rendering",
        ))
    }
}
impl TrayPort for X11Desktop {
    fn set_tray_owner(&mut self, _output: &OutputId) -> FlameResult<()> {
        Err(FlameError::unsupported(
            "X11 desktop adapter does not implement system tray ownership",
        ))
    }
    fn clear_tray_owner(&mut self) -> FlameResult<()> {
        Err(FlameError::unsupported(
            "X11 desktop adapter does not implement system tray ownership",
        ))
    }
    fn tray_owner(&self) -> FlameResult<Option<OutputId>> {
        Err(FlameError::unsupported(
            "X11 desktop adapter does not implement system tray ownership",
        ))
    }
}
impl WorkAreaPort for X11Desktop {
    fn base_work_areas(&self) -> FlameResult<Vec<(OutputId, Rect)>> {
        let work_area = self.root_work_area()?;
        let outputs = self.display_snapshot()?;
        Ok(shared::base_work_areas(work_area, &outputs.outputs))
    }
    fn apply_flame_reservations(&mut self, _reservations: &[(OutputId, Rect)]) -> FlameResult<()> {
        Err(FlameError::unsupported(
            "X11 desktop adapter does not own work-area reservations",
        ))
    }
    fn request_recompute(&mut self) {}
}

impl DisplayPort for X11Desktop {
    fn display_snapshot(&self) -> FlameResult<DisplaySnapshot> {
        let resources = self
            .conn
            .randr_get_screen_resources_current(self.root)
            .map_err(FlameError::io)?
            .reply()
            .map_err(FlameError::io)?;
        // Stable physical output identity: never infer primary from RandR
        // resource enumeration order (it can change after hotplug/rotation).
        let primary_output = self
            .conn
            .randr_get_output_primary(self.root)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map_or(0, |reply| reply.output);
        let mut outputs = Vec::new();
        for output in resources.outputs {
            let info = self
                .conn
                .randr_get_output_info(output, resources.config_timestamp)
                .map_err(FlameError::io)?
                .reply()
                .map_err(FlameError::io)?;
            // X lists any output scanning out from a CRTC as a monitor, even when the
            // driver cannot sense a sink (virtual heads, some docks). Follow that rule.
            if u8::from(info.connection) != u8::from(randr::Connection::CONNECTED) && info.crtc == 0
            {
                continue;
            }
            let crtc = info.crtc;
            let (geometry, current_mode, rotation) = if crtc != 0 {
                let c = self
                    .conn
                    .randr_get_crtc_info(crtc, resources.config_timestamp)
                    .map_err(FlameError::io)?
                    .reply()
                    .map_err(FlameError::io)?;
                (
                    Rect::new(
                        i32::from(c.x),
                        i32::from(c.y),
                        i32::from(c.width),
                        i32::from(c.height),
                    ),
                    ModeId(u64::from(c.mode)),
                    crate::topology::rotation_from_bit(c.rotation),
                )
            } else {
                (
                    Rect::default(),
                    ModeId(0),
                    flamewm_api::display::Rotation::Normal,
                )
            };
            let modes = shared::display_modes(&info.modes, &resources.modes);
            outputs.push(OutputSnapshot {
                id: OutputId::new(String::from_utf8_lossy(&info.name).into_owned()),
                connector: String::from_utf8_lossy(&info.name).into_owned(),
                edid_identity: String::new(),
                connected: true,
                primary: output == primary_output,
                geometry,
                current_mode,
                modes,
                shell_scale_percent: 100,
                rotation,
            });
        }
        // Some nested and legacy X servers do not implement RandR primary.
        // In that case stabilize fallback by connector, not enumeration order.
        if !outputs.iter().any(|output| output.primary) {
            outputs.sort_by(|a, b| a.connector.cmp(&b.connector));
            if let Some(output) = outputs.iter_mut().find(|output| output.geometry.is_valid()) {
                output.primary = true;
            }
        }
        Ok(DisplaySnapshot {
            generation: u64::from(resources.config_timestamp),
            outputs,
            pending: None,
        })
    }
    fn apply_mode(&mut self, output_name: &OutputId, mode: ModeId) -> FlameResult<TransactionId> {
        if self.pending_topology.is_some() {
            return Err(FlameError::new(
                ErrorCode::Busy,
                "RandR topology transaction already pending",
            ));
        }
        shared::apply_mode(
            &self.conn,
            self.root,
            &mut self.pending_mode,
            &mut self.next_transaction,
            output_name,
            mode,
        )
    }
    fn apply_topology(&mut self, change: &TopologyChange) -> FlameResult<TransactionId> {
        if self.pending_mode.is_some() || self.pending_topology.is_some() {
            return Err(FlameError::new(
                ErrorCode::Busy,
                "RandR display transaction already pending",
            ));
        }
        let baseline = topology::apply(&self.conn, self.root, change)?;
        self.next_transaction = self.next_transaction.saturating_add(1).max(1);
        let transaction = TransactionId(self.next_transaction);
        self.pending_topology = Some(topology::PendingTopology {
            transaction,
            baseline,
        });
        Ok(transaction)
    }
    fn keep_mode(&mut self, transaction: TransactionId) -> FlameResult<()> {
        if self
            .pending_topology
            .take_if(|p| p.transaction == transaction)
            .is_some()
        {
            return Ok(());
        }
        if self.pending_mode.map(|p| p.transaction) != Some(transaction) {
            return Err(FlameError::not_found("RandR mode transaction not found"));
        }
        self.pending_mode = None;
        Ok(())
    }
    fn revert_mode(&mut self, transaction: TransactionId) -> FlameResult<()> {
        if let Some(pending) = self
            .pending_topology
            .take_if(|p| p.transaction == transaction)
        {
            return topology::restore(&self.conn, self.root, &pending.baseline);
        }
        let pending = self
            .pending_mode
            .ok_or_else(|| FlameError::not_found("RandR mode transaction not found"))?;
        if pending.transaction != transaction {
            return Err(FlameError::not_found("RandR mode transaction not found"));
        }
        // The product coordinator retires the transaction before this call, so the adapter must
        // retire it too even when RandR refuses or fails the rollback; otherwise apply_mode
        // returns Busy for every later mode change.
        self.pending_mode = None;
        let resources = self
            .conn
            .randr_get_screen_resources_current(self.root)
            .map_err(FlameError::io)?
            .reply()
            .map_err(FlameError::io)?;
        let status = self
            .conn
            .randr_set_crtc_config(
                pending.crtc,
                CURRENT_TIME,
                resources.config_timestamp,
                pending.old_x,
                pending.old_y,
                pending.old_mode,
                pending.old_rotation,
                &[pending.output],
            )
            .map_err(FlameError::io)?
            .reply()
            .map_err(FlameError::io)?
            .status;
        if u8::from(status) != u8::from(randr::SetConfig::SUCCESS) {
            return Err(FlameError::new(
                ErrorCode::EngineRejected,
                "RandR rejected mode rollback",
            ));
        }
        self.flush()
    }
}

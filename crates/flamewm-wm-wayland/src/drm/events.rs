use super::*;

/// Paints through the shared compositor if it is not already in use.
///
/// Every caller goes through here because the compositor is behind a `RefCell`: a request that
/// finds it borrowed means a paint is already running on this thread, and the request is left in
/// `state`'s dirty flag so that in-flight pass or the next vblank picks it up.
pub(super) fn render_all(state: &mut State, drm: &Rc<RefCell<Drm>>) {
    let Ok(mut d) = drm.try_borrow_mut() else {
        return;
    };
    d.paint(state, drm);
}

/// The one bounded deadline for the first presented frame fired.
///
/// Nothing else in the backend ends a startup on its own, so this is the only way out of a session
/// that owns DRM master and has nothing on the panel. It names why — `no_first_queue` when not even a
/// flip was queued, `pageflip_timeout` when one was queued and never came back (the black-panel case)
/// — with the counters, the CRTC, the elapsed milliseconds and the last render error, then stops the
/// loop so `main` exits non-zero and the backend is torn down. Input was never started at this point,
/// so the keyboard still reaches the kernel either way.
pub(super) fn on_first_frame_deadline(state: &mut State, drm: &Rc<RefCell<Drm>>) -> TimeoutAction {
    let Ok(mut d) = drm.try_borrow_mut() else {
        // A paint is in flight on this thread; it cannot be, but never block the loop on it.
        return TimeoutAction::Drop;
    };
    d.first_frame_deadline = None;
    if d.first_frame.ready() {
        // The presented frame already cancelled this timer; a late fire changes nothing.
        return TimeoutAction::Drop;
    }
    let elapsed = Duration::from_millis(u64::try_from(elapsed_ms()).unwrap_or(u64::MAX));
    let reason = d.first_frame.failure(elapsed);
    if reason == NO_FAILURE {
        // The session was paused away: that is not a wedged GPU, and a resume arms a fresh deadline.
        return TimeoutAction::Drop;
    }
    let queued = d.first_frame.queued_count();
    let presented = d.first_frame.presented_count();
    let crtc = match d.first_frame.queued_crtc() {
        Some(crtc) => crtc.to_string(),
        None => "none".to_owned(),
    };
    let error = d.first_frame.last_error().unwrap_or("none").to_owned();
    eprintln!(
        "flamewm-wayland: first frame not presented detail={reason} queued={queued} \
         presented={presented} crtc={crtc} elapsed={}ms last_render_error={error}",
        elapsed.as_millis()
    );
    if queued == 0 {
        eprintln!(
            "flamewm-wayland: nothing reached KMS (session or output unusable); giving the VT back"
        );
    } else {
        eprintln!(
            "flamewm-wayland: the flip was queued and never completed (black panel); giving the VT back"
        );
    }
    // The stage that failed is the frame marker, not `ready`: the event loop did start. A reader (and
    // the session wrapper's failure report) greps the last `status=fail` line for the reason.
    stage_line(
        Stage::Firstframe,
        StageStatus::Fail,
        &d.first_frame.failure_detail(reason, elapsed),
    );
    // The steps that did complete, with `present=none`: this is what a hardware log needs.
    d.timing.emit();
    state.report_startup_failure(reason);
    state.loop_signal.stop();
    TimeoutAction::Drop
}

pub(super) fn on_drm_event(
    state: &mut State,
    drm: &Rc<RefCell<Drm>>,
    event: DrmEvent,
    vblank: Option<DrmEventMetadata>,
) {
    match event {
        DrmEvent::VBlank(crtc) => {
            let Ok(mut d) = drm.try_borrow_mut() else {
                // A paint is in flight and replans this output itself.
                return;
            };
            d.on_vblank(state, crtc, drm, vblank);
        }
        DrmEvent::Error(error) => eprintln!("flamewm-wayland: drm error: {error}"),
    }
}

/// One logind event. Pauses stop scanout and then answer logind; resumes reclaim the device.
pub(super) fn on_logind_event(state: &mut State, drm: &Rc<RefCell<Drm>>, event: LogindEvent) {
    match event {
        LogindEvent::Pause { major, minor, kind } => {
            {
                let mut d = drm.borrow_mut();
                if !pauses_scanout(d.primary_dev, major, minor) {
                    // An input device: logind revokes its evdev descriptor and libinput drops it.
                    // logind also sends `gone` for every `ReleaseDevice`, e.g. each node libinput
                    // opens and closes again while probing the seat, so this must never touch
                    // scanout (wlroots and libseat filter on the DRM device the same way).
                    if kind.needs_ack() {
                        if let Err(error) = d.session.ack_pause(major, minor) {
                            eprintln!("flamewm-wayland: pause ack {major}:{minor} failed: {error}");
                        }
                    }
                    return;
                }
                d.quiesce();
                // The reply is owed only for `pause`: logind waits for it to finish a VT switch,
                // so it has to leave this callback after scanout stopped. `force` and `gone` are
                // already revoked by logind and must not be answered.
                if kind.needs_ack() {
                    if let Err(error) = d.session.ack_pause(major, minor) {
                        eprintln!("flamewm-wayland: pause ack {major}:{minor} failed: {error}");
                    }
                }
                if kind == PauseKind::Gone {
                    drop_outputs(&mut d, state);
                }
            }
        }
        LogindEvent::Resume { major, minor, fd } => resume_device(state, drm, major, minor, fd),
        LogindEvent::ActivateSession => {
            let resumed = {
                let mut d = drm.borrow_mut();
                match kept_fd_resume(&mut d, state, drm) {
                    Ok(()) => true,
                    Err(error) => {
                        eprintln!(
                            "flamewm-wayland: session resume without a new fd failed: {error}"
                        );
                        false
                    }
                }
            };
            if resumed {
                stage_line(Stage::Drm, StageStatus::Ok, "session resumed");
                render_all(state, drm);
            }
        }
    }
}

/// Reclaims the DRM device we already hold. Cheap path: the descriptor logind hands back belongs to
/// the same card (`dev_t` matched by the caller), so the existing device, GBM device, EGL stack and
/// compositors are kept and only master and scanout state are re-established.
///
/// Master is re-taken through the device we hold: this smithay version exposes no
/// `ControlDevice::set_master` on `DrmDevice` (E0599), so `activate` on the existing fd is the
/// operation that fails on a revoked descriptor — and that failure is what routes the resume to
/// Frame callbacks for every surface that can be on screen; same set as the nested backend.
pub(super) fn send_frame_callbacks(state: &State) {
    let time = Duration::from_millis(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    );
    let throttle = Some(Duration::from_millis(16));
    let output = &state.output;
    for (_, top) in &state.toplevels {
        let surface = top.wl_surface();
        send_frames_surface_tree(surface, output, time, throttle, |_, _| Some(output.clone()));
        for (popup, _) in PopupManager::popups_for_surface(surface) {
            send_frames_surface_tree(popup.wl_surface(), output, time, throttle, |_, _| {
                Some(output.clone())
            });
        }
    }
    let x11 = state
        .x11
        .iter()
        .map(|(_, window)| window)
        .chain(state.unmanaged.iter().map(|entry| &entry.surface));
    for surface in x11.filter_map(smithay::xwayland::X11Surface::wl_surface) {
        send_frames_surface_tree(&surface, output, time, throttle, |_, _| {
            Some(output.clone())
        });
    }
    let layers: Vec<_> = layer_map_for_output(output).layers().cloned().collect();
    for layer in layers {
        send_frames_surface_tree(
            layer.layer_surface().wl_surface(),
            output,
            time,
            throttle,
            |_, _| Some(output.clone()),
        );
    }
    render::send_cursor_frame(state, time);
}

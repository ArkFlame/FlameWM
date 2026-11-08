use super::*;

/// The error type is `Box<dyn Error>` because this crate has no `anyhow` dependency; `main` only
/// needs `Display` for its stage line.
pub fn init(
    state: &mut State,
    handle: &LoopHandle<'static, State>,
) -> Result<DrmHandle, Box<dyn Error>> {
    let mut timing = StartupTiming::default();
    match init_backend(state, handle, &mut timing) {
        Ok(drm) => Ok(drm),
        Err(error) => {
            timing.emit();
            Err(error)
        }
    }
}

/// The body of [`init`]; `timing` is filled in step by step so a failure can still report it.
pub(super) fn init_backend(
    state: &mut State,
    handle: &LoopHandle<'static, State>,
    timing: &mut StartupTiming,
) -> Result<DrmHandle, Box<dyn Error>> {
    // 1. logind session: open, activate, wait for `Active`.
    let step = Instant::now();
    let session = match LogindSession::new() {
        Ok(session) => session,
        Err(error) => {
            stage_line(Stage::Seat, StageStatus::Fail, &format!("{error}"));
            return Err(error.into());
        }
    };
    timing.session_ms = step.elapsed().as_millis();
    stage_line(
        Stage::Seat,
        StageStatus::Ok,
        &format!("logind_control elapsed_{}ms", timing.session_ms),
    );
    // Belt and braces after the fail-closed activation in `LogindSession::new`: taking DRM master on
    // an inactive session blanks a VT that belongs to the greeter, no frame is ever presented and the
    // console cannot be reached. Refuse before any GPU is opened.
    if !session.is_active() {
        stage_line(
            Stage::Seat,
            StageStatus::Fail,
            "inactive_refusing_drm_master",
        );
        return Err("logind session is not active; refusing to take DRM master".into());
    }
    let seat = session.seat();

    // 2. GPU: logind `TakeDevice` plus DRM master on the card that can drive the panel.
    let step = Instant::now();
    let (gpu, gpu_detail) = device::open_scanout_gpu(&session, &seat)?;
    timing.gpu_ms = step.elapsed().as_millis();
    let device::OpenedGpu {
        name: gpu_name,
        dev: primary_dev,
        device,
        fd: gpu_fd,
        notifier: drm_notifier,
    } = gpu;
    stage_line(
        Stage::Drm,
        StageStatus::Ok,
        &format!("{gpu_name} master {gpu_detail} elapsed_{}ms", timing.gpu_ms),
    );

    // 3. GBM device for buffer allocation.
    let step = Instant::now();
    let gbm = GbmDevice::new(gpu_fd)?;
    timing.gbm_ms = step.elapsed().as_millis();
    stage_line(
        Stage::Drm,
        StageStatus::Ok,
        &format!("gbm elapsed_{}ms", timing.gbm_ms),
    );

    // 4. EGL display and context on that GBM device.
    let step = Instant::now();
    eprintln!("flamewm-wayland: init phase=egl_display");
    // SAFETY: the EGL display is created on the GBM device owned by this compositor and lives as
    // long as the renderer in `Drm`, which is the only user of the context.
    let egl = unsafe { EGLDisplay::new(gbm.clone())? };
    eprintln!("flamewm-wayland: init phase=egl_context");
    let context = EGLContext::new(&egl)?;
    timing.egl_ms = step.elapsed().as_millis();
    stage_line(
        Stage::Drm,
        StageStatus::Ok,
        &format!("egl_display_context elapsed_{}ms", timing.egl_ms),
    );

    // 5. GLES renderer: the step that loads the GL entry points and is slow on real hardware.
    let step = Instant::now();
    eprintln!("flamewm-wayland: init phase=renderer");
    // SAFETY: the context is current only on this thread, the single compositor thread.
    let mut renderer = unsafe { GlowRenderer::new(context)? };
    crate::render::log_gl_renderer(&mut renderer, "drm");
    let formats: Vec<Format> = renderer.dmabuf_formats().into_iter().collect();
    timing.renderer_ms = step.elapsed().as_millis();
    stage_line(
        Stage::Drm,
        StageStatus::Ok,
        &format!("renderer elapsed_{}ms", timing.renderer_ms),
    );

    let udev_backend = UdevBackend::new(&seat)?;

    let drm = Rc::new(RefCell::new(Drm {
        session,
        device,
        gbm,
        renderer,
        formats,
        // Deliberately not opened yet: see the module docs on input.
        libinput: None,
        seat,
        keyboards: Vec::new(),
        leds: smithay::input::keyboard::LedState::default(),
        gpu_name,
        primary_dev,
        surfaces: Vec::new(),
        paused: false,
        gate: None,
        retry_scheduled: false,
        retry_attempts: 0,
        scene: None,
        first_frame: FirstFrameGate::new(),
        first_frame_deadline: None,
        timing: *timing,
        sync_defer_logged: false,
        sync_fail_logged: false,
        handle: handle.clone(),
    }));
    {
        // 6. Connectors: the first frame can only be rendered once these exist.
        let step = Instant::now();
        let mut d = drm.borrow_mut();
        // Only a device-level failure is fatal here: a single connector that cannot be set up is
        // logged and skipped, like anvil does.
        if let Err(error) = scan_connectors(&mut d, state) {
            stage_line(Stage::Outputs, StageStatus::Fail, &format!("{error}"));
            return Err(error);
        }
        d.timing.scan_ms = step.elapsed().as_millis();
        timing.scan_ms = d.timing.scan_ms;
        if scan_failed(d.surfaces.len()) {
            stage_line(Stage::Outputs, StageStatus::Fail, "no_connector");
            return Err("no connected connector with a usable mode".into());
        }
    }
    let outputs = drm.borrow().surfaces.len();
    stage_line(
        Stage::Outputs,
        StageStatus::Ok,
        &format!(
            "{outputs} out {}x{} elapsed_{}ms",
            state.output_size.w, state.output_size.h, timing.scan_ms
        ),
    );

    let drm_events = Rc::clone(&drm);
    handle.insert_source(drm_notifier, move |event, metadata, state| {
        on_drm_event(state, &drm_events, event, metadata.take());
    })?;

    // logind signals (VT pause and resume) arrive on the system bus. The bus fd wakes the loop only
    // when logind has data, so an idle session costs no wakeups. The events are queued by the
    // session and dispatched here, outside the session borrow.
    let session_fd = drm
        .borrow()
        .session
        .bus_fd()
        .ok_or("logind bus has no descriptor")?;
    let session_drm = Rc::clone(&drm);
    handle.insert_source(
        Generic::new(
            BorrowedRawFd(session_fd),
            Interest::READ,
            smithay::reexports::calloop::Mode::Level,
        ),
        move |_, _, state| {
            let events = {
                let d = session_drm.borrow();
                d.session.pump();
                d.session.take_events()
            };
            for event in events {
                on_logind_event(state, &session_drm, event);
            }
            Ok(PostAction::Continue)
        },
    )?;

    // A hotplug is a burst of uevents: the first one arms a single settle timer, and the rescan
    // runs once when the burst has been quiet for `HOTPLUG_SETTLE`. Events inside the burst cost
    // nothing beyond the flag.
    let udev_drm = Rc::clone(&drm);
    let udev_handle = handle.clone();
    let burst = Rc::new(RefCell::new(hotplug::HotplugDebounce::default()));
    handle.insert_source(udev_backend, move |event, (), _state| {
        if let UdevEvent::Changed { device_id } = event {
            let primary = udev_drm.borrow().primary_dev;
            if device_id == primary && burst.borrow_mut().event() {
                let settle_burst = Rc::clone(&burst);
                let settle_drm = Rc::clone(&udev_drm);
                let armed = udev_handle.insert_source(
                    Timer::from_duration(hotplug::HOTPLUG_SETTLE),
                    move |_, (), state| {
                        settle_burst.borrow_mut().fire();
                        if let Ok(mut d) = settle_drm.try_borrow_mut()
                            && let Err(error) = scan_connectors(&mut d, state)
                        {
                            eprintln!("flamewm-wayland: connector rescan failed: {error}");
                        }
                        // An unplugged or resized output changes the usable area: pull windows that
                        // now sit off-screen back in, the same way a nested host resize does.
                        state.reflow_if_work_area_changed();
                        state.request_full_redraw();
                        render_all(state, &settle_drm);
                        TimeoutAction::Drop
                    },
                );
                if let Err(error) = armed {
                    eprintln!("flamewm-wayland: hotplug settle timer failed: {error}");
                    burst.borrow_mut().fire();
                }
            }
        }
    })?;

    // The wake pipe: `State::request_full_redraw` pings it from anywhere (input, protocol
    // handlers, timers). The paint is deferred to an idle callback, which calloop runs only after
    // every source ready in this dispatch has been handled, so pointer and key events that arrived
    // together are applied before the frame that shows them.
    let (ping, ping_source) = make_ping()?;
    let waker_drm = Rc::clone(&drm);
    let idle = handle.clone();
    let paint_queued = Rc::new(std::cell::Cell::new(false));
    handle.insert_source(ping_source, move |(), (), _state| {
        if !paint_queued.replace(true) {
            let queued = Rc::clone(&paint_queued);
            let drm_idle = Rc::clone(&waker_drm);
            idle.insert_idle(move |state| {
                queued.set(false);
                render_all(state, &drm_idle);
            });
        }
    })?;
    state.set_drm_waker(ping);

    // The bounded deadline for the first *presented* frame. It is armed here, before anything is
    // painted, and only a completed flip removes it: a queue must never disarm it, because the
    // symptom this guards against is a flip that was queued and never came back.
    {
        let drm_handle = Rc::clone(&drm);
        let mut d = drm.borrow_mut();
        d.arm_first_frame_deadline(&drm_handle);
    }

    // 7. Input before the first frame, like anvil: `TakeControl` already switched the VT keyboard
    // to K_OFF, so without libinput nothing — not even Ctrl+Alt+Fn — reaches anyone.
    {
        let input_drm = Rc::clone(&drm);
        drm.borrow_mut().start_input(&input_drm);
    }

    // 8. The first frame.
    state.request_full_redraw();
    render_all(state, &drm);
    Ok(DrmHandle(drm))
}

/// Releases the session: input and scanout are stopped, then logind gets control back. Idempotent
/// and borrow-safe, so it can run from `Drop` on a normal exit, an error path or a panic.
pub fn finish(handle: &DrmHandle) {
    let Ok(mut d) = handle.0.try_borrow_mut() else {
        return;
    };
    d.quiesce();
    if let Err(error) = d.session.release_control() {
        eprintln!("flamewm-wayland: logind ReleaseControl failed: {error}");
    }
}

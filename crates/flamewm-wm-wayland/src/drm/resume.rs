use super::*;

/// the rebuild path instead of pretending the device works.
pub(super) fn kept_fd_resume(
    d: &mut Drm,
    state: &mut State,
    drm: &Rc<RefCell<Drm>>,
) -> Result<(), Box<dyn Error>> {
    d.device.activate(false)?;
    d.resume_input();
    for out in &mut d.surfaces {
        out.frames.wake();
        if let Err(error) = out.compositor.reset_state() {
            eprintln!(
                "flamewm-wayland: reset failed on {}: {error}",
                out.output.name()
            );
        }
    }
    d.paused = false;
    d.retry_scheduled = false;
    d.retry_attempts = 0;
    // The session was taken away, so any flip that was in flight is gone and the deadline it
    // consumed says nothing about this one: a resumed startup that never presents gets a fresh,
    // equally bounded one.
    d.first_frame.resume();
    d.arm_first_frame_deadline(drm);
    state.request_full_redraw();
    Ok(())
}

/// Whether a logind `PauseDevice` for `major:minor` is about the scanout GPU this backend drives.
/// Pauses of any other device (input nodes, other cards) leave the outputs alone.
pub(super) fn pauses_scanout(primary_dev: u64, major: u32, minor: u32) -> bool {
    smithay::reexports::rustix::fs::makedev(major, minor) == primary_dev
}

/// A resume logind delivered with a descriptor. Only the primary scanout device is ours; input
/// devices are reopened by libinput through the session's `TakeDevice`.
pub(super) fn resume_device(
    state: &mut State,
    drm: &Rc<RefCell<Drm>>,
    major: u32,
    minor: u32,
    fd: OwnedFd,
) {
    let rdev = match device::device_rdev(&fd) {
        Ok(rdev) => rdev,
        Err(error) => {
            eprintln!("flamewm-wayland: resume {major}:{minor} detail=fstat-failed: {error}");
            return;
        }
    };
    if rdev != drm.borrow().primary_dev {
        // Not leaked and not dropped silently: logind resumed a device this backend does not own,
        // so the descriptor is simply released here.
        eprintln!("flamewm-wayland: resume {major}:{minor} detail=not-primary-device released");
        return;
    }
    let kept = {
        let mut d = drm.borrow_mut();
        match kept_fd_resume(&mut d, state, drm) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("flamewm-wayland: resume detail=kept-fd-failed: {error}");
                false
            }
        }
    };
    if kept {
        eprintln!("flamewm-wayland: resume {major}:{minor} detail=kept-fd duplicate-fd-closed");
        stage_line(Stage::Drm, StageStatus::Ok, "resume kept-fd");
        render_all(state, drm);
        return;
    }
    match rebuild(state, drm, fd) {
        Ok(()) => {
            stage_line(Stage::Drm, StageStatus::Ok, "resume rebuilt");
            render_all(state, drm);
        }
        Err(error) => {
            stage_line(Stage::Drm, StageStatus::Fail, "resume rebuilt");
            eprintln!("flamewm-wayland: device rebuild failed: {error}");
        }
    }
}

/// Builds device, GBM, EGL, renderer and outputs again from the descriptor logind handed back.
///
/// The new objects are created before anything is dropped, so a failure leaves the compositor in
/// its paused state instead of half rebuilt.
pub(super) fn rebuild(
    state: &mut State,
    drm: &Rc<RefCell<Drm>>,
    fd: OwnedFd,
) -> Result<(), Box<dyn Error>> {
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (device, notifier) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd)?;
    // SAFETY: the EGL display is created on the new GBM device and only used by the replacement
    // renderer, which lives exactly as long as the `Drm` that owns it.
    let egl = unsafe { EGLDisplay::new(gbm.clone())? };
    let context = EGLContext::new(&egl)?;
    // SAFETY: single compositor thread; the context is current only here.
    let mut renderer = unsafe { GlowRenderer::new(context)? };
    crate::render::log_gl_renderer(&mut renderer, "drm");
    let formats: Vec<Format> = renderer.dmabuf_formats().into_iter().collect();

    let mut d = drm.borrow_mut();
    // Scanout buffers of the old device have to go before the old GBM device does.
    drop_outputs(&mut d, state);
    d.device = device;
    d.gbm = gbm;
    d.renderer = renderer;
    d.formats = formats;
    d.paused = false;
    d.retry_scheduled = false;
    d.retry_attempts = 0;
    d.resume_input();
    let handle = d.handle.clone();
    let again = Rc::clone(drm);
    // The replacement device needs its own event source; the old one watches a dead descriptor and
    // stays registered (calloop has no cheap removal) but never reports again.
    handle.insert_source(notifier, move |event, metadata, state| {
        on_drm_event(state, &again, event, metadata.take());
    })?;
    scan_connectors(&mut d, state)?;
    // Same rule as the kept-fd resume: the session is ours again, so the startup deadline starts over.
    d.first_frame.resume();
    d.arm_first_frame_deadline(drm);
    state.request_full_redraw();
    Ok(())
}

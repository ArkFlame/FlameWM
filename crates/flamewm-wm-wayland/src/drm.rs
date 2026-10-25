//! Tty backend (cargo feature `drm`): logind session, udev device discovery, DRM/KMS outputs
//! through a GBM-backed `DrmCompositor`, GLES on EGL/GBM, libinput and VT switching.
//! Selected at runtime with `FLAMEWM_WAYLAND_BACKEND=drm`.
//!
//! Scope: the first connected connector drives `State::output`; further connectors show the same
//! scene. Frames are paced by vblank events. Hotplug rescans the primary GPU on udev change events.
//! The cursor is a software square drawn at the pointer.
//!
//! # Frame scheduling
//!
//! Nothing paints unless somebody asked for it: `State::request_full_redraw` sets a coalescing
//! dirty flag and pings the calloop waker installed here, and the per-output rules live in
//! `state::plan_frame`/`state::OutputFrames` so they are testable without a GPU. There is no
//! periodic repaint: idle means zero paints, and the only timer is a bounded retry for a real sync
//! or buffer error. A vblank clears `queued` before the next plan, so a request that arrives while
//! a frame is in flight is deferred to that vblank instead of being dropped.
//!
//! # Session
//!
//! logind pause/resume events are drained by the poll timer, never from a paint, so
//! `PauseDeviceComplete` is sent from that callback once scanout has stopped. A resume reclaims
//! master on the descriptor we already hold when logind hands back the same card, and rebuilds
//! device, GBM, EGL, renderer and outputs from the new descriptor when it does not.
//!
//! # GPU selection and partial bring-up
//!
//! The boot GPU is opened first and used as soon as it reports a connected connector. Only when it
//! has nothing connected are the seat's other GPUs probed and the first one with a connected
//! internal panel picked (see `state::pick_gpu`), which is what a hybrid laptop whose panel hangs
//! off the non-boot card needs. A connector that cannot be set up is skipped and logged, never
//! fatal: startup fails only when zero outputs were created.
//!
//! # Client GPU buffers
//!
//! Not wired yet: clients use wl_shm. dmabuf import needs smithay's `ImportEgl::bind_wl_display`,
//! which is compiled only with smithay's `use_system_lib` feature, plus a `zwp_linux_dmabuf_v1`
//! global. Neither is available here, so no dmabuf global is offered.

use std::cell::RefCell;
use std::error::Error;
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{Format, Fourcc};
use smithay::backend::drm::compositor::{DrmCompositor, FrameError, FrameFlags, RenderFrameError};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmError, DrmEvent};
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::input::{
    Event as _, InputEvent, KeyState, KeyboardKeyEvent, PointerMotionEvent,
};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::ImportDma;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::session::Session;
use smithay::backend::udev::{self, UdevBackend, UdevEvent};
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::desktop::{PopupManager, layer_map_for_output};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::ping::make_ping;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::drm::control::{
    Device as ControlDevice, Mode as DrmMode, ModeTypeFlags, ResourceHandles, connector, crtc,
};
use smithay::reexports::input::Libinput;
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::utils::{DeviceFd, Physical, Point, Rectangle, Size, Transform};

use crate::Stage;
use crate::StageStatus;
use crate::logind::{Event as LogindEvent, LogindSession, PauseKind};
use crate::render::{self, CURSOR_COLOR, CURSOR_SIZE, Elem};
use crate::stage_line;
use crate::state::{
    FrameDecision, GpuProbe, OutputFrames, State, arm_retry, pick_gpu, scan_failed,
};

type Compositor =
    DrmCompositor<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

const CLEAR: [f32; 4] = [0.07, 0.08, 0.11, 1.0];
/// Delay of the retry armed only for a real sync or buffer error.
const FRAME_RETRY: Duration = Duration::from_millis(16);
/// How often one error may be retried before the output is left alone.
const MAX_RETRIES: u32 = 3;
/// logind signals are read by polling the system bus: there is no blocking bus source.
const SESSION_POLL: Duration = Duration::from_millis(20);

/// One scanned-out connector. `global` is `None` for the output that is `State::output`.
struct OutputSurface {
    connector: connector::Handle,
    crtc: crtc::Handle,
    output: Output,
    global: Option<GlobalId>,
    compositor: Compositor,
    /// Frame scheduling state: in flight, stale, retry owed.
    frames: OutputFrames,
}

pub struct Drm {
    session: LogindSession,
    device: DrmDevice,
    gbm: GbmDevice<DrmDeviceFd>,
    renderer: GlowRenderer,
    formats: Vec<Format>,
    libinput: Libinput,
    /// `dev_t` of the GPU this backend drives; udev change events and resume events for it are ours.
    primary_dev: u64,
    surfaces: Vec<OutputSurface>,
    cursor_id: Id,
    /// Render, vblank and device work are no-ops while the session is not ours.
    paused: bool,
    /// Why the render gate was last logged as closed, so each transition is reported once.
    gate: Option<&'static str>,
    retry_scheduled: bool,
    retry_attempts: u32,
    handle: LoopHandle<'static, State>,
}

/// Owns the running tty backend. `main` holds it for the whole run and hands it back to
/// [`finish`] on every exit path, including a panic.
pub struct DrmHandle(pub Rc<RefCell<Drm>>);

/// Starts the tty backend. Registers the session, udev, libinput, DRM and redraw-wake sources on
/// `handle` and renders the first frame; vblank events drive the rest.
///
/// The error type is `Box<dyn Error>` because this crate has no `anyhow` dependency; `main` only
/// needs `Display` for its stage line.
pub fn init(
    state: &mut State,
    handle: &LoopHandle<'static, State>,
) -> Result<DrmHandle, Box<dyn Error>> {
    let session = match LogindSession::new() {
        Ok(session) => session,
        Err(error) => {
            stage_line(Stage::Seat, StageStatus::Fail, &format!("{error}"));
            return Err(error.into());
        }
    };
    stage_line(Stage::Seat, StageStatus::Ok, "logind control");
    let seat = session.seat();
    let (gpu, gpu_detail) = open_scanout_gpu(&session, &seat)?;
    let OpenedGpu {
        name: gpu_name,
        dev: primary_dev,
        device,
        fd: gpu_fd,
        notifier: drm_notifier,
    } = gpu;
    let gbm = GbmDevice::new(gpu_fd)?;
    // SAFETY: the EGL display is created on the GBM device owned by this compositor and lives as
    // long as the renderer in `Drm`, which is the only user of the context.
    let egl = unsafe { EGLDisplay::new(gbm.clone())? };
    let context = EGLContext::new(&egl)?;
    // SAFETY: the context is current only on this thread, the single compositor thread.
    let renderer = unsafe { GlowRenderer::new(context)? };
    let formats: Vec<Format> = renderer.dmabuf_formats().into_iter().collect();
    stage_line(
        Stage::Drm,
        StageStatus::Ok,
        &format!("{gpu_name} master {gpu_detail}"),
    );

    let mut libinput =
        Libinput::new_with_udev::<LibinputSessionInterface<LogindSession>>(session.clone().into());
    if libinput.udev_assign_seat(&seat).is_err() {
        stage_line(Stage::Libinput, StageStatus::Fail, "seat assign");
        return Err("libinput could not take the seat".into());
    }
    // Reported after the connector scan so the stage lines come out in `Stage` order.
    let libinput_detail = format!("{seat} devices");
    let input_backend = LibinputInputBackend::new(libinput.clone());
    let udev_backend = UdevBackend::new(&seat)?;

    let drm = Rc::new(RefCell::new(Drm {
        session,
        device,
        gbm,
        renderer,
        formats,
        libinput,
        primary_dev,
        surfaces: Vec::new(),
        cursor_id: Id::new(),
        paused: false,
        gate: None,
        retry_scheduled: false,
        retry_attempts: 0,
        handle: handle.clone(),
    }));
    {
        let mut d = drm.borrow_mut();
        // Only a device-level failure is fatal here: a single connector that cannot be set up is
        // logged and skipped, like anvil does.
        if let Err(error) = scan_connectors(&mut d, state) {
            stage_line(Stage::Outputs, StageStatus::Fail, &format!("{error}"));
            return Err(error);
        }
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
            "{outputs} out {}x{}",
            state.output_size.w, state.output_size.h
        ),
    );
    stage_line(Stage::Libinput, StageStatus::Ok, &libinput_detail);

    let drm_events = Rc::clone(&drm);
    handle.insert_source(drm_notifier, move |event, _, state| {
        on_drm_event(state, &drm_events, event)
    })?;

    let input_drm = Rc::clone(&drm);
    handle.insert_source(input_backend, move |event, _, state| {
        handle_input(state, &input_drm, event)
    })?;

    // logind signals (VT pause and resume) are read by polling the bus; the events are queued by
    // the session and dispatched here, outside the session borrow.
    let session_drm = Rc::clone(&drm);
    handle.insert_source(Timer::from_duration(SESSION_POLL), move |_, _, state| {
        let events = {
            let d = session_drm.borrow();
            d.session.pump();
            d.session.take_events()
        };
        for event in events {
            on_logind_event(state, &session_drm, event);
        }
        TimeoutAction::ToDuration(SESSION_POLL)
    })?;

    let udev_drm = Rc::clone(&drm);
    handle.insert_source(udev_backend, move |event, _, state| {
        if let UdevEvent::Changed { device_id } = event {
            let primary = udev_drm.borrow().primary_dev;
            if device_id == primary {
                if let Ok(mut d) = udev_drm.try_borrow_mut() {
                    if let Err(error) = scan_connectors(&mut d, state) {
                        eprintln!("flamewm-wayland: connector rescan failed: {error}");
                    }
                }
                state.request_full_redraw();
                render_all(state, &udev_drm);
            }
        }
    })?;

    // The wake pipe: `State::request_full_redraw` pings it from anywhere (input, protocol
    // handlers, timers) so a request never waits for an unrelated event to be painted.
    let (ping, ping_source) = make_ping()?;
    let waker_drm = Rc::clone(&drm);
    handle.insert_source(ping_source, move |_, _, state| {
        render_all(state, &waker_drm)
    })?;
    state.set_drm_waker(ping);

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

impl Drm {
    /// Stops every device exactly once and forgets all in-flight frames. Idempotent: a second
    /// pause before a resume does not suspend twice, which makes it safe from both the pause
    /// handler and [`finish`].
    fn quiesce(&mut self) {
        self.retry_scheduled = false;
        self.retry_attempts = 0;
        for out in &mut self.surfaces {
            out.frames.park();
        }
        if self.paused {
            return;
        }
        self.paused = true;
        self.libinput.suspend();
        self.device.pause();
    }

    /// One scheduling pass: plans every output, paints the ones that need it, writes a pending
    /// dump and sends frame callbacks. Idle outputs paint nothing.
    fn paint(&mut self, state: &mut State, drm: &Rc<RefCell<Drm>>) {
        // The gate is reported on every transition, so a session that was taken away can never look
        // like a silent black screen.
        let gate = if self.paused {
            Some("session-paused")
        } else if !self.session.is_active() {
            Some("session-inactive")
        } else {
            None
        };
        self.log_gate(gate);
        if gate.is_some() || self.surfaces.is_empty() {
            return;
        }
        // A paint answers the frame callbacks, and a client that wants another frame commits a
        // buffer, which calls `State::request_redraw` (see `handlers::commit`). The dirty flag is
        // therefore this compositor's "a client frame callback is outstanding" signal, read before
        // the request is consumed.
        let has_callback = state.redraw_pending();
        // Coalesced: every request since the last paint becomes one frame.
        let dirty = state.take_redraw();
        if dirty {
            for out in &mut self.surfaces {
                out.frames.dirty = true;
            }
        }
        // Only `Draw` is a reason to build elements: a `Deferred` output has a frame in flight and
        // is planned again by its vblank, and an `Idle` one has nothing to do.
        let wanted = self
            .surfaces
            .iter()
            .any(|out| out.frames.plan(has_callback, self.paused) == FrameDecision::Draw);
        if !wanted {
            return;
        }

        let app = render::build_elements(state, &mut self.renderer);
        let pointer = Point::<i32, Physical>::from((
            state.pointer.x.round() as i32,
            state.pointer.y.round() as i32,
        ));
        let cursor = SolidColorRenderElement::new(
            self.cursor_id.clone(),
            Rectangle::new(pointer, Size::from((CURSOR_SIZE, CURSOR_SIZE))),
            CommitCounter::default(),
            CURSOR_COLOR,
            Kind::Unspecified,
        );
        let mut elements: Vec<Elem> = Vec::with_capacity(app.len() + 1);
        elements.push(Elem::Solid(cursor));
        elements.extend(app);

        // `FLAMEWM_WAYLAND_DUMP`: one readback of the primary output. Taking the path makes a set
        // request be consumed exactly once, and `state.dumped` stops main.rs's re-arming timer.
        if let Some(path) = state.dump.take() {
            let size = Size::<i32, Physical>::from((state.output_size.w, state.output_size.h));
            match render::dump_offscreen(
                &mut self.renderer,
                &self.surfaces[0].output,
                &elements,
                size,
                &path,
            ) {
                Ok(()) => {
                    state.dumped = true;
                    if state.exit_after_dump {
                        state.loop_signal.stop();
                    }
                }
                Err(error) => eprintln!("flamewm-wayland: frame dump failed: {error}"),
            }
        }

        if self.draw_pass(&elements, has_callback) {
            // The kernel rejected an atomic state. That is a leftover from whoever owned the CRTCs
            // before us rather than an unusable output, so the device state is reset and the pass
            // is repeated once, exactly like anvil's `render_surface`. It cannot loop: the second
            // pass either succeeds or hands the output to the bounded retry below.
            eprintln!("flamewm-wayland: retry detail=device-reset");
            match self.device.reset_state() {
                Ok(()) => {
                    for out in &mut self.surfaces {
                        out.frames.wake();
                        if let Err(error) = out.compositor.reset_state() {
                            eprintln!(
                                "flamewm-wayland: reset failed on {}: {error}",
                                out.output.name()
                            );
                        }
                    }
                    self.draw_pass(&elements, has_callback);
                }
                Err(error) => eprintln!("flamewm-wayland: device reset failed: {error}"),
            }
        }

        let retry = self
            .surfaces
            .iter()
            .any(|out| arm_retry(out.frames.needs_retry, out.frames.dirty, self.paused));
        if retry && !self.retry_scheduled && self.retry_attempts < MAX_RETRIES {
            self.retry_scheduled = true;
            self.retry_attempts += 1;
            let again = Rc::clone(drm);
            let _ =
                self.handle
                    .insert_source(Timer::from_duration(FRAME_RETRY), move |_, _, state| {
                        if let Ok(mut d) = again.try_borrow_mut() {
                            d.retry_scheduled = false;
                        }
                        render_all(state, &again);
                        TimeoutAction::Drop
                    });
        } else if !retry {
            self.retry_attempts = 0;
        }
        send_frame_callbacks(state);
    }

    /// Reports the render gate exactly once per transition.
    fn log_gate(&mut self, closed: Option<&'static str>) {
        if self.gate == closed {
            return;
        }
        match closed {
            Some(detail) => eprintln!("flamewm-wayland: render gate closed detail={detail}"),
            None => eprintln!("flamewm-wayland: render gate open"),
        }
        self.gate = closed;
    }

    /// One pass over every output that wants a frame right now.
    ///
    /// Returns true when the kernel rejected an atomic state for one of them (`DrmError::TestFailed`
    /// inside the render error): that output stays dirty instead of being handed to the retry timer,
    /// because `paint` resets the device state and repeats the pass once.
    fn draw_pass(&mut self, elements: &[Elem], has_callback: bool) -> bool {
        let mut test_failed = false;
        for out in self.surfaces.iter_mut() {
            // `Deferred` is not a reason to paint: a frame is still in flight, so this output is
            // re-planned by the vblank that clears `queued`, and painting now would queue over it.
            if out.frames.plan(has_callback, self.paused) != FrameDecision::Draw {
                continue;
            }
            match out.compositor.render_frame(
                &mut self.renderer,
                elements,
                CLEAR,
                FrameFlags::DEFAULT,
            ) {
                Ok(result) if result.is_empty => {
                    // Nothing was drawn, so no vblank follows. Only a pending state sync is worth
                    // another pass; otherwise the output is idle and must not arm a timer.
                    out.frames.empty(result.needs_sync());
                }
                Ok(_) => match out.compositor.queue_frame(()) {
                    Ok(()) => out.frames.queue(),
                    Err(error) => {
                        out.frames.fail();
                        eprintln!(
                            "flamewm-wayland: queue_frame failed on {}: {error}",
                            out.output.name()
                        );
                    }
                },
                Err(error) => {
                    if is_test_failed(&error) {
                        test_failed = true;
                        eprintln!(
                            "flamewm-wayland: render rejected on {} detail=test-failed",
                            out.output.name()
                        );
                        // Kept dirty for the retry pass `paint` runs after the device reset.
                        out.frames.wake();
                        continue;
                    }
                    out.frames.fail();
                    eprintln!(
                        "flamewm-wayland: render failed on {}: {error}",
                        out.output.name()
                    );
                }
            }
        }
        test_failed
    }

    /// A frame reached the screen: nothing is in flight for that output any more, so the next plan
    /// sees requests that arrived while it was queued.
    fn on_vblank(&mut self, state: &mut State, crtc: crtc::Handle, drm: &Rc<RefCell<Drm>>) {
        let Some(out) = self.surfaces.iter_mut().find(|out| out.crtc == crtc) else {
            return;
        };
        out.frames.presented();
        if let Err(error) = out.compositor.frame_submitted() {
            out.frames.fail();
            eprintln!(
                "flamewm-wayland: frame_submitted failed on {}: {error}",
                out.output.name()
            );
        }
        self.paint(state, drm);
    }
}

/// Whether a render error is the kernel rejecting the atomic state (`DrmError::TestFailed`).
///
/// The error type of `DrmCompositor::render_frame` cannot be named outside smithay, but the pattern
/// is public, so the check is a pattern match on the outer error. Its source chain is checked too,
/// because a `PrepareFrame` wraps the `FrameError` that carries the DRM error.
fn is_test_failed<A, B, F, R>(error: &RenderFrameError<A, B, F, R>) -> bool
where
    A: std::error::Error + Send + Sync + 'static,
    B: std::error::Error + Send + Sync + 'static,
    F: std::error::Error + Send + Sync + 'static,
    R: std::error::Error,
{
    match error {
        RenderFrameError::PrepareFrame(FrameError::DrmError(DrmError::TestFailed(_))) => true,
        _ => matches!(error.source(), Some(source)
            if matches!(source.downcast_ref::<DrmError>(), Some(DrmError::TestFailed(_)))),
    }
}

/// One seat GPU that opened, with the descriptor kept for GBM.
struct OpenedGpu {
    name: String,
    /// `dev_t` of the card: udev change and resume events are matched against it.
    dev: u64,
    device: DrmDevice,
    fd: DrmDeviceFd,
    notifier: DrmDeviceNotifier,
}

/// Opens the seat's GPUs and picks the one to drive, plus a short detail token for the stage line.
///
/// The boot GPU is tried first and wins as soon as it reports a connected connector: that is the
/// common single-GPU and hybrid-desktop case and costs exactly one `TakeDevice` roundtrip. Only when
/// it has nothing connected are the seat's other GPUs probed — a hybrid laptop whose only panel
/// hangs off the non-boot card reports zero connectors on the boot GPU, which used to mean zero
/// connectors for the whole session. The choice itself is `state::pick_gpu`, unit-tested without a
/// GPU.
fn open_scanout_gpu(
    session: &LogindSession,
    seat: &str,
) -> Result<(OpenedGpu, String), Box<dyn Error>> {
    let boot = udev::primary_gpu(seat)?;
    let mut paths: Vec<PathBuf> = boot.iter().cloned().collect();
    for path in udev::all_gpus(seat)? {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        return Err("no DRM device on seat".into());
    }

    let mut opened: Vec<(OpenedGpu, GpuProbe)> = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        match open_gpu(session, path) {
            Ok(gpu) => {
                let probe = probe_gpu(&gpu.device);
                // Only the boot GPU short-circuits: on any other card the rest of the seat is still
                // worth looking at, because a panel is what is being looked for.
                let boot_gpu = index == 0 && boot.is_some();
                opened.push((gpu, probe));
                if boot_gpu && probe.connected {
                    break;
                }
            }
            Err(error) => eprintln!("flamewm-wayland: gpu {name} skip: {error}"),
        }
    }

    let probes: Vec<GpuProbe> = opened.iter().map(|(_, probe)| *probe).collect();
    let chosen = pick_gpu(&probes).ok_or("no DRM device on seat")?;
    let detail = if probes[chosen].internal {
        "detail=panel"
    } else if probes[chosen].connected {
        "detail=connected"
    } else {
        "detail=none-connected"
    };
    let (gpu, _) = opened.swap_remove(chosen);
    // The cards that did not win are dropped here: their descriptors close and their master is
    // released before the compositor starts.
    Ok((gpu, detail.to_owned()))
}

/// Takes one DRM device from the session and opens it.
fn open_gpu(session: &LogindSession, path: &Path) -> Result<OpenedGpu, Box<dyn Error>> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dev = std::fs::metadata(path)?.rdev();
    let fd = session.clone().open(
        path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
    )?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (device, notifier) = DrmDevice::new(fd.clone(), true)?;
    Ok(OpenedGpu {
        name,
        dev,
        device,
        fd,
        notifier,
    })
}

/// What one card can drive, without setting anything up: internal (panel) connectors and all
/// connected connectors.
fn probe_gpu(device: &DrmDevice) -> GpuProbe {
    let Ok(resources) = device.resource_handles() else {
        return GpuProbe::default();
    };
    let mut probe = GpuProbe::default();
    for conn in resources.connectors() {
        let Ok(info) = device.get_connector(*conn, false) else {
            continue;
        };
        if info.state() != connector::State::Connected {
            continue;
        }
        probe.connected = true;
        if matches!(
            info.interface(),
            connector::Interface::LVDS
                | connector::Interface::EmbeddedDisplayPort
                | connector::Interface::DSI
                | connector::Interface::DPI
        ) {
            probe.internal = true;
        }
    }
    probe
}

/// Reports one connector that was left out, as `stage=outputs status=skip detail=<connector> <why>`.
///
/// A connector that cannot be set up never aborts the session: it is skipped here and the remaining
/// connectors still come up.
fn skip_connector(device: &DrmDevice, conn: connector::Handle, reason: &str) {
    let name = match device.get_connector(conn, false) {
        Ok(info) => format!("{}-{}", info.interface().as_str(), info.interface_id()),
        Err(_) => "unknown".to_owned(),
    };
    stage_line(
        Stage::Outputs,
        StageStatus::Skip,
        &format!("{name} {reason}"),
    );
}

/// Adds connectors that became connected and drops outputs whose connector went away.
///
/// Returns an error only for a device-level failure (the connector list cannot be read at all). A
/// single connector that fails to come up is logged with `status=skip` and skipped; startup decides
/// with `state::scan_failed` whether nothing at all came up.
fn scan_connectors(d: &mut Drm, state: &mut State) -> Result<(), Box<dyn Error>> {
    let resources = d.device.resource_handles()?;

    let mut index = 0;
    while index < d.surfaces.len() {
        let connector = d.surfaces[index].connector;
        let connected = d
            .device
            .get_connector(connector, false)
            .map(|info| info.state() == connector::State::Connected)
            .unwrap_or(false);
        if connected {
            index += 1;
            continue;
        }
        let gone = d.surfaces.remove(index);
        if let Some(global) = gone.global {
            state.display.remove_global::<State>(global);
        }
        eprintln!(
            "flamewm-wayland: output {} disconnected",
            gone.output.name()
        );
    }

    for conn in resources.connectors().to_vec() {
        if d.surfaces.iter().any(|out| out.connector == conn) {
            continue;
        }
        let info = match d.device.get_connector(conn, true) {
            Ok(info) => info,
            Err(error) => {
                skip_connector(&d.device, conn, &format!("read {error}"));
                continue;
            }
        };
        if info.state() != connector::State::Connected {
            continue;
        }
        let used: Vec<crtc::Handle> = d.surfaces.iter().map(|out| out.crtc).collect();
        let Some(crtc) = pick_crtc(&d.device, &resources, &info, &used) else {
            skip_connector(&d.device, conn, "no-crtc");
            continue;
        };
        let Some(mode) = info
            .modes()
            .iter()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or(info.modes().first())
            .copied()
        else {
            skip_connector(&d.device, conn, "no-mode");
            continue;
        };
        // Two attempts, like anvil: a KMS state left behind by whoever owned this connector before
        // us is the usual reason a surface cannot be created, and one `DrmDevice::reset_state`
        // clears it. A connector that still fails is skipped, never fatal.
        let mut attempt = 0;
        loop {
            match build_output(d, state, conn, crtc, mode, &info) {
                Ok(out) => {
                    eprintln!(
                        "flamewm-wayland: output {} {}x{}",
                        out.output.name(),
                        i32::from(mode.size().0),
                        i32::from(mode.size().1)
                    );
                    d.surfaces.push(out);
                    break;
                }
                Err(error) => {
                    if attempt > 0 {
                        skip_connector(&d.device, conn, &format!("setup {error}"));
                        break;
                    }
                    attempt += 1;
                    eprintln!("flamewm-wayland: output setup retry detail={error}");
                    if let Err(error) = d.device.reset_state() {
                        skip_connector(&d.device, conn, &format!("reset {error}"));
                        break;
                    }
                    // A device reset disables every connector and plane, so the outputs that are
                    // already up have to be painted again in full.
                    for out in &mut d.surfaces {
                        out.frames.wake();
                        if let Err(error) = out.compositor.reset_state() {
                            eprintln!(
                                "flamewm-wayland: reset failed on {}: {error}",
                                out.output.name()
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// The first CRTC one of this connector's encoders can drive that no output uses yet.
fn pick_crtc(
    device: &DrmDevice,
    resources: &ResourceHandles,
    info: &connector::Info,
    used: &[crtc::Handle],
) -> Option<crtc::Handle> {
    for encoder in info.encoders() {
        let Ok(encoder) = device.get_encoder(*encoder) else {
            continue;
        };
        let mut candidates: Vec<crtc::Handle> = encoder.crtc().into_iter().collect();
        candidates.extend(resources.filter_crtcs(encoder.possible_crtcs()));
        if let Some(crtc) = candidates.into_iter().find(|crtc| !used.contains(crtc)) {
            return Some(crtc);
        }
    }
    None
}

/// Creates the scanout surface, the wayland `Output` (or the shared primary one) and the
/// `DrmCompositor` for one connector.
fn build_output(
    d: &mut Drm,
    state: &mut State,
    conn: connector::Handle,
    crtc: crtc::Handle,
    mode: DrmMode,
    info: &connector::Info,
) -> Result<OutputSurface, Box<dyn Error>> {
    let surface = d.device.create_surface(crtc, mode, &[conn])?;
    let (width, height) = mode.size();
    let mode = Mode {
        size: (i32::from(width), i32::from(height)).into(),
        refresh: i32::try_from(mode.vrefresh()).unwrap_or(60) * 1000,
    };
    // The first output is `State::output`, which already has a global; later ones get their own.
    let (output, global) = if d.surfaces.iter().any(|out| out.global.is_none()) {
        let name = format!("{}-{}", info.interface().as_str(), info.interface_id());
        let size_mm = info
            .size()
            .map(|(w, h)| Size::from((w as i32, h as i32)))
            .unwrap_or_default();
        let output = Output::new(
            name,
            PhysicalProperties {
                size: size_mm,
                subpixel: Subpixel::Unknown,
                make: "FlameWM".to_owned(),
                model: "DRM".to_owned(),
            },
        );
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            None,
        );
        output.set_preferred(mode);
        let global = output.create_global::<State>(&state.display);
        (output, Some(global))
    } else {
        state.output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            Some((0, 0).into()),
        );
        state.output.set_preferred(mode);
        state.output_size = Size::from((mode.size.w, mode.size.h));
        (state.output.clone(), None)
    };
    let allocator = GbmAllocator::new(
        d.gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(d.gbm.clone(), None);
    let compositor = match DrmCompositor::new(
        &output,
        surface,
        None,
        allocator,
        exporter,
        [Fourcc::Argb8888, Fourcc::Xrgb8888],
        d.formats.iter().copied(),
        d.device.cursor_size(),
        Some(d.gbm.clone()),
    ) {
        Ok(compositor) => compositor,
        Err(error) => {
            // The global belongs to an output that will never appear: withdraw it before the
            // caller retries, so a retry does not leave a second one behind.
            if let Some(global) = global {
                state.display.remove_global::<State>(global);
            }
            return Err(error.into());
        }
    };
    Ok(OutputSurface {
        connector: conn,
        crtc,
        output,
        global,
        compositor,
        // A connector that just appeared has never been painted.
        frames: OutputFrames {
            dirty: true,
            ..OutputFrames::default()
        },
    })
}

/// Drops every output surface and its compositor. The connectors go back to `State`, which keeps
/// the primary `Output` global alive.
fn drop_outputs(d: &mut Drm, state: &mut State) {
    for out in d.surfaces.drain(..) {
        if let Some(global) = out.global {
            state.display.remove_global::<State>(global);
        }
    }
}

/// Paints through the shared compositor if it is not already in use.
///
/// Every caller goes through here because the compositor is behind a `RefCell`: a request that
/// finds it borrowed means a paint is already running on this thread, and the request is left in
/// `state`'s dirty flag so that in-flight pass or the next vblank picks it up.
fn render_all(state: &mut State, drm: &Rc<RefCell<Drm>>) {
    let Ok(mut d) = drm.try_borrow_mut() else {
        return;
    };
    d.paint(state, drm);
}

fn on_drm_event(state: &mut State, drm: &Rc<RefCell<Drm>>, event: DrmEvent) {
    match event {
        DrmEvent::VBlank(crtc) => {
            let Ok(mut d) = drm.try_borrow_mut() else {
                // A paint is in flight and replans this output itself.
                return;
            };
            d.on_vblank(state, crtc, drm);
        }
        DrmEvent::Error(error) => eprintln!("flamewm-wayland: drm error: {error}"),
    }
}

fn handle_input(
    state: &mut State,
    drm: &Rc<RefCell<Drm>>,
    event: InputEvent<LibinputInputBackend>,
) {
    match event {
        InputEvent::PointerMotion { event } => {
            // Relative motion: the same absolute path the nested backend uses, clamped to the output.
            let size = state.output_size;
            let x = (state.pointer.x + event.delta_x()).clamp(0.0, f64::from((size.w - 1).max(0)));
            let y = (state.pointer.y + event.delta_y()).clamp(0.0, f64::from((size.h - 1).max(0)));
            state.on_motion((x, y).into(), event.time_msec());
            state.request_redraw();
        }
        InputEvent::Keyboard { event } => {
            match vt_for_key(state, &event) {
                // Ctrl+Alt+Fn is consumed here: the session switches and the key never reaches clients.
                Some(vt) => {
                    if let Err(error) = drm.borrow_mut().session.change_vt(vt) {
                        eprintln!("flamewm-wayland: VT switch to {vt} failed: {error}");
                    }
                }
                None => state.process_input::<LibinputInputBackend>(InputEvent::Keyboard { event }),
            }
        }
        other => state.process_input(other),
    }
}

/// One logind event. Pauses stop scanout and then answer logind; resumes reclaim the device.
fn on_logind_event(state: &mut State, drm: &Rc<RefCell<Drm>>, event: LogindEvent) {
    match event {
        LogindEvent::Pause { major, minor, kind } => {
            {
                let mut d = drm.borrow_mut();
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
                match kept_fd_resume(&mut d, state) {
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
/// the rebuild path instead of pretending the device works.
fn kept_fd_resume(d: &mut Drm, state: &mut State) -> Result<(), Box<dyn Error>> {
    d.device.activate(false)?;
    if d.libinput.resume().is_err() {
        eprintln!("flamewm-wayland: libinput resume failed");
    }
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
    state.request_full_redraw();
    Ok(())
}

/// A resume logind delivered with a descriptor. Only the primary scanout device is ours; input
/// devices are reopened by libinput through the session's `TakeDevice`.
fn resume_device(state: &mut State, drm: &Rc<RefCell<Drm>>, major: u32, minor: u32, fd: OwnedFd) {
    let rdev = match device_rdev(&fd) {
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
        match kept_fd_resume(&mut d, state) {
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
fn rebuild(state: &mut State, drm: &Rc<RefCell<Drm>>, fd: OwnedFd) -> Result<(), Box<dyn Error>> {
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (device, notifier) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd)?;
    // SAFETY: the EGL display is created on the new GBM device and only used by the replacement
    // renderer, which lives exactly as long as the `Drm` that owns it.
    let egl = unsafe { EGLDisplay::new(gbm.clone())? };
    let context = EGLContext::new(&egl)?;
    // SAFETY: single compositor thread; the context is current only here.
    let renderer = unsafe { GlowRenderer::new(context)? };
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
    if d.libinput.resume().is_err() {
        eprintln!("flamewm-wayland: libinput resume failed");
    }
    let handle = d.handle.clone();
    let again = Rc::clone(drm);
    // The replacement device needs its own event source; the old one watches a dead descriptor and
    // stays registered (calloop has no cheap removal) but never reports again.
    handle.insert_source(notifier, move |event, _, state| {
        on_drm_event(state, &again, event)
    })?;
    if let Err(error) = scan_connectors(&mut d, state) {
        return Err(error);
    }
    state.request_full_redraw();
    Ok(())
}

/// `fstat` of a descriptor logind handed over, as the `dev_t` to compare with the primary GPU.
fn device_rdev(fd: &OwnedFd) -> std::io::Result<u64> {
    let probe = fd.try_clone()?;
    Ok(std::fs::File::from(probe).metadata()?.rdev())
}

/// Frame callbacks for every surface that can be on screen; same set as the nested backend.
fn send_frame_callbacks(state: &State) {
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
    for surface in x11.filter_map(|window| window.wl_surface()) {
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
}

/// Ctrl+Alt+F1..F12 (evdev KEY_F1..KEY_F12) select virtual terminals 1..12.
fn vt_for_key(state: &State, event: &impl KeyboardKeyEvent<LibinputInputBackend>) -> Option<i32> {
    if event.state() != KeyState::Pressed {
        return None;
    }
    let keyboard = state.seat.get_keyboard()?;
    let mods = keyboard.modifier_state();
    if !(mods.ctrl && mods.alt) {
        return None;
    }
    // xkb keycodes are evdev codes plus 8.
    let code = event.key_code().raw().checked_sub(8)?;
    match code {
        59..=68 => Some((code - 58) as i32),
        87 => Some(11),
        88 => Some(12),
        _ => None,
    }
}

//! Tty backend (cargo feature `drm`): logind session, udev device discovery, DRM/KMS outputs
//! through a GBM-backed `DrmCompositor`, GLES on EGL/GBM, libinput and VT switching.
//! Selected at runtime with `FLAMEWM_WAYLAND_BACKEND=drm`.
//!
//! Scope: the first connected connector drives `State::output`; further connectors show the same
//! scene. Frames are paced by vblank events. Hotplug rescans the primary GPU on udev change events.
//! The pointer is the client's cursor surface or a themed xcursor image (`crate::cursor`), drawn as
//! `Kind::Cursor` so the `DrmCompositor` can put it on the hardware cursor plane.
//!
//! # Frame scheduling
//!
//! Nothing paints unless somebody asked for it: `State::request_full_redraw` sets a coalescing
//! dirty flag and pings the calloop waker installed here, and the per-output rules live in
//! `state::plan_frame`/`state::OutputFrames` so they are testable without a GPU. There is no
//! periodic repaint: idle means zero paints, and the only paint-related timer is a bounded retry for a
//! real sync or buffer error (the session poll and the one startup deadline never paint). A vblank
//! clears `queued` before the next plan, so a request that arrives while a frame is in flight is
//! deferred to that vblank instead of being dropped.
//!
//! # The GPU sync contract
//!
//! `DrmCompositor::render_frame` returns a `RenderFrameResult` whose `needs_sync()` is true when KMS
//! cannot be made to wait for our rendering itself (no explicit fencing, or a sync point that cannot
//! be exported as a native fence). Smithay's contract for that case is explicit — see the docs of
//! `DrmCompositor::queue_frame`/`commit_frame` (`smithay-0.7.0/src/backend/drm/compositor/mod.rs:2424`
//! and `:2474`) and of `RenderFrameResult::needs_sync`
//! (`smithay-0.7.0/src/backend/drm/compositor/frame_result.rs:59`): the caller must synchronize
//! before it renders or submits again. `anvil` does exactly that in `anvil/src/udev.rs:1641-1643`
//! (`PrimaryPlaneElement::Swapchain(element) => element.sync.wait()`).
//!
//! This backend waits on the frame's `SyncPoint` (`PrimarySwapchainElement::sync`,
//! `frame_result.rs:435`) instead of queueing straight away, and it does so *bounded*: an `EGLFence`
//! is waited on with `EGLFence::client_wait(Some(budget), false)`
//! (`smithay-0.7.0/src/backend/egl/fence.rs:157`), so the compositor thread can never block on a dead
//! GPU forever. If the fence does not signal, the frame is **not** queued: the output is deferred
//! (`state::sync_wait_decision` -> `SyncWait::Defer`, then `SyncWait::Fail`) and the bounded retry
//! re-plans it. Skipping this wait is what produces the repeated buffer/IO errors that end in a
//! wedged pipeline and a black screen.
//!
//! # Input
//!
//! libinput comes up at init, before the first frame, exactly like anvil. `TakeControl` makes
//! logind switch the VT to `KD_GRAPHICS` with the kernel keyboard set to `K_OFF`, so from that moment
//! only this compositor can turn Ctrl+Alt+Fn into a VT switch; keeping input down until a frame is
//! presented only meant that any startup stall left the machine without a keyboard.
//!
//! # Queue, present, and the escape hatch
//!
//! Two boundaries, and only the second one is readiness (see `crate::startup`):
//!
//! * A successful `DrmCompositor::queue_frame` means the atomic commit was handed to KMS. It emits
//!   `stage=firstframe status=ok code=7` — diagnostic only. It starts nothing, it grabs nothing and
//!   it does **not** cancel the startup deadline: a queued flip can still never complete, and a
//!   compositor that treats "queued" as "on screen" holds DRM master and the keyboard in front of a
//!   black panel.
//! * The vblank whose `DrmCompositor::frame_submitted()` answers `Some(..)` means that flip completed.
//!   It emits `stage=presented status=ok code=8` exactly once and cancels the deadline. A vblank for a CRTC without a queued flip, or while the session is paused, satisfies
//!   nothing.
//!
//! One bounded deadline (8 s, `startup::FIRST_FRAME_DEADLINE`) is armed when the backend initialises
//! and is cancelled by nothing but a presented frame. When it fires, the session is unrecoverable in
//! place — the CRTC is ours and nothing is on it — so it reports itself, stops the loop and lets
//! `main` exit non-zero, which tears the backend down (`device.pause`, `ReleaseControl`) and gives the VT back. The reason distinguishes
//! `detail=no_first_queue` (nothing ever reached KMS) from `detail=pageflip_timeout` (a flip was
//! queued and never completed, the black-panel case), with the counters, the CRTC, the elapsed time
//! and the last render error.
//!
//! # Session
//!
//! logind pause/resume events are drained by the poll timer, never from a paint, so
//! `PauseDeviceComplete` is sent from that callback once scanout has stopped. A resume reclaims
//! master on the descriptor we already hold when logind hands back the same card, and rebuilds
//! device, GBM, EGL, renderer and outputs from the new descriptor when it does not.
//!
//! Only pauses of the scanout card stop the session. logind pauses input nodes on a VT switch and
//! sends `gone` for every `ReleaseDevice`, including the nodes libinput opens and closes while it
//! probes the seat; treating those as a session pause blanked the panel and suspended input for good.
//!
//! `LogindSession::new` activates the session and fails when it cannot, so `init` refuses to take DRM
//! master on an inactive session (see the `is_active` check there).
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
use std::time::{Duration, Instant};

use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{Buffer, Format, Fourcc};
use smithay::backend::drm::compositor::{
    DrmCompositor, FrameError, FrameFlags, PrimaryPlaneElement, RenderFrameError, RenderFrameResult,
};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::{
    DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmError, DrmEvent, Framebuffer,
};
use smithay::backend::egl::fence::EGLFence;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::input::{
    Event as _, InputEvent, KeyState, KeyboardKeyEvent, PointerMotionEvent,
};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::ImportDma;
use smithay::backend::renderer::glow::GlowRenderer;
use smithay::backend::renderer::sync::SyncPoint;
use smithay::backend::session::Session;
use smithay::backend::udev::{self, UdevBackend, UdevEvent};
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::desktop::{PopupManager, layer_map_for_output};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::calloop::ping::make_ping;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::drm::Device as _;
use smithay::reexports::drm::control::{
    Device as ControlDevice, Mode as DrmMode, ModeTypeFlags, ResourceHandles, connector, crtc,
};
use smithay::reexports::input::Libinput;
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::utils::{DeviceFd, Physical, Size, Transform};

use crate::Stage;
use crate::StageStatus;
use crate::StartupTiming;
use crate::elapsed_ms;
use crate::logind::{Event as LogindEvent, LogindSession, PauseKind};
mod device;
mod input;

use crate::render::{self, Elem};
use crate::stage_line;
use crate::startup::{FIRST_FRAME_DEADLINE, FirstFrameGate, NO_FAILURE};
use crate::state::{
    FrameDecision, GpuProbe, OutputFrames, SYNC_WAIT_BUDGET, State, SyncWait, pick_gpu, retry_owed,
    scan_failed, sync_wait_decision,
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
    /// libinput, started at init. `None` only when the seat could not be assigned.
    libinput: Option<Libinput>,
    /// Seat name libinput is assigned to.
    seat: String,
    /// libinput keyboards, so Caps/Num/Scroll Lock LEDs follow the seat's xkb state.
    keyboards: Vec<smithay::reexports::input::Device>,
    /// LED state last written to `keyboards`.
    leds: smithay::input::keyboard::LedState,
    /// Card name of the scanout GPU, for the first-frame marker's `detail` and the log.
    gpu_name: String,
    /// `dev_t` of the GPU this backend drives; udev change events and resume events for it are ours.
    primary_dev: u64,
    surfaces: Vec<OutputSurface>,
    /// Render, vblank and device work are no-ops while the session is not ours.
    paused: bool,
    /// Why the render gate was last logged as closed, so each transition is reported once.
    gate: Option<&'static str>,
    retry_scheduled: bool,
    retry_attempts: u32,
    /// The queue/present boundary of this session: what reached KMS and what came back from it. It is
    /// also what the bounded startup deadline asks, and the only source of its counters.
    first_frame: FirstFrameGate,
    /// The one-shot deadline for the first presented frame. Armed at init and after a resume, and
    /// removed by nothing but a presented frame: a queued flip must never disarm it.
    first_frame_deadline: Option<RegistrationToken>,
    /// Per-step startup timings, reported once (first presented frame or failed startup).
    timing: StartupTiming,
    /// A deferred GPU fence and a wedged one are each reported once, not per paint.
    sync_defer_logged: bool,
    sync_fail_logged: bool,
    handle: LoopHandle<'static, State>,
}

/// Owns the running tty backend. `main` holds it for the whole run and hands it back to
/// [`finish`] on every exit path, including a panic.
pub struct DrmHandle(pub Rc<RefCell<Drm>>);

/// Starts the tty backend. Registers the session, DRM, udev and redraw-wake sources on `handle` and
/// starts libinput and renders the first frame; vblank events drive the rest.
///
/// On a failed startup the one-shot `startup timing` line is emitted with the steps that completed,
/// so a black session can be localized from the log alone.
///
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
fn init_backend(
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
    let renderer = unsafe { GlowRenderer::new(context)? };
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
    handle.insert_source(drm_notifier, move |event, _, state| {
        on_drm_event(state, &drm_events, event)
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

impl Drm {
    /// Stops every device exactly once and forgets all in-flight frames. Idempotent: a second
    /// pause before a resume does not suspend twice, which makes it safe from both the pause
    /// handler and [`finish`].
    ///
    /// Only a logind `PauseDevice` for the scanout card (or [`finish`]) reaches this.
    fn quiesce(&mut self) {
        self.retry_scheduled = false;
        self.retry_attempts = 0;
        for out in &mut self.surfaces {
            out.frames.park();
        }
        // A flip that was in flight is gone with the session, so the first-frame gate forgets it: a
        // vblank of the new owner of this CRTC must not be read as our presented frame.
        self.first_frame.pause();
        if self.paused {
            return;
        }
        self.paused = true;
        if let Some(libinput) = self.libinput.as_mut() {
            libinput.suspend();
        }
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

        // Cursor first: it is the topmost element and the one the cursor plane can take.
        let mut elements: Vec<Elem> =
            state
                .cursors
                .elements(&mut self.renderer, &state.cursor_status, state.pointer);
        elements.extend(render::build_elements(state, &mut self.renderer));

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

        // A deferred GPU fence owes the bounded retry too: the output is still dirty, so the retry
        // pass is what paints it again once the fence has had time to signal. That timer is a
        // *retry*, never a substitute for synchronizing: no frame is ever queued over an unfinished
        // buffer (see [`frame_sync_wait`]).
        let retry = self.surfaces.iter().any(|out| {
            retry_owed(
                out.frames.needs_retry,
                out.frames.dirty,
                out.frames.sync_waited_ms,
                self.paused,
            ) || (self.first_frame.queued_count() == 0 && out.frames.dirty)
        });
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
        // One entry per flip handed to KMS in this pass: the queue boundary, recorded after the loop
        // so the gate is never mutated while an output is borrowed.
        let mut queued: Vec<u32> = Vec::new();
        let mut last_error: Option<String> = None;
        let mut deferred = 0u32;
        let mut wedged = 0u32;
        for out in self.surfaces.iter_mut() {
            // `Deferred` is not a reason to paint: a frame is still in flight, so this output is
            // re-planned by the vblank that clears `queued`, and painting now would queue over it.
            if out.frames.plan(has_callback, self.paused) != FrameDecision::Draw {
                continue;
            }
            let already_waited = out.frames.sync_waited_ms;
            match out.compositor.render_frame(
                &mut self.renderer,
                elements,
                CLEAR,
                FrameFlags::DEFAULT,
            ) {
                Ok(result) if result.is_empty => {
                    // Nothing was drawn, so no vblank follows. A pending state sync is *not* a reason
                    // to arm a timer here: this pass owns no unfinished frame, so re-rendering after
                    // 16 ms would be the very retry that corrupts the pipeline.
                    out.frames.empty(false);
                    if self.first_frame.queued_count() == 0 {
                        // An empty *first* pass would otherwise leave the output black until an
                        // unrelated event pokes the scene. Anvil reschedules in this case too; the
                        // retry stays bounded by MAX_RETRIES and the first-frame deadline.
                        out.frames.wake();
                    }
                }
                Ok(result) => {
                    // Smithay's sync contract: with `needs_sync` the caller must synchronize with the
                    // render before this buffer goes to KMS, or the swapchain and the scanout get out
                    // of step and the kernel starts reporting buffer/IO errors.
                    let (decision, waited) = frame_sync_wait(&result, already_waited);
                    match decision {
                        SyncWait::Wait => match out.compositor.queue_frame(()) {
                            Ok(()) => {
                                out.frames.queue();
                                queued.push(u32::from(out.crtc));
                            }
                            Err(error) => {
                                out.frames.fail();
                                last_error =
                                    Some(format!("queue_frame {}: {error}", out.output.name()));
                                eprintln!(
                                    "flamewm-wayland: queue_frame failed on {}: {error}",
                                    out.output.name()
                                );
                            }
                        },
                        SyncWait::Defer | SyncWait::Fail => {
                            if decision == SyncWait::Fail {
                                wedged += 1;
                            } else {
                                deferred += 1;
                            }
                            // Not queued: the GPU is still writing that buffer. The output keeps its
                            // frame request and the bounded retry re-plans it.
                            out.frames.defer_sync(waited);
                        }
                    }
                }
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
                    last_error = Some(format!("render {}: {error}", out.output.name()));
                    eprintln!(
                        "flamewm-wayland: render failed on {}: {error}",
                        out.output.name()
                    );
                }
            }
        }
        if let Some(error) = last_error {
            self.first_frame.note_error(error);
        }
        // Logged once each, not once per paint: a wedged GPU would otherwise flood the log that a
        // hardware diagnosis depends on.
        if deferred > 0 && !self.sync_defer_logged {
            self.sync_defer_logged = true;
            eprintln!(
                "flamewm-wayland: frame sync detail=deferred outputs={deferred} detail=not_queued"
            );
        }
        if wedged > 0 && !self.sync_fail_logged {
            self.sync_fail_logged = true;
            eprintln!(
                "flamewm-wayland: frame sync detail=wedged outputs={wedged} detail=needs_retry"
            );
        }
        // The queue boundary, recorded after the loop so the gate is never mutated while an output is
        // borrowed. A queue is a diagnostic only: it starts nothing and disarms nothing.
        for crtc in queued {
            self.note_queued(crtc);
        }
        test_failed
    }

    /// The **queue** boundary: a flip was handed to KMS.
    ///
    /// This is a diagnostic and nothing else. `stage=firstframe status=ok code=7` says the pipeline
    /// got as far as submitting a frame, which is worth having in a hardware log — but a queued flip
    /// can still fail to complete, so this must not start libinput (that would grab the keyboard in
    /// front of a possibly black panel) and must not cancel the startup deadline (that would disarm
    /// the only way out of a flip that never comes back).
    fn note_queued(&mut self, crtc: u32) {
        if !self.first_frame.queued(crtc) {
            return;
        }
        self.timing.queue_ms = self.first_frame.queued_ms();
        eprintln!(
            "flamewm-wayland: first frame queued on {} crtc={crtc} detail=not_presented",
            self.gpu_name
        );
        stage_line(Stage::Firstframe, StageStatus::Ok, &self.gpu_name);
    }

    /// The **present** boundary: the flip we queued is on the panel. This is readiness.
    ///
    /// `stage=presented status=ok code=8` is emitted once, followed immediately by the one-shot
    /// `startup timing` summary, and only here is the bounded deadline disarmed. A vblank that does not
    /// belong to a queued flip of this output (`FirstFrameGate::presented` refuses it) changes nothing.
    fn note_presented(&mut self, crtc: u32) {
        // The guard first: only the CRTC of an output that has a flip in flight can present one.
        if !self.first_frame.presented_for(crtc) {
            return;
        }
        if !self.first_frame.presented(crtc) {
            return;
        }
        self.timing.present_ms = self.first_frame.presented_ms();
        if let Some(token) = self.first_frame_deadline.take() {
            self.handle.remove(token);
            eprintln!("flamewm-wayland: first frame presented, startup deadline cancelled");
        }
        eprintln!(
            "flamewm-wayland: first frame presented gpu={} crtc={crtc} t={}ms",
            self.gpu_name,
            elapsed_ms()
        );
        stage_line(
            Stage::Presented,
            StageStatus::Ok,
            &format!("{} crtc={crtc}", self.gpu_name),
        );
        self.timing.emit();
    }

    /// Arms the one bounded deadline for the first presented frame.
    ///
    /// Called once from [`init`] (before the first paint) and again after a resume, because a pause
    /// takes the session — and with it the deadline it consumed — away. It is never called from a
    /// queue, and it refuses to arm when the gate is already satisfied.
    fn arm_first_frame_deadline(&mut self, drm: &Rc<RefCell<Drm>>) {
        if self.first_frame.ready() || self.first_frame_deadline.is_some() {
            return;
        }
        let armed = Rc::clone(drm);
        match self.handle.insert_source(
            Timer::from_duration(FIRST_FRAME_DEADLINE),
            move |_, _, state| on_first_frame_deadline(state, &armed),
        ) {
            Ok(token) => self.first_frame_deadline = Some(token),
            Err(error) => {
                eprintln!("flamewm-wayland: first-frame deadline could not be armed: {error}");
            }
        }
    }

    /// Brings up libinput on the seat and registers its event source. Called once from [`init`].
    fn start_input(&mut self, drm: &Rc<RefCell<Drm>>) {
        if self.libinput.is_some() {
            return;
        }
        let step = Instant::now();
        let mut libinput = Libinput::new_with_udev::<LibinputSessionInterface<LogindSession>>(
            self.session.clone().into(),
        );
        if libinput.udev_assign_seat(&self.seat).is_err() {
            // No device is grabbed: libinput only takes the ones it could open.
            stage_line(Stage::Libinput, StageStatus::Fail, "seat_assign");
            return;
        }
        let elapsed = step.elapsed().as_millis();
        let input_backend = LibinputInputBackend::new(libinput.clone());
        self.libinput = Some(libinput);
        let input_drm = Rc::clone(drm);
        let seat = self.seat.clone();
        if let Err(error) = self
            .handle
            .insert_source(input_backend, move |event, _, state| {
                input::handle_input(state, &input_drm, event)
            })
        {
            eprintln!(
                "flamewm-wayland: input source registration failed: {error}; releasing the seat"
            );
            if let Some(libinput) = self.libinput.as_mut() {
                libinput.suspend();
            }
            stage_line(Stage::Libinput, StageStatus::Fail, "source_register");
            return;
        }
        stage_line(
            Stage::Libinput,
            StageStatus::Ok,
            &format!("{seat} devices elapsed_{elapsed}ms"),
        );
    }

    /// Reopens the input devices logind handed back after a pause.
    ///
    /// A no-op when libinput could not be started.
    fn resume_input(&mut self) {
        let Some(libinput) = self.libinput.as_mut() else {
            return;
        };
        if libinput.resume().is_err() {
            eprintln!("flamewm-wayland: libinput resume failed");
        }
    }

    /// A vblank arrived for `crtc`.
    ///
    /// `DrmCompositor::frame_submitted` answers `Some(..)` when the atomic commit this output queued
    /// completed — that is the *present* boundary (`smithay-0.7.0/src/backend/drm/compositor/mod.rs:2604`:
    /// "Needs to be called, after the vblank event of the matching `DrmDevice` was received after
    /// calling `queue_frame`"). `None` means no flip of ours was pending, so this event is not
    /// evidence that anything was presented and must not satisfy the gate. The vblank also releases
    /// the output, so the next plan sees requests that arrived while the frame was in flight.
    fn on_vblank(&mut self, state: &mut State, crtc: crtc::Handle, drm: &Rc<RefCell<Drm>>) {
        let Some(index) = self.surfaces.iter().position(|out| out.crtc == crtc) else {
            return;
        };
        let presented = {
            let out = &mut self.surfaces[index];
            out.frames.presented();
            match out.compositor.frame_submitted() {
                Ok(submitted) => submitted.is_some(),
                Err(error) => {
                    out.frames.fail();
                    eprintln!(
                        "flamewm-wayland: frame_submitted failed on {}: {error}",
                        out.output.name()
                    );
                    false
                }
            }
        };
        if presented {
            self.note_presented(u32::from(crtc));
        }
        self.paint(state, drm);
    }
}

/// Synchronizes with the GPU fence of a frame that `render_frame` reported as `needs_sync`, bounded.
///
/// This is Smithay's documented contract, not an optimisation: `DrmCompositor::queue_frame` says
/// "it is your responsibility to synchronize rendering if the `RenderFrameResult` returned by the
/// previous `render_frame` call returns `true` on `RenderFrameResult::needs_sync`"
/// (`smithay-0.7.0/src/backend/drm/compositor/mod.rs:2424`), and `RenderFrameResult::needs_sync` says
/// the same for the swapchain element (`.../compositor/frame_result.rs:59`). `anvil` implements it by
/// waiting on the element's sync point (`anvil/src/udev.rs:1641-1643`).
///
/// The APIs used here, all from the pinned `smithay` 0.7.0:
/// * `RenderFrameResult::needs_sync` — `.../drm/compositor/frame_result.rs:60`;
/// * `PrimaryPlaneElement::Swapchain(PrimarySwapchainElement { sync, .. })` — same file, `:182`, `:435`;
/// * `SyncPoint::is_reached`/`get::<EGLFence>` — `.../renderer/sync/mod.rs:77` and `:70`;
/// * `EGLFence::client_wait(Some(timeout), flush)` (`eglClientWaitSyncKHR`) —
///   `.../egl/fence.rs:157`, the only fence wait in this version that accepts a timeout.
///
/// `already_waited` is the cumulative time this output has spent on fences since it last queued a
/// frame; it is what makes repeated passes end in `SyncWait::Fail` instead of waiting forever.
/// Returns the decision and the new cumulative wait.
fn frame_sync_wait<B, F, E>(
    result: &RenderFrameResult<'_, B, F, E>,
    already_waited: u128,
) -> (SyncWait, u128)
where
    B: Buffer,
    F: Framebuffer,
{
    if !result.needs_sync() {
        // KMS waits for our rendering itself: nothing to synchronize, the frame may be queued.
        return (SyncWait::Wait, 0);
    }
    let Some(element) = (match &result.primary_element {
        PrimaryPlaneElement::Swapchain(element) => Some(element),
        // Direct scan-out of a client buffer: no composited buffer of ours is in flight.
        PrimaryPlaneElement::Element(_) => None,
    }) else {
        return (SyncWait::Wait, 0);
    };
    let sync: &SyncPoint = &element.sync;
    if sync.is_reached() {
        // The fence already signaled: the wait below would not block at all.
        return (SyncWait::Wait, 0);
    }
    let Some(fence) = sync.get::<EGLFence>() else {
        // A fence type without a timeout API. Never block the compositor thread on it: report and let
        // the output be retried. Smithay's GLES renderer only produces an `EGLFence` or an already
        // signaled point (`.../renderer/gles/mod.rs:2328-2345`), so this is a safety net.
        return (SyncWait::Fail, already_waited);
    };
    let start = Instant::now();
    // One bounded kernel wait, not a spin: `eglClientWaitSyncKHR` returns as soon as the fence
    // signals, and at the latest after the budget.
    let signaled = fence
        .client_wait(Some(SYNC_WAIT_BUDGET), false)
        .unwrap_or(false);
    let waited = already_waited.saturating_add(start.elapsed().as_millis());
    if signaled {
        return (SyncWait::Wait, 0);
    }
    (sync_wait_decision(true, waited), waited)
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
        // A forced probe reads EDID and can stall for seconds on a flaky external connector, which
        // delays the panel behind it. The cached state is enough unless it reports a connected
        // connector with no modes.
        let mut info = match d.device.get_connector(conn, false) {
            Ok(info) => info,
            Err(error) => {
                skip_connector(&d.device, conn, &format!("read {error}"));
                continue;
            }
        };
        if info.state() == connector::State::Connected && info.modes().is_empty() {
            match d.device.get_connector(conn, true) {
                Ok(forced) => info = forced,
                Err(error) => {
                    skip_connector(&d.device, conn, &format!("read {error}"));
                    continue;
                }
            }
        }
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

/// True when the DRM driver identifies as NVIDIA; anvil disables overlay planes for that driver
/// because using them breaks the atomic commit and the first frame never reaches the panel.
fn is_nvidia(device: &DrmDevice) -> bool {
    device
        .get_driver()
        .map(|driver| {
            let name = driver.name().to_string_lossy().to_lowercase();
            let description = driver.description().to_string_lossy().to_lowercase();
            name.contains("nvidia") || description.contains("nvidia")
        })
        .unwrap_or(false)
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
    eprintln!("flamewm-wayland: init phase=create_surface");
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
    // Anvil disables overlay planes on NVIDIA: using them there breaks the atomic commit and the
    // first frame never reaches the panel. Dropping them costs one extra composition, not correctness.
    let mut planes = surface.planes().clone();
    if is_nvidia(&d.device) {
        planes.overlay.clear();
    }
    eprintln!("flamewm-wayland: init phase=compositor");
    let compositor = match DrmCompositor::new(
        &output,
        surface,
        Some(planes),
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

/// The one bounded deadline for the first presented frame fired.
///
/// Nothing else in the backend ends a startup on its own, so this is the only way out of a session
/// that owns DRM master and has nothing on the panel. It names why — `no_first_queue` when not even a
/// flip was queued, `pageflip_timeout` when one was queued and never came back (the black-panel case)
/// — with the counters, the CRTC, the elapsed milliseconds and the last render error, then stops the
/// loop so `main` exits non-zero and the backend is torn down. Input was never started at this point,
/// so the keyboard still reaches the kernel either way.
fn on_first_frame_deadline(state: &mut State, drm: &Rc<RefCell<Drm>>) -> TimeoutAction {
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

/// One logind event. Pauses stop scanout and then answer logind; resumes reclaim the device.
fn on_logind_event(state: &mut State, drm: &Rc<RefCell<Drm>>, event: LogindEvent) {
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
/// the rebuild path instead of pretending the device works.
fn kept_fd_resume(
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
fn pauses_scanout(primary_dev: u64, major: u32, minor: u32) -> bool {
    smithay::reexports::rustix::fs::makedev(major, minor) == primary_dev
}

/// A resume logind delivered with a descriptor. Only the primary scanout device is ours; input
/// devices are reopened by libinput through the session's `TakeDevice`.
fn resume_device(state: &mut State, drm: &Rc<RefCell<Drm>>, major: u32, minor: u32, fd: OwnedFd) {
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
    d.resume_input();
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
    // Same rule as the kept-fd resume: the session is ours again, so the startup deadline starts over.
    d.first_frame.resume();
    d.arm_first_frame_deadline(drm);
    state.request_full_redraw();
    Ok(())
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
    render::send_cursor_frame(state, time);
}

#[cfg(test)]
mod tests {
    use super::pauses_scanout;
    use smithay::reexports::rustix::fs::makedev;

    #[test]
    fn only_the_scanout_card_pauses_scanout() {
        let card = makedev(226, 1);
        assert!(pauses_scanout(card, 226, 1));
        assert!(!pauses_scanout(card, 226, 0), "another card");
        assert!(
            !pauses_scanout(card, 13, 64),
            "an evdev node released by libinput"
        );
    }
}

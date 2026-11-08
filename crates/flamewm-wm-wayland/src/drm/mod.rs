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
//! Not wired yet: clients use `wl_shm`. dmabuf import needs smithay's `ImportEgl::bind_wl_display`,
//! which is compiled only with smithay's `use_system_lib` feature, plus a `zwp_linux_dmabuf_v1`
//! global. Neither is available here, so no dmabuf global is offered.

use std::cell::RefCell;
use std::error::Error;
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use flamewm_reactor::BorrowedRawFd;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{Buffer, Format, Fourcc};
use smithay::backend::drm::compositor::{
    DrmCompositor, FrameError, FrameFlags, PrimaryPlaneElement, RenderFrameError, RenderFrameResult,
};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::{
    DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmError, DrmEvent, DrmEventMetadata, DrmEventTime,
    Framebuffer,
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
use smithay::desktop::utils::{OutputPresentationFeedback, send_frames_surface_tree};
use smithay::desktop::{PopupManager, layer_map_for_output};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::ping::make_ping;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Interest, PostAction};
use smithay::reexports::drm::Device as _;
use smithay::reexports::drm::control::{
    Device as ControlDevice, Mode as DrmMode, ModeTypeFlags, ResourceHandles, connector, crtc,
};
use smithay::reexports::input::Libinput;
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::utils::{DeviceFd, Size, Transform};

use crate::Stage;
use crate::StageStatus;
use crate::StartupTiming;
use crate::elapsed_ms;
use crate::logind::{Event as LogindEvent, LogindSession, PauseKind};
use crate::output_layout::{ModeCandidate, select_mode};
use crate::presentation_scale;
mod init;
pub use init::*;
mod sync;
use sync::*;
mod outputs;
use outputs::*;
mod events;
use events::*;
mod resume;
use resume::*;

mod device;
mod hotplug;
mod input;

use crate::render::{self, Elem};
use crate::stage_line;
use crate::startup::{FIRST_FRAME_DEADLINE, FirstFrameGate, NO_FAILURE};
use crate::state::{
    FrameDecision, GpuProbe, OutputFrames, SYNC_WAIT_BUDGET, SceneBuild, State, SyncWait, pick_gpu,
    retry_owed, scan_failed, scene_build, sync_wait_decision,
};

/// The feedback of the frame a flip carries, handed back by `frame_submitted` on its vblank.
type Compositor = DrmCompositor<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    Option<OutputPresentationFeedback>,
    DrmDeviceFd,
>;

const CLEAR: [f32; 4] = [0.07, 0.08, 0.11, 1.0];
/// Delay of the retry armed only for a real sync or buffer error.
const FRAME_RETRY: Duration = Duration::from_millis(16);
/// How often one error may be retried before the output is left alone.
const MAX_RETRIES: u32 = 3;

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

/// The scene element list of the last full paint. Its first `cursor_len` entries are the cursor.
#[derive(Default)]
struct SceneCache {
    elements: Vec<Elem>,
    cursor_len: usize,
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
    /// Last full scene (cursor first), reused by cursor-only paints. `None` until the first paint and
    /// after a session pause, when its textures can no longer be trusted.
    scene: Option<SceneCache>,
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
impl Drm {
    /// Stops every device exactly once and forgets all in-flight frames. Idempotent: a second
    /// pause before a resume does not suspend twice, which makes it safe from both the pause
    /// handler and [`finish`].
    ///
    /// Only a logind `PauseDevice` for the scanout card (or [`finish`]) reaches this.
    fn quiesce(&mut self) {
        self.retry_scheduled = false;
        self.retry_attempts = 0;
        self.scene = None;
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
        state.flush_pending_configure();
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

        // Cursor first: it is the topmost element and the one the cursor plane can take. It is rebuilt
        // on every paint, since its position is the common reason to paint at all.
        let cursor = state.cursors.elements(
            &mut self.renderer,
            &state.cursor_status,
            state.pointer,
            state.output_scale(),
        );
        let scene_dirty = state.take_scene_dirty();
        let cached = self.scene.take();
        let scene = if let (SceneBuild::CursorOnly, Some(mut scene)) =
            (scene_build(cached.is_some(), scene_dirty), cached)
        {
            // Pointer motion: the window, decoration and backdrop elements are still valid, so
            // only the cursor slice of the cached list is swapped.
            let count = cursor.len();
            scene
                .elements
                .splice(0..scene.cursor_len, cursor)
                .for_each(drop);
            scene.cursor_len = count;
            scene
        } else {
            // Same blurred desktop capture as the winit path, so DRM menus and titlebars match.
            let output_size = state.physical_output_size();
            if let Err(error) =
                render::refresh_desktop_backdrop(state, &mut self.renderer, output_size)
            {
                eprintln!("flamewm-wayland: desktop backdrop failed: {error}");
            }
            let cursor_len = cursor.len();
            let mut elements = cursor;
            elements.extend(render::build_elements(state, &mut self.renderer));
            SceneCache {
                elements,
                cursor_len,
            }
        };
        let elements = &scene.elements;

        // `FLAMEWM_WAYLAND_DUMP`: one readback of the primary output. Taking the path makes a set
        // request be consumed exactly once, and `state.dumped` stops main.rs's re-arming timer.
        if let Some(path) = state.dump.take() {
            let size = state.physical_output_size();
            match render::dump_offscreen(
                &mut self.renderer,
                &self.surfaces[0].output,
                elements,
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

        if self.draw_pass(state, elements, has_callback) {
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
                    self.draw_pass(state, elements, has_callback);
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
            let _ = self.handle.insert_source(
                Timer::from_duration(FRAME_RETRY),
                move |_, (), state| {
                    if let Ok(mut d) = again.try_borrow_mut() {
                        d.retry_scheduled = false;
                    }
                    render_all(state, &again);
                    TimeoutAction::Drop
                },
            );
        } else if !retry {
            self.retry_attempts = 0;
        }
        self.scene = Some(scene);
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
    fn draw_pass(&mut self, state: &State, elements: &[Elem], has_callback: bool) -> bool {
        let mut test_failed = false;
        // One entry per flip handed to KMS in this pass: the queue boundary, recorded after the loop
        // so the gate is never mutated while an output is borrowed.
        let mut queued: Vec<u32> = Vec::new();
        let mut last_error: Option<String> = None;
        let mut deferred = 0u32;
        let mut wedged = 0u32;
        for out in &mut self.surfaces {
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
                        SyncWait::Wait => match out.compositor.queue_frame(Some(
                            presentation_scale::collect_feedback(state, &out.output),
                        )) {
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
            move |_, (), state| on_first_frame_deadline(state, &armed),
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
            .insert_source(input_backend, move |event, (), state| {
                input::handle_input(state, &input_drm, event);
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
    fn on_vblank(
        &mut self,
        state: &mut State,
        crtc: crtc::Handle,
        drm: &Rc<RefCell<Drm>>,
        vblank: Option<DrmEventMetadata>,
    ) {
        let Some(index) = self.surfaces.iter().position(|out| out.crtc == crtc) else {
            return;
        };
        let (presented, feedback) = {
            let out = &mut self.surfaces[index];
            out.frames.presented();
            match out.compositor.frame_submitted() {
                Ok(submitted) => (submitted.is_some(), submitted.flatten()),
                Err(error) => {
                    out.frames.fail();
                    eprintln!(
                        "flamewm-wayland: frame_submitted failed on {}: {error}",
                        out.output.name()
                    );
                    (false, None)
                }
            }
        };
        if presented {
            if let Some(feedback) = feedback {
                let time = match vblank.map(|m| m.time) {
                    Some(DrmEventTime::Monotonic(time)) => Some(time),
                    _ => None,
                };
                let sequence = vblank.map_or(0, |m| m.sequence);
                presentation_scale::presented_on_vblank(
                    feedback,
                    &self.surfaces[index].output,
                    time,
                    sequence,
                );
            }
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

//! Hosts the voice OSD in a child process of this binary (`flamewm-assistant --osd`).
//!
//! The chat window runs its own event loop with one surface, so the overlay needs a loop of its
//! own. The child exists only while the OSD is visible: no window and no timer when hidden.
//! The parent writes one state word per line to the child's stdin (`listening`, `working`,
//! `speaking`, `hide`) and reads `click` lines from its stdout.

use std::cell::{Cell, RefCell};
use std::env;
use std::io::{BufRead, BufReader, Write, stdin, stdout};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use flamewm_reactor::Reactor;
use flamewm_ui_x11::{
    ActionPhase, SurfaceConfig, SurfaceEvent, SurfaceHandle, SurfaceInputMode, SurfaceRole,
    SurfaceRuntime, UiBackendError, UiTemplate, decode_document, run_surface_runtime_until,
};

use crate::osd::{self, Osd, OsdFrame, OsdState};

const OSD_UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-assistant-osd.rwr"));
/// Command-line flag that selects the OSD child in `main`.
pub const HOST_ARG: &str = "--osd";
const CLICK_ACTION: &str = "osd.click";
const CLICK_LINE: &str = "click";
/// Gap between the top edge of the work area and the OSD when it is docked at the top.
const TOP_MARGIN: i32 = 24;

fn state_word(state: OsdState) -> &'static str {
    match state {
        OsdState::Hidden => "hide",
        OsdState::Listening => "listening",
        OsdState::Working => "working",
        OsdState::Speaking => "speaking",
    }
}

fn parse_state(line: &str) -> Option<OsdState> {
    match line {
        "listening" => Some(OsdState::Listening),
        "working" => Some(OsdState::Working),
        "speaking" => Some(OsdState::Speaking),
        _ => None,
    }
}

/// Parent side: keeps at most one OSD child and forwards state words to it.
#[derive(Default)]
pub struct OsdProcess {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
}

impl OsdProcess {
    /// Make the screen match `state`. `click` runs on a reader thread when the user clicks the OSD.
    /// Hidden asks the child to exit; the child is spawned only for a visible state.
    pub fn sync(&mut self, state: OsdState, click: impl Fn() + Send + 'static) {
        let exited = match self.child.as_mut() {
            Some(child) => !matches!(child.try_wait(), Ok(None)),
            None => false,
        };
        if exited {
            self.child = None;
            self.stdin = None;
        }
        if state == OsdState::Hidden {
            if let Some(mut stdin) = self.stdin.take() {
                let _ = writeln!(stdin, "{}", state_word(state));
            }
            self.child = None;
            return;
        }
        if self.stdin.is_none() {
            let Ok(exe) = env::current_exe() else { return };
            let Ok(mut child) = Command::new(exe)
                .arg(HOST_ARG)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
            else {
                return;
            };
            if let Some(out) = child.stdout.take() {
                thread::spawn(move || {
                    for line in BufReader::new(out).lines() {
                        match line {
                            Ok(line) if line == CLICK_LINE => click(),
                            Ok(_) => {}
                            Err(_) => break,
                        }
                    }
                });
            }
            self.stdin = child.stdin.take();
            self.child = Some(child);
        }
        let failed = match self.stdin.as_mut() {
            Some(stdin) => writeln!(stdin, "{}", state_word(state))
                .and_then(|()| stdin.flush())
                .is_err(),
            None => false,
        };
        if failed {
            self.stdin = None;
            self.child = None;
        }
    }
}

struct Host {
    osd: Osd,
    surface: SurfaceHandle,
    rx: Receiver<String>,
    done: bool,
}

/// Child side: runs one overlay surface until it receives `hide` or stdin closes.
pub fn run_host() -> Result<(), String> {
    let mut runtime =
        SurfaceRuntime::new().map_err(|error| format!("create UI runtime: {error:?}"))?;
    let document = decode_document(OSD_UI)?;
    let surface = runtime
        .create_surface(
            UiTemplate::new(document),
            SurfaceConfig {
                width: osd::WIDTH,
                height: osd::HEIGHT,
                title: "FlameWM Assistant OSD".into(),
                role: SurfaceRole::Overlay,
                input: SurfaceInputMode::Interactive,
                initially_visible: false,
                x: 0,
                y: 0,
            },
        )
        .map_err(|error| format!("create OSD surface: {error:?}"))?;
    runtime.watch_root_changes();

    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line.trim().to_owned()).is_err() {
                return;
            }
        }
        // The parent closed its end: nothing can drive the OSD any more.
        let _ = tx.send(state_word(OsdState::Hidden).into());
    });

    let mut reactor = Reactor::new().map_err(|error| error.to_string())?;
    let pending_frames = Rc::new(Cell::new(0u32));
    let frames = Rc::clone(&pending_frames);
    reactor
        .register_timer(Osd::frame_interval(), true, move || {
            frames.set(frames.get().saturating_add(1));
        })
        .map_err(|error| error.to_string())?;

    let host = Rc::new(RefCell::new(Host {
        osd: Osd::new(),
        surface,
        rx,
        done: false,
    }));
    let tick_host = Rc::clone(&host);
    let stop_host = Rc::clone(&host);
    run_surface_runtime_until(
        &mut runtime,
        &mut reactor,
        |event: SurfaceEvent, _runtime: &mut SurfaceRuntime| {
            if event.action.action == CLICK_ACTION && event.action.phase == ActionPhase::Release {
                let mut out = stdout().lock();
                let _ = writeln!(out, "{CLICK_LINE}").and_then(|()| out.flush());
            }
            Ok(())
        },
        move |runtime: &mut SurfaceRuntime| {
            tick(runtime, &mut tick_host.borrow_mut(), &pending_frames)
        },
        move |_runtime: &SurfaceRuntime| !stop_host.borrow().done,
    )
    .map_err(|error| format!("run OSD: {error:?}"))
}

fn tick(
    runtime: &mut SurfaceRuntime,
    host: &mut Host,
    pending_frames: &Cell<u32>,
) -> Result<(), UiBackendError> {
    for line in host.rx.try_iter().collect::<Vec<_>>() {
        if line == state_word(OsdState::Hidden) {
            host.osd.hide();
            host.done = true;
            return runtime.hide(host.surface);
        }
        let Some(state) = parse_state(&line) else {
            continue;
        };
        host.osd.show(state);
        place(runtime, host.surface, state)?;
        runtime.show(host.surface)?;
    }
    for _ in 0..pending_frames.replace(0) {
        host.osd.tick();
    }
    if host.osd.wants_timer() {
        let frame: OsdFrame = host.osd.view();
        runtime.with_document(host.surface, |doc| osd::paint(&frame, doc))?;
        runtime.mark_full(host.surface)?;
        runtime.redraw_dirty()?;
    }
    Ok(())
}

/// Listening is centred on the work area; the other states dock at the top centre.
fn place(
    runtime: &mut SurfaceRuntime,
    surface: SurfaceHandle,
    state: OsdState,
) -> Result<(), UiBackendError> {
    let (root_w, root_h) = runtime.root_geometry();
    let x = (root_w as i32 - osd::WIDTH as i32) / 2;
    let y = match state {
        OsdState::Listening => (root_h as i32 - osd::HEIGHT as i32) / 2,
        _ => TOP_MARGIN,
    };
    runtime.move_resize(surface, x, y, osd::WIDTH, osd::HEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_words_round_trip_and_hide_is_not_a_show_state() {
        for state in [OsdState::Listening, OsdState::Working, OsdState::Speaking] {
            assert_eq!(parse_state(state_word(state)), Some(state));
        }
        assert_eq!(state_word(OsdState::Hidden), "hide");
        assert_eq!(parse_state("hide"), None);
        assert_eq!(parse_state("bogus"), None);
    }
}

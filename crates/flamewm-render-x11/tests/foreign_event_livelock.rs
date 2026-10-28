//! Event-loop liveness on a private Xvfb: events this client receives for windows other than its
//! own (client messages other clients address to the root, as EWMH requests do) must be dropped,
//! never re-queued. A re-queued event is read again on the next pass forever: the app spins at
//! full CPU and never handles input, paints or closes again.
//! Skips with a message when no `Xvfb` binary is installed or no display is free.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use flamewm_reactor::Reactor;
use flamewm_render_core::{CompiledDocument, CompiledNode, NodeKind, RuntimeDocument, Style};
use flamewm_render_x11::{
    RawPressWatch, WindowGeometrySlot, X11Config, X11WindowRole,
    run_with_controller_events_with_reactor_geometry,
};
use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask};
use x11rb::rust_connection::RustConnection;

const TITLE: &str = "foreign-event-livelock-test";
const WAIT: Duration = Duration::from_secs(10);

struct TestServer(std::process::Child);

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_server() -> Option<(TestServer, String)> {
    for n in 120..140 {
        let display = format!(":{n}");
        if Path::new(&format!("/tmp/.X{n}-lock")).exists() {
            continue;
        }
        let child = Command::new("Xvfb")
            .args([&display, "-screen", "0", "640x480x24", "-nolisten", "tcp"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let server = TestServer(child);
        let deadline = Instant::now() + WAIT;
        while Instant::now() < deadline {
            if x11rb::connect(Some(&display)).is_ok() {
                let owned = std::fs::read_to_string(format!("/tmp/.X{n}-lock"))
                    .is_ok_and(|pid| pid.trim() == server.0.id().to_string());
                if owned {
                    return Some((server, display));
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    None
}

fn document() -> RuntimeDocument {
    RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes: vec![CompiledNode {
            kind: NodeKind::Element,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: std::borrow::Cow::Borrowed(""),
            action: std::borrow::Cow::Borrowed(""),
            text: std::borrow::Cow::Borrowed(""),
            image: None,
            style: Style::default().into(),
            hover_style: None,
            active_style: None,
        }],
    })
    .expect("single-node fixture validates")
}

fn atom(conn: &RustConnection, name: &str) -> u32 {
    conn.intern_atom(false, name.as_bytes())
        .expect("intern request")
        .reply()
        .expect("intern reply")
        .atom
}

fn find_window(conn: &RustConnection, title: &str) -> Option<u32> {
    let root = conn.setup().roots[0].root;
    let children = conn.query_tree(root).ok()?.reply().ok()?.children;
    children.into_iter().find(|&window| {
        conn.get_property(false, window, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 256)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|reply| reply.value == title.as_bytes())
    })
}

fn connect_test(display: &str) -> RustConnection {
    let deadline = Instant::now() + WAIT;
    loop {
        match x11rb::connect(Some(display)) {
            Ok((connection, _)) => return connection,
            Err(error) if Instant::now() >= deadline => panic!("test connection: {error}"),
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

#[test]
fn events_for_other_windows_never_stall_the_event_loop() {
    let Some((_server, display)) = start_server() else {
        eprintln!("skipped: Xvfb not available or no free display");
        return;
    };
    // SAFETY: this integration test is the only code in its process touching DISPLAY, and it
    // sets the variable before any display connection is opened.
    unsafe { std::env::set_var("DISPLAY", &display) };

    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let config = X11Config {
            width: 200,
            height: 100,
            x: 20,
            y: 20,
            title: TITLE.to_string(),
            role: X11WindowRole::Normal,
            identity: None,
            backdrop: None,
            file_drag: None,
        };
        let result = Reactor::new()
            .map_err(|error| error.to_string())
            .and_then(|mut reactor| {
                run_with_controller_events_with_reactor_geometry(
                    document(),
                    config,
                    &mut reactor,
                    WindowGeometrySlot::default(),
                    RawPressWatch::default(),
                    |_, _| Ok(()),
                    |_, _| Ok(()),
                    |_| Ok(()),
                )
            });
        let _ = done.send(result);
    });

    let conn = connect_test(&display);
    let root = conn.setup().roots[0].root;
    let deadline = Instant::now() + WAIT;
    let window = loop {
        if let Some(window) = find_window(&conn, TITLE) {
            break window;
        }
        assert!(Instant::now() < deadline, "app window appears");
        std::thread::sleep(Duration::from_millis(10));
    };
    let send = |window_field: u32, kind: u32, data: [u32; 5]| {
        // An empty event mask delivers to the client that created the destination window.
        let event = ClientMessageEvent::new(32, window_field, kind, data);
        conn.send_event(false, window, EventMask::NO_EVENT, event)
            .expect("send");
        conn.flush().expect("flush");
    };
    let active = atom(&conn, "_NET_ACTIVE_WINDOW");
    let state = atom(&conn, "_NET_WM_STATE");
    for _ in 0..16 {
        send(root, active, [2, CURRENT_TIME, 0, 0, 0]);
        send(root, state, [1, 0, 0, 1, 0]);
    }

    let protocols = atom(&conn, "WM_PROTOCOLS");
    let delete = atom(&conn, "WM_DELETE_WINDOW");
    send(window, protocols, [delete, CURRENT_TIME, 0, 0, 0]);
    let result = finished
        .recv_timeout(WAIT)
        .expect("the loop handles WM_DELETE_WINDOW after the foreign events instead of spinning");
    result.expect("app loop returns cleanly after close");
}

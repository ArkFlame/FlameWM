//! XDND drop target survives a drag source that vanishes mid-drag. The target answers
//! XdndPosition with XdndStatus sent to the source window; once the source is destroyed that
//! XSendEvent fails with BadWindow. The protocol error must be non-fatal: the target loop keeps
//! running and still closes through WM_DELETE_WINDOW.
//! Runs on a private Xvfb on displays 240..260 (never the host or the shared 96..120 range).
//! Skips with a message when no `Xvfb` binary is installed or no display is free.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use flamewm_reactor::Reactor;
use flamewm_render_core::{CompiledDocument, CompiledNode, NodeKind, RuntimeDocument, Style};
use flamewm_render_x11::{
    FileDragSlot, RawPressWatch, WindowGeometrySlot, X11Config, X11WindowRole,
    run_with_controller_events_with_reactor_geometry,
};
use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, WindowClass,
};
use x11rb::rust_connection::RustConnection;

const TARGET_TITLE: &str = "xdnd-source-gone-target";
const WAIT: Duration = Duration::from_secs(10);
const TARGET_X: i32 = 40;
const TARGET_Y: i32 = 50;

struct TestServer(std::process::Child);

impl Drop for TestServer {
    /// Kills only the Xvfb process this test started.
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_server() -> Option<(TestServer, String)> {
    for n in 240..260 {
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
                // Accept the display only when its lock file names the Xvfb this test started.
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
    let net_name = atom(conn, "_NET_WM_NAME");
    let utf8 = atom(conn, "UTF8_STRING");
    let children = conn.query_tree(root).ok()?.reply().ok()?.children;
    children.into_iter().find(|&window| {
        let net = conn
            .get_property(false, window, net_name, utf8, 0, 256)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        let legacy = conn
            .get_property(false, window, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 256)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        [net, legacy]
            .into_iter()
            .flatten()
            .any(|reply| reply.value == title.as_bytes())
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

fn wait_for<T>(mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if let Some(value) = probe() {
            return Some(value);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    None
}

#[test]
fn xdnd_target_survives_source_destroyed_mid_drag() {
    let Some((_server, display)) = start_server() else {
        eprintln!("skipped: Xvfb not available or no free display in 240..260");
        return;
    };
    // SAFETY: this integration test is the only code in its process touching DISPLAY, and it
    // sets the variable before any display connection is opened.
    unsafe { std::env::set_var("DISPLAY", &display) };

    let target = std::thread::spawn(|| {
        let config = X11Config {
            width: 200,
            height: 100,
            x: TARGET_X,
            y: TARGET_Y,
            title: TARGET_TITLE.to_string(),
            role: X11WindowRole::Normal,
            identity: None,
            backdrop: None,
            file_drag: Some(FileDragSlot::default()),
        };
        let mut reactor = Reactor::new().map_err(|error| error.to_string())?;
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

    let conn = connect_test(&display);
    let root = conn.setup().roots[0].root;
    let window = wait_for(|| find_window(&conn, TARGET_TITLE)).expect("target window appears");

    let enter = atom(&conn, "XdndEnter");
    let position = atom(&conn, "XdndPosition");
    let protocols = atom(&conn, "WM_PROTOCOLS");
    let delete = atom(&conn, "WM_DELETE_WINDOW");
    let copy = atom(&conn, "XdndActionCopy");
    let uri_list = atom(&conn, "text/uri-list");
    let text_plain = atom(&conn, "text/plain");

    // The external source: an input-only window that starts a drag, then disappears in the same
    // flush as its Enter and Position. The target's XdndStatus reply then targets a dead window.
    let source = conn.generate_id().expect("id");
    conn.create_window(
        0,
        source,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new(),
    )
    .expect("source window");
    let send = |to: u32, kind: u32, data: [u32; 5]| {
        conn.send_event(
            false,
            to,
            EventMask::NO_EVENT,
            ClientMessageEvent::new(32, to, kind, data),
        )
        .expect("send");
    };
    send(window, enter, [source, 5 << 24, uri_list, text_plain, 0]);
    let (root_x, root_y) = (TARGET_X + 20, TARGET_Y + 20);
    let packed = ((root_x as u16 as u32) << 16) | (root_y as u16 as u32);
    send(window, position, [source, 0, packed, 1234, copy]);
    conn.destroy_window(source).expect("destroy source");
    conn.flush().expect("flush");

    // Give the target time to read Enter/Position and attempt its XdndStatus reply.
    std::thread::sleep(Duration::from_millis(800));
    assert!(
        !target.is_finished(),
        "target loop must survive the BadWindow from replying to a destroyed source"
    );
    assert!(
        find_window(&conn, TARGET_TITLE).is_some(),
        "target window still exists after the vanished source"
    );

    // Close through the normal WM_DELETE_WINDOW path: the loop must still return cleanly.
    send(window, protocols, [delete, CURRENT_TIME, 0, 0, 0]);
    conn.flush().expect("flush");
    target
        .join()
        .expect("target thread did not panic")
        .expect("target window loop returns after close");
}

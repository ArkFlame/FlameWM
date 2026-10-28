//! XDND drop target on a private Xvfb: an XDND-enabled FlameWM window receives a file drop
//! from an external XDND source (simulated over x11rb) and reports the dropped paths.
//! Skips with a message when no `Xvfb` binary is installed or no display is free.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flamewm_reactor::Reactor;
use flamewm_render_core::{CompiledDocument, CompiledNode, NodeKind, RuntimeDocument, Style};
use flamewm_render_x11::{
    ActionEvent, FILE_DRAG_DROP_ACTION, FILE_DRAG_ENTER_ACTION, FileDragSlot, RawPressWatch,
    WindowGeometrySlot, X11Config, X11WindowRole, parse_uri_list,
    run_with_controller_events_with_reactor_geometry,
};
use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, PropMode,
    SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, WindowClass,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const TARGET_TITLE: &str = "xdnd-target-test";
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
    for n in 96..120 {
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
                // Another server may have claimed this number after the lock check; accept
                // the display only when its lock file names the Xvfb this test started.
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

/// Finds the top-level window whose `_NET_WM_NAME` or `WM_NAME` equals `title`.
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

/// Opens a test connection, retrying briefly: a freshly started private server can reset the
/// first connection attempt (observed intermittently on Xvfb), and the retry is bounded by WAIT.
fn connect_test(display: &str) -> (RustConnection, usize) {
    let deadline = Instant::now() + WAIT;
    loop {
        match x11rb::connect(Some(display)) {
            Ok(connection) => return connection,
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

/// Next client message of `kind` addressed to `source`, as its five data longs.
fn next_message(conn: &RustConnection, kind: u32) -> Option<[u32; 5]> {
    wait_for(|| {
        while let Some(event) = conn.poll_for_event().ok()? {
            if let Event::ClientMessage(message) = event
                && message.type_ == kind
            {
                return Some(message.data.as_data32());
            }
        }
        None
    })
}

/// One private Xvfb for every XDND scenario; each scenario opens its own connections.
#[test]
fn xdnd_two_windows_on_private_xvfb() {
    // Each scenario gets its own private Xvfb so no X state carries over between them.
    for scenario in [drop_scenario as fn(&str), drag_scenario as fn(&str)] {
        let Some((_server, display)) = start_server() else {
            eprintln!("skipped: Xvfb not available or no free display");
            return;
        };
        // SAFETY: this integration test is the only code in its process touching DISPLAY, and
        // it sets the variable before any display connection is opened.
        unsafe { std::env::set_var("DISPLAY", &display) };
        scenario(&display);
    }
}

fn drop_scenario(display: &str) {
    let display = display.to_string();
    let events: Arc<Mutex<Vec<ActionEvent>>> = Arc::default();
    let sink = Arc::clone(&events);
    let target = std::thread::spawn(move || {
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
            move |event, _document| {
                let flamewm_render_core::ControllerEvent::Action(action) = event;
                if action.action.starts_with("__flamewm.window.drag.") {
                    sink.lock()
                        .map_err(|error| error.to_string())?
                        .push(action.clone());
                }
                Ok(())
            },
            |_, _| Ok(()),
            |_| Ok(()),
        )
    });

    let (conn, _) = connect_test(&display);
    let root = conn.setup().roots[0].root;
    let window = wait_for(|| find_window(&conn, TARGET_TITLE)).expect("target window appears");
    let aware = atom(&conn, "XdndAware");
    let version = wait_for(|| {
        conn.get_property(false, window, aware, AtomEnum::ATOM, 0, 1)
            .ok()?
            .reply()
            .ok()
            .and_then(|reply| reply.value32()?.next())
    })
    .expect("target advertises XdndAware");
    assert_eq!(version, 5, "XDND protocol version five");

    // External source: an input-only window that owns XdndSelection and serves uri-list.
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
    let xdnd_selection = atom(&conn, "XdndSelection");
    let uri_list = atom(&conn, "text/uri-list");
    let text_plain = atom(&conn, "text/plain");
    let enter = atom(&conn, "XdndEnter");
    let position = atom(&conn, "XdndPosition");
    let status = atom(&conn, "XdndStatus");
    let drop_msg = atom(&conn, "XdndDrop");
    let finished = atom(&conn, "XdndFinished");
    let copy = atom(&conn, "XdndActionCopy");
    conn.set_selection_owner(source, xdnd_selection, CURRENT_TIME)
        .expect("own XdndSelection")
        .check()
        .expect("selection owner set");

    let drop_path = std::env::temp_dir().join("xdnd test/ünï.txt");
    let send = |to: u32, kind: u32, data: [u32; 5]| {
        let event = ClientMessageEvent::new(32, to, kind, data);
        conn.send_event(false, to, EventMask::NO_EVENT, event)
            .expect("send");
        conn.flush().expect("flush");
    };
    send(window, enter, [source, 5 << 24, uri_list, text_plain, 0]);
    // Root coordinates of the pointer; the window origin is (TARGET_X, TARGET_Y).
    let (root_x, root_y) = (TARGET_X + 20, TARGET_Y + 20);
    let packed = ((root_x as u16 as u32) << 16) | (root_y as u16 as u32);
    send(window, position, [source, 0, packed, 1234, copy]);
    let reply = next_message(&conn, status).expect("XdndStatus reply");
    assert_eq!(reply[1] & 1, 1, "uri-list offered, so the drop is accepted");

    send(window, drop_msg, [source, 0, 1234, 0, 0]);
    // The target converts XdndSelection; serve it the uri-list it asked for.
    let served = wait_for(|| {
        while let Some(event) = conn.poll_for_event().ok()? {
            if let Event::SelectionRequest(request) = event
                && request.selection == xdnd_selection
            {
                let bytes = flamewm_render_x11::encode_uri_list(std::slice::from_ref(&drop_path));
                conn.change_property8(
                    PropMode::REPLACE,
                    request.requestor,
                    request.property,
                    uri_list,
                    &bytes,
                )
                .ok()?;
                conn.send_event(
                    false,
                    request.requestor,
                    EventMask::NO_EVENT,
                    SelectionNotifyEvent {
                        response_type: SELECTION_NOTIFY_EVENT,
                        sequence: 0,
                        time: request.time,
                        requestor: request.requestor,
                        selection: request.selection,
                        target: request.target,
                        property: request.property,
                    },
                )
                .ok()?;
                conn.flush().ok()?;
                return Some(());
            }
        }
        None
    });
    assert!(
        served.is_some(),
        "target requested XdndSelection after the drop"
    );

    let finish = next_message(&conn, finished).expect("XdndFinished reply");
    assert_eq!(finish[1] & 1, 1, "drop completed as accepted");

    // Close the target through the normal WM_DELETE_WINDOW path so its loop returns.
    let protocols = atom(&conn, "WM_PROTOCOLS");
    let delete = atom(&conn, "WM_DELETE_WINDOW");
    send(window, protocols, [delete, CURRENT_TIME, 0, 0, 0]);
    target
        .join()
        .expect("target thread")
        .expect("target window loop returns after close");

    let seen = events.lock().expect("events").clone();
    let enter_event = seen
        .iter()
        .find(|event| event.action == FILE_DRAG_ENTER_ACTION)
        .expect("enter event delivered");
    assert_eq!(
        (enter_event.x, enter_event.y),
        (20.0, 20.0),
        "window-relative position"
    );
    let drop_event = seen
        .iter()
        .find(|event| event.action == FILE_DRAG_DROP_ACTION)
        .expect("drop event delivered");
    let paths: Vec<PathBuf> = parse_uri_list(drop_event.text.as_deref().unwrap_or("").as_bytes());
    assert_eq!(paths, vec![drop_path]);
}

const DRAG_TITLE: &str = "xdnd-drag-source-test";

/// Outgoing drag: a FlameWM window starts `begin_file_drag` (via the reactor-side slot), an
/// XTEST pointer carries the drag over an external XdndAware window, and the external target
/// receives the file list through XdndSelection.
fn drag_scenario(display: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let display = display.to_string();
    let drop_path = std::env::temp_dir().join("xdnd drag/ünï.txt");
    let begin = Arc::new(AtomicBool::new(false));
    let begin_flag = Arc::clone(&begin);
    let served_path = drop_path.clone();
    let source = std::thread::spawn(move || {
        let config = X11Config {
            width: 200,
            height: 100,
            x: TARGET_X,
            y: TARGET_Y,
            title: DRAG_TITLE.to_string(),
            role: X11WindowRole::Normal,
            identity: None,
            backdrop: None,
            file_drag: Some(FileDragSlot::default()),
        };
        let slot = config.file_drag.clone().expect("drag slot");
        let mut reactor = Reactor::new().map_err(|error| error.to_string())?;
        run_with_controller_events_with_reactor_geometry(
            document(),
            config,
            &mut reactor,
            WindowGeometrySlot::default(),
            RawPressWatch::default(),
            |_, _| Ok(()),
            |_, _| Ok(()),
            move |_| {
                if begin_flag.swap(false, Ordering::SeqCst) {
                    *slot.borrow_mut() = Some(vec![served_path.clone()]);
                }
                Ok(())
            },
        )
    });

    let (conn, _) = connect_test(&display);
    let root = conn.setup().roots[0].root;
    let flamewm = wait_for(|| find_window(&conn, DRAG_TITLE)).expect("source window appears");
    let aware = atom(&conn, "XdndAware");
    let target = conn.generate_id().expect("id");
    conn.create_window(
        0,
        target,
        root,
        300,
        200,
        100,
        100,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new(),
    )
    .expect("target window");
    conn.change_property32(PropMode::REPLACE, target, aware, AtomEnum::ATOM, &[5])
        .expect("XdndAware on target");
    conn.map_window(target).expect("map target");
    conn.flush().expect("flush");

    let names = |name: &str| atom(&conn, name);
    let (enter, position, status, drop_msg, finished) = (
        names("XdndEnter"),
        names("XdndPosition"),
        names("XdndStatus"),
        names("XdndDrop"),
        names("XdndFinished"),
    );
    let (copy, uri_list, xdnd_selection, wake) = (
        names("XdndActionCopy"),
        names("text/uri-list"),
        names("XdndSelection"),
        names("FLAMEWM_TEST_WAKE"),
    );
    let send = |to: u32, kind: u32, data: [u32; 5]| {
        conn.send_event(
            false,
            to,
            EventMask::NO_EVENT,
            ClientMessageEvent::new(32, to, kind, data),
        )
        .expect("send");
        conn.flush().expect("flush");
    };

    // Ask the source to start the drag: the reactor callback takes the request, then the loop
    // starts the modal drag after this wake event is handled.
    begin.store(true, Ordering::SeqCst);
    send(flamewm, wake, [0; 5]);
    let owner = wait_for(|| {
        (conn
            .get_selection_owner(xdnd_selection)
            .ok()?
            .reply()
            .ok()?
            .owner
            == flamewm)
            .then_some(())
    });
    assert!(owner.is_some(), "source owns XdndSelection while dragging");

    // Press the button so the release below is a real release; X ignores a release of a
    // button that is not down.
    conn.xtest_fake_input(4, 1, 0, root, 0, 0, 0)
        .expect("button press");
    conn.flush().expect("flush");

    // Pointer over the external target: XTEST motion inside its rectangle.
    let (pointer_x, pointer_y) = (TARGET_X_EXTERNAL + 40, TARGET_Y_EXTERNAL + 40);
    let enter_msg = wait_for(|| {
        conn.xtest_fake_input(6, 0, 0, root, pointer_x as i16, pointer_y as i16, 0)
            .ok()?;
        conn.flush().ok()?;
        std::thread::sleep(Duration::from_millis(20));
        while let Some(event) = conn.poll_for_event().ok()? {
            if let Event::ClientMessage(message) = event
                && message.type_ == enter
            {
                return Some(message.data.as_data32());
            }
        }
        None
    })
    .expect("XdndEnter reaches the external target");
    assert_eq!(enter_msg[0], flamewm, "enter names the source window");
    assert_eq!(enter_msg[2], uri_list, "uri-list is offered first");

    let position_msg = next_message(&conn, position).expect("XdndPosition");
    assert_eq!(position_msg[0], flamewm);
    send(flamewm, status, [target, 1, 0, 0, copy]);

    // Release the button: the source drops on the accepting target.
    conn.xtest_fake_input(5, 1, 0, root, 0, 0, 0)
        .expect("button release");
    conn.flush().expect("flush");
    let drop_msg = next_message(&conn, drop_msg).expect("XdndDrop");
    assert_eq!(drop_msg[0], flamewm);
    let drop_time = drop_msg[2];

    // The target converts XdndSelection, which the source serves in its modal loop.
    let property = names("FLAMEWM_TEST_TARGET_PROPERTY");
    conn.convert_selection(target, xdnd_selection, uri_list, property, drop_time)
        .expect("convert");
    conn.flush().expect("flush");
    let notify = wait_for(|| {
        while let Some(event) = conn.poll_for_event().ok()? {
            if let Event::SelectionNotify(note) = event
                && note.requestor == target
            {
                return Some(note.property);
            }
        }
        None
    })
    .expect("selection notify");
    assert_ne!(notify, 0, "source served the uri-list");
    let data = conn
        .get_property(true, target, property, uri_list, 0, 1 << 16)
        .expect("read data")
        .reply()
        .expect("data reply");
    assert_eq!(parse_uri_list(&data.value), vec![drop_path.clone()]);

    send(flamewm, finished, [target, 1, copy, 0, 0]);
    let protocols = names("WM_PROTOCOLS");
    let delete = names("WM_DELETE_WINDOW");
    send(flamewm, protocols, [delete, CURRENT_TIME, 0, 0, 0]);
    source
        .join()
        .expect("source thread")
        .expect("source window loop returns after close");
}

const TARGET_X_EXTERNAL: i32 = 300;
const TARGET_Y_EXTERNAL: i32 = 200;

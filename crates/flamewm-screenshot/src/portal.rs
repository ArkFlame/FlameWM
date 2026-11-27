//! `org.freedesktop.impl.portal.Screenshot` backend, served on the session bus by `flamewm-screenshot --portal`.
//!
//! `xdg-desktop-portal` forwards `Screenshot(handle, app_id, window, options)` here and takes the
//! answer as the reply itself: `(response u, results a{sv})`, with `uri` holding the saved PNG.
//! Only the non-interactive path exists: `interactive=true` is refused with response 2 because the
//! selection UI is not served by this backend. The pixels come from the full output: zwlr-screencopy
//! on a native Wayland session, the X11 root otherwise.
//!
//! The bus is driven through `flamewm-dbus-reactor::BusPump`, the owner of D-Bus I/O.

use std::path::{Path, PathBuf};
use std::time::Duration;

use flamewm_dbus_reactor::{
    BusKind, BusPump, Message, MessageDisposition, MessageItem, MessageItemDict, Signature,
};

use crate::capture::{self, Image};
use crate::screencast;
use crate::wayland;

/// Well-known name the portal frontend activates (`packaging/dbus-1/...service`).
pub const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.flamewm";
const OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";
const INTERFACE: &str = "org.freedesktop.impl.portal.Screenshot";
const MEMBER: &str = "Screenshot";

/// Portal response codes: 0 success, 2 other failure.
const RESPONSE_SUCCESS: u32 = 0;
pub const RESPONSE_OTHER: u32 = 2;

/// Bounds how long the service blocks in one bus wait before it re-checks the connection.
const WAIT: Duration = Duration::from_millis(500);

/// The validated arguments of one `Screenshot` call.
#[derive(Debug, PartialEq, Eq)]
pub struct Request {
    pub interactive: bool,
}

/// Parses `(o handle, s app_id, s window_identifier, a{sv} options)`. A missing `interactive`
/// option means false, as the portal specifies.
pub fn parse_request(items: &[MessageItem]) -> Result<Request, String> {
    let [handle, app_id, window, options] = items else {
        return Err(format!("expected 4 arguments, got {}", items.len()));
    };
    if !matches!(handle, MessageItem::ObjectPath(_)) {
        return Err("handle must be an object path".into());
    }
    if !matches!(app_id, MessageItem::Str(_)) || !matches!(window, MessageItem::Str(_)) {
        return Err("app_id and window_identifier must be strings".into());
    }
    let MessageItem::Dict(dict) = options else {
        return Err("options must be a dictionary".into());
    };
    let mut interactive = false;
    for (key, value) in dict.clone().into_vec() {
        let (MessageItem::Str(key), MessageItem::Variant(value)) = (key, value) else {
            return Err("options must map strings to variants".into());
        };
        if key == "interactive" {
            // libdbus may surface the `v` wrapper of `a{sv}` values once more than the schema says.
            let mut inner = *value;
            while let MessageItem::Variant(next) = inner {
                inner = *next;
            }
            let MessageItem::Bool(flag) = inner else {
                return Err("option interactive must be a boolean".into());
            };
            interactive = flag;
        }
    }
    Ok(Request { interactive })
}

/// The reply body `(u response, a{sv} results)`. `uri` is present only on success.
pub fn reply_items(response: u32, uri: Option<&str>) -> Result<Vec<MessageItem>, String> {
    let entries = uri.map_or_else(Vec::new, |uri| {
        vec![(
            MessageItem::Str("uri".into()),
            MessageItem::Variant(Box::new(MessageItem::Str(uri.into()))),
        )]
    });
    let dict = MessageItemDict::new(entries, signature("s")?, signature("v")?)
        .map_err(|e| format!("results dictionary: {e:?}"))?;
    let results = MessageItem::Dict(dict);
    Ok(vec![MessageItem::UInt32(response), results])
}

fn signature(s: &str) -> Result<Signature<'static>, String> {
    Signature::new(s).map_err(|e| format!("signature {s}: {e}"))
}

/// `file://` URI for an absolute path, percent-encoding everything outside the unreserved set and `/`.
pub fn file_uri(path: &Path) -> String {
    use std::fmt::Write as _;
    use std::os::unix::ffi::OsStrExt as _;
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

/// `$HOME/Pictures/Screenshots`, the portal's conventional screenshot folder.
fn output_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join("Pictures").join("Screenshots"))
}

/// Backends of the full-output fallback chain, in priority order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Portal,
    Screencopy,
    X11,
}

impl Backend {
    fn as_str(self) -> &'static str {
        match self {
            Self::Portal => "portal",
            Self::Screencopy => "screencopy",
            Self::X11 => "x11",
        }
    }
}

/// Which backends this host may attempt. Pure data; no I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Availability {
    portal: bool,
    screencopy: bool,
    x11: bool,
}

/// No portal client exists here: BUS_NAME is the impl-portal backend this process serves,
/// so there is no frontend to call. The slot stays first in the chain for a future client.
/// No KDE/xfce/smithay precedent supplies a portal-client fallback chain, so local source rules.
const PORTAL_CLIENT_AVAILABLE: bool = false;

/// Pure chain chooser: portal, then screencopy, then x11; skip unavailable backends.
/// No I/O, no panic: the same input always yields the same ordered chain.
fn choose_chain(a: Availability) -> Vec<Backend> {
    [
        (a.portal, Backend::Portal),
        (a.screencopy, Backend::Screencopy),
        (a.x11, Backend::X11),
    ]
    .into_iter()
    .filter_map(|(available, backend)| available.then_some(backend))
    .collect()
}

/// The only impure step: read the environment to decide which backends may be tried.
fn probe_availability() -> Availability {
    let session_bus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some();
    Availability {
        portal: PORTAL_CLIENT_AVAILABLE && session_bus,
        screencopy: std::env::var_os("WAYLAND_DISPLAY").is_some(),
        x11: std::env::var_os("DISPLAY").is_some(),
    }
}

/// Renders one line: each backend's name and why it worked or did not.
fn report_attempts(attempts: &[(Backend, Result<(), String>)]) -> String {
    attempts
        .iter()
        .map(|(backend, outcome)| match outcome {
            Ok(()) => format!("{}: ok", backend.as_str()),
            Err(reason) => format!("{}: {reason}", backend.as_str()),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Full-output pixels through one ordered fallback chain, with a single consolidated report.
/// Screencopy Ok(None) is an unavailable backend; Err is recorded, never silently replaced.
fn capture_full_output() -> Result<Image, String> {
    let chain = choose_chain(probe_availability());
    let mut attempts: Vec<(Backend, Result<(), String>)> = Vec::new();
    let mut image = None;

    for backend in [Backend::Portal, Backend::Screencopy, Backend::X11] {
        if image.is_some() {
            attempts.push((backend, Err("skipped".into())));
            continue;
        }
        if !chain.contains(&backend) {
            let reason = if backend == Backend::Portal {
                "unavailable (no portal client)"
            } else {
                "unavailable"
            };
            attempts.push((backend, Err(reason.into())));
            continue;
        }
        let outcome = match backend {
            Backend::Portal => Err("unavailable (no portal client)".into()),
            Backend::Screencopy => match wayland::capture_output() {
                Ok(Some(captured)) => {
                    image = Some(captured);
                    Ok(())
                }
                Ok(None) => Err("unavailable".into()),
                Err(error) => Err(error),
            },
            Backend::X11 => match capture::capture_root() {
                Ok(captured) => {
                    image = Some(captured);
                    Ok(())
                }
                Err(error) => Err(error),
            },
        };
        attempts.push((backend, outcome));
    }

    let report = report_attempts(&attempts);
    match image {
        Some(image) => {
            eprintln!("flamewm-screenshot portal: capture chain: {report}");
            Ok(image)
        }
        None => Err(format!("capture chain: {report}")),
    }
}

/// Captures, encodes and saves one PNG; returns its `file://` URI.
fn capture_to_file() -> Result<String, String> {
    let bytes = capture_full_output()?.png()?;
    let name = format!("Screenshot-{}.png", flamewm_api::clock::unix_nanos());
    let path = output_dir()?.join(name);
    crate::new_file(&path, &bytes)?;
    Ok(file_uri(&path))
}

/// Answers one call: `(response, uri)`. Every failure becomes response 2 and a stderr line.
fn answer(request: &Request) -> (u32, Option<String>) {
    if request.interactive {
        eprintln!("flamewm-screenshot portal: interactive screenshots are not supported");
        return (RESPONSE_OTHER, None);
    }
    match capture_to_file() {
        Ok(uri) => (RESPONSE_SUCCESS, Some(uri)),
        Err(error) => {
            eprintln!("flamewm-screenshot portal: {error}");
            (RESPONSE_OTHER, None)
        }
    }
}

fn handle(message: &Message) -> MessageDisposition {
    let is_screencast = message
        .interface()
        .zip(message.member())
        .is_some_and(|(i, m)| screencast::is_call(&i, &m));
    if is_screencast {
        eprintln!("flamewm-screenshot portal: {}", screencast::REFUSAL);
        return reply(message, RESPONSE_OTHER, None);
    }
    let is_screenshot = message.interface().is_some_and(|i| &*i == INTERFACE)
        && message.member().is_some_and(|m| &*m == MEMBER);
    if !is_screenshot {
        return MessageDisposition::Unhandled;
    }
    let (response, uri) = match parse_request(&message.get_items()) {
        Ok(request) => answer(&request),
        Err(error) => {
            eprintln!("flamewm-screenshot portal: invalid call: {error}");
            (RESPONSE_OTHER, None)
        }
    };
    reply(message, response, uri.as_deref())
}

/// Builds the method return `(response, results)` for one call.
fn reply(message: &Message, response: u32, uri: Option<&str>) -> MessageDisposition {
    match reply_items(response, uri) {
        Ok(items) => {
            let mut reply = message.method_return();
            reply.append_items(&items);
            MessageDisposition::Reply(reply)
        }
        Err(error) => {
            eprintln!("flamewm-screenshot portal: reply: {error}");
            MessageDisposition::Unhandled
        }
    }
}

/// Dispatches the portal calls queued on `pump`; the caller owns the wait loop.
fn dispatch(pump: &BusPump) -> Result<usize, String> {
    pump.on_ready_with_object_path(OBJECT_PATH, handle, |_| MessageDisposition::Unhandled)
        .map_err(|e| e.to_string())
}

/// Serves the backend until the bus connection fails.
pub fn serve() -> Result<(), String> {
    let pump = BusPump::connect(BusKind::Session).map_err(|e| e.to_string())?;
    pump.own_name(BUS_NAME).map_err(|e| e.to_string())?;
    loop {
        pump.wait_ready(WAIT).map_err(|e| e.to_string())?;
        dispatch(&pump)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_dbus_reactor::Path as DbusPath;
    use std::sync::mpsc;

    fn options(entries: Vec<(&str, MessageItem)>) -> MessageItem {
        let v = entries
            .into_iter()
            .map(|(k, value)| {
                (
                    MessageItem::Str(k.into()),
                    MessageItem::Variant(Box::new(value)),
                )
            })
            .collect();
        MessageItem::Dict(dict_of(v, "s", "v"))
    }

    fn dict_of(v: Vec<(MessageItem, MessageItem)>, k: &str, val: &str) -> MessageItemDict {
        MessageItemDict::new(v, signature(k).unwrap(), signature(val).unwrap()).unwrap()
    }

    fn call(options: MessageItem) -> Vec<MessageItem> {
        vec![
            MessageItem::ObjectPath(dbus_path("/org/freedesktop/portal/desktop/request/x")),
            MessageItem::Str("org.example.App".into()),
            MessageItem::Str(String::new()),
            options,
        ]
    }

    fn dbus_path(p: &str) -> DbusPath<'static> {
        DbusPath::new(p.to_owned()).unwrap()
    }

    #[test]
    fn missing_interactive_option_means_non_interactive() {
        let request = parse_request(&call(options(vec![]))).unwrap();
        assert_eq!(request, Request { interactive: false });
    }

    #[test]
    fn interactive_true_is_parsed_for_refusal() {
        let request = parse_request(&call(options(vec![(
            "interactive",
            MessageItem::Bool(true),
        )])))
        .unwrap();
        assert!(request.interactive);
    }

    #[test]
    fn doubly_wrapped_interactive_flag_is_read() {
        let inner = MessageItem::Variant(Box::new(MessageItem::Bool(true)));
        let entry = (
            MessageItem::Str("interactive".into()),
            MessageItem::Variant(Box::new(inner)),
        );
        let dict = MessageItem::Dict(dict_of(vec![entry], "s", "v"));
        assert!(parse_request(&call(dict)).unwrap().interactive);
    }

    #[test]
    fn malformed_calls_are_rejected() {
        assert!(parse_request(&call(options(vec![]))[..3]).is_err());
        let mut bad_handle = call(options(vec![]));
        bad_handle[0] = MessageItem::Str("x".into());
        assert!(parse_request(&bad_handle).is_err());
        let bad_flag = call(options(vec![(
            "interactive",
            MessageItem::Str("yes".into()),
        )]));
        assert!(parse_request(&bad_flag).is_err());
        let mut not_dict = call(options(vec![]));
        not_dict[3] = MessageItem::Bool(false);
        assert!(parse_request(&not_dict).is_err());
    }

    #[test]
    fn success_reply_carries_response_zero_and_uri() {
        let items = reply_items(RESPONSE_SUCCESS, Some("file:///tmp/a.png")).unwrap();
        assert_eq!(items[0], MessageItem::UInt32(0));
        let MessageItem::Dict(results) = &items[1] else {
            panic!("results must be a dictionary");
        };
        let entries = results.clone().into_vec();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, MessageItem::Str("uri".into()));
        assert_eq!(
            entries[0].1,
            MessageItem::Variant(Box::new(MessageItem::Str("file:///tmp/a.png".into())))
        );
    }

    #[test]
    fn failure_reply_has_empty_results() {
        let items = reply_items(RESPONSE_OTHER, None).unwrap();
        assert_eq!(items[0], MessageItem::UInt32(2));
        let MessageItem::Dict(results) = &items[1] else {
            panic!("results must be a dictionary");
        };
        assert_eq!(results.clone().into_vec(), []);
    }

    #[test]
    fn file_uri_escapes_spaces_and_keeps_slashes() {
        let uri = file_uri(Path::new("/home/a b/Pictures/x%y.png"));
        assert_eq!(uri, "file:///home/a%20b/Pictures/x%25y.png");
    }

    fn available(portal: bool, screencopy: bool, x11: bool) -> Availability {
        Availability {
            portal,
            screencopy,
            x11,
        }
    }

    #[test]
    fn choose_chain_uses_full_priority_order_when_all_available() {
        assert_eq!(
            choose_chain(available(true, true, true)),
            vec![Backend::Portal, Backend::Screencopy, Backend::X11]
        );
    }

    #[test]
    fn choose_chain_skips_unavailable_portal() {
        assert_eq!(
            choose_chain(available(false, true, true)),
            vec![Backend::Screencopy, Backend::X11]
        );
    }

    #[test]
    fn choose_chain_skips_unavailable_screencopy() {
        assert_eq!(
            choose_chain(available(true, false, true)),
            vec![Backend::Portal, Backend::X11]
        );
    }

    #[test]
    fn choose_chain_keeps_only_x11_when_others_unavailable() {
        assert_eq!(
            choose_chain(available(false, false, true)),
            vec![Backend::X11]
        );
    }

    #[test]
    fn choose_chain_is_empty_when_nothing_is_available() {
        assert!(choose_chain(available(false, false, false)).is_empty());
    }

    #[test]
    fn choose_chain_order_is_portal_then_screencopy_then_x11() {
        for chain in [
            choose_chain(available(true, true, true)),
            choose_chain(available(true, false, true)),
            choose_chain(available(false, true, true)),
            choose_chain(available(false, false, true)),
        ] {
            for pair in chain.windows(2) {
                assert!(
                    matches!(
                        (pair[0], pair[1]),
                        (Backend::Portal, Backend::Screencopy)
                            | (Backend::Portal, Backend::X11)
                            | (Backend::Screencopy, Backend::X11)
                    ),
                    "{:?} is out of order",
                    chain
                );
            }
        }
    }

    #[test]
    fn choose_chain_is_pure() {
        let input = available(true, true, false);
        assert_eq!(choose_chain(input), choose_chain(input));
    }

    #[test]
    fn backend_names_are_stable() {
        assert_eq!(Backend::Portal.as_str(), "portal");
        assert_eq!(Backend::Screencopy.as_str(), "screencopy");
        assert_eq!(Backend::X11.as_str(), "x11");
    }

    #[test]
    fn report_attempts_renders_one_line_with_every_backend() {
        let report = report_attempts(&[
            (
                Backend::Portal,
                Err("unavailable (no portal client)".into()),
            ),
            (Backend::Screencopy, Ok(())),
            (Backend::X11, Err("skipped".into())),
        ]);
        assert_eq!(
            report,
            "portal: unavailable (no portal client); screencopy: ok; x11: skipped"
        );
        assert!(!report.contains('\n'));
    }

    /// Private `dbus-daemon` started for one test; killed on drop.
    struct Daemon {
        child: std::process::Child,
        address: String,
    }

    impl Drop for Daemon {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// Starts a private session-style `dbus-daemon`, or `None` when it is unavailable (test skips).
    fn start_daemon(dir: &Path) -> Option<Daemon> {
        use std::io::BufRead as _;
        std::fs::create_dir_all(dir).ok()?;
        let child = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!("--address=unix:tmpdir={}", dir.display()))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let mut daemon = Daemon {
            child,
            address: String::new(),
        };
        let mut line = String::new();
        let stdout = daemon.child.stdout.take()?;
        std::io::BufReader::new(stdout).read_line(&mut line).ok()?;
        daemon.address = line.trim().to_owned();
        (!daemon.address.is_empty()).then_some(daemon)
    }

    /// Owns `BUS_NAME` on the private bus and dispatches portal calls until `stop` fires.
    fn run_backend(
        address: &str,
        ready: &mpsc::Sender<()>,
        stop: &mpsc::Receiver<()>,
    ) -> Result<(), String> {
        let pump = BusPump::connect_address(address).map_err(|e| e.to_string())?;
        pump.own_name(BUS_NAME).map_err(|e| e.to_string())?;
        let _ = ready.send(());
        while stop.try_recv().is_err() {
            pump.wait_ready(Duration::from_millis(20))
                .map_err(|e| e.to_string())?;
            dispatch(&pump)?;
        }
        Ok(())
    }

    /// One method call to the backend; `Err` carries a D-Bus error reply or a transport failure.
    fn bus_call(
        client: &BusPump,
        interface: &str,
        member: &str,
        args: &[MessageItem],
    ) -> Result<Vec<MessageItem>, String> {
        let mut message = Message::new_method_call(BUS_NAME, OBJECT_PATH, interface, member)?;
        message.append_items(args);
        let reply = client
            .send_with_reply(message, Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        Ok(reply.get_items())
    }

    fn is_empty_results(item: &MessageItem) -> bool {
        matches!(item, MessageItem::Dict(dict) if dict.clone().into_vec().is_empty())
    }

    #[test]
    fn screencast_and_interactive_screenshot_calls_are_refused_over_the_bus() {
        let dir = std::env::temp_dir().join(format!("flamewm-portal-e2e-{}", std::process::id()));
        let Some(daemon) = start_daemon(&dir) else {
            eprintln!("skipped: dbus-daemon unavailable");
            return;
        };
        let (ready_tx, ready_rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel();
        let address = daemon.address.clone();
        let backend = std::thread::spawn(move || run_backend(&address, &ready_tx, &stop_rx));
        ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("backend owns the name");
        let client = BusPump::connect_address(&daemon.address).expect("client connects");

        let handle =
            MessageItem::ObjectPath(dbus_path("/org/freedesktop/portal/desktop/request/x"));
        let session =
            MessageItem::ObjectPath(dbus_path("/org/freedesktop/portal/desktop/session/x"));
        let app = MessageItem::Str("org.example.App".into());
        let sc_iface = screencast::INTERFACE;
        // Sequence a frontend runs: CreateSession, SelectSources, Start. Each must get a prompt
        // response 2 with empty results, never a timeout.
        let sequence: [(&str, Vec<MessageItem>); 3] = [
            (
                "CreateSession",
                vec![
                    handle.clone(),
                    session.clone(),
                    app.clone(),
                    options(vec![]),
                ],
            ),
            (
                "SelectSources",
                vec![
                    handle.clone(),
                    session.clone(),
                    app.clone(),
                    options(vec![]),
                ],
            ),
            (
                "Start",
                vec![
                    handle.clone(),
                    session.clone(),
                    app.clone(),
                    MessageItem::Str(String::new()),
                    options(vec![]),
                ],
            ),
        ];
        for (member, args) in &sequence {
            let items = bus_call(&client, sc_iface, member, args).expect(member);
            assert_eq!(items[0], MessageItem::UInt32(RESPONSE_OTHER), "{member}");
            assert!(
                is_empty_results(&items[1]),
                "{member} must carry no streams"
            );
        }

        // Interactive Screenshot is refused with response 2 before any capture is attempted.
        let interactive = vec![
            handle.clone(),
            app.clone(),
            MessageItem::Str(String::new()),
            options(vec![("interactive", MessageItem::Bool(true))]),
        ];
        let items = bus_call(&client, INTERFACE, MEMBER, &interactive).expect("Screenshot");
        assert_eq!(items[0], MessageItem::UInt32(RESPONSE_OTHER));

        // A ScreenCast member FlameWM does not serve gets a bus error, not a silent drop.
        assert!(bus_call(&client, sc_iface, "OpenPipeWireRemote", &[]).is_err());

        let _ = stop_tx.send(());
        backend
            .join()
            .expect("backend thread")
            .expect("backend ran");
    }
}

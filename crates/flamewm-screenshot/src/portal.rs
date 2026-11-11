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

/// Full-output pixels: screencopy when the compositor offers it, the X11 root otherwise.
fn capture_full_output() -> Result<Image, String> {
    if let Some(image) = wayland::capture_output()? {
        return Ok(image);
    }
    capture::capture_root()
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

/// Serves the backend until the bus connection fails.
pub fn serve() -> Result<(), String> {
    let pump = BusPump::connect(BusKind::Session).map_err(|e| e.to_string())?;
    pump.own_name(BUS_NAME).map_err(|e| e.to_string())?;
    loop {
        pump.wait_ready(WAIT).map_err(|e| e.to_string())?;
        pump.on_ready_with_object_path(OBJECT_PATH, handle, |_| MessageDisposition::Unhandled)
            .map_err(|e| e.to_string())?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_dbus_reactor::Path as DbusPath;

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
        assert!(results.clone().into_vec().is_empty());
    }

    #[test]
    fn file_uri_escapes_spaces_and_keeps_slashes() {
        let uri = file_uri(Path::new("/home/a b/Pictures/x%y.png"));
        assert_eq!(uri, "file:///home/a%20b/Pictures/x%25y.png");
    }
}

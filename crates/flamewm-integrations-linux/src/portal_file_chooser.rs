//! Blocking XDG portal FileChooser provider for one local wallpaper file.
//!
//! The Settings worker owns the blocking boundary.  The provider uses the
//! existing libdbus pump and does not create a second async runtime or shell
//! fallback.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use dbus::Message;
use dbus::arg::messageitem::{MessageItem, MessageItemArray};
use dbus::strings::Signature;
use flamewm_api::{ErrorCode, FlameError, FlameResult};
use flamewm_dbus_reactor::{BusKind, BusPump, MessageDisposition};
use flamewm_image_core::ImageSourceFormat;

pub const PORTAL_SERVICE: &str = "org.freedesktop.portal.Desktop";
pub const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
pub const FILE_CHOOSER_INTERFACE: &str = "org.freedesktop.portal.FileChooser";
pub const REQUEST_INTERFACE: &str = "org.freedesktop.portal.Request";
pub const OPEN_FILE: &str = "OpenFile";
pub const RESPONSE: &str = "Response";
pub const WALLPAPER_TITLE: &str = "Choose Wallpaper";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

static NEXT_HANDLE_TOKEN: AtomicU64 = AtomicU64::new(1);

/// One-session-bus FileChooser provider. Calls intentionally block the caller;
/// Settings must invoke this provider from its worker boundary.
pub struct PortalFileChooser {
    pump: BusPump,
    timeout: Duration,
}

impl PortalFileChooser {
    /// Connects to the session bus used by the desktop portal.
    pub fn connect() -> FlameResult<Self> {
        Self::connect_with_timeout(DEFAULT_TIMEOUT)
    }

    /// Connects with a bounded response wait, primarily for worker policy and tests.
    pub fn connect_with_timeout(timeout: Duration) -> FlameResult<Self> {
        Ok(Self {
            pump: BusPump::connect(BusKind::Session)?,
            timeout,
        })
    }

    /// Opens the portal's single-selection wallpaper chooser.
    pub fn choose_wallpaper(&self, parent_window: &str) -> FlameResult<Option<PathBuf>> {
        self.open_file(parent_window)
    }

    /// Performs one blocking `org.freedesktop.portal.FileChooser.OpenFile` call.
    pub fn open_file(&self, parent_window: &str) -> FlameResult<Option<PathBuf>> {
        validate_parent_window(parent_window)?;
        let handle_token = format!(
            "flamewm_wallpaper_{}",
            NEXT_HANDLE_TOKEN.fetch_add(1, Ordering::Relaxed)
        );
        let expected_request_path = self.expected_request_path(&handle_token)?;
        self.add_response_match(&expected_request_path)?;
        let options = wallpaper_options(&handle_token)?;
        let call = Message::new_method_call(
            PORTAL_SERVICE,
            PORTAL_PATH,
            FILE_CHOOSER_INTERFACE,
            OPEN_FILE,
        )
        .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?
        .append3(
            parent_window.to_owned(),
            WALLPAPER_TITLE.to_owned(),
            options,
        );
        let reply = self.pump.send_with_reply(call, self.timeout)?;
        let request_path = one_object_path(&reply.get_items(), "portal OpenFile handle")?;
        if request_path != expected_request_path {
            self.add_response_match(&request_path)?;
        }

        self.wait_for_response(&request_path)
    }

    fn expected_request_path(&self, handle_token: &str) -> FlameResult<String> {
        let sender = self
            .pump
            .unique_name()
            .ok_or_else(|| FlameError::unavailable("session D-Bus unique name is unavailable"))?;
        let sender = sender.strip_prefix(':').unwrap_or(sender).replace('.', "_");
        Ok(format!("{PORTAL_PATH}/request/{sender}/{handle_token}"))
    }

    fn add_response_match(&self, request_path: &str) -> FlameResult<()> {
        let match_rule = format!(
            "type='signal',path='{request_path}',interface='{REQUEST_INTERFACE}',member='{RESPONSE}'"
        );
        let add_match = Message::new_method_call(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "AddMatch",
        )
        .map_err(|error| FlameError::new(ErrorCode::InvalidArgument, error))?
        .append1(match_rule);
        self.pump.send_with_reply(add_match, self.timeout)?;
        Ok(())
    }

    fn wait_for_response(&self, request_path: &str) -> FlameResult<Option<PathBuf>> {
        let deadline = Instant::now() + self.timeout;
        loop {
            let mut result = None;
            self.pump.on_ready(|message| {
                if is_response_signal(message, request_path) {
                    result = Some(parse_response(message.get_items()));
                    MessageDisposition::Handled
                } else {
                    MessageDisposition::Unhandled
                }
            })?;
            if let Some(result) = result {
                return result;
            }
            if Instant::now() >= deadline {
                return Err(FlameError::new(
                    ErrorCode::Timeout,
                    "portal FileChooser response timed out",
                ));
            }
            thread::park_timeout(Duration::from_millis(10));
        }
    }
}

fn wallpaper_options(handle_token: &str) -> FlameResult<MessageItem> {
    let filters = [("PNG", "*.png"), ("SVG", "*.svg"), ("PPM", "*.ppm")]
        .into_iter()
        .map(|(name, pattern)| {
            let patterns = MessageItem::Array(
                MessageItemArray::new(
                    vec![MessageItem::Struct(vec![
                        MessageItem::UInt32(0),
                        MessageItem::Str(pattern.to_owned()),
                    ])],
                    signature("a(us)")?,
                )
                .map_err(|error| {
                    invalid(format!("portal filter patterns are invalid: {error:?}"))
                })?,
            );
            Ok(MessageItem::Struct(vec![
                MessageItem::Str(name.to_owned()),
                patterns,
            ]))
        })
        .collect::<FlameResult<Vec<_>>>()?;
    let filters = MessageItem::Array(
        MessageItemArray::new(filters, signature("a(sa(us))")?)
            .map_err(|error| invalid(format!("portal filters are invalid: {error:?}")))?,
    );
    MessageItem::from_dict(
        vec![
            ("multiple".to_owned(), MessageItem::Bool(false)),
            ("directory".to_owned(), MessageItem::Bool(false)),
            ("modal".to_owned(), MessageItem::Bool(true)),
            (
                "handle_token".to_owned(),
                MessageItem::Str(handle_token.to_owned()),
            ),
            ("filters".to_owned(), filters),
        ]
        .into_iter()
        .map(Ok::<_, FlameError>),
    )
}

fn signature(value: &'static str) -> FlameResult<Signature<'static>> {
    Signature::new(value).map_err(|error| invalid(format!("invalid D-Bus signature: {error}")))
}

fn validate_parent_window(parent_window: &str) -> FlameResult<()> {
    if parent_window.contains('\0') {
        return Err(invalid("portal parent window contains a NUL byte"));
    }
    Ok(())
}

fn is_response_signal(message: &Message, request_path: &str) -> bool {
    message.msg_type() == dbus::MessageType::Signal
        && message.path().is_some_and(|value| value == request_path)
        && message
            .interface()
            .is_some_and(|value| value == REQUEST_INTERFACE)
        && message.member().is_some_and(|value| value == RESPONSE)
}

fn parse_response(items: Vec<MessageItem>) -> FlameResult<Option<PathBuf>> {
    if items.len() != 2 {
        return Err(invalid("portal FileChooser response has invalid arguments"));
    }
    let response = item_u32(&items[0], "portal FileChooser response code")?;
    if response == 1 {
        return Ok(None);
    }
    if response != 0 {
        return Err(FlameError::new(
            ErrorCode::Unavailable,
            format!("portal FileChooser returned response code {response}"),
        ));
    }
    let MessageItem::Dict(results) = unwrap_variant(items[1].clone()) else {
        return Err(invalid("portal FileChooser results are not a dictionary"));
    };
    let mut uris = None;
    for (key, value) in results.into_vec() {
        if item_string(&key, "portal FileChooser result key")? == "uris" {
            uris = Some(item_strings(
                &unwrap_variant(value),
                "portal FileChooser URIs",
            )?);
        }
    }
    let uris = uris.ok_or_else(|| invalid("portal FileChooser response has no URI list"))?;
    if uris.len() != 1 {
        return Err(invalid(
            "portal FileChooser returned a non-single wallpaper selection",
        ));
    }
    file_uri_to_path(&uris[0]).map(Some)
}

/// Converts one local `file://` URI into a validated supported wallpaper path.
pub fn file_uri_to_path(uri: &str) -> FlameResult<PathBuf> {
    let rest = uri
        .strip_prefix("file://")
        .ok_or_else(|| invalid("portal FileChooser returned a non-file URI"))?;
    let slash = rest
        .find('/')
        .ok_or_else(|| invalid("portal file URI has no local path"))?;
    let authority = &rest[..slash];
    if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
        return Err(invalid("portal file URI is not local"));
    }
    let decoded = percent_decode(&rest[slash..])?;
    if decoded.is_empty() || decoded.chars().any(char::is_control) {
        return Err(invalid("portal file URI contains an invalid path"));
    }
    let path = PathBuf::from(decoded);
    if !path.is_absolute() {
        return Err(invalid("portal file URI path is not absolute"));
    }
    if ImageSourceFormat::from_extension(&path).is_none() {
        return Err(invalid(
            "portal file URI does not name a supported wallpaper format",
        ));
    }
    Ok(path)
}

fn percent_decode(value: &str) -> FlameResult<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(invalid("portal file URI has an incomplete percent escape"));
        }
        let high = hex_value(bytes[index + 1])?;
        let low = hex_value(bytes[index + 2])?;
        decoded.push(high << 4 | low);
        index += 3;
    }
    String::from_utf8(decoded)
        .map_err(|_| invalid("portal file URI is not valid UTF-8 after decoding"))
}

fn hex_value(value: u8) -> FlameResult<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(invalid("portal file URI has an invalid percent escape")),
    }
}

fn one_object_path(items: &[MessageItem], context: &str) -> FlameResult<String> {
    if items.len() != 1 {
        return Err(invalid(context));
    }
    match &items[0] {
        MessageItem::ObjectPath(value) => Ok(value.to_string()),
        _ => Err(invalid(context)),
    }
}

fn item_string(item: &MessageItem, context: &str) -> FlameResult<String> {
    match item {
        MessageItem::Str(value) => Ok(value.clone()),
        _ => Err(invalid(context)),
    }
}

fn item_u32(item: &MessageItem, context: &str) -> FlameResult<u32> {
    match item {
        MessageItem::UInt32(value) => Ok(*value),
        _ => Err(invalid(context)),
    }
}

fn item_strings(item: &MessageItem, context: &str) -> FlameResult<Vec<String>> {
    let MessageItem::Array(values) = item else {
        return Err(invalid(context));
    };
    values
        .clone()
        .into_vec()
        .into_iter()
        .map(|value| item_string(&value, context))
        .collect()
}

fn unwrap_variant(item: MessageItem) -> MessageItem {
    match item {
        MessageItem::Variant(value) => unwrap_variant(*value),
        item => item,
    }
}

fn invalid(message: impl Into<String>) -> FlameError {
    FlameError::new(ErrorCode::InvalidArgument, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn wallpaper_uri_decodes_local_percent_escapes() {
        assert_eq!(
            file_uri_to_path("file:///tmp/wall%20paper.png")
                .expect("local wallpaper URI")
                .as_path(),
            Path::new("/tmp/wall paper.png")
        );
        assert_eq!(
            file_uri_to_path("file://localhost/tmp/wall.svg")
                .expect("localhost URI")
                .as_path(),
            Path::new("/tmp/wall.svg")
        );
    }

    #[test]
    fn wallpaper_uri_rejects_non_file_remote_and_unsupported_paths() {
        for uri in [
            "https://example.test/wall.png",
            "file://remote/tmp/wall.png",
            "file:///tmp/wall.jpg",
            "file:///tmp/wall%ZZ.png",
        ] {
            assert!(
                file_uri_to_path(uri).is_err(),
                "URI must be rejected: {uri}"
            );
        }
    }

    #[test]
    fn portal_options_are_single_selection_with_decoder_formats_only() {
        let MessageItem::Dict(options) = wallpaper_options("test_token").expect("portal options")
        else {
            panic!("portal options must be a dictionary");
        };
        let values = options.into_vec();
        assert!(values.iter().any(|(key, value)| {
            matches!(key, MessageItem::Str(name) if name == "multiple")
                && matches!(value.peel(), MessageItem::Bool(false))
        }));
        let serialized = format!("{values:?}");
        assert!(serialized.contains("*.png"));
        assert!(serialized.contains("*.svg"));
        assert!(serialized.contains("*.ppm"));
        assert!(!serialized.contains("*.jpg"));
        assert!(!serialized.contains("*.jpeg"));
        assert!(!serialized.contains("*.webp"));
    }

    #[test]
    fn cancelled_portal_response_is_none() {
        let results =
            MessageItem::from_dict(std::iter::empty::<Result<(String, MessageItem), FlameError>>())
                .expect("empty results");
        assert_eq!(
            parse_response(vec![MessageItem::UInt32(1), results]),
            Ok(None)
        );
    }

    #[test]
    fn response_signal_matches_borrowed_path_interface_and_member() {
        let signal = Message::new_signal("/request/1", REQUEST_INTERFACE, RESPONSE)
            .expect("valid portal response signal");
        assert!(is_response_signal(&signal, "/request/1"));
        assert!(!is_response_signal(&signal, "/request/2"));

        let wrong_interface =
            Message::new_signal("/request/1", "org.example.Other", RESPONSE).expect("valid signal");
        assert!(!is_response_signal(&wrong_interface, "/request/1"));

        let wrong_member =
            Message::new_signal("/request/1", REQUEST_INTERFACE, "Other").expect("valid signal");
        assert!(!is_response_signal(&wrong_member, "/request/1"));
    }
}

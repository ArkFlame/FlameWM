//! Polkit authentication agent core: registration, `BeginAuthentication` and
//! `CancelAuthentication` handling. No UI and no password I/O.
//!
//! The caller owns the system-bus pump (`flamewm_dbus_reactor::BusPump`) and routes calls on
//! `AGENT_PATH` through `handle_agent_call`. Prompting the user goes through `AuthPrompt`; the
//! password step (polkit-agent-helper-1) is out of scope here.

use std::collections::BTreeMap;
use std::ffi::CString;

use dbus::arg::messageitem::{MessageItem, MessageItemDict};
use dbus::strings::ErrorName;
use dbus::{Message, MessageType, Path};
use flamewm_api::{ErrorCode, FlameError, FlameResult};
use flamewm_dbus_reactor::MessageDisposition;

pub const AUTHORITY_DEST: &str = "org.freedesktop.PolicyKit1";
pub const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
pub const AUTHORITY_IFACE: &str = "org.freedesktop.PolicyKit1.Authority";
pub const AGENT_IFACE: &str = "org.freedesktop.PolicyKit1.AuthenticationAgent";
/// Exported object path for the agent callbacks; polkit calls back on the unique bus name.
pub const AGENT_PATH: &str = "/org/flamewm/PolkitAgent";
const REGISTER: &str = "RegisterAuthenticationAgent";
const UNREGISTER: &str = "UnregisterAuthenticationAgent";
const BEGIN: &str = "BeginAuthentication";
const CANCEL: &str = "CancelAuthentication";
const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";

/// One `(sa{sv})` identity. Only the unix uid matters to the prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub kind: String,
    pub uid: Option<u32>,
}

/// Parsed `BeginAuthentication(action_id, message, icon_name, details, cookie, identities)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginRequest {
    pub action_id: String,
    pub message: String,
    pub icon_name: String,
    pub details: Vec<(String, String)>,
    pub cookie: String,
    pub identities: Vec<Identity>,
}

/// The UI side. `show` starts a prompt for one cookie; `dismiss` withdraws it on cancel.
pub trait AuthPrompt {
    fn show(&mut self, request: &BeginRequest);
    fn dismiss(&mut self, cookie: &str);
}

/// Pending authentication requests keyed by polkit cookie.
#[derive(Debug, Default)]
pub struct AgentSessions {
    pending: BTreeMap<String, BeginRequest>,
}

impl AgentSessions {
    /// Registers a request and shows its prompt. Duplicate cookies are rejected.
    pub fn begin(
        &mut self,
        prompt: &mut impl AuthPrompt,
        request: BeginRequest,
    ) -> FlameResult<()> {
        if self.pending.contains_key(&request.cookie) {
            return Err(invalid("duplicate authentication cookie"));
        }
        prompt.show(&request);
        self.pending.insert(request.cookie.clone(), request);
        Ok(())
    }

    /// Cancels a request. Returns `false` for unknown or already finished cookies, which polkit
    /// may send harmlessly.
    pub fn cancel(&mut self, prompt: &mut impl AuthPrompt, cookie: &str) -> bool {
        let found = self.pending.remove(cookie).is_some();
        if found {
            prompt.dismiss(cookie);
        }
        found
    }

    /// Removes a request once the UI has answered it (success or user abort).
    pub fn finish(&mut self, cookie: &str) -> Option<BeginRequest> {
        self.pending.remove(cookie)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// Builds `RegisterAuthenticationAgent(subject, locale, object_path)` for a `unix-session` subject.
///
/// # Errors
/// Returns an error when the subject dictionary cannot be encoded.
pub fn register_message(session_id: &str, locale: &str) -> FlameResult<Message> {
    let subject = session_subject(session_id)?;
    let mut call = authority_call(REGISTER)?;
    call.append_items(&[
        subject,
        MessageItem::Str(locale.to_owned()),
        MessageItem::ObjectPath(Path::from(AGENT_PATH)),
    ]);
    Ok(call)
}

/// Builds `UnregisterAuthenticationAgent(subject, object_path)`.
///
/// # Errors
/// Returns an error when the subject dictionary cannot be encoded.
pub fn unregister_message(session_id: &str) -> FlameResult<Message> {
    let mut call = authority_call(UNREGISTER)?;
    call.append_items(&[
        session_subject(session_id)?,
        MessageItem::ObjectPath(Path::from(AGENT_PATH)),
    ]);
    Ok(call)
}

fn authority_call(method: &str) -> FlameResult<Message> {
    Message::new_method_call(AUTHORITY_DEST, AUTHORITY_PATH, AUTHORITY_IFACE, method)
        .map_err(|error| invalid(&format!("polkit call {method}: {error}")))
}

fn session_subject(session_id: &str) -> FlameResult<MessageItem> {
    let props = MessageItem::new_dict(vec![(
        MessageItem::Str("session-id".to_owned()),
        MessageItem::Variant(Box::new(MessageItem::Str(session_id.to_owned()))),
    )])
    .map_err(|error| invalid(&format!("polkit subject: {error:?}")))?;
    Ok(MessageItem::Struct(vec![
        MessageItem::Str("unix-session".to_owned()),
        props,
    ]))
}

/// Parses the six arguments of `BeginAuthentication`.
///
/// # Errors
/// Returns `InvalidArgument` for a wrong arity, a wrong type, or an empty cookie.
pub fn parse_begin(items: &[MessageItem]) -> FlameResult<BeginRequest> {
    let [action, message, icon, details, cookie, identities] = items else {
        return Err(invalid("BeginAuthentication expects 6 arguments"));
    };
    let cookie = text(cookie).ok_or_else(|| invalid("cookie is not a string"))?;
    if cookie.is_empty() {
        return Err(invalid("empty authentication cookie"));
    }
    let identities = match identities {
        MessageItem::Array(list) => list
            .iter()
            .map(parse_identity)
            .collect::<FlameResult<_>>()?,
        _ => return Err(invalid("identities is not an array")),
    };
    Ok(BeginRequest {
        action_id: text(action).ok_or_else(|| invalid("action_id is not a string"))?,
        message: text(message).ok_or_else(|| invalid("message is not a string"))?,
        icon_name: text(icon).ok_or_else(|| invalid("icon_name is not a string"))?,
        details: string_pairs(details)?,
        cookie,
        identities,
    })
}

/// Parses the single cookie argument of `CancelAuthentication`.
///
/// # Errors
/// Returns `InvalidArgument` when the argument is not exactly one string.
pub fn parse_cancel(items: &[MessageItem]) -> FlameResult<String> {
    match items {
        [cookie] => text(cookie).ok_or_else(|| invalid("cookie is not a string")),
        _ => Err(invalid("CancelAuthentication expects 1 argument")),
    }
}

/// Routes one incoming method call on `AGENT_PATH`. Unknown members fall through as `Unhandled`
/// so the pump returns the standard D-Bus error reply.
pub fn handle_agent_call(
    sessions: &mut AgentSessions,
    prompt: &mut impl AuthPrompt,
    message: &Message,
) -> MessageDisposition {
    if message.msg_type() != MessageType::MethodCall {
        return MessageDisposition::Unhandled;
    }
    let (Some(iface), Some(member)) = (message.interface(), message.member()) else {
        return MessageDisposition::Unhandled;
    };
    let iface = iface.to_string();
    if iface != AGENT_IFACE {
        return MessageDisposition::Unhandled;
    }
    let outcome = match member.to_string().as_str() {
        BEGIN => parse_begin(&message.get_items()).and_then(|req| sessions.begin(prompt, req)),
        CANCEL => parse_cancel(&message.get_items()).map(|cookie| {
            sessions.cancel(prompt, &cookie);
        }),
        _ => return MessageDisposition::Unhandled,
    };
    match outcome {
        Ok(()) => MessageDisposition::Reply(message.method_return()),
        Err(error) => MessageDisposition::Reply(error_reply(message, &error)),
    }
}

fn error_reply(call: &Message, error: &FlameError) -> Message {
    let name = ErrorName::new(INVALID_ARGS).ok();
    let text = CString::new(error.to_string()).unwrap_or_default();
    match name {
        Some(name) => call.error(&name, &text),
        None => call.method_return(),
    }
}

fn parse_identity(item: &MessageItem) -> FlameResult<Identity> {
    let MessageItem::Struct(fields) = item else {
        return Err(invalid("identity is not a struct"));
    };
    let [kind, props] = fields.as_slice() else {
        return Err(invalid("identity struct needs 2 fields"));
    };
    let kind = text(kind).ok_or_else(|| invalid("identity kind is not a string"))?;
    let uid = match props {
        MessageItem::Dict(dict) => dict_value(dict, "uid").and_then(as_u32),
        _ => return Err(invalid("identity properties are not a dict")),
    };
    Ok(Identity { kind, uid })
}

fn string_pairs(item: &MessageItem) -> FlameResult<Vec<(String, String)>> {
    let MessageItem::Dict(dict) = item else {
        return Err(invalid("details is not a dict"));
    };
    dict.iter()
        .map(|(k, v)| {
            let key = text(k).ok_or_else(|| invalid("detail key is not a string"))?;
            let value = text(v).ok_or_else(|| invalid("detail value is not a string"))?;
            Ok((key, value))
        })
        .collect()
}

fn dict_value<'a>(dict: &'a MessageItemDict, key: &str) -> Option<&'a MessageItem> {
    dict.iter().find_map(|(k, v)| match k {
        MessageItem::Str(name) if name == key => Some(unwrap_variant(v)),
        _ => None,
    })
}

fn unwrap_variant(item: &MessageItem) -> &MessageItem {
    match item {
        MessageItem::Variant(inner) => unwrap_variant(inner),
        other => other,
    }
}

fn as_u32(item: &MessageItem) -> Option<u32> {
    match unwrap_variant(item) {
        MessageItem::UInt32(value) => Some(*value),
        _ => None,
    }
}

fn text(item: &MessageItem) -> Option<String> {
    match item {
        MessageItem::Str(value) => Some(value.clone()),
        _ => None,
    }
}

fn invalid(message: &str) -> FlameError {
    FlameError::new(ErrorCode::InvalidArgument, format!("polkit: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MockPrompt {
        shown: Vec<String>,
        dismissed: Vec<String>,
    }

    impl AuthPrompt for MockPrompt {
        fn show(&mut self, request: &BeginRequest) {
            self.shown.push(request.cookie.clone());
        }
        fn dismiss(&mut self, cookie: &str) {
            self.dismissed.push(cookie.to_owned());
        }
    }

    fn identity(uid: u32) -> MessageItem {
        let props = MessageItem::new_dict(vec![(
            MessageItem::Str("uid".to_owned()),
            MessageItem::Variant(Box::new(MessageItem::UInt32(uid))),
        )])
        .expect("dict");
        MessageItem::Struct(vec![MessageItem::Str("unix-user".to_owned()), props])
    }

    fn begin_items(cookie: &str) -> Vec<MessageItem> {
        let details = MessageItem::new_dict(vec![(
            MessageItem::Str("polkit.message".to_owned()),
            MessageItem::Str("Authentication is required".to_owned()),
        )])
        .expect("dict");
        let identities = MessageItem::new_array(vec![identity(1000)]).expect("array");
        vec![
            MessageItem::Str("org.freedesktop.udisks2.filesystem-mount".to_owned()),
            MessageItem::Str("Authenticate to mount".to_owned()),
            MessageItem::Str("drive-removable-media".to_owned()),
            details,
            MessageItem::Str(cookie.to_owned()),
            identities,
        ]
    }

    fn call(method: &str, items: &[MessageItem]) -> Message {
        let mut msg =
            Message::new_method_call(":1.42", AGENT_PATH, AGENT_IFACE, method).expect("call");
        msg.append_items(items);
        // Incoming calls always carry a serial; libdbus aborts building a reply without one.
        msg.set_serial(7);
        msg
    }

    #[test]
    fn register_message_carries_session_subject_and_path() {
        let msg = register_message("c1", "en_US.UTF-8").expect("register");
        assert_eq!(msg.member().expect("member").to_string(), REGISTER);
        let items = msg.get_items();
        assert_eq!(items.len(), 3);
        assert_eq!(items[1], MessageItem::Str("en_US.UTF-8".to_owned()));
        let MessageItem::ObjectPath(path) = &items[2] else {
            panic!("third argument must be an object path");
        };
        assert_eq!(path.to_string(), AGENT_PATH);
    }

    #[test]
    fn begin_parses_identity_uid_and_details() {
        let req = parse_begin(&begin_items("cookie-a")).expect("begin");
        assert_eq!(req.cookie, "cookie-a");
        assert_eq!(
            req.identities,
            vec![Identity {
                kind: "unix-user".into(),
                uid: Some(1000)
            }]
        );
        assert_eq!(req.details[0].0, "polkit.message");
    }

    #[test]
    fn begin_rejects_bad_arity_and_empty_cookie() {
        assert!(parse_begin(&begin_items("")).is_err());
        assert!(parse_begin(&begin_items("x")[..5]).is_err());
    }

    #[test]
    fn begin_then_cancel_dismisses_prompt_once() {
        let mut sessions = AgentSessions::default();
        let mut prompt = MockPrompt::default();
        let r = handle_agent_call(&mut sessions, &mut prompt, &call(BEGIN, &begin_items("c2")));
        assert!(matches!(r, MessageDisposition::Reply(_)));
        assert_eq!(prompt.shown, vec!["c2".to_owned()]);
        assert_eq!(sessions.len(), 1);

        let cancel = call(CANCEL, &[MessageItem::Str("c2".to_owned())]);
        assert!(matches!(
            handle_agent_call(&mut sessions, &mut prompt, &cancel),
            MessageDisposition::Reply(_)
        ));
        assert_eq!(prompt.dismissed, vec!["c2".to_owned()]);
        assert!(sessions.is_empty());

        // Second cancel for the same cookie is a harmless no-op.
        assert!(!sessions.cancel(&mut prompt, "c2"));
        assert_eq!(prompt.dismissed.len(), 1);
    }

    #[test]
    fn duplicate_cookie_is_rejected_without_second_prompt() {
        let mut sessions = AgentSessions::default();
        let mut prompt = MockPrompt::default();
        sessions
            .begin(&mut prompt, parse_begin(&begin_items("d")).expect("p"))
            .expect("first");
        assert!(
            sessions
                .begin(&mut prompt, parse_begin(&begin_items("d")).expect("p"))
                .is_err()
        );
        assert_eq!(prompt.shown.len(), 1);
    }

    #[test]
    fn finish_removes_request_and_unknown_members_are_unhandled() {
        let mut sessions = AgentSessions::default();
        let mut prompt = MockPrompt::default();
        sessions
            .begin(&mut prompt, parse_begin(&begin_items("e")).expect("p"))
            .expect("begin");
        assert!(sessions.finish("e").is_some());
        assert!(sessions.finish("e").is_none());
        let other = call("Bogus", &[]);
        assert!(matches!(
            handle_agent_call(&mut sessions, &mut prompt, &other),
            MessageDisposition::Unhandled
        ));
    }
}

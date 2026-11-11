//! Polkit authentication agent core: registration, `BeginAuthentication` and
//! `CancelAuthentication` handling. No UI and no password I/O.
//!
//! The caller owns the system-bus pump (`flamewm_dbus_reactor::BusPump`) and routes calls on
//! `AGENT_PATH` through `handle_agent_call`. Prompting the user goes through `AuthPrompt`; the
//! password step (polkit-agent-helper-1) is out of scope here.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::CString;
use std::time::{Duration, Instant};

use dbus::arg::messageitem::{MessageItem, MessageItemDict};
use dbus::strings::ErrorName;
use dbus::{Message, MessageType, Path};
use flamewm_api::{ErrorCode, FlameError, FlameResult};
use flamewm_dbus_reactor::{BusKind, BusPump, BusWatch, MessageDisposition};

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
/// polkit error names for `BeginAuthentication` outcomes (polkit `AuthenticationAgent` spec).
const CANCELLED_ERR: &str = "org.freedesktop.PolicyKit1.Error.Cancelled";
const FAILED_ERR: &str = "org.freedesktop.PolicyKit1.Error.Failed";
/// Longest a prompt may stay unanswered before its `BeginAuthentication` fails. Bounds every
/// pending call so polkit never waits forever on a prompt the user ignores.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);

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

/// The UI side. `show` starts a prompt for one cookie and may refuse it (`Err` becomes an error
/// reply to polkit); `dismiss` withdraws it on cancel.
pub trait AuthPrompt {
    /// # Errors
    /// Returns an error when the prompt cannot be accepted, for example a full queue.
    fn show(&mut self, request: &BeginRequest) -> FlameResult<()>;
    fn dismiss(&mut self, cookie: &str);
}

/// Default bound on prompt events the shell has not drained yet.
pub const MAX_QUEUED_EVENTS: usize = 16;

/// Typed event the shell drains from `PolkitEventQueue` and turns into UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolkitEvent {
    Show(BeginRequest),
    Dismiss(String),
}

/// Bounded FIFO between the bus callback and the shell. `show` fails when full, so polkit gets an
/// error reply instead of an unbounded backlog. `dismiss` for a `Show` the shell has not drained
/// removes that `Show`, so the shell never displays a cancelled prompt. Otherwise `Dismiss` is
/// queued; those are only produced for prompts the shell already drained.
#[derive(Debug)]
pub struct PolkitEventQueue {
    events: VecDeque<PolkitEvent>,
    capacity: usize,
}

impl Default for PolkitEventQueue {
    fn default() -> Self {
        Self::with_capacity(MAX_QUEUED_EVENTS)
    }
}

impl PolkitEventQueue {
    /// Creates a queue holding at most `capacity` undrained `Show` events (minimum 1).
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            events: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    /// Takes every queued event in arrival order.
    pub fn drain(&mut self) -> std::collections::vec_deque::Drain<'_, PolkitEvent> {
        self.events.drain(..)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

impl AuthPrompt for PolkitEventQueue {
    fn show(&mut self, request: &BeginRequest) -> FlameResult<()> {
        if self.events.len() >= self.capacity {
            return Err(FlameError::new(
                ErrorCode::Busy,
                "polkit: prompt queue full",
            ));
        }
        self.events.push_back(PolkitEvent::Show(request.clone()));
        Ok(())
    }

    fn dismiss(&mut self, cookie: &str) {
        let undrained = self
            .events
            .iter()
            .position(|event| matches!(event, PolkitEvent::Show(req) if req.cookie == cookie));
        match undrained {
            Some(index) => {
                self.events.remove(index);
            }
            None => self
                .events
                .push_back(PolkitEvent::Dismiss(cookie.to_owned())),
        }
    }
}

/// Outcome of one prompt, chosen by the UI side. Each maps to one reply to the pending
/// `BeginAuthentication` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthOutcome {
    Success,
    Cancelled,
    Failed,
}

/// The three possible replies to one `BeginAuthentication` call, built from the call while it is
/// still borrowed, so the deferred reply needs no copy of the message.
pub struct Replies {
    success: Message,
    cancelled: Message,
    failed: Message,
}

impl Replies {
    #[must_use]
    pub fn for_call(call: &Message) -> Self {
        Self {
            success: call.method_return(),
            cancelled: error_named(call, CANCELLED_ERR, "authentication cancelled"),
            failed: error_named(call, FAILED_ERR, "authentication failed"),
        }
    }
}

struct Pending {
    request: BeginRequest,
    replies: Replies,
    since: Instant,
}

/// Pending authentication requests keyed by polkit cookie. A `BeginAuthentication` call stays
/// unanswered until its outcome is known; the reply is queued in `outbox` and the owner sends it.
#[derive(Default)]
pub struct AgentSessions {
    pending: BTreeMap<String, Pending>,
    outbox: Vec<Message>,
}

impl AgentSessions {
    /// Registers a request and shows its prompt. Duplicate cookies are rejected.
    pub fn begin(
        &mut self,
        prompt: &mut impl AuthPrompt,
        request: BeginRequest,
        replies: Replies,
    ) -> FlameResult<()> {
        if self.pending.contains_key(&request.cookie) {
            return Err(invalid("duplicate authentication cookie"));
        }
        prompt.show(&request)?;
        self.pending.insert(
            request.cookie.clone(),
            Pending {
                request,
                replies,
                since: Instant::now(),
            },
        );
        Ok(())
    }

    /// Cancels a request and answers its pending call with `Cancelled`. Returns `false` for unknown
    /// or already finished cookies, which polkit may send harmlessly.
    pub fn cancel(&mut self, prompt: &mut impl AuthPrompt, cookie: &str) -> bool {
        match self.pending.remove(cookie) {
            Some(pending) => {
                prompt.dismiss(cookie);
                self.outbox.push(pending.replies.cancelled);
                true
            }
            None => false,
        }
    }

    /// Answers a request with its outcome once the UI has decided it.
    pub fn finish(&mut self, cookie: &str, outcome: AuthOutcome) -> Option<BeginRequest> {
        let pending = self.pending.remove(cookie)?;
        self.outbox.push(match outcome {
            AuthOutcome::Success => pending.replies.success,
            AuthOutcome::Cancelled => pending.replies.cancelled,
            AuthOutcome::Failed => pending.replies.failed,
        });
        Some(pending.request)
    }

    /// Fails every prompt pending for at least `max_age` as `Failed`. Returns how many expired.
    pub fn expire(
        &mut self,
        prompt: &mut impl AuthPrompt,
        now: Instant,
        max_age: Duration,
    ) -> usize {
        let stale: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| now.saturating_duration_since(p.since) >= max_age)
            .map(|(cookie, _)| cookie.clone())
            .collect();
        for cookie in &stale {
            if let Some(pending) = self.pending.remove(cookie) {
                prompt.dismiss(cookie);
                self.outbox.push(pending.replies.failed);
            }
        }
        stale.len()
    }

    /// Takes the replies to send on the bus, in the order they were decided.
    pub fn take_outbox(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.outbox)
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
        // The reply is deferred: `Handled` here, the real answer is queued by `finish`/`cancel`.
        BEGIN => {
            return match parse_begin(&message.get_items()) {
                Ok(req) => match sessions.begin(prompt, req, Replies::for_call(message)) {
                    Ok(()) => MessageDisposition::Handled,
                    Err(error) => MessageDisposition::Reply(error_reply(message, &error)),
                },
                Err(error) => MessageDisposition::Reply(error_reply(message, &error)),
            };
        }
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
    error_named(call, INVALID_ARGS, &error.to_string())
}

fn error_named(call: &Message, name: &str, text: &str) -> Message {
    let text = CString::new(text).unwrap_or_default();
    match ErrorName::new(name).ok() {
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

/// Logind lookup of the session that owns a process. The real implementation is a
/// `org.freedesktop.login1.Manager.GetSessionByPID` call on the system bus that maps the returned
/// object path to its `Id` property; this crate does not provide it yet (see `pol-sessid`).
pub trait SessionLookup {
    /// Returns the logind session id (the `Id` property, e.g. `"2"`) for `pid`.
    ///
    /// # Errors
    /// Returns an error when logind has no session for the process or the call fails.
    fn session_id_for_pid(&self, pid: u32) -> FlameResult<String>;
}

/// Resolves the polkit `unix-session` subject id.
///
/// `XDG_SESSION_ID` (passed in by the caller, never read here) wins when it is non-blank. Otherwise
/// the logind lookup is consulted for `pid`. A lookup failure or a blank logind answer is an error:
/// there is no guessed fallback, because a wrong session id registers the agent for the wrong seat.
///
/// # Errors
/// Returns `InvalidArgument` when neither source yields a non-blank id, or the lookup error.
pub fn resolve_session_id(
    env_session_id: Option<&str>,
    pid: u32,
    lookup: &impl SessionLookup,
) -> FlameResult<String> {
    if let Some(id) = env_session_id.map(str::trim).filter(|id| !id.is_empty()) {
        return Ok(id.to_owned());
    }
    let id = lookup.session_id_for_pid(pid)?;
    let id = id.trim();
    if id.is_empty() {
        return Err(invalid("logind returned an empty session id"));
    }
    Ok(id.to_owned())
}

const LOGIN_DEST: &str = "org.freedesktop.login1";
const LOGIN_MANAGER_PATH: &str = "/org/freedesktop/login1";
const LOGIN_MANAGER_IFACE: &str = "org.freedesktop.login1.Manager";
const LOGIN_SESSION_IFACE: &str = "org.freedesktop.login1.Session";
const NO_SESSION_FOR_PID: &str = "org.freedesktop.login1.NoSessionForPID";
const LOGIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const AUTHORITY_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// The two logind calls the lookup needs. `SystemLogind` is the system-bus implementation; tests
/// use a fake, so no test touches a bus.
pub trait LogindWire {
    /// `Manager.GetSessionByPID(pid)`: the session object path.
    fn session_path_for_pid(&self, pid: u32) -> FlameResult<String>;
    /// Reads the `Id` property of a session object path.
    fn session_id_of(&self, session_path: &str) -> FlameResult<String>;
}

/// `SessionLookup` over a `LogindWire`: `GetSessionByPID`, then the session's `Id` property.
pub struct LogindLookup<W: LogindWire> {
    wire: W,
}

impl<W: LogindWire> LogindLookup<W> {
    #[must_use]
    pub fn new(wire: W) -> Self {
        Self { wire }
    }
}

impl<W: LogindWire> SessionLookup for LogindLookup<W> {
    fn session_id_for_pid(&self, pid: u32) -> FlameResult<String> {
        let path = self.wire.session_path_for_pid(pid)?;
        let id = self.wire.session_id_of(&path)?;
        let id = id.trim();
        if id.is_empty() {
            return Err(invalid("logind session has an empty Id"));
        }
        Ok(id.to_owned())
    }
}

/// Blocking system-bus logind client, bounded by `LOGIN_TIMEOUT`. Owned by the caller that needs
/// a session id; this module adds no event source for it.
pub struct SystemLogind {
    conn: dbus::blocking::Connection,
}

impl SystemLogind {
    /// # Errors
    /// Returns `Unavailable` when the system bus cannot be reached.
    pub fn connect() -> FlameResult<Self> {
        dbus::blocking::Connection::new_system()
            .map(|conn| Self { conn })
            .map_err(|error| {
                FlameError::new(
                    ErrorCode::Unavailable,
                    format!("polkit: system bus: {error}"),
                )
            })
    }
}

impl LogindWire for SystemLogind {
    fn session_path_for_pid(&self, pid: u32) -> FlameResult<String> {
        let manager = self
            .conn
            .with_proxy(LOGIN_DEST, LOGIN_MANAGER_PATH, LOGIN_TIMEOUT);
        let (path,): (dbus::Path<'static>,) = manager
            .method_call(LOGIN_MANAGER_IFACE, "GetSessionByPID", (pid,))
            .map_err(logind_error)?;
        Ok(path.to_string())
    }

    fn session_id_of(&self, session_path: &str) -> FlameResult<String> {
        use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
        let path = dbus::Path::new(session_path)
            .map(dbus::Path::into_static)
            .map_err(|error| invalid(&format!("session path {session_path}: {error}")))?;
        let session = self.conn.with_proxy(LOGIN_DEST, path, LOGIN_TIMEOUT);
        session
            .get::<String>(LOGIN_SESSION_IFACE, "Id")
            .map_err(logind_error)
    }
}

/// Maps a logind failure: "no session for this pid" is `NotFound`, anything else `Unavailable`.
fn logind_error(error: dbus::Error) -> FlameError {
    let code = if error.name() == Some(NO_SESSION_FOR_PID) {
        ErrorCode::NotFound
    } else {
        ErrorCode::Unavailable
    };
    FlameError::new(code, format!("polkit: logind: {error}"))
}

/// Shell-owned polkit agent: one system-bus pump, the pending prompt state and the bounded event
/// queue the shell drains. `connect_system` registers the agent; `shutdown` unregisters it on
/// orderly exit only. No thread and no event source of its own.
pub struct PolkitAgent {
    pump: BusPump,
    session_id: String,
    sessions: AgentSessions,
    queue: PolkitEventQueue,
}

impl PolkitAgent {
    /// Opens the system bus, resolves this process's logind session and registers the agent.
    ///
    /// # Errors
    /// Returns the bus, session-lookup or registration error; the caller runs without an agent.
    pub fn connect_system(env_session_id: Option<&str>, locale: &str) -> FlameResult<Self> {
        let pump = BusPump::connect(BusKind::System)?;
        let lookup = LogindLookup::new(SystemLogind::connect()?);
        let session_id = resolve_session_id(env_session_id, std::process::id(), &lookup)?;
        Self::register_on(pump, &session_id, locale)
    }

    /// Registers the agent for `session_id` over an already-open bus. `connect_system` uses it
    /// after logind lookup; tests use it with a private bus.
    ///
    /// # Errors
    /// Returns the encoding or bus error of the register call.
    pub fn register_on(pump: BusPump, session_id: &str, locale: &str) -> FlameResult<Self> {
        pump.send_with_reply(
            register_message(session_id, locale)?,
            AUTHORITY_REPLY_TIMEOUT,
        )?;
        Ok(Self {
            pump,
            session_id: session_id.to_owned(),
            sessions: AgentSessions::default(),
            queue: PolkitEventQueue::default(),
        })
    }

    #[must_use]
    pub fn watch(&self) -> BusWatch {
        self.pump.watch()
    }

    /// Dispatches incoming calls on `AGENT_PATH` into the session state and prompt queue.
    ///
    /// # Errors
    /// Returns the bus error from the pump turn.
    pub fn on_ready(&mut self) -> FlameResult<usize> {
        let Self {
            pump,
            sessions,
            queue,
            ..
        } = self;
        let visited = pump.on_ready_with_object_path(
            AGENT_PATH,
            |message| handle_agent_call(sessions, queue, message),
            |_| MessageDisposition::Unhandled,
        )?;
        sessions.expire(queue, Instant::now(), PROMPT_TIMEOUT);
        flush(pump, sessions)?;
        Ok(visited)
    }

    /// Takes the prompt events queued since the last call, in arrival order.
    pub fn take_events(&mut self) -> Vec<PolkitEvent> {
        self.queue.drain().collect()
    }

    /// Removes a request once the UI has answered it or the shell cannot show it.
    ///
    /// # Errors
    /// Returns the bus error when the outcome reply cannot be queued on the bus.
    pub fn finish(
        &mut self,
        cookie: &str,
        outcome: AuthOutcome,
    ) -> FlameResult<Option<BeginRequest>> {
        let request = self.sessions.finish(cookie, outcome);
        flush(&self.pump, &mut self.sessions)?;
        Ok(request)
    }

    /// Unregisters the agent for this session. Call only on orderly exit.
    ///
    /// # Errors
    /// Returns the encoding or bus error of the unregister call.
    pub fn shutdown(&self) -> FlameResult<()> {
        let call = unregister_message(&self.session_id)?;
        self.pump
            .send_with_reply(call, AUTHORITY_REPLY_TIMEOUT)
            .map(|_| ())
    }
}

/// Sends every queued `BeginAuthentication` reply. Sends all of them even after one fails, then
/// reports the first error.
fn flush(pump: &BusPump, sessions: &mut AgentSessions) -> FlameResult<()> {
    let mut first_error = None;
    for reply in sessions.take_outbox() {
        if let Err(error) = pump.send(reply) {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod session_id_tests {
    use std::cell::Cell;

    use super::*;

    struct FakeLogind {
        reply: Option<&'static str>,
        calls: Cell<u32>,
    }

    impl FakeLogind {
        fn new(reply: Option<&'static str>) -> Self {
            Self {
                reply,
                calls: Cell::new(0),
            }
        }
    }

    impl SessionLookup for FakeLogind {
        fn session_id_for_pid(&self, pid: u32) -> FlameResult<String> {
            self.calls.set(self.calls.get() + 1);
            assert_eq!(pid, 4242, "lookup must be asked for the agent's pid");
            self.reply
                .map(str::to_owned)
                .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "fake: no session for pid"))
        }
    }

    #[test]
    fn env_session_id_wins_and_skips_logind() {
        let fake = FakeLogind::new(Some("99"));
        assert_eq!(
            resolve_session_id(Some("7"), 4242, &fake).expect("env"),
            "7"
        );
        assert_eq!(fake.calls.get(), 0);
    }

    #[test]
    fn env_is_trimmed_before_use() {
        let fake = FakeLogind::new(None);
        assert_eq!(
            resolve_session_id(Some("  3\n"), 4242, &fake).expect("env"),
            "3"
        );
        assert_eq!(fake.calls.get(), 0);
    }

    #[test]
    fn missing_env_falls_back_to_logind() {
        let fake = FakeLogind::new(Some("12"));
        assert_eq!(resolve_session_id(None, 4242, &fake).expect("logind"), "12");
        assert_eq!(fake.calls.get(), 1);
    }

    #[test]
    fn blank_env_falls_back_to_logind() {
        let fake = FakeLogind::new(Some("5"));
        assert_eq!(
            resolve_session_id(Some("   "), 4242, &fake).expect("logind"),
            "5"
        );
        assert_eq!(fake.calls.get(), 1);
    }

    #[test]
    fn logind_failure_is_reported_when_env_absent() {
        let fake = FakeLogind::new(None);
        let err = resolve_session_id(None, 4242, &fake).expect_err("no session");
        assert!(matches!(err.code, ErrorCode::NotFound));
    }

    #[test]
    fn blank_logind_answer_is_rejected() {
        let fake = FakeLogind::new(Some(" "));
        let err = resolve_session_id(Some(""), 4242, &fake).expect_err("blank");
        assert!(matches!(err.code, ErrorCode::InvalidArgument));
    }
}

#[cfg(test)]
mod logind_lookup_tests {
    use super::*;

    const SESSION: &str = "/org/freedesktop/login1/session/_32";

    /// Mock logind: no bus. `by_pid` is the session path `GetSessionByPID` returns, if any.
    struct FakeWire {
        by_pid: Option<&'static str>,
        id: &'static str,
    }

    impl LogindWire for FakeWire {
        fn session_path_for_pid(&self, pid: u32) -> FlameResult<String> {
            assert_eq!(pid, 4242, "lookup must be asked for the agent's pid");
            self.by_pid
                .map(str::to_owned)
                .ok_or_else(|| FlameError::new(ErrorCode::NotFound, "fake: no session"))
        }

        fn session_id_of(&self, session_path: &str) -> FlameResult<String> {
            assert_eq!(session_path, SESSION);
            Ok(self.id.to_owned())
        }
    }

    #[test]
    fn get_session_by_pid_then_id_property() {
        let lookup = LogindLookup::new(FakeWire {
            by_pid: Some(SESSION),
            id: "2",
        });
        assert_eq!(lookup.session_id_for_pid(4242).expect("id"), "2");
    }

    #[test]
    fn no_session_for_pid_is_typed_not_found() {
        let lookup = LogindLookup::new(FakeWire {
            by_pid: None,
            id: "2",
        });
        let err = resolve_session_id(None, 4242, &lookup).expect_err("no session");
        assert!(matches!(err.code, ErrorCode::NotFound));
    }

    #[test]
    fn blank_id_property_is_rejected() {
        let lookup = LogindLookup::new(FakeWire {
            by_pid: Some(SESSION),
            id: "  ",
        });
        let err = lookup.session_id_for_pid(4242).expect_err("blank");
        assert!(matches!(err.code, ErrorCode::InvalidArgument));
    }

    #[test]
    fn env_session_id_takes_precedence_over_logind() {
        let lookup = LogindLookup::new(FakeWire {
            by_pid: Some(SESSION),
            id: "9",
        });
        assert_eq!(
            resolve_session_id(Some("3"), 4242, &lookup).expect("env"),
            "3"
        );
        assert_eq!(
            resolve_session_id(None, 4242, &lookup).expect("logind"),
            "9"
        );
    }

    #[test]
    fn no_session_error_name_maps_to_not_found_other_errors_unavailable() {
        let no_session = dbus::Error::new_custom(NO_SESSION_FOR_PID, "no session");
        assert!(matches!(logind_error(no_session).code, ErrorCode::NotFound));
        let other = dbus::Error::new_custom("org.freedesktop.DBus.Error.NoReply", "timeout");
        assert!(matches!(logind_error(other).code, ErrorCode::Unavailable));
    }
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
        fn show(&mut self, request: &BeginRequest) -> FlameResult<()> {
            self.shown.push(request.cookie.clone());
            Ok(())
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

    fn replies(cookie: &str) -> Replies {
        Replies::for_call(&call(BEGIN, &begin_items(cookie)))
    }

    #[test]
    fn begin_then_cancel_dismisses_prompt_once() {
        let mut sessions = AgentSessions::default();
        let mut prompt = MockPrompt::default();
        let r = handle_agent_call(&mut sessions, &mut prompt, &call(BEGIN, &begin_items("c2")));
        assert!(matches!(r, MessageDisposition::Handled));
        assert!(
            sessions.take_outbox().is_empty(),
            "begin must not reply before the outcome"
        );
        assert_eq!(prompt.shown, vec!["c2".to_owned()]);
        assert_eq!(sessions.len(), 1);

        let cancel = call(CANCEL, &[MessageItem::Str("c2".to_owned())]);
        assert!(matches!(
            handle_agent_call(&mut sessions, &mut prompt, &cancel),
            MessageDisposition::Reply(_)
        ));
        assert_eq!(prompt.dismissed, vec!["c2".to_owned()]);
        assert!(sessions.is_empty());
        let outbox = sessions.take_outbox();
        assert_eq!(outbox.len(), 1, "cancel must complete the pending begin");
        assert_eq!(outbox[0].msg_type(), MessageType::Error);

        // Second cancel for the same cookie is a harmless no-op.
        assert!(!sessions.cancel(&mut prompt, "c2"));
        assert_eq!(prompt.dismissed.len(), 1);
        assert!(sessions.take_outbox().is_empty());
    }

    #[test]
    fn success_replies_only_on_finish() {
        let mut sessions = AgentSessions::default();
        let mut prompt = MockPrompt::default();
        let begin = call(BEGIN, &begin_items("s"));
        assert!(matches!(
            handle_agent_call(&mut sessions, &mut prompt, &begin),
            MessageDisposition::Handled
        ));
        assert!(sessions.take_outbox().is_empty());
        assert!(sessions.finish("s", AuthOutcome::Success).is_some());
        let outbox = sessions.take_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].msg_type(), MessageType::MethodReturn);
        assert!(sessions.finish("s", AuthOutcome::Success).is_none());
    }

    #[test]
    fn failed_finish_sends_error_reply() {
        let mut sessions = AgentSessions::default();
        let mut prompt = MockPrompt::default();
        sessions
            .begin(
                &mut prompt,
                parse_begin(&begin_items("f")).expect("p"),
                replies("f"),
            )
            .expect("begin");
        sessions.finish("f", AuthOutcome::Failed);
        assert_eq!(sessions.take_outbox()[0].msg_type(), MessageType::Error);
    }

    #[test]
    fn unanswered_prompt_expires_as_failed() {
        let mut sessions = AgentSessions::default();
        let mut prompt = MockPrompt::default();
        sessions
            .begin(
                &mut prompt,
                parse_begin(&begin_items("t")).expect("p"),
                replies("t"),
            )
            .expect("begin");
        let later = Instant::now() + PROMPT_TIMEOUT + Duration::from_secs(1);
        assert_eq!(sessions.expire(&mut prompt, later, PROMPT_TIMEOUT), 1);
        assert!(sessions.is_empty());
        assert_eq!(prompt.dismissed, vec!["t".to_owned()]);
        assert_eq!(sessions.take_outbox()[0].msg_type(), MessageType::Error);
        assert_eq!(sessions.expire(&mut prompt, later, PROMPT_TIMEOUT), 0);
    }

    #[test]
    fn duplicate_cookie_is_rejected_without_second_prompt() {
        let mut sessions = AgentSessions::default();
        let mut prompt = MockPrompt::default();
        sessions
            .begin(
                &mut prompt,
                parse_begin(&begin_items("d")).expect("p"),
                replies("d"),
            )
            .expect("first");
        assert!(
            sessions
                .begin(
                    &mut prompt,
                    parse_begin(&begin_items("d")).expect("p"),
                    replies("d")
                )
                .is_err()
        );
        assert_eq!(prompt.shown.len(), 1);
    }

    #[test]
    fn full_queue_rejects_second_prompt_without_pending_entry() {
        let mut sessions = AgentSessions::default();
        let mut queue = PolkitEventQueue::with_capacity(1);
        let first = parse_begin(&begin_items("q1")).expect("p");
        sessions
            .begin(&mut queue, first.clone(), replies("q1"))
            .expect("first");
        let second = call(BEGIN, &begin_items("q2"));
        assert!(matches!(
            handle_agent_call(&mut sessions, &mut queue, &second),
            MessageDisposition::Reply(_)
        ));
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            queue.drain().collect::<Vec<_>>(),
            vec![PolkitEvent::Show(first)]
        );
    }

    #[test]
    fn dismiss_before_drain_cancels_undelivered_show() {
        let mut sessions = AgentSessions::default();
        let mut queue = PolkitEventQueue::default();
        sessions
            .begin(
                &mut queue,
                parse_begin(&begin_items("c")).expect("p"),
                replies("c"),
            )
            .expect("begin");
        assert!(sessions.cancel(&mut queue, "c"));
        assert!(queue.is_empty());
    }

    #[test]
    fn dismiss_after_drain_is_queued_for_shell() {
        let mut sessions = AgentSessions::default();
        let mut queue = PolkitEventQueue::default();
        sessions
            .begin(
                &mut queue,
                parse_begin(&begin_items("c")).expect("p"),
                replies("c"),
            )
            .expect("begin");
        assert_eq!(queue.drain().count(), 1);
        assert!(sessions.cancel(&mut queue, "c"));
        assert_eq!(
            queue.drain().collect::<Vec<_>>(),
            vec![PolkitEvent::Dismiss("c".to_owned())]
        );
    }

    #[test]
    fn finish_removes_request_and_unknown_members_are_unhandled() {
        let mut sessions = AgentSessions::default();
        let mut prompt = MockPrompt::default();
        sessions
            .begin(
                &mut prompt,
                parse_begin(&begin_items("e")).expect("p"),
                replies("e"),
            )
            .expect("begin");
        assert!(sessions.finish("e", AuthOutcome::Success).is_some());
        assert!(sessions.finish("e", AuthOutcome::Success).is_none());
        let other = call("Bogus", &[]);
        assert!(matches!(
            handle_agent_call(&mut sessions, &mut prompt, &other),
            MessageDisposition::Unhandled
        ));
    }
}

#[cfg(test)]
mod e2e_tests;

//! Hermes API server client: health, models and agent turns over loopback HTTP.
//!
//! A turn is a Hermes run: `POST /v1/runs` creates it with the session id, then
//! `GET /v1/runs/{id}/events` streams its progress. The run keeps executing on
//! the server if the stream drops, so the client polls `GET /v1/runs/{id}` to
//! recover the final answer or report the reply as interrupted.

use std::fmt;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::http::{self, Endpoint, Request};
use crate::runs::{decode_event, decode_status, is_terminal, RunEvent, RunStatus};
use crate::sse::SseParser;
use crate::{AssistantError, CancelToken};

pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8642";
/// Ports probed after the configured URL and the `.env` port: the documented default.
pub const PROBE_PORTS: [u16; 1] = [8642];
/// Header carrying the Hermes session id on the chat-completions route. Kept for
/// compatibility; runs carry the session in the body as `session_id`.
pub const SESSION_HEADER: &str = "X-Hermes-Session-Id";
/// Per-run system instruction that keeps answers short.
pub const SHORT_ANSWER_SYSTEM: &str =
    "Answer in one or two short sentences unless the user asks for more detail.";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const PROBE_IDLE: Duration = Duration::from_secs(2);
/// The server sends a keepalive comment every 10 seconds, so silence beyond this is a hang.
const STREAM_IDLE: Duration = Duration::from_secs(120);
const MAX_JSON: usize = 1 << 20;
const MAX_ERROR_BODY: usize = 4096;
/// Status polls after a dropped stream, one second apart.
const RECOVER_POLLS: u32 = 3;

/// Connection settings. `api_key` is sent as a Bearer token when set.
#[derive(Clone, PartialEq, Eq)]
pub struct HermesConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
}

impl Default for HermesConfig {
    fn default() -> Self {
        Self { base_url: DEFAULT_BASE_URL.to_owned(), api_key: None, model: "hermes-agent".to_owned() }
    }
}

impl fmt::Debug for HermesConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HermesConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("model", &self.model)
            .finish()
    }
}

/// Result of `GET /health`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerHealth {
    pub status: String,
    pub platform: Option<String>,
    pub authenticated: Option<bool>,
}

impl ServerHealth {
    /// True only for a Hermes agent server that reports ok. Other services on the
    /// probed ports can answer `/health` too, and their run routes do not exist.
    pub fn is_ok(&self) -> bool {
        self.status == "ok" && self.platform.as_deref() == Some("hermes-agent")
    }
}

/// One user turn. `system` is sent as the run's `instructions` on every turn.
#[derive(Clone, Copy, Debug)]
pub struct TurnRequest<'a> {
    pub user_text: &'a str,
    pub session: Option<&'a str>,
    pub system: &'a str,
}

impl<'a> TurnRequest<'a> {
    pub fn new(user_text: &'a str, session: Option<&'a str>) -> Self {
        Self { user_text, session, system: SHORT_ANSWER_SYSTEM }
    }
}

/// Live events delivered to the sink while a turn runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnEvent {
    /// The run was accepted; `session` is the id it runs in.
    Started { session: Option<String> },
    Delta(String),
    /// Progress text for the UI, such as the tool in use.
    Activity(String),
}

/// How a turn ended. Transport and HTTP failures and failed runs are `Err` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed { text: String, session: Option<String> },
    /// Cancelled by the user; `partial` is the text streamed so far. Save it as `MsgState::Cancelled`.
    Cancelled { partial: String },
    /// The connection dropped and the run did not finish in time. The run may still
    /// finish on the server. Save `partial` as `MsgState::Partial` and say why.
    Interrupted { partial: String, message: String },
}

/// Values read from `~/.hermes/.env`. Key names only; the file is never written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HermesEnv {
    pub api_server_enabled: bool,
    pub port: Option<u16>,
    pub api_key: Option<String>,
}

/// Parses `API_SERVER_ENABLED`, `API_SERVER_PORT` and `API_SERVER_KEY` from `<home>/.env`.
/// A missing file yields the default (disabled, no key).
pub fn read_env(home: &Path) -> HermesEnv {
    let Ok(text) = std::fs::read_to_string(home.join(".env")) else {
        return HermesEnv::default();
    };
    let mut env = HermesEnv::default();
    for raw in text.lines() {
        let line = raw.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
        match key.trim() {
            "API_SERVER_ENABLED" => env.api_server_enabled = value.eq_ignore_ascii_case("true") || value == "1",
            "API_SERVER_PORT" => env.port = value.parse().ok(),
            "API_SERVER_KEY" => env.api_key = (!value.is_empty()).then(|| value.to_owned()),
            _ => {}
        }
    }
    env
}

/// Builds the probe order: configured URL, then the `.env` port, then `PROBE_PORTS`. Duplicates are dropped.
pub fn candidate_urls(configured: Option<&str>, env: &HermesEnv) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |url: String| {
        if !out.contains(&url) {
            out.push(url);
        }
    };
    if let Some(url) = configured.map(str::trim).filter(|u| !u.is_empty()) {
        push(url.trim_end_matches('/').to_owned());
    }
    if let Some(port) = env.port {
        push(format!("http://127.0.0.1:{port}"));
    }
    for port in PROBE_PORTS {
        push(format!("http://127.0.0.1:{port}"));
    }
    out
}

/// Returns the first candidate whose `/health` reports ok, with its health.
pub fn discover(candidates: &[String], api_key: Option<&str>) -> Option<(String, ServerHealth)> {
    candidates.iter().find_map(|url| {
        let config = HermesConfig {
            base_url: url.clone(),
            api_key: api_key.map(str::to_owned),
            ..HermesConfig::default()
        };
        let health = HermesClient::new(&config).ok()?.health().ok()?;
        health.is_ok().then(|| (url.clone(), health))
    })
}

/// Client for one Hermes API server.
pub struct HermesClient {
    endpoint: Endpoint,
    api_key: Option<String>,
    model: String,
}

impl HermesClient {
    pub fn new(config: &HermesConfig) -> Result<Self, AssistantError> {
        Ok(Self {
            endpoint: Endpoint::parse(&config.base_url)?,
            api_key: config.api_key.clone(),
            model: config.model.clone(),
        })
    }

    pub fn url(&self) -> String {
        self.endpoint.url()
    }

    /// The model name sent with runs.
    pub fn model(&self) -> &str {
        &self.model
    }

    fn auth_headers(&self) -> Vec<(&'static str, String)> {
        self.api_key.iter().map(|key| ("Authorization", format!("Bearer {key}"))).collect()
    }

    fn get_json(&self, path: &str) -> Result<Value, AssistantError> {
        let resp = http::send(
            &self.endpoint,
            Request { method: "GET", path, headers: self.auth_headers(), body: None },
            &CancelToken::new(),
            CONNECT_TIMEOUT,
            PROBE_IDLE,
        )?;
        let status = resp.status;
        let mut body = resp.body;
        let bytes = body.read_limited(MAX_JSON)?;
        if status != 200 {
            return Err(http_error(status, &bytes));
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| AssistantError::Protocol(format!("bad JSON from {path}: {e}")))
    }

    /// `GET /health`.
    pub fn health(&self) -> Result<ServerHealth, AssistantError> {
        let value = self.get_json("/health")?;
        Ok(ServerHealth {
            status: value.get("status").and_then(Value::as_str).unwrap_or_default().to_owned(),
            platform: value.get("platform").and_then(Value::as_str).map(str::to_owned),
            authenticated: value.get("authenticated").and_then(Value::as_bool),
        })
    }

    /// `GET /v1/models`: advertised model ids.
    pub fn models(&self) -> Result<Vec<String>, AssistantError> {
        let value = self.get_json("/v1/models")?;
        Ok(value
            .get("data")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|m| m.get("id").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default())
    }

    /// `GET /v1/runs/{id}`: the run's current state.
    pub fn run_status(&self, run_id: &str) -> Result<RunStatus, AssistantError> {
        let resp = http::send(
            &self.endpoint,
            Request { method: "GET", path: &format!("/v1/runs/{run_id}"), headers: self.auth_headers(), body: None },
            &CancelToken::new(),
            CONNECT_TIMEOUT,
            PROBE_IDLE,
        )?;
        let status = resp.status;
        let mut body = resp.body;
        let bytes = body.read_limited(MAX_JSON)?;
        if status != 200 {
            return Err(http_error(status, &bytes));
        }
        decode_status(&bytes)
    }

    /// `POST /v1/runs/{id}/stop`. Best effort: the run settles as cancelled on the server.
    pub fn stop_run(&self, run_id: &str) -> Result<(), AssistantError> {
        self.post_control(run_id, "stop", "{}")
    }

    /// `POST /v1/runs/{id}/approval` with `choice` (`once`, `session`, `always` or `deny`).
    pub fn respond_approval(&self, run_id: &str, choice: &str) -> Result<(), AssistantError> {
        self.post_control(run_id, "approval", &json!({ "choice": choice }).to_string())
    }

    fn post_control(&self, run_id: &str, action: &str, body: &str) -> Result<(), AssistantError> {
        let resp = http::send(
            &self.endpoint,
            Request {
                method: "POST",
                path: &format!("/v1/runs/{run_id}/{action}"),
                headers: self.auth_headers(),
                body: Some(body),
            },
            &CancelToken::new(),
            CONNECT_TIMEOUT,
            PROBE_IDLE,
        )?;
        let status = resp.status;
        let mut body = resp.body;
        let bytes = body.read_limited(MAX_ERROR_BODY)?;
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(http_error(status, &bytes))
        }
    }

    /// Starts a run and streams it until it ends. Blocks, so call it from a worker
    /// thread. Stop it by cancelling `cancel`; the run is stopped on the server.
    pub fn run_turn(
        &self,
        req: &TurnRequest<'_>,
        cancel: &CancelToken,
        sink: &mut dyn FnMut(TurnEvent),
    ) -> Result<TurnOutcome, AssistantError> {
        match self.run_turn_inner(req, cancel, sink) {
            Err(AssistantError::Cancelled) => Ok(TurnOutcome::Cancelled { partial: String::new() }),
            other => other,
        }
    }

    fn run_turn_inner(
        &self,
        req: &TurnRequest<'_>,
        cancel: &CancelToken,
        sink: &mut dyn FnMut(TurnEvent),
    ) -> Result<TurnOutcome, AssistantError> {
        if cancel.is_cancelled() {
            return Ok(TurnOutcome::Cancelled { partial: String::new() });
        }
        let session = req.session.map_or_else(new_session_id, str::to_owned);
        let payload = json!({
            "input": req.user_text,
            "session_id": session,
            "instructions": req.system,
        })
        .to_string();
        let resp = http::send(
            &self.endpoint,
            Request { method: "POST", path: "/v1/runs", headers: self.auth_headers(), body: Some(&payload) },
            cancel,
            CONNECT_TIMEOUT,
            PROBE_IDLE,
        )?;
        let status = resp.status;
        let mut body = resp.body;
        let bytes = body.read_limited(MAX_ERROR_BODY)?;
        if status != 200 && status != 202 {
            return Err(http_error(status, &bytes));
        }
        let run_id = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|v| v.get("run_id").and_then(Value::as_str).map(str::to_owned))
            .filter(|id| !id.is_empty())
            .ok_or_else(|| AssistantError::Protocol("run response has no run_id".into()))?;
        sink(TurnEvent::Started { session: Some(session.clone()) });

        let mut stream = Stream { text: String::new(), sink };
        let mut resp = http::send(
            &self.endpoint,
            Request {
                method: "GET",
                path: &format!("/v1/runs/{run_id}/events"),
                headers: self.auth_headers(),
                body: None,
            },
            cancel,
            CONNECT_TIMEOUT,
            STREAM_IDLE,
        )?;
        if resp.status != 200 {
            let bytes = resp.body.read_limited(MAX_ERROR_BODY).unwrap_or_default();
            return Err(http_error(resp.status, &bytes));
        }

        let mut parser = SseParser::default();
        let mut frames = Vec::new();
        let mut finish = None;
        let mut dropped = false;
        'read: loop {
            match resp.body.next_data() {
                Ok(Some(piece)) => {
                    parser.feed(&piece, &mut frames);
                }
                Ok(None) => break 'read,
                Err(AssistantError::Cancelled) => {
                    let _ = self.stop_run(&run_id);
                    return Ok(TurnOutcome::Cancelled { partial: stream.text });
                }
                Err(_) => {
                    dropped = true;
                    break 'read;
                }
            }
            for data in frames.drain(..) {
                let event = decode_event(&data)?;
                match event {
                    RunEvent::Approval { .. } => {
                        // No approval UI yet: deny, so the run cannot run a gated tool unseen.
                        let _ = self.respond_approval(&run_id, "deny");
                        (stream.sink)(TurnEvent::Activity(
                            "Hermes asked to run a command that needs approval. It was denied.".into(),
                        ));
                    }
                    other => {
                        if let Some(done) = stream.apply(other) {
                            finish = Some(done);
                            break 'read;
                        }
                    }
                }
            }
        }
        if finish.is_none() && !dropped {
            parser.finish(&mut frames);
            for data in frames.drain(..) {
                if let Some(done) = stream.apply(decode_event(&data)?) {
                    finish = Some(done);
                    break;
                }
            }
        }
        let text = stream.text;
        match finish {
            Some(Finish::Completed(output)) => Ok(TurnOutcome::Completed {
                text: output.unwrap_or(text),
                session: Some(session),
            }),
            Some(Finish::Failed(message)) => Err(AssistantError::RunFailed(message)),
            Some(Finish::Cancelled) => Ok(TurnOutcome::Cancelled { partial: text }),
            Some(Finish::Interrupted(message)) => Ok(TurnOutcome::Interrupted {
                partial: text,
                message: message.unwrap_or_else(|| "Hermes stopped before the reply finished.".into()),
            }),
            None => self.recover(&run_id, session, text, cancel),
        }
    }

    /// The stream ended without a terminal event. Polls the run and reports what it found.
    fn recover(
        &self,
        run_id: &str,
        session: String,
        partial: String,
        cancel: &CancelToken,
    ) -> Result<TurnOutcome, AssistantError> {
        for attempt in 0..RECOVER_POLLS {
            if attempt > 0 {
                std::thread::sleep(Duration::from_secs(1));
            }
            if cancel.is_cancelled() {
                return Ok(TurnOutcome::Cancelled { partial });
            }
            let Ok(status) = self.run_status(run_id) else {
                continue;
            };
            if !is_terminal(&status.status) {
                continue;
            }
            return match status.status.as_str() {
                "completed" => Ok(TurnOutcome::Completed {
                    text: status.output.unwrap_or(partial),
                    session: status.session_id.or(Some(session)),
                }),
                "failed" => Err(AssistantError::RunFailed(
                    status.error.unwrap_or_else(|| "the run failed".into()),
                )),
                "cancelled" => Ok(TurnOutcome::Cancelled { partial }),
                _ => Ok(TurnOutcome::Interrupted {
                    partial,
                    message: status.error.unwrap_or_else(|| "Hermes stopped before the reply finished.".into()),
                }),
            };
        }
        Ok(TurnOutcome::Interrupted {
            partial,
            message: "The connection to Hermes dropped before the reply finished.".into(),
        })
    }
}

/// How the event stream reached its end.
enum Finish {
    Completed(Option<String>),
    Failed(String),
    Cancelled,
    Interrupted(Option<String>),
}

/// Accumulates deltas and forwards UI events. `apply` returns `Some` at a terminal event.
struct Stream<'s> {
    text: String,
    sink: &'s mut dyn FnMut(TurnEvent),
}

impl Stream<'_> {
    fn apply(&mut self, event: RunEvent) -> Option<Finish> {
        match event {
            RunEvent::Delta(delta) => {
                self.text.push_str(&delta);
                (self.sink)(TurnEvent::Delta(delta));
                None
            }
            RunEvent::ToolStarted { tool } => {
                (self.sink)(TurnEvent::Activity(format!("Using {tool}...")));
                None
            }
            RunEvent::ToolCompleted { tool, error } => {
                let text = if error { format!("{tool} failed, continuing...") } else { format!("{tool} finished.") };
                (self.sink)(TurnEvent::Activity(text));
                None
            }
            RunEvent::Completed { output } => Some(Finish::Completed(output)),
            RunEvent::Failed(message) => Some(Finish::Failed(message)),
            RunEvent::Cancelled => Some(Finish::Cancelled),
            RunEvent::Interrupted(message) => Some(Finish::Interrupted(message)),
            RunEvent::Approval { .. } | RunEvent::Other => None,
        }
    }
}

/// A fresh session id for runs that are not resuming one. Unique per process start and time.
fn new_session_id() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("flamewm-{:x}-{:x}", nanos, std::process::id())
}

fn http_error(status: u16, body: &[u8]) -> AssistantError {
    let message = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_else(|| String::from_utf8_lossy(&body[..body.len().min(200)]).trim().to_owned());
    AssistantError::Http { status, message }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::test_support::{ok_response, serve, test_dir};

    fn client(url: &str, key: Option<&str>) -> HermesClient {
        HermesClient::new(&HermesConfig {
            base_url: url.to_owned(),
            api_key: key.map(str::to_owned),
            ..HermesConfig::default()
        })
        .unwrap()
    }

    /// Response head for an event stream closed by the server (no length, no chunking).
    fn sse_head() -> Vec<u8> {
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n".to_vec()
    }

    fn accepted(run_id: &str) -> Vec<Vec<u8>> {
        ok_response("", &format!(r#"{{"run_id":"{run_id}","status":"started","replayed":false}}"#))
    }

    fn collect() -> (Arc<Mutex<Vec<TurnEvent>>>, impl FnMut(TurnEvent)) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = Arc::clone(&events);
        (events, move |e: TurnEvent| sink_events.lock().unwrap().push(e))
    }

    fn sse(lines: &[&str]) -> Vec<u8> {
        lines.iter().map(|l| format!("data: {l}\n\n")).collect::<String>().into_bytes()
    }

    #[test]
    fn health_and_models_parse_with_bearer_auth() {
        let (url, server) = serve(vec![
            ok_response("", r#"{"status":"ok","platform":"hermes-agent","authenticated":true}"#),
            ok_response("", r#"{"data":[{"id":"hermes-agent"},{"id":"other"}]}"#),
        ]);
        let c = client(&url, Some("k1"));
        let health = c.health().unwrap();
        assert!(health.is_ok());
        assert_eq!(health.authenticated, Some(true));
        assert_eq!(c.models().unwrap(), vec!["hermes-agent".to_owned(), "other".to_owned()]);
        let seen = server.join().unwrap();
        assert!(seen[0].starts_with("GET /health "));
        assert!(seen[1].to_lowercase().contains("authorization: bearer k1"));
    }

    #[test]
    fn run_streams_deltas_and_completes_with_session() {
        let mut stream = sse_head();
        stream.extend(sse(&[
            r#"{"event":"run.started","run_id":"run_1"}"#,
            r#"{"event":"message.delta","run_id":"run_1","delta":"Hel"}"#,
            r#"{"event":"message.delta","run_id":"run_1","delta":"lo"}"#,
            r#"{"event":"run.completed","run_id":"run_1","output":"Hello."}"#,
        ]));
        let (url, server) = serve(vec![accepted("run_1"), vec![stream]]);
        let (events, mut sink) = collect();
        let outcome = client(&url, None)
            .run_turn(&TurnRequest::new("hi", Some("sess-1")), &CancelToken::new(), &mut sink)
            .unwrap();
        assert_eq!(outcome, TurnOutcome::Completed { text: "Hello.".into(), session: Some("sess-1".into()) });
        let events = events.lock().unwrap();
        assert_eq!(events[0], TurnEvent::Started { session: Some("sess-1".into()) });
        assert_eq!(events[1], TurnEvent::Delta("Hel".into()));
        assert_eq!(events[2], TurnEvent::Delta("lo".into()));

        let seen = server.join().unwrap();
        assert!(seen[0].starts_with("POST /v1/runs "));
        assert!(seen[0].contains(r#""session_id":"sess-1""#));
        assert!(seen[0].contains(r#""input":"hi""#));
        assert!(seen[0].contains(r#""instructions":"#));
        assert!(seen[1].starts_with("GET /v1/runs/run_1/events "));
    }

    #[test]
    fn new_session_is_generated_when_none_given() {
        let (url, server) = serve(vec![accepted("run_2"), vec![{
            let mut s = sse_head();
            s.extend(sse(&[r#"{"event":"run.completed","output":"ok"}"#]));
            s
        }]]);
        let (events, mut sink) = collect();
        let outcome = client(&url, None).run_turn(&TurnRequest::new("hi", None), &CancelToken::new(), &mut sink).unwrap();
        let TurnOutcome::Completed { session, .. } = outcome else { panic!("not completed") };
        let session = session.unwrap();
        assert!(session.starts_with("flamewm-"));
        assert_eq!(events.lock().unwrap()[0], TurnEvent::Started { session: Some(session) });
        assert!(server.join().unwrap()[0].contains(r#""session_id":"flamewm-"#));
    }

    #[test]
    fn approval_is_denied_and_reported() {
        let mut stream = sse_head();
        stream.extend(sse(&[
            r#"{"event":"approval.request","choices":["once","deny"],"request_id":"p1"}"#,
            r#"{"event":"run.completed","output":"Not run."}"#,
        ]));
        let (url, server) = serve(vec![accepted("run_3"), vec![stream], ok_response("", "{}")]);
        let (events, mut sink) = collect();
        let outcome = client(&url, None).run_turn(&TurnRequest::new("rm", None), &CancelToken::new(), &mut sink).unwrap();
        assert_eq!(outcome, TurnOutcome::Completed { text: "Not run.".into(), session: outcome_session(&outcome) });
        let seen = server.join().unwrap();
        assert!(seen[2].starts_with("POST /v1/runs/run_3/approval "));
        assert!(seen[2].contains(r#""choice":"deny""#));
        assert!(events.lock().unwrap().iter().any(|e| matches!(e, TurnEvent::Activity(t) if t.contains("denied"))));
    }

    fn outcome_session(outcome: &TurnOutcome) -> Option<String> {
        match outcome {
            TurnOutcome::Completed { session, .. } => session.clone(),
            _ => None,
        }
    }

    #[test]
    fn failed_run_is_an_error_with_the_server_message() {
        let mut stream = sse_head();
        stream.extend(sse(&[r#"{"event":"run.failed","error":"Hermes is not logged in."}"#]));
        let (url, server) = serve(vec![accepted("run_4"), vec![stream]]);
        let err = client(&url, None).run_turn(&TurnRequest::new("hi", None), &CancelToken::new(), &mut |_| {}).unwrap_err();
        assert_eq!(err, AssistantError::RunFailed("Hermes is not logged in.".into()));
        server.join().unwrap();
    }

    #[test]
    fn dropped_stream_recovers_the_final_answer_from_status() {
        let mut partial = sse_head();
        partial.extend(sse(&[r#"{"event":"message.delta","delta":"Hel"}"#]));
        let status = ok_response("", r#"{"status":"completed","output":"Hello there.","session_id":"s-9"}"#);
        let (url, server) = serve(vec![accepted("run_5"), vec![partial], status]);
        let outcome = client(&url, None)
            .run_turn(&TurnRequest::new("hi", Some("s-9")), &CancelToken::new(), &mut |_| {})
            .unwrap();
        assert_eq!(outcome, TurnOutcome::Completed { text: "Hello there.".into(), session: Some("s-9".into()) });
        assert!(server.join().unwrap()[2].starts_with("GET /v1/runs/run_5 "));
    }

    #[test]
    fn dropped_stream_with_run_still_going_is_interrupted_with_partial() {
        let mut partial = sse_head();
        partial.extend(sse(&[r#"{"event":"message.delta","delta":"Hel"}"#]));
        let mut status = Vec::new();
        for _ in 0..RECOVER_POLLS {
            status.push(ok_response("", r#"{"status":"running","output":""}"#));
        }
        let mut responses = vec![accepted("run_6"), vec![partial]];
        responses.extend(status);
        let (url, server) = serve(responses);
        let outcome = client(&url, None)
            .run_turn(&TurnRequest::new("hi", None), &CancelToken::new(), &mut |_| {})
            .unwrap();
        assert!(matches!(outcome, TurnOutcome::Interrupted { ref partial, .. } if partial == "Hel"), "{outcome:?}");
        server.join().unwrap();
    }

    #[test]
    fn cancel_mid_stream_returns_partial_and_stops_the_run() {
        let mut first = sse_head();
        first.extend(sse(&[r#"{"event":"message.delta","delta":"Hel"}"#]));
        let (url, server) = serve(vec![accepted("run_7"), vec![first], ok_response("", "{}")]);
        let cancel = CancelToken::new();
        let trigger = cancel.clone();
        let mut sink = move |e: TurnEvent| {
            if matches!(e, TurnEvent::Delta(_)) {
                trigger.cancel();
            }
        };
        let outcome = client(&url, None).run_turn(&TurnRequest::new("hi", None), &cancel, &mut sink).unwrap();
        assert_eq!(outcome, TurnOutcome::Cancelled { partial: "Hel".into() });
        let seen = server.join().unwrap();
        assert!(seen[2].starts_with("POST /v1/runs/run_7/stop "), "{}", seen[2]);
    }

    #[test]
    fn create_run_404_reports_session_gone() {
        let body = r#"{"error":{"message":"session not found"}}"#;
        let response = format!("HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        let (url, server) = serve(vec![vec![response.into_bytes()]]);
        let err = client(&url, None)
            .run_turn(&TurnRequest::new("hi", Some("gone")), &CancelToken::new(), &mut |_| {})
            .unwrap_err();
        assert!(err.is_session_gone(), "{err:?}");
        assert!(matches!(err, AssistantError::Http { status: 404, ref message } if message == "session not found"));
        server.join().unwrap();
    }

    #[test]
    fn unauthorized_run_reports_401() {
        let body = r#"{"error":{"message":"Invalid API key"}}"#;
        let response = format!("HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        let (url, server) = serve(vec![vec![response.into_bytes()]]);
        let err = client(&url, Some("bad"))
            .run_turn(&TurnRequest::new("hi", None), &CancelToken::new(), &mut |_| {})
            .unwrap_err();
        assert!(matches!(err, AssistantError::Http { status: 401, .. }), "{err:?}");
        server.join().unwrap();
    }

    #[test]
    fn pre_cancelled_turn_sends_nothing() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let outcome = client("http://127.0.0.1:1", None)
            .run_turn(&TurnRequest::new("hi", None), &cancel, &mut |_| {})
            .unwrap();
        assert_eq!(outcome, TurnOutcome::Cancelled { partial: String::new() });
    }

    #[test]
    fn env_file_and_candidates() {
        let dir = test_dir("env-candidates");
        std::fs::write(
            dir.join(".env"),
            "# comment\nAPI_SERVER_ENABLED=true\nexport API_SERVER_PORT=\"9001\"\nAPI_SERVER_KEY='secret'\n",
        )
        .unwrap();
        let env = read_env(&dir);
        assert_eq!(
            env,
            HermesEnv { api_server_enabled: true, port: Some(9001), api_key: Some("secret".into()) }
        );
        assert_eq!(read_env(&dir.join("missing")), HermesEnv::default());
        let urls = candidate_urls(Some("http://127.0.0.1:8642/"), &env);
        assert_eq!(
            urls,
            vec![
                "http://127.0.0.1:8642".to_owned(),
                "http://127.0.0.1:9001".to_owned(),
            ]
        );
    }

    #[test]
    fn discover_skips_dead_candidate() {
        let (url, server) = serve(vec![ok_response("", r#"{"status":"ok","platform":"hermes-agent"}"#)]);
        let found = discover(&["http://127.0.0.1:1".to_owned(), url.clone()], None).unwrap();
        assert_eq!(found.0, url);
        server.join().unwrap();
    }

    #[test]
    fn discover_rejects_other_services_that_report_ok() {
        let (url, server) = serve(vec![ok_response("", r#"{"status":"ok","upstream":"Nous Portal"}"#)]);
        assert!(discover(&[url], None).is_none());
        server.join().unwrap();
    }

    /// Drives the real local Hermes API server. Run by hand: `cargo test -p flamewm-assistant-core -- --ignored real_hermes_run`.
    /// Sends one harmless prompt in a fresh session. Needs a configured model.
    #[test]
    #[ignore = "calls the real Hermes API server and its model"]
    fn real_hermes_run() {
        let hermes_home = match std::env::var_os("HERMES_HOME") {
            Some(home) => std::path::PathBuf::from(home),
            None => Path::new(&std::env::var_os("HOME").expect("HOME")).join(".hermes"),
        };
        let env = read_env(&hermes_home);
        let urls = candidate_urls(None, &env);
        let (url, health) = discover(&urls, env.api_key.as_deref()).expect("no Hermes API server answered");
        eprintln!("hermes at {url} health={health:?}");
        let client = HermesClient::new(&HermesConfig {
            base_url: url,
            api_key: env.api_key.clone(),
            ..HermesConfig::default()
        })
        .unwrap();
        let session = new_session_id();
        let outcome = client
            .run_turn(&TurnRequest::new("say hi in three words", Some(&session)), &CancelToken::new(), &mut |_| {})
            .unwrap();
        eprintln!("outcome={outcome:?}");
        assert!(matches!(outcome, TurnOutcome::Completed { ref text, .. } if !text.trim().is_empty()));
    }

    #[test]
    fn debug_redacts_key() {
        let config = HermesConfig { api_key: Some("supersecret".into()), ..HermesConfig::default() };
        assert!(!format!("{config:?}").contains("supersecret"));
    }
}

//! Hermes API server client: health, models and streamed chat turns over loopback HTTP.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use crate::http::{self, Endpoint, Request};
use crate::sse::{parse_chat_chunk, ChatChunk, SseParser};
use crate::{AssistantError, CancelToken};

pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8642";
/// Ports probed after the configured URL: the documented default and the port
/// the live API server was seen on.
pub const PROBE_PORTS: [u16; 2] = [8642, 8645];
/// Header carrying the Hermes session id; sent on requests and echoed on responses.
pub const SESSION_HEADER: &str = "X-Hermes-Session-Id";
/// Per-request system message that keeps answers short.
pub const SHORT_ANSWER_SYSTEM: &str =
    "Answer in one or two short sentences unless the user asks for more detail.";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const PROBE_IDLE: Duration = Duration::from_secs(2);
const CHAT_IDLE: Duration = Duration::from_secs(120);
const MAX_JSON: usize = 1 << 20;
const MAX_ERROR_BODY: usize = 4096;

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
    pub authenticated: Option<bool>,
}

impl ServerHealth {
    pub fn is_ok(&self) -> bool {
        self.status == "ok"
    }
}

/// One user turn. `system` is sent as a system message on every request.
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

/// Live events delivered to the sink while a turn streams.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnEvent {
    /// Response headers arrived; `session` is the id Hermes assigned or echoed.
    Started { session: Option<String> },
    Delta(String),
}

/// How a streamed turn ended. Transport and HTTP failures are `Err` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed { text: String, session: Option<String> },
    /// Cancelled; `partial` is the text streamed so far. Save it as `MsgState::Cancelled`.
    Cancelled { partial: String },
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

    /// `POST /v1/chat/completions` with `stream: true`. Blocks until the turn
    /// ends; call from a worker thread. Stop it by cancelling `cancel`.
    pub fn run_turn(
        &self,
        req: &TurnRequest<'_>,
        cancel: &CancelToken,
        sink: &mut dyn FnMut(TurnEvent),
    ) -> Result<TurnOutcome, AssistantError> {
        if cancel.is_cancelled() {
            return Ok(TurnOutcome::Cancelled { partial: String::new() });
        }
        let payload = json!({
            "model": self.model,
            "stream": true,
            "messages": [
                {"role": "system", "content": req.system},
                {"role": "user", "content": req.user_text},
            ],
        })
        .to_string();
        let mut headers = self.auth_headers();
        if let Some(session) = req.session {
            headers.push((SESSION_HEADER, session.to_owned()));
        }
        let mut resp = http::send(
            &self.endpoint,
            Request { method: "POST", path: "/v1/chat/completions", headers, body: Some(&payload) },
            cancel,
            CONNECT_TIMEOUT,
            CHAT_IDLE,
        )?;
        let session = resp.header("x-hermes-session-id").map(str::to_owned);
        if resp.status != 200 {
            let bytes = resp.body.read_limited(MAX_ERROR_BODY).unwrap_or_default();
            return Err(http_error(resp.status, &bytes));
        }
        sink(TurnEvent::Started { session: session.clone() });

        let mut parser = SseParser::default();
        let mut events = Vec::new();
        let mut text = String::new();
        let mut done = false;
        while !done {
            let piece = match resp.body.next_data() {
                Ok(Some(piece)) => piece,
                Ok(None) => break,
                Err(AssistantError::Cancelled) => return Ok(TurnOutcome::Cancelled { partial: text }),
                Err(e) => return Err(e),
            };
            parser.feed(&piece, &mut events);
            done = drain_events(&mut events, &mut text, sink)?;
        }
        if !done {
            parser.finish(&mut events);
            drain_events(&mut events, &mut text, sink)?;
        }
        Ok(TurnOutcome::Completed { text, session })
    }
}

fn drain_events(
    events: &mut Vec<String>,
    text: &mut String,
    sink: &mut dyn FnMut(TurnEvent),
) -> Result<bool, AssistantError> {
    for data in events.drain(..) {
        match parse_chat_chunk(&data)? {
            ChatChunk::Delta(delta) => {
                text.push_str(&delta);
                sink(TurnEvent::Delta(delta));
            }
            ChatChunk::Ignore => {}
            ChatChunk::Done => return Ok(true),
        }
    }
    Ok(false)
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

    #[test]
    fn health_and_models_parse_with_bearer_auth() {
        let (url, server) = serve(vec![
            ok_response("", r#"{"status":"ok","authenticated":true}"#),
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
    fn chunked_stream_yields_text_session_and_sends_system_message() {
        let body1 = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n";
        let body2 = "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\ndata: [DONE]\n\n";
        let chunk = |s: &str| format!("{:x}\r\n{s}\r\n", s.len()).into_bytes();
        let mut pieces = vec![
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nX-Hermes-Session-Id: sess-9\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec(),
        ];
        // Split the chunk data mid-line to exercise framing across reads.
        let c1 = chunk(body1);
        pieces.push(c1[..7].to_vec());
        pieces.push(c1[7..].to_vec());
        pieces.push(chunk(body2));
        pieces.push(b"0\r\n\r\n".to_vec());
        let (url, server) = serve(vec![pieces]);

        let events = Arc::new(Mutex::new(Vec::new()));
        let sink_events = Arc::clone(&events);
        let mut sink = move |e: TurnEvent| sink_events.lock().unwrap().push(e);
        let outcome = client(&url, None)
            .run_turn(&TurnRequest::new("hi", Some("sess-1")), &CancelToken::new(), &mut sink)
            .unwrap();
        assert_eq!(
            outcome,
            TurnOutcome::Completed { text: "Hello".into(), session: Some("sess-9".into()) }
        );
        let events = events.lock().unwrap();
        assert_eq!(events[0], TurnEvent::Started { session: Some("sess-9".into()) });
        assert_eq!(events[1], TurnEvent::Delta("Hel".into()));
        assert_eq!(events[2], TurnEvent::Delta("lo".into()));

        let request = &server.join().unwrap()[0];
        assert!(request.starts_with("POST /v1/chat/completions "));
        assert!(request.to_lowercase().contains("x-hermes-session-id: sess-1"));
        assert!(request.contains(r#""role":"system""#));
        assert!(request.contains(r#""stream":true"#));
    }

    #[test]
    fn http_404_reports_session_gone() {
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
    fn cancel_mid_stream_returns_partial() {
        let first = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        let piece = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n";
        let chunk = format!("{:x}\r\n{piece}\r\n", piece.len()).into_bytes();
        let (url, server) = serve(vec![vec![first, chunk]]);
        let cancel = CancelToken::new();
        let trigger = cancel.clone();
        let mut sink = move |e: TurnEvent| {
            if matches!(e, TurnEvent::Delta(_)) {
                trigger.cancel();
            }
        };
        let outcome = client(&url, None)
            .run_turn(&TurnRequest::new("hi", None), &cancel, &mut sink)
            .unwrap();
        assert_eq!(outcome, TurnOutcome::Cancelled { partial: "Hel".into() });
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
                "http://127.0.0.1:8645".to_owned(),
            ]
        );
    }

    #[test]
    fn discover_skips_dead_candidate() {
        let (url, server) = serve(vec![ok_response("", r#"{"status":"ok"}"#)]);
        let found = discover(&["http://127.0.0.1:1".to_owned(), url.clone()], None).unwrap();
        assert_eq!(found.0, url);
        server.join().unwrap();
    }

    #[test]
    fn debug_redacts_key() {
        let config = HermesConfig { api_key: Some("supersecret".into()), ..HermesConfig::default() };
        assert!(!format!("{config:?}").contains("supersecret"));
    }
}

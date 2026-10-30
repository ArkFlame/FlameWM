//! OpenAI-compatible client shared by ARK AI and the Hermes proxy: model listing and streamed chat.
//!
//! HTTP runs through a `curl` subprocess, as model downloads do, so no TLS code lives here.
//! The request (URL, `Authorization` header and JSON body) goes to curl on stdin as a config
//! block, so the bearer token never appears in argv, in a file, or in an error message.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::sse::SseParser;
use crate::{AssistantError, CancelToken};

/// Keeps replies short. Sent before every chat.
pub const SYSTEM_PROMPT: &str = "Answer briefly: one to three short sentences unless the user asks for detail.";
/// Most recent chat turns sent with each request.
pub const HISTORY_TURNS: usize = 12;

const POLL: Duration = Duration::from_millis(100);
const MAX_ERROR_BODY: usize = 64 * 1024;
const MAX_MESSAGE_CHARS: usize = 160;

/// Where to reach one OpenAI-compatible server. `label` names it in user-facing messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub label: &'static str,
    /// Includes the `/v1` prefix. Paths such as `/models` are appended to it.
    pub base_url: String,
    pub bearer: String,
    /// Whole-request limit in seconds. Set only for quick probes, never for streamed chat.
    pub max_time: Option<u32>,
}

/// One earlier message in the chat window. `role` is `user` or `assistant`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub role: &'static str,
    pub content: String,
}

/// Client for one endpoint. Only `https`, or `http` on 127.0.0.1 or localhost, is accepted.
pub struct OpenAiClient {
    endpoint: Endpoint,
    proto: &'static str,
}

/// What `exchange` hands the caller: one SSE `data:` payload, or a whole JSON body.
enum Event {
    Data(String),
    Json(Vec<u8>),
}

impl OpenAiClient {
    pub fn new(endpoint: Endpoint) -> Result<Self, AssistantError> {
        let proto = scheme_proto(&endpoint.base_url)?;
        Ok(Self { endpoint, proto })
    }

    /// Full URL for a path under the base, such as `/models`.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.endpoint.base_url.trim_end_matches('/'))
    }

    /// `GET` of a full URL that must answer with a JSON object. HTML or plain text is an error.
    pub fn get_json(&self, url: &str) -> Result<Value, AssistantError> {
        let mut body = None;
        self.exchange(url, None, &CancelToken::new(), &mut |event| {
            if let Event::Json(bytes) = event {
                body = Some(bytes);
            }
            Ok(false)
        })?;
        let bytes = body.ok_or_else(|| self.protocol("reply is not JSON"))?;
        serde_json::from_slice(&bytes).map_err(|_| self.protocol("reply is not JSON"))
    }

    /// `GET /models`: the model ids in the order the server lists them.
    pub fn list_models(&self) -> Result<Vec<String>, AssistantError> {
        let value = self.get_json(&self.url("/models"))?;
        self.parse_models(&value)
    }

    /// `POST /chat/completions` with streaming. `on_delta` gets each text piece as it arrives.
    /// A server that answers with one JSON body instead of a stream is read as one delta.
    /// Returns the full reply text. Only the last `HISTORY_TURNS` turns are sent.
    pub fn chat(
        &self,
        model: &str,
        history: &[Turn],
        cancel: &CancelToken,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<String, AssistantError> {
        let body = request_body(model, history);
        let mut text = String::new();
        self.exchange(&self.url("/chat/completions"), Some(&body), cancel, &mut |event| {
            let piece = match event {
                Event::Data(data) => stream_delta(&data)?,
                Event::Json(bytes) => Some(json_reply(&bytes)?),
            };
            if let Some(piece) = piece {
                text.push_str(&piece);
                on_delta(&piece);
            }
            Ok(false)
        })?;
        if text.is_empty() {
            return Err(self.protocol("sent no text"));
        }
        Ok(text)
    }

    fn protocol(&self, what: &str) -> AssistantError {
        AssistantError::Protocol(format!("{} {what}", self.endpoint.label))
    }

    fn parse_models(&self, value: &Value) -> Result<Vec<String>, AssistantError> {
        let ids: Vec<String> = value
            .get("data")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("id").and_then(Value::as_str))
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            return Err(self.protocol("models list is empty"));
        }
        Ok(ids)
    }

    /// Runs one curl request and feeds the caller. A 200 body is an SSE stream unless it is a
    /// JSON object. Any other status becomes an error whose text is only the JSON `error.message`.
    /// The sink returns `Ok(true)` to stop early. Cancel kills curl and returns `Cancelled`.
    fn exchange(
        &self,
        url: &str,
        body: Option<&str>,
        cancel: &CancelToken,
        on_event: &mut dyn FnMut(Event) -> Result<bool, AssistantError>,
    ) -> Result<(), AssistantError> {
        if cancel.is_cancelled() {
            return Err(AssistantError::Cancelled);
        }
        let max_time = self.endpoint.max_time.map(|secs| secs.to_string());
        let mut args = vec!["--silent", "--show-error", "--include", "--no-buffer"];
        args.extend(["--connect-timeout", "15", "--speed-time", "60", "--speed-limit", "1"]);
        if let Some(secs) = max_time.as_deref() {
            args.extend(["--max-time", secs]);
        }
        args.extend(["--proto", self.proto, "--proto-redir", self.proto, "-K", "-"]);
        let mut child = Command::new("curl")
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(spawn_error)?;
        let (rx, reader) = match read_stdout(&mut child) {
            Ok(pair) => pair,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(curl_config(url, &self.endpoint.bearer, body).as_bytes());
        }

        let mut pending = Vec::new();
        let mut head: Option<Head> = None;
        let mut json_mode: Option<bool> = None;
        let mut json_body = Vec::new();
        let mut error_body = Vec::new();
        let mut sse = SseParser::default();
        let mut frames = Vec::new();
        let mut cancelled = false;
        let mut halted = false;
        loop {
            if cancel.is_cancelled() {
                cancelled = true;
                break;
            }
            let chunk = match rx.recv_timeout(POLL) {
                Ok(chunk) => chunk,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let part = if head.is_some() {
                chunk
            } else {
                pending.extend_from_slice(&chunk);
                match split_head(&mut pending) {
                    Some(parsed) => {
                        head = Some(parsed);
                        std::mem::take(&mut pending)
                    }
                    None => continue,
                }
            };
            let Some(current) = &head else { continue };
            if current.status != 200 {
                if error_body.len() < MAX_ERROR_BODY {
                    error_body.extend_from_slice(&part);
                }
                continue;
            }
            if json_mode.is_none() {
                if let Some(&first) = part.iter().find(|b| !b.is_ascii_whitespace()) {
                    json_mode = Some(current.json || first == b'{');
                }
            }
            if json_mode == Some(true) {
                json_body.extend_from_slice(&part);
                continue;
            }
            sse.feed(&part, &mut frames);
            if drain(&mut frames, on_event)? {
                halted = true;
                break;
            }
        }

        if cancelled || halted {
            let _ = child.kill();
        }
        let _ = reader.join();
        let exit = child.wait().ok().and_then(|status| status.code());
        if cancelled {
            return Err(AssistantError::Cancelled);
        }
        if halted {
            return Ok(());
        }
        let Some(current) = head else {
            return Err(match exit {
                Some(0) | None => self.protocol("sent no reply"),
                code => curl_error(self.endpoint.label, code),
            });
        };
        if matches!(exit, Some(code) if code != 0) {
            return Err(curl_error(self.endpoint.label, exit));
        }
        if current.status != 200 {
            return Err(http_error(current.status, &error_body));
        }
        if json_mode == Some(true) {
            on_event(Event::Json(json_body))?;
            return Ok(());
        }
        sse.finish(&mut frames);
        drain(&mut frames, on_event)?;
        Ok(())
    }
}

/// `=https` for https URLs. `=http` only for loopback http URLs, which tests and the local proxy use.
fn scheme_proto(url: &str) -> Result<&'static str, AssistantError> {
    if url.starts_with("https://") {
        return Ok("=https");
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        if matches!(host, "127.0.0.1" | "localhost") {
            return Ok("=http");
        }
    }
    Err(AssistantError::Config("The endpoint must use https, or http on 127.0.0.1.".into()))
}

/// Starts a reader thread for curl's stdout. The thread ends when stdout closes.
fn read_stdout(child: &mut Child) -> Result<(mpsc::Receiver<Vec<u8>>, thread::JoinHandle<()>), AssistantError> {
    let mut stdout = child.stdout.take().ok_or_else(|| AssistantError::Io("curl has no stdout".into()))?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let reader = thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    Ok((rx, reader))
}

/// Sends every complete SSE payload to the sink. Returns `true` when the sink or `[DONE]` stops.
fn drain(
    frames: &mut Vec<String>,
    on_event: &mut dyn FnMut(Event) -> Result<bool, AssistantError>,
) -> Result<bool, AssistantError> {
    for data in frames.drain(..) {
        if data.trim() == "[DONE]" {
            return Ok(true);
        }
        if on_event(Event::Data(data))? {
            return Ok(true);
        }
    }
    Ok(false)
}

struct Head {
    status: u16,
    json: bool,
}

/// Removes the final response head from `buf` (skipping `100 Continue`) and leaves the body in `buf`.
fn split_head(buf: &mut Vec<u8>) -> Option<Head> {
    loop {
        let end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
        let block: Vec<u8> = buf.drain(..end + 4).collect();
        let text = String::from_utf8_lossy(&block).to_ascii_lowercase();
        let status: u16 = text.lines().next()?.split_whitespace().nth(1)?.parse().ok()?;
        if (100..200).contains(&status) {
            continue;
        }
        let json = text
            .lines()
            .any(|line| line.starts_with("content-type:") && line.contains("application/json"));
        return Some(Head { status, json });
    }
}

/// The curl config block sent on stdin. Values are quoted and escaped, so the token and body stay intact.
fn curl_config(url: &str, bearer: &str, body: Option<&str>) -> String {
    let mut out = format!(
        "url = \"{}\"\nheader = \"Accept: text/event-stream, application/json\"\nheader = \"Authorization: Bearer {}\"\n",
        quote(url),
        quote(bearer)
    );
    if let Some(body) = body {
        out.push_str(&format!(
            "header = \"Content-Type: application/json\"\ndata-raw = \"{}\"\n",
            quote(body)
        ));
    }
    out
}

fn quote(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

fn request_body(model: &str, history: &[Turn]) -> String {
    let recent = &history[history.len().saturating_sub(HISTORY_TURNS)..];
    let mut messages = vec![json!({"role": "system", "content": SYSTEM_PROMPT})];
    messages.extend(recent.iter().map(|t| json!({"role": t.role, "content": t.content})));
    json!({"model": model, "messages": messages, "stream": true}).to_string()
}

fn stream_delta(data: &str) -> Result<Option<String>, AssistantError> {
    let value: Value =
        serde_json::from_str(data).map_err(|_| AssistantError::Protocol("stream chunk is not JSON".into()))?;
    if let Some(message) = error_message(&value) {
        return Err(AssistantError::RunFailed(message));
    }
    Ok(value
        .pointer("/choices/0/delta/content")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned))
}

fn json_reply(bytes: &[u8]) -> Result<String, AssistantError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| AssistantError::Protocol("reply is not JSON".into()))?;
    if let Some(message) = error_message(&value) {
        return Err(AssistantError::RunFailed(message));
    }
    value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| AssistantError::Protocol("reply has no text".into()))
}

/// A short, one-line message from a JSON error object, if the body has one.
fn error_message(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    let raw = error.get("message").and_then(Value::as_str).or_else(|| error.as_str())?;
    let line = raw.lines().next().unwrap_or("").trim();
    Some(line.chars().take(MAX_MESSAGE_CHARS).collect())
}

fn http_error(status: u16, body: &[u8]) -> AssistantError {
    let message = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| error_message(&value))
        .unwrap_or_default();
    AssistantError::Http { status, message }
}

fn spawn_error(error: std::io::Error) -> AssistantError {
    match error.kind() {
        std::io::ErrorKind::NotFound => {
            AssistantError::NotConfigured("curl is required to reach the assistant. Install curl and retry.".into())
        }
        _ => AssistantError::Io(format!("curl: {error}")),
    }
}

fn curl_error(label: &str, exit: Option<i32>) -> AssistantError {
    match exit {
        Some(28) => AssistantError::Timeout,
        Some(code) => AssistantError::Unreachable(format!("{label}: curl exit {code}")),
        None => AssistantError::Unreachable(format!("{label}: curl stopped")),
    }
}

/// The message shown to the user. Never raw HTML or JSON bodies, and never the key.
pub fn human_error(error: &AssistantError, label: &str) -> String {
    match error {
        AssistantError::Http { status: 401 | 403, .. } => {
            "API key rejected. Check the key in Settings > Assistant.".into()
        }
        AssistantError::Http { status: 429, .. } => format!("{label} is busy (rate limited). Try again shortly."),
        AssistantError::Http { status: 500..=599, .. } => format!("{label} had a server error. Try again."),
        AssistantError::Http { message, .. } if !message.is_empty() => message.clone(),
        AssistantError::Http { status, .. } => format!("{label} rejected the request (HTTP {status})."),
        AssistantError::Unreachable(_) => format!("Can't reach {label}. Check your connection and try again."),
        AssistantError::Timeout => format!("{label} stopped responding. Try again."),
        AssistantError::Protocol(_) => format!("{label} sent a reply this app cannot read."),
        AssistantError::RunFailed(message) => format!("{label} could not finish: {message}"),
        AssistantError::Config(message) | AssistantError::NotConfigured(message) => message.clone(),
        AssistantError::Cancelled => "Stopped.".into(),
        _ => format!("Something went wrong with {label}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{reply, serve, sse};

    fn client(base: &str) -> OpenAiClient {
        OpenAiClient::new(Endpoint {
            label: "ARK AI",
            base_url: format!("{base}/v1"),
            bearer: "ark_test".into(),
            max_time: None,
        })
        .unwrap()
    }

    #[test]
    fn https_or_loopback_http_only() {
        assert!(scheme_proto("https://ai.arkflame.com/v1").is_ok());
        assert!(scheme_proto("http://127.0.0.1:9/v1").is_ok());
        assert!(scheme_proto("http://localhost:9/v1").is_ok());
        assert!(scheme_proto("http://ai.arkflame.com/v1").is_err());
        assert!(scheme_proto("http://127.0.0.1.evil.test/v1").is_err());
    }

    #[test]
    fn curl_config_keeps_token_and_body_inside_quotes() {
        let config = curl_config("https://x/v1/chat", "ark_a\"b", Some("{\"q\":\"a\\\"b\"}\n"));
        assert!(config.contains("header = \"Authorization: Bearer ark_a\\\"b\"\n"));
        assert!(config.contains("data-raw = \"{\\\"q\\\":\\\"a\\\\\\\"b\\\"}\\n\"\n"));
        assert_eq!(config.lines().count(), 5);
        assert!(!curl_config("https://x", "ark_k", None).contains("data-raw"));
    }

    #[test]
    fn lists_models_with_bearer_header() {
        let (url, server) = serve(vec![reply(
            "200 OK",
            "application/json",
            r#"{"object":"list","data":[{"id":"lite"},{"id":"flash"},{"id":"pro"}]}"#,
        )]);
        let ids = client(&url).list_models().unwrap();
        assert_eq!(ids, vec!["lite", "flash", "pro"]);
        let seen = server.join().unwrap();
        assert!(seen[0].starts_with("GET /v1/models "));
        assert!(seen[0].to_lowercase().contains("authorization: bearer ark_test"));
    }

    #[test]
    fn html_body_is_never_read_as_json() {
        let (url, server) = serve(vec![reply("200 OK", "text/html", "<!DOCTYPE HTML><html></html>")]);
        let error = client(&url).get_json(&format!("{url}/v1/models")).unwrap_err();
        assert!(matches!(error, AssistantError::Protocol(_)), "{error:?}");
        server.join().unwrap();
    }

    #[test]
    fn unauthorized_is_a_clean_message() {
        let (url, server) = serve(vec![reply("401 Unauthorized", "application/json", r#"{"error":{"message":"bad key"}}"#)]);
        let error = client(&url).list_models().unwrap_err();
        assert!(matches!(error, AssistantError::Http { status: 401, .. }), "{error:?}");
        assert_eq!(human_error(&error, "ARK AI"), "API key rejected. Check the key in Settings > Assistant.");
        server.join().unwrap();
    }

    #[test]
    fn html_501_never_reaches_the_user() {
        let html = "<!DOCTYPE HTML><html><body>Not Implemented</body></html>";
        let (url, server) = serve(vec![reply("501 Not Implemented", "text/html", html)]);
        let error = client(&url).list_models().unwrap_err();
        let text = human_error(&error, "ARK AI");
        assert_eq!(text, "ARK AI had a server error. Try again.");
        assert!(!text.contains('<'));
        server.join().unwrap();
    }

    #[test]
    fn rate_limit_is_explained() {
        let (url, server) = serve(vec![reply("429 Too Many Requests", "application/json", "{}")]);
        let error = client(&url).list_models().unwrap_err();
        assert!(human_error(&error, "ARK AI").contains("rate limited"));
        server.join().unwrap();
    }

    #[test]
    fn offline_is_reported_as_unreachable() {
        let error = client("http://127.0.0.1:1").list_models().unwrap_err();
        assert!(matches!(error, AssistantError::Unreachable(_)), "{error:?}");
        assert_eq!(
            human_error(&error, "ARK AI"),
            "Can't reach ARK AI. Check your connection and try again."
        );
    }

    #[test]
    fn streams_deltas_with_prompt_history_model_and_bearer() {
        let (url, server) = serve(vec![sse(&[
            r#"{"choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"{"choices":[{"delta":{"content":"lo"}}]}"#,
            "[DONE]",
        ])]);
        let history = vec![
            Turn { role: "user", content: "hi \"there\" \\ ok".into() },
            Turn { role: "assistant", content: "hey".into() },
        ];
        let mut deltas = Vec::new();
        let text = client(&url)
            .chat("flash", &history, &CancelToken::new(), &mut |d| deltas.push(d.to_owned()))
            .unwrap();
        assert_eq!(text, "Hello");
        assert_eq!(deltas, vec!["Hel", "lo"]);
        let seen = server.join().unwrap();
        assert!(seen[0].starts_with("POST /v1/chat/completions "));
        assert!(seen[0].contains(r#""stream":true"#));
        assert!(seen[0].contains(r#""model":"flash""#));
        assert!(seen[0].contains(SYSTEM_PROMPT));
        assert!(seen[0].contains(r#"hi \"there\" \\ ok"#));
        assert!(seen[0].to_lowercase().contains("authorization: bearer ark_test"));
    }

    #[test]
    fn history_is_bounded_to_recent_turns() {
        let history: Vec<Turn> = (0..20)
            .map(|i| Turn { role: "user", content: format!("turn-{i}") })
            .collect();
        let body = request_body("lite", &history);
        assert!(body.contains("turn-19\""));
        assert!(body.contains("turn-8\""));
        assert!(!body.contains("turn-7\""));
    }

    #[test]
    fn non_streaming_json_reply_is_one_delta() {
        let (url, server) = serve(vec![reply(
            "200 OK",
            "application/json",
            r#"{"choices":[{"message":{"role":"assistant","content":"All set."}}]}"#,
        )]);
        let mut deltas = Vec::new();
        let text = client(&url)
            .chat("lite", &[], &CancelToken::new(), &mut |d| deltas.push(d.to_owned()))
            .unwrap();
        assert_eq!(text, "All set.");
        assert_eq!(deltas, vec!["All set."]);
        server.join().unwrap();
    }

    #[test]
    fn stream_without_done_keeps_all_text() {
        let (url, server) = serve(vec![sse(&[r#"{"choices":[{"delta":{"content":"ok"}}]}"#])]);
        let text = client(&url).chat("flash", &[], &CancelToken::new(), &mut |_| {}).unwrap();
        assert_eq!(text, "ok");
        server.join().unwrap();
    }

    #[test]
    fn stream_error_event_is_a_short_run_failure() {
        let (url, server) = serve(vec![sse(&[r#"{"error":{"message":"quota exceeded\nlong detail"}}"#])]);
        let error = client(&url).chat("flash", &[], &CancelToken::new(), &mut |_| {}).unwrap_err();
        assert_eq!(error, AssistantError::RunFailed("quota exceeded".into()));
        assert_eq!(human_error(&error, "ARK AI"), "ARK AI could not finish: quota exceeded");
        server.join().unwrap();
    }

    #[test]
    fn cancel_before_send_opens_no_connection() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let error = client("http://127.0.0.1:1").chat("flash", &[], &cancel, &mut |_| {}).unwrap_err();
        assert_eq!(error, AssistantError::Cancelled);
    }

    #[test]
    fn cancel_mid_stream_stops_after_the_current_piece() {
        let (url, server) = serve(vec![sse(&[r#"{"choices":[{"delta":{"content":"one"}}]}"#])]);
        let cancel = CancelToken::new();
        let mut deltas = 0;
        let error = client(&url)
            .chat("flash", &[], &cancel, &mut |_| {
                deltas += 1;
                cancel.cancel();
            })
            .unwrap_err();
        assert_eq!(error, AssistantError::Cancelled);
        assert_eq!(deltas, 1);
        server.join().unwrap();
    }
}

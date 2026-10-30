//! Decoding for the Hermes `/v1/runs` event stream. Each SSE `data:` payload is a
//! JSON object whose `event` field names the event. Pure functions only.

use serde_json::Value;

use crate::AssistantError;

/// One decoded run event. Unknown events map to `Other` and are ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunEvent {
    Delta(String),
    /// `output` is the final answer when the server sends one.
    Completed { output: Option<String> },
    Failed(String),
    Cancelled,
    Interrupted(Option<String>),
    /// `preview` is Hermes' one-line summary of the call (command, path, query), when sent.
    ToolStarted { tool: String, preview: Option<String> },
    ToolCompleted { tool: String, error: bool },
    /// A tool call needs a human decision. Carries the choices the server accepts, the request id
    /// to echo back, and the command (already redacted by the server).
    Approval { choices: Vec<String>, request_id: Option<String>, command: Option<String> },
    Other,
}

/// Terminal status strings reported by `GET /v1/runs/{id}`.
pub fn is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled" | "interrupted")
}

/// Decodes one SSE `data:` payload.
pub fn decode_event(data: &str) -> Result<RunEvent, AssistantError> {
    let value: Value = serde_json::from_str(data.trim())
        .map_err(|e| AssistantError::Protocol(format!("bad run event JSON: {e}")))?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let event = value.get("event").and_then(Value::as_str).unwrap_or_default();
    Ok(match event {
        "message.delta" => match text("delta") {
            Some(delta) if !delta.is_empty() => RunEvent::Delta(delta),
            _ => RunEvent::Other,
        },
        "run.completed" => RunEvent::Completed { output: text("output").filter(|o| !o.is_empty()) },
        "run.failed" => RunEvent::Failed(text("error").unwrap_or_else(|| "the run failed".into())),
        "run.cancelled" => RunEvent::Cancelled,
        "run.interrupted" => RunEvent::Interrupted(text("error")),
        "tool.started" => RunEvent::ToolStarted {
            tool: text("tool").unwrap_or_else(|| "tool".into()),
            preview: text("preview").filter(|p| !p.trim().is_empty()),
        },
        "tool.completed" => RunEvent::ToolCompleted {
            tool: text("tool").unwrap_or_else(|| "tool".into()),
            error: value.get("error").and_then(Value::as_bool).unwrap_or(false),
        },
        "approval.request" => RunEvent::Approval {
            choices: value
                .get("choices")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default(),
            request_id: text("request_id"),
            command: text("command"),
        },
        _ => RunEvent::Other,
    })
}

/// Reads `status`, `output` and `session_id` from a `GET /v1/runs/{id}` body.
pub fn decode_status(body: &[u8]) -> Result<RunStatus, AssistantError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|e| AssistantError::Protocol(format!("bad run status JSON: {e}")))?;
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Ok(RunStatus {
        status: text("status").unwrap_or_default(),
        output: text("output").filter(|o| !o.is_empty()),
        error: text("error"),
        session_id: text("session_id"),
    })
}

/// Polled run state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunStatus {
    pub status: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub session_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_documented_event_shapes() {
        let cases = [
            (r#"{"event":"message.delta","run_id":"r","delta":"Hi"}"#, RunEvent::Delta("Hi".into())),
            (r#"{"event":"message.delta","delta":""}"#, RunEvent::Other),
            (r#"{"event":"run.completed","output":"Done."}"#, RunEvent::Completed { output: Some("Done.".into()) }),
            (r#"{"event":"run.completed","usage":{}}"#, RunEvent::Completed { output: None }),
            (r#"{"event":"run.failed","error":"no login"}"#, RunEvent::Failed("no login".into())),
            (r#"{"event":"run.cancelled"}"#, RunEvent::Cancelled),
            (r#"{"event":"run.interrupted","error":"restart"}"#, RunEvent::Interrupted(Some("restart".into()))),
            (r#"{"event":"tool.started","tool":"terminal","preview":"ls"}"#, RunEvent::ToolStarted { tool: "terminal".into(), preview: Some("ls".into()) }),
            (
                r#"{"event":"tool.completed","tool":"terminal","error":true}"#,
                RunEvent::ToolCompleted { tool: "terminal".into(), error: true },
            ),
            (
                r#"{"event":"approval.request","choices":["once","deny"],"request_id":"p1","command":"rm -rf x"}"#,
                RunEvent::Approval {
                    choices: vec!["once".into(), "deny".into()],
                    request_id: Some("p1".into()),
                    command: Some("rm -rf x".into()),
                },
            ),
            (r#"{"event":"run.stopping"}"#, RunEvent::Other),
        ];
        for (raw, want) in cases {
            assert_eq!(decode_event(raw).unwrap(), want, "{raw}");
        }
    }

    #[test]
    fn rejects_bad_json() {
        assert!(matches!(decode_event("not json"), Err(AssistantError::Protocol(_))));
    }

    #[test]
    fn terminal_statuses() {
        for s in ["completed", "failed", "cancelled", "interrupted"] {
            assert!(is_terminal(s));
        }
        for s in ["running", "started", "queued", "waiting_for_approval", "stopping", "unknown"] {
            assert!(!is_terminal(s));
        }
    }

    #[test]
    fn decodes_status_body() {
        let status = decode_status(br#"{"status":"completed","output":"Hi there.","session_id":"s1"}"#).unwrap();
        assert_eq!(status.status, "completed");
        assert_eq!(status.output.as_deref(), Some("Hi there."));
        assert_eq!(status.session_id.as_deref(), Some("s1"));
        assert_eq!(decode_status(br#"{"status":"running","output":""}"#).unwrap().output, None);
    }
}

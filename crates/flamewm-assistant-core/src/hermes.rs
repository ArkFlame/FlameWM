//! Hermes proxy provider: an OpenAI-compatible server on loopback. It is detected by a short
//! probe and never installed, started or configured from here.

use std::time::Duration;

use serde_json::Value;

use crate::CancelToken;
use crate::openai::{Endpoint, OpenAiClient};

pub const PROXY_BASE_URL: &str = "http://127.0.0.1:8645/v1";
/// Test hook: a loopback base URL that replaces `PROXY_BASE_URL`. Other hosts are ignored.
pub const BASE_URL_ENV: &str = "FLAMEWM_ASSISTANT_PROXY_URL";
pub const LABEL: &str = "Hermes proxy";
/// The proxy accepts any bearer token. This one only identifies the app.
const BEARER: &str = "flamewm-assistant";
/// Limit for each probe request. Loopback answers well inside this; a silent port fails fast.
const PROBE_LIMIT: Duration = Duration::from_millis(300);

/// The proxy base URL in use: the loopback test hook when set, otherwise `PROXY_BASE_URL`.
pub fn base_url() -> String {
    std::env::var(BASE_URL_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| {
            value.starts_with("http://127.0.0.1") || value.starts_with("http://localhost")
        })
        .unwrap_or_else(|| PROXY_BASE_URL.to_owned())
}

/// Endpoint for chat and model listing. No time limit, because replies stream.
pub fn endpoint(base_url: &str) -> Endpoint {
    Endpoint {
        label: LABEL,
        base_url: base_url.to_owned(),
        bearer: BEARER.to_owned(),
        max_time: None,
    }
}

/// Health body of a Hermes model proxy. The agent server reports `platform` and is not a proxy.
/// `authenticated: false` means the proxy has no upstream login yet.
pub fn is_proxy_health(health: &Value) -> bool {
    health.get("status").and_then(Value::as_str) == Some("ok")
        && health.get("platform").is_none()
        && health
            .get("upstream")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
        && health.get("authenticated").and_then(Value::as_bool) != Some(false)
}

/// Checks whether a Hermes model proxy answers at `base_url` and returns its model ids.
/// `None` means nothing usable is there: refused, slow, HTML, the agent server, or not logged in.
pub fn probe(base_url: &str, cancel: &CancelToken) -> Option<Vec<String>> {
    let mut ep = endpoint(base_url);
    ep.max_time = Some(PROBE_LIMIT);
    let client = OpenAiClient::new(ep).ok()?;
    let root = base_url.trim_end_matches('/').trim_end_matches("/v1");
    let health = client.get_json(&format!("{root}/health"), cancel).ok()?;
    if !is_proxy_health(&health) {
        return None;
    }
    client
        .list_models(cancel)
        .ok()
        .filter(|ids| !ids.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CancelToken;
    use crate::test_support::{reply, serve, serve_prompt, sse};

    const GOOD_HEALTH: &str = r#"{"status":"ok","upstream":"Nous Portal","authenticated":true}"#;

    fn base(url: &str) -> String {
        format!("{url}/v1")
    }

    #[test]
    fn health_rules_accept_only_a_model_proxy() {
        let ok: Value = serde_json::from_str(GOOD_HEALTH).unwrap();
        assert!(is_proxy_health(&ok));
        let agent: Value =
            serde_json::from_str(r#"{"status":"ok","platform":"hermes-agent","upstream":"x"}"#)
                .unwrap();
        assert!(!is_proxy_health(&agent));
        let logged_out: Value = serde_json::from_str(
            r#"{"status":"ok","upstream":"Nous Portal","authenticated":false}"#,
        )
        .unwrap();
        assert!(!is_proxy_health(&logged_out));
        let no_upstream: Value = serde_json::from_str(r#"{"status":"ok"}"#).unwrap();
        assert!(!is_proxy_health(&no_upstream));
        let down: Value = serde_json::from_str(r#"{"status":"down","upstream":"x"}"#).unwrap();
        assert!(!is_proxy_health(&down));
    }

    #[test]
    fn probe_finds_a_proxy_and_its_models() {
        let (url, server) = serve_prompt(vec![
            reply("200 OK", "application/json", GOOD_HEALTH),
            reply(
                "200 OK",
                "application/json",
                r#"{"data":[{"id":"a/b:free"},{"id":"c/d"}]}"#,
            ),
        ]);
        assert_eq!(
            probe(&base(&url), &CancelToken::new()),
            Some(vec!["a/b:free".into(), "c/d".into()])
        );
        let seen = server.join().unwrap();
        assert!(seen[0].starts_with("GET /health "));
        assert!(seen[1].starts_with("GET /v1/models "));
    }

    #[test]
    fn probe_rejects_html_and_agent_servers() {
        let (url, server) = serve(vec![reply(
            "200 OK",
            "text/html",
            "<!DOCTYPE HTML><html></html>",
        )]);
        assert_eq!(probe(&base(&url), &CancelToken::new()), None);
        server.join().unwrap();

        let (url, server) = serve(vec![reply(
            "200 OK",
            "application/json",
            r#"{"status":"ok","platform":"hermes-agent","upstream":"x"}"#,
        )]);
        assert_eq!(probe(&base(&url), &CancelToken::new()), None);
        server.join().unwrap();
    }

    #[test]
    fn probe_rejects_non_json_model_lists() {
        let (url, server) = serve(vec![
            reply("200 OK", "application/json", GOOD_HEALTH),
            reply("200 OK", "text/html", "<html>nope</html>"),
        ]);
        assert_eq!(probe(&base(&url), &CancelToken::new()), None);
        server.join().unwrap();
    }

    #[test]
    fn probe_of_a_closed_port_is_none() {
        assert_eq!(probe("http://127.0.0.1:1/v1", &CancelToken::new()), None);
    }

    #[test]
    fn chat_streams_through_the_proxy_with_a_dummy_bearer() {
        let (url, server) = serve(vec![sse(&[
            r#"{"choices":[{"delta":{"content":"hi"}}]}"#,
            "[DONE]",
        ])]);
        let client = OpenAiClient::new(endpoint(&base(&url))).unwrap();
        let text = client
            .chat("c/d", &[], &CancelToken::new(), &mut |_| {})
            .unwrap();
        assert_eq!(text, "hi");
        let seen = server.join().unwrap();
        assert!(seen[0].starts_with("POST /v1/chat/completions "));
        assert!(
            seen[0]
                .to_lowercase()
                .contains("authorization: bearer flamewm-assistant")
        );
    }
}

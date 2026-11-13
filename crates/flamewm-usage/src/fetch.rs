//! Blocking usage fetch through the system `curl` binary (the same pattern as the weather
//! and assistant downloaders, so no HTTP crate is added). Hosts call it on a worker thread.
//!
//! Secrets never reach argv or logs: headers are written to curl's stdin via `--config -`.

use std::io::Write;
use std::process::{Command, Stdio};

use crate::{
    ProviderSnapshot, ProviderState, UsageProvider, parse_claude_usage, parse_codex_usage,
    parse_commandcode_usage,
};

/// Production endpoints. Tests pass a loopback base URL instead.
pub const CLAUDE_BASE: &str = "https://api.anthropic.com";
pub const CODEX_BASE: &str = "https://chatgpt.com/backend-api";
pub const COMMANDCODE_BASE: &str = "https://api.commandcode.ai";

/// Hard upper bound for one request, so a stalled provider never wedges its worker.
pub const TIMEOUT_SECS: u32 = 10;

/// Largest usage payload accepted (bytes).
const MAX_BODY_BYTES: u32 = 1_048_576;

/// Fetches one provider's usage. `token` is `None` when the provider is not configured.
/// `account_id` is the Codex account sent as `ChatGPT-Account-Id` when present.
/// Errors are fixed strings: neither the token nor the raw body is ever stored or returned.
#[must_use]
pub fn fetch_snapshot(
    provider: UsageProvider,
    token: Option<&str>,
    account_id: Option<&str>,
    base: &str,
    now: i64,
) -> ProviderSnapshot {
    let state = match (provider, token) {
        (UsageProvider::Codex, Some(token)) => {
            let mut headers = vec![("Authorization", format!("Bearer {token}"))];
            if let Some(account) = account_id {
                headers.push(("ChatGPT-Account-Id", account.to_owned()));
            }
            request(&format!("{base}/wham/usage"), &headers)
                .and_then(|body| {
                    parse_codex_usage(&body).map_err(|_| "unexpected Codex usage response")
                })
                .map_or_else(ProviderState::Failed, ProviderState::Ready)
        }
        (UsageProvider::Claude, Some(token)) => {
            let headers = [
                ("Authorization", format!("Bearer {token}")),
                ("anthropic-beta", "oauth-2025-04-20".to_owned()),
            ];
            request(&format!("{base}/api/oauth/usage"), &headers)
                .and_then(|body| {
                    parse_claude_usage(&body).map_err(|_| "unexpected Claude usage response")
                })
                .map_or_else(ProviderState::Failed, ProviderState::Ready)
        }
        (UsageProvider::CommandCode, Some(token)) => {
            let headers = [("Authorization", format!("Bearer {token}"))];
            request(&format!("{base}/alpha/billing/credits"), &headers)
                .and_then(|body| {
                    parse_commandcode_usage(&body).map_err(|_| "unexpected Command Code response")
                })
                .map_or_else(ProviderState::Failed, ProviderState::Ready)
        }
        _ => ProviderState::NotConfigured,
    };
    ProviderSnapshot {
        provider,
        state,
        updated_at: Some(now),
    }
}

/// GET `url` with `headers` through curl. The protocol follows the URL scheme; production
/// endpoints are `https`, and `http` is only reachable through an explicit test base.
fn request(url: &str, headers: &[(&str, String)]) -> Result<String, &'static str> {
    let proto = if url.starts_with("http://") {
        "http"
    } else {
        "https"
    };
    let mut config = format!("url = \"{url}\"\n");
    for (name, value) in headers {
        if value.contains(['"', '\n', '\r', '\\']) {
            return Err("credential contains unsupported characters");
        }
        config.push_str(&format!("header = \"{name}: {value}\"\n"));
    }
    let mut child = Command::new("curl")
        .args([
            "--silent",
            "--fail",
            "--max-time",
            &TIMEOUT_SECS.to_string(),
            "--max-filesize",
            &MAX_BODY_BYTES.to_string(),
        ])
        .args(["--proto", proto, "--proto-redir", proto, "--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "curl is not available")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(config.as_bytes())
            .map_err(|_| "request could not be sent")?;
    }
    let output = child.wait_with_output().map_err(|_| "request failed")?;
    if !output.status.success() {
        return Err("request failed or timed out");
    }
    String::from_utf8(output.stdout).map_err(|_| "response was not UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// Serves one canned response on loopback and reports the request head to the test.
    /// `hold_secs > 0` accepts the request and stalls without answering.
    fn serve(
        status: &'static str,
        body: &'static str,
        hold_secs: u64,
    ) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if let Some(Ok(mut stream)) = listener.incoming().next() {
                let mut head = vec![0u8; 4096];
                let n = stream.read(&mut head).unwrap_or(0);
                let _ = tx.send(String::from_utf8_lossy(&head[..n]).into_owned());
                if hold_secs > 0 {
                    std::thread::sleep(std::time::Duration::from_secs(hold_secs));
                    return;
                }
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (base, rx)
    }

    const CLAUDE_BODY: &str =
        r#"{"five_hour":{"utilization":12,"resets_at":"2026-11-12T10:00:00Z"}}"#;

    #[test]
    fn claude_snapshot_sends_bearer_and_beta_header_to_base() {
        let (base, seen) = serve("200 OK", CLAUDE_BODY, 0);
        let snap = fetch_snapshot(
            UsageProvider::Claude,
            Some("fixture-token"),
            None,
            &base,
            42,
        );
        let head = seen.recv().unwrap().to_ascii_lowercase();
        assert!(head.starts_with("get /api/oauth/usage"), "{head}");
        assert!(
            head.contains("authorization: bearer fixture-token"),
            "{head}"
        );
        assert!(head.contains("anthropic-beta: oauth-2025-04-20"), "{head}");
        assert_eq!(snap.updated_at, Some(42));
        let ProviderState::Ready(windows) = snap.state else {
            panic!("expected Ready")
        };
        assert_eq!(windows[0].used_percent, 12.0);
    }

    #[test]
    fn codex_snapshot_sends_bearer_and_account_header_to_base() {
        const CODEX_BODY: &str = r#"{"rate_limit":{"primary_window":{"used_percent":37.5,"limit_window_seconds":18000,"reset_at":1000}}}"#;
        let (base, seen) = serve("200 OK", CODEX_BODY, 0);
        let snap = fetch_snapshot(
            UsageProvider::Codex,
            Some("codex-fixture"),
            Some("acct-1"),
            &base,
            7,
        );
        let head = seen.recv().unwrap().to_ascii_lowercase();
        assert!(head.starts_with("get /wham/usage"), "{head}");
        assert!(
            head.contains("authorization: bearer codex-fixture"),
            "{head}"
        );
        assert!(head.contains("chatgpt-account-id: acct-1"), "{head}");
        let ProviderState::Ready(windows) = snap.state else {
            panic!("expected Ready")
        };
        assert_eq!(windows[0].used_percent, 37.5);
    }

    #[test]
    fn missing_token_is_not_configured_without_network() {
        let snap = fetch_snapshot(
            UsageProvider::CommandCode,
            None,
            None,
            "http://127.0.0.1:9",
            1,
        );
        assert_eq!(snap.state, ProviderState::NotConfigured);
    }

    #[test]
    fn http_error_becomes_failed_without_leaking_token() {
        let (base, _seen) = serve("401 Unauthorized", r#"{"error":"echo"}"#, 0);
        let snap = fetch_snapshot(UsageProvider::Claude, Some("fixture-token"), None, &base, 1);
        let ProviderState::Failed(message) = snap.state else {
            panic!("expected Failed")
        };
        assert!(!message.contains("fixture-token"));
    }

    #[test]
    fn stalled_server_times_out() {
        let (base, _seen) = serve("200 OK", CLAUDE_BODY, 30);
        let started = std::time::Instant::now();
        assert!(request(&format!("{base}/x"), &[]).is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(u64::from(TIMEOUT_SECS) + 5));
    }

    #[test]
    fn header_values_with_quotes_are_refused() {
        let headers = [("Authorization", "Bearer a\"b".to_owned())];
        assert_eq!(
            request("http://127.0.0.1:9/x", &headers),
            Err("credential contains unsupported characters")
        );
    }
}

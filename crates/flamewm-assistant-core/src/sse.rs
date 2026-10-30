//! Server-sent events line framing and Chat Completions chunk decoding.

use serde_json::Value;

use crate::AssistantError;

/// Incremental SSE parser. Feed bytes; complete event payloads are appended to `out`.
#[derive(Debug, Default)]
pub struct SseParser {
    line: Vec<u8>,
    data: String,
    has_data: bool,
}

impl SseParser {
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<String>) {
        for &b in bytes {
            if b == b'\n' {
                let line = std::mem::take(&mut self.line);
                self.line_done(&String::from_utf8_lossy(&line), out);
            } else {
                self.line.push(b);
            }
        }
    }

    /// Flushes a trailing line and event that were not closed by a blank line.
    pub fn finish(&mut self, out: &mut Vec<String>) {
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            self.line_done(&String::from_utf8_lossy(&line), out);
        }
        self.dispatch(out);
    }

    fn line_done(&mut self, raw: &str, out: &mut Vec<String>) {
        let line = raw.trim_end_matches('\r');
        if line.is_empty() {
            self.dispatch(out);
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        if field == "data" {
            if self.has_data {
                self.data.push('\n');
            }
            self.data.push_str(value);
            self.has_data = true;
        }
    }

    fn dispatch(&mut self, out: &mut Vec<String>) {
        if self.has_data {
            out.push(std::mem::take(&mut self.data));
            self.has_data = false;
        }
    }
}

/// One decoded Chat Completions stream chunk.
#[derive(Debug, PartialEq, Eq)]
pub enum ChatChunk {
    Delta(String),
    Ignore,
    Done,
}

/// Decodes one `data:` payload of `/v1/chat/completions` with `stream: true`.
pub fn parse_chat_chunk(data: &str) -> Result<ChatChunk, AssistantError> {
    let data = data.trim();
    if data == "[DONE]" {
        return Ok(ChatChunk::Done);
    }
    let value: Value = serde_json::from_str(data)
        .map_err(|e| AssistantError::Protocol(format!("bad stream JSON: {e}")))?;
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map_or_else(|| error.to_string(), str::to_owned);
        return Err(AssistantError::Protocol(format!("stream error: {message}")));
    }
    Ok(match value.pointer("/choices/0/delta/content").and_then(Value::as_str) {
        Some(text) if !text.is_empty() => ChatChunk::Delta(text.to_owned()),
        _ => ChatChunk::Ignore,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_split_across_feeds_and_crlf() {
        let mut parser = SseParser::default();
        let mut out = Vec::new();
        parser.feed(b"data: {\"a\"", &mut out);
        assert!(out.is_empty());
        parser.feed(b":1}\r\n\r\n: comment\n", &mut out);
        parser.feed(b"event: x\ndata: one\ndata: two\n\n", &mut out);
        assert_eq!(out, vec!["{\"a\":1}".to_owned(), "one\ntwo".to_owned()]);
    }

    #[test]
    fn finish_flushes_unterminated_event() {
        let mut parser = SseParser::default();
        let mut out = Vec::new();
        parser.feed(b"data: tail", &mut out);
        parser.finish(&mut out);
        assert_eq!(out, vec!["tail".to_owned()]);
    }

    #[test]
    fn chunk_decoding() {
        let delta = r#"{"choices":[{"delta":{"content":"Hi"}}]}"#;
        assert_eq!(parse_chat_chunk(delta).unwrap(), ChatChunk::Delta("Hi".into()));
        assert_eq!(parse_chat_chunk(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#).unwrap(), ChatChunk::Ignore);
        assert_eq!(parse_chat_chunk("[DONE]").unwrap(), ChatChunk::Done);
        assert!(matches!(parse_chat_chunk(r#"{"error":{"message":"boom"}}"#), Err(AssistantError::Protocol(m)) if m.contains("boom")));
        assert!(matches!(parse_chat_chunk("{not json"), Err(AssistantError::Protocol(_))));
    }
}

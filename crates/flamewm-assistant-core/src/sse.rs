//! Server-sent events line framing.

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
}

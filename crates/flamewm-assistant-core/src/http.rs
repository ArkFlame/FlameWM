//! Minimal HTTP/1.1 client for loopback Hermes endpoints. Plain TCP only.

use std::fmt::Write as _;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::{AssistantError, CancelToken};

/// Read wake-up interval, so cancellation is observed while a stream is idle.
const WAKE: Duration = Duration::from_millis(200);
const MAX_LINE: usize = 64 * 1024;

/// A parsed `http://host:port[/base]` URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    host: String,
    port: u16,
    base_path: String,
}

impl Endpoint {
    pub fn parse(url: &str) -> Result<Self, AssistantError> {
        let url = url.trim();
        let rest = url.strip_prefix("http://").ok_or_else(|| {
            AssistantError::Config(format!("only http:// loopback URLs are supported, got {url:?}"))
        })?;
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], rest[i..].trim_end_matches('/')),
            None => (rest, ""),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => {
                let port = port
                    .parse::<u16>()
                    .map_err(|_| AssistantError::Config(format!("invalid port in {url:?}")))?;
                (host, port)
            }
            None => (authority, 80),
        };
        if host.is_empty() || host.contains(['[', ']']) {
            return Err(AssistantError::Config(format!("unsupported host in {url:?}")));
        }
        Ok(Self { host: host.to_owned(), port, base_path: path.to_owned() })
    }

    pub fn url(&self) -> String {
        format!("http://{}:{}{}", self.host, self.port, self.base_path)
    }

    fn connect(&self, timeout: Duration) -> Result<TcpStream, AssistantError> {
        let addr = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|e| AssistantError::Unreachable(format!("resolve {}: {e}", self.host)))?
            .next()
            .ok_or_else(|| AssistantError::Unreachable(format!("no address for {}", self.host)))?;
        let stream = TcpStream::connect_timeout(&addr, timeout)
            .map_err(|e| AssistantError::Unreachable(format!("{}: {e}", self.url())))?;
        stream.set_read_timeout(Some(WAKE))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_nodelay(true)?;
        Ok(stream)
    }
}

/// One request. Header names are static; values must not contain CR or LF.
pub struct Request<'a> {
    pub method: &'static str,
    pub path: &'a str,
    pub headers: Vec<(&'static str, String)>,
    pub body: Option<&'a str>,
}

#[derive(Clone, Copy, Debug)]
enum Framing {
    Length(u64),
    Chunked,
    UntilClose,
}

#[derive(Clone, Copy, Debug)]
enum Chunk {
    Size,
    Data(u64),
    DataEnd,
    Trailer,
}

/// Streaming response body. Yields decoded payload pieces until the end.
pub struct Body {
    stream: TcpStream,
    raw: Vec<u8>,
    framing: Framing,
    chunk: Chunk,
    done: bool,
    cancel: CancelToken,
    idle: Duration,
    last: Instant,
}

/// Response head plus its body.
pub struct Response {
    pub status: u16,
    headers: Vec<(String, String)>,
    pub body: Body,
}

impl Response {
    /// Looks up a header by lower-case name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// Sends `req` and reads the response head. The body is read lazily.
/// `timeout` bounds connect and write; `idle` bounds silence while reading.
pub fn send(
    ep: &Endpoint,
    req: Request<'_>,
    cancel: &CancelToken,
    timeout: Duration,
    idle: Duration,
) -> Result<Response, AssistantError> {
    for (name, value) in &req.headers {
        if value.contains(['\r', '\n']) {
            return Err(AssistantError::Config(format!("header {name} contains a line break")));
        }
    }
    if cancel.is_cancelled() {
        return Err(AssistantError::Cancelled);
    }
    let mut stream = ep.connect(timeout)?;
    let mut head = format!(
        "{} {}{} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nAccept: application/json, text/event-stream\r\n",
        req.method, ep.base_path, req.path, ep.host, ep.port
    );
    for (name, value) in &req.headers {
        let _ = write!(head, "{name}: {value}\r\n");
    }
    if let Some(body) = req.body {
        let _ = write!(head, "Content-Type: application/json\r\nContent-Length: {}\r\n", body.len());
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    if let Some(body) = req.body {
        stream.write_all(body.as_bytes())?;
    }

    let mut body = Body {
        stream,
        raw: Vec::new(),
        framing: Framing::UntilClose,
        chunk: Chunk::Size,
        done: false,
        cancel: cancel.clone(),
        idle,
        last: Instant::now(),
    };
    let status_line = body.take_line()?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| AssistantError::Protocol(format!("bad status line {status_line:?}")))?;
    let mut headers = Vec::new();
    loop {
        let line = body.take_line()?;
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    let chunked = headers
        .iter()
        .any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
    let declared = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<u64>().ok());
    body.framing = if status == 204 || status == 304 {
        Framing::Length(0)
    } else if chunked {
        Framing::Chunked
    } else if let Some(len) = declared {
        Framing::Length(len)
    } else {
        Framing::UntilClose
    };
    Ok(Response { status, headers, body })
}

impl Body {
    /// Returns the next decoded payload piece, or `None` at the end of the body.
    pub fn next_data(&mut self) -> Result<Option<Vec<u8>>, AssistantError> {
        loop {
            if self.done {
                return Ok(None);
            }
            match self.framing {
                Framing::UntilClose => {
                    if self.raw.is_empty() && self.fill()? == 0 {
                        self.done = true;
                        return Ok(None);
                    }
                    return Ok(Some(std::mem::take(&mut self.raw)));
                }
                Framing::Length(0) => self.done = true,
                Framing::Length(remaining) => {
                    if self.raw.is_empty() && self.fill()? == 0 {
                        return Err(AssistantError::Protocol("connection closed before end of body".into()));
                    }
                    let n = usize::try_from(remaining).unwrap_or(usize::MAX).min(self.raw.len());
                    let data: Vec<u8> = self.raw.drain(..n).collect();
                    self.framing = Framing::Length(remaining - n as u64);
                    return Ok(Some(data));
                }
                Framing::Chunked => match self.chunk {
                    Chunk::Size => {
                        let line = self.take_line()?;
                        let hex = line.split(';').next().unwrap_or("").trim();
                        let size = u64::from_str_radix(hex, 16)
                            .map_err(|_| AssistantError::Protocol(format!("bad chunk size {hex:?}")))?;
                        self.chunk = if size == 0 { Chunk::Trailer } else { Chunk::Data(size) };
                    }
                    Chunk::Data(remaining) => {
                        if self.raw.is_empty() && self.fill()? == 0 {
                            return Err(AssistantError::Protocol("connection closed inside chunk".into()));
                        }
                        let n = usize::try_from(remaining).unwrap_or(usize::MAX).min(self.raw.len());
                        let data: Vec<u8> = self.raw.drain(..n).collect();
                        let left = remaining - n as u64;
                        self.chunk = if left == 0 { Chunk::DataEnd } else { Chunk::Data(left) };
                        return Ok(Some(data));
                    }
                    Chunk::DataEnd => {
                        let _ = self.take_line()?;
                        self.chunk = Chunk::Size;
                    }
                    Chunk::Trailer => {
                        if self.take_line()?.is_empty() {
                            self.done = true;
                        }
                    }
                },
            }
        }
    }

    /// Reads the whole body, failing when it exceeds `max` bytes.
    pub fn read_limited(&mut self, max: usize) -> Result<Vec<u8>, AssistantError> {
        let mut out = Vec::new();
        while let Some(piece) = self.next_data()? {
            if out.len() + piece.len() > max {
                return Err(AssistantError::Protocol("response body too large".into()));
            }
            out.extend_from_slice(&piece);
        }
        Ok(out)
    }

    fn take_line(&mut self) -> Result<String, AssistantError> {
        loop {
            if let Some(pos) = self.raw.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.raw.drain(..=pos).collect();
                return Ok(String::from_utf8_lossy(&line).trim_end().to_owned());
            }
            if self.raw.len() > MAX_LINE {
                return Err(AssistantError::Protocol("line too long".into()));
            }
            if self.fill()? == 0 {
                return Err(AssistantError::Protocol("unexpected end of stream".into()));
            }
        }
    }

    /// Reads more bytes into `raw`. Returns 0 on end of stream.
    fn fill(&mut self) -> Result<usize, AssistantError> {
        let mut buf = [0u8; 8192];
        loop {
            if self.cancel.is_cancelled() {
                return Err(AssistantError::Cancelled);
            }
            match self.stream.read(&mut buf) {
                Ok(0) => return Ok(0),
                Ok(n) => {
                    self.raw.extend_from_slice(&buf[..n]);
                    self.last = Instant::now();
                    return Ok(n);
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {
                    if self.last.elapsed() >= self.idle {
                        return Err(AssistantError::Timeout);
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

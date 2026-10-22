//! The single owner of FlameWM's X11 CLIPBOARD file selection.
//!
//! `set_files` publishes a file list from a background owner thread that serves
//! ICCCM requests until another client takes CLIPBOARD. `get_files` reads the
//! current owner with a bounded wait. Payload encoding lives in `codec`.
//!
//! Migration note: `crates/flamewm-screenshot/src/clipboard.rs` still carries its own
//! image/png owner. Move it onto `owner.rs` (a generic payload list) when that binary
//! is next touched; this crate does not edit the screenshot binary.

mod codec;
mod owner;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt as _, Property};
use x11rb::rust_connection::RustConnection;
use x11rb::{CURRENT_TIME, NONE};

/// Upper bound for one `get_files` call, across all targets and INCR chunks.
const GET_TIMEOUT: Duration = Duration::from_millis(500);
const DATA_PROPERTY: &str = "FLAMEWM_CLIPBOARD_DATA";
const UTF8_STRING: &str = "UTF8_STRING";
const STRING: &str = "STRING";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOp {
    Copy,
    Cut,
}

/// Thread-safe handle. Owners run on their own connections, so a handle can be
/// shared with `Arc` and used from several threads.
pub struct Clipboard {
    display: Option<String>,
    requester: Mutex<Requester>,
}

impl Clipboard {
    /// Connects using `$DISPLAY`.
    pub fn connect() -> Result<Self, String> {
        Self::connect_display(None)
    }

    /// Connects to `display`, or `$DISPLAY` when `None` (tests and multi-display tools).
    pub fn connect_display(display: Option<&str>) -> Result<Self, String> {
        Ok(Self {
            display: display.map(str::to_owned),
            requester: Mutex::new(Requester::connect(display)?),
        })
    }

    /// Takes CLIPBOARD and serves `paths` until another client replaces it. Returns once
    /// the selection is owned. Paths must be absolute.
    pub fn set_files(&self, paths: &[PathBuf], op: FileOp) -> Result<(), String> {
        if paths.is_empty() || !paths.iter().all(|path| path.is_absolute()) {
            return Err("clipboard files must be a non-empty list of absolute paths".into());
        }
        owner::spawn(self.display.clone(), "CLIPBOARD", codec::payloads(paths, op))
    }

    /// Takes CLIPBOARD and serves `text` as UTF8_STRING (and STRING for ASCII text) until another
    /// client replaces it. Returns once the selection is owned.
    pub fn set_text(&self, text: &str) -> Result<(), String> {
        owner::spawn(self.display.clone(), "CLIPBOARD", codec::text_payloads(text))
    }

    /// Takes PRIMARY (the middle-click selection) and serves `text` like `set_text`.
    pub fn set_primary_text(&self, text: &str) -> Result<(), String> {
        owner::spawn(self.display.clone(), "PRIMARY", codec::text_payloads(text))
    }

    /// Reads text from PRIMARY with the same fallback and timeout as `get_text`.
    pub fn get_primary_text(&self) -> Result<Option<String>, String> {
        let deadline = Instant::now() + GET_TIMEOUT;
        let requester = self.requester.lock().map_err(err)?;
        let primary = requester.primary;
        if let Some(bytes) = requester.read_selection(primary, requester.utf8_string, deadline)? {
            return Ok(Some(String::from_utf8_lossy(&bytes).into_owned()));
        }
        Ok(requester
            .read_selection(primary, requester.string, deadline)?
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Reads text from CLIPBOARD, preferring UTF8_STRING and falling back to STRING. Returns
    /// `Ok(None)` when the clipboard holds no text or the owner does not answer within 500 ms.
    pub fn get_text(&self) -> Result<Option<String>, String> {
        let deadline = Instant::now() + GET_TIMEOUT;
        let requester = self.requester.lock().map_err(err)?;
        if let Some(bytes) = requester.read(requester.utf8_string, deadline)? {
            return Ok(Some(String::from_utf8_lossy(&bytes).into_owned()));
        }
        Ok(requester
            .read(requester.string, deadline)?
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// Reads the file list on CLIPBOARD. Returns `Ok(None)` when the clipboard holds no
    /// file list, or when the owner does not answer within 500 ms.
    pub fn get_files(&self) -> Result<Option<(Vec<PathBuf>, FileOp)>, String> {
        let deadline = Instant::now() + GET_TIMEOUT;
        let requester = self.requester.lock().map_err(err)?;
        if let Some(bytes) = requester.read(requester.gnome, deadline)?
            && let Some((op, paths)) = codec::parse_gnome_files(&bytes)
            && !paths.is_empty()
        {
            return Ok(Some((paths, op)));
        }
        let Some(uri_list) = requester.read(requester.uri_list, deadline)? else {
            return Ok(None);
        };
        let paths = codec::parse_uri_list(&uri_list);
        if paths.is_empty() {
            return Ok(None);
        }
        let op = match requester.read(requester.kde_cut, deadline)? {
            Some(bytes) => codec::parse_kde_cut(&bytes),
            None => FileOp::Copy,
        };
        Ok(Some((paths, op)))
    }
}

/// Requesting side: one connection and one helper window reused for every read.
struct Requester {
    conn: RustConnection,
    window: u32,
    selection: Atom,
    incr: Atom,
    data: Atom,
    uri_list: Atom,
    gnome: Atom,
    kde_cut: Atom,
    utf8_string: Atom,
    string: Atom,
    primary: Atom,
}

impl Requester {
    fn connect(display: Option<&str>) -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(display).map_err(err)?;
        let atoms = intern(
            &conn,
            &[
                "CLIPBOARD",
                "INCR",
                DATA_PROPERTY,
                codec::URI_LIST,
                codec::GNOME_FILES,
                codec::KDE_CUT,
                UTF8_STRING,
                STRING,
                "PRIMARY",
            ],
        )?;
        let window = owner::helper_window(&conn, screen)?;
        Ok(Self {
            conn,
            window,
            selection: atoms[0],
            incr: atoms[1],
            data: atoms[2],
            uri_list: atoms[3],
            gnome: atoms[4],
            kde_cut: atoms[5],
            utf8_string: atoms[6],
            string: atoms[7],
            primary: atoms[8],
        })
    }

    /// Requests `target` from CLIPBOARD; `None` when refused, absent, or past `deadline`.
    fn read(&self, target: Atom, deadline: Instant) -> Result<Option<Vec<u8>>, String> {
        self.read_selection(self.selection, target, deadline)
    }

    /// Requests `target` from `selection`; `None` when refused, absent, or past `deadline`.
    fn read_selection(
        &self,
        selection: Atom,
        target: Atom,
        deadline: Instant,
    ) -> Result<Option<Vec<u8>>, String> {
        if Instant::now() >= deadline {
            return Ok(None);
        }
        self.conn
            .convert_selection(self.window, selection, target, self.data, CURRENT_TIME)
            .map_err(err)?;
        self.conn.flush().map_err(err)?;
        let notify = next_event(
            &self.conn,
            deadline,
            |event| matches!(event, Event::SelectionNotify(n) if n.requestor == self.window && n.target == target),
        )?;
        let Some(Event::SelectionNotify(notify)) = notify else {
            return Ok(None);
        };
        if notify.property == NONE {
            return Ok(None);
        }
        let reply = self.take_property(AtomEnum::ANY)?;
        if reply.type_ != self.incr {
            return Ok(Some(reply.value));
        }
        // ICCCM INCR: each chunk is announced by NewValue; reading it deletes it, which
        // asks the owner for the next one. An empty chunk ends the transfer.
        let mut data = Vec::new();
        loop {
            let ready = next_event(
                &self.conn,
                deadline,
                |event| matches!(event, Event::PropertyNotify(p) if p.window == self.window && p.atom == self.data && p.state == Property::NEW_VALUE),
            )?;
            if ready.is_none() {
                return Ok(None);
            }
            let chunk = self.take_property(AtomEnum::ANY)?.value;
            if chunk.is_empty() {
                return Ok(Some(data));
            }
            data.extend_from_slice(&chunk);
        }
    }

    /// Reads and deletes the data property. Offset 0 with a large length returns the whole value.
    fn take_property(
        &self,
        ty: impl Into<Atom>,
    ) -> Result<x11rb::protocol::xproto::GetPropertyReply, String> {
        self.conn
            .get_property(true, self.window, self.data, ty, 0, 1 << 24)
            .map_err(err)?
            .reply()
            .map_err(err)
    }
}

/// Waits for an event matching `wanted` or returns `None` at `deadline`. Never blocks forever.
fn next_event(
    conn: &RustConnection,
    deadline: Instant,
    mut wanted: impl FnMut(&Event) -> bool,
) -> Result<Option<Event>, String> {
    loop {
        while let Some(event) = conn.poll_for_event().map_err(err)? {
            if wanted(&event) {
                return Ok(Some(event));
            }
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Interns `names` with all requests pipelined before the first reply.
fn intern(conn: &RustConnection, names: &[&str]) -> Result<Vec<Atom>, String> {
    let cookies = names
        .iter()
        .map(|name| conn.intern_atom(false, name.as_bytes()).map_err(err))
        .collect::<Result<Vec<_>, _>>()?;
    cookies
        .into_iter()
        .map(|cookie| Ok(cookie.reply().map_err(err)?.atom))
        .collect()
}

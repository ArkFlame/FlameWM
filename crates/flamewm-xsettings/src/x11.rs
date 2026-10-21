//! X11 adapter for [`ManagerPort`]. Owns one long-lived connection for the
//! lifetime of the manager; dropping the manager closes it and releases the
//! selection on the server.

use std::collections::BTreeMap;
use std::os::fd::{AsRawFd, RawFd};

use flamewm_api::{FlameError, FlameResult};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, PropMode, Window,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::manager::{
    MANAGER_MESSAGE_ATOM, ManagerPort, SETTINGS_PROPERTY_ATOM, SelectionClear, XSettingsManager,
    acquire_manager,
};
use crate::{XSETTINGS_SELECTION, XSettingsValue};

const TIMESTAMP_ATOM: &str = "_FLAMEWM_XSETTINGS_TIMESTAMP";
const MAX_TIMESTAMP_EVENTS: usize = 64;

/// Manager type returned by [`claim_x11_manager`].
pub type X11XSettingsManager = XSettingsManager<X11ManagerPort>;

/// Real X11 connection implementing the manager side effects.
pub struct X11ManagerPort {
    conn: RustConnection,
    root: Window,
}

impl std::fmt::Debug for X11ManagerPort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("X11ManagerPort")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl X11ManagerPort {
    fn connect() -> FlameResult<Self> {
        let (conn, screen_num) = x11rb::connect(None).map_err(x11_error)?;
        let root = conn
            .setup()
            .roots
            .get(screen_num)
            .map(|screen| screen.root)
            .ok_or_else(|| FlameError::unavailable("X11 screen missing from setup"))?;
        Ok(Self { conn, root })
    }

    fn atom(&self, name: &str) -> FlameResult<u32> {
        Ok(self
            .conn
            .intern_atom(false, name.as_bytes())
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?
            .atom)
    }
}

impl ManagerPort for X11ManagerPort {
    fn selection_owner(&mut self, selection: &str) -> FlameResult<u32> {
        let atom = self.atom(selection)?;
        Ok(self
            .conn
            .get_selection_owner(atom)
            .map_err(x11_error)?
            .reply()
            .map_err(x11_error)?
            .owner)
    }

    fn create_manager_window(&mut self) -> FlameResult<u32> {
        let window = self.conn.generate_id().map_err(x11_error)?;
        // Unmapped 1x1 window far off-screen; it exists only to own the selection.
        let aux = CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE);
        self.conn
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                self.root,
                -100,
                -100,
                1,
                1,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &aux,
            )
            .map_err(x11_error)?
            .check()
            .map_err(x11_error)?;
        Ok(window)
    }

    fn acquire_timestamp(&mut self, window: u32) -> FlameResult<u32> {
        let atom = self.atom(TIMESTAMP_ATOM)?;
        // A zero-length append still generates a PropertyNotify carrying the
        // current server time, which is the canonical way to obtain one.
        self.conn
            .change_property(PropMode::APPEND, window, atom, AtomEnum::STRING, 8, 0, &[])
            .map_err(x11_error)?
            .check()
            .map_err(x11_error)?;
        for _ in 0..MAX_TIMESTAMP_EVENTS {
            match self.conn.wait_for_event().map_err(x11_error)? {
                Event::PropertyNotify(event) if event.window == window => return Ok(event.time),
                _ => {}
            }
        }
        Err(FlameError::unavailable(
            "X11 server did not report a timestamp for the XSettings window",
        ))
    }

    fn set_selection_owner(
        &mut self,
        selection: &str,
        window: u32,
        timestamp: u32,
    ) -> FlameResult<()> {
        let atom = self.atom(selection)?;
        self.conn
            .set_selection_owner(window, atom, timestamp)
            .map_err(x11_error)?
            .check()
            .map_err(x11_error)
    }

    fn announce_manager(
        &mut self,
        timestamp: u32,
        selection: &str,
        window: u32,
    ) -> FlameResult<()> {
        let manager = self.atom(MANAGER_MESSAGE_ATOM)?;
        let selection_atom = self.atom(selection)?;
        let event = ClientMessageEvent::new(
            32,
            self.root,
            manager,
            [timestamp, selection_atom, window, 0, 0],
        );
        self.conn
            .send_event(false, self.root, EventMask::STRUCTURE_NOTIFY, event)
            .map_err(x11_error)?
            .check()
            .map_err(x11_error)?;
        self.conn.flush().map_err(x11_error)
    }

    fn write_settings(&mut self, window: u32, blob: &[u8]) -> FlameResult<()> {
        let property = self.atom(SETTINGS_PROPERTY_ATOM)?;
        let length = u32::try_from(blob.len())
            .map_err(|_| FlameError::unavailable("XSettings blob too large"))?;
        self.conn
            .change_property(
                PropMode::REPLACE,
                window,
                property,
                property,
                8,
                length,
                blob,
            )
            .map_err(x11_error)?
            .check()
            .map_err(x11_error)?;
        self.conn.flush().map_err(x11_error)
    }

    fn wake_fd(&self) -> Option<RawFd> {
        // The connection socket becomes readable when the server queues events
        // (including SelectionClear). The reactor registers it; it is not owned here.
        Some(self.conn.stream().as_raw_fd())
    }

    fn next_selection_clear(&mut self) -> FlameResult<Option<SelectionClear>> {
        // Non-blocking: drain whatever the server already queued on this
        // connection. Non-clear events are discarded; only SelectionClear matters.
        while let Some(event) = self.conn.poll_for_event().map_err(x11_error)? {
            if let Event::SelectionClear(clear) = event {
                let name = self
                    .conn
                    .get_atom_name(clear.selection)
                    .map_err(x11_error)?
                    .reply()
                    .map_err(x11_error)?
                    .name;
                return Ok(Some(SelectionClear {
                    window: clear.owner,
                    selection: String::from_utf8_lossy(&name).into_owned(),
                }));
            }
        }
        Ok(None)
    }
}

/// Claim `_XSETTINGS_S0` on the default display and publish `desired`.
///
/// Returns an error (and leaves any foreign owner untouched) when the selection
/// is already owned or the server cannot be queried.
pub fn claim_x11_manager(
    desired: &BTreeMap<String, XSettingsValue>,
) -> FlameResult<X11XSettingsManager> {
    let port = X11ManagerPort::connect()?;
    acquire_manager(port, XSETTINGS_SELECTION, desired)
}

fn x11_error(error: impl std::fmt::Display) -> FlameError {
    FlameError::unavailable(format!("XSettings X11 request failed: {error}"))
}

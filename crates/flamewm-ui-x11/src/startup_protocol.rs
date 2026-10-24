//! Narrow X11 startup-notification receiver. This backend owns X11 atoms,
//! subscriptions, window metadata and active-only pointer sampling. A product
//! controller consumes typed events and owns sequence lifetimes separately.
use std::collections::HashSet;
use std::os::fd::{AsRawFd, RawFd};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask, MapState, Window,
};
use x11rb::rust_connection::RustConnection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupX11Event {
    Fragment {
        sender: Window,
        begin: bool,
        data: [u8; 20],
    },
    WindowIdentity {
        startup_id: Option<String>,
        classes: Vec<String>,
    },
}

pub struct StartupX11Observer {
    conn: RustConnection,
    root: Window,
    begin: Atom,
    cont: Atom,
    startup_id: Atom,
    client_leader: Atom,
    utf8: Atom,
    client_list: Atom,
    active_window: Atom,
    known_clients: HashSet<Window>,
}

impl StartupX11Observer {
    pub fn connect() -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let root = conn.setup().roots[screen].root;
        let atom = |name: &[u8]| -> Result<Atom, String> {
            conn.intern_atom(false, name)
                .map_err(|e| e.to_string())?
                .reply()
                .map(|reply| reply.atom)
                .map_err(|e| e.to_string())
        };
        let begin = atom(b"_NET_STARTUP_INFO_BEGIN")?;
        let cont = atom(b"_NET_STARTUP_INFO")?;
        let startup_id = atom(b"_NET_STARTUP_ID")?;
        let client_leader = atom(b"WM_CLIENT_LEADER")?;
        let utf8 = atom(b"UTF8_STRING")?;
        let client_list = atom(b"_NET_CLIENT_LIST")?;
        let active_window = atom(b"_NET_ACTIVE_WINDOW")?;
        let known_clients = read_windows(&conn, root, client_list).into_iter().collect();
        conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new()
                .event_mask(EventMask::PROPERTY_CHANGE | EventMask::SUBSTRUCTURE_NOTIFY),
        )
        .map_err(|e| e.to_string())?;
        conn.flush().map_err(|e| e.to_string())?;
        Ok(Self {
            conn,
            root,
            begin,
            cont,
            startup_id,
            client_leader,
            utf8,
            client_list,
            active_window,
            known_clients,
        })
    }

    pub fn fd(&self) -> RawFd {
        self.conn.stream().as_raw_fd()
    }

    /// Drain the X queue in wire order. Identity queries are conditional on
    /// an active or newly announced launch, never performed continuously.
    pub fn on_ready(&mut self, has_active_launch: bool) -> Result<Vec<StartupX11Event>, String> {
        let mut events = Vec::new();
        let mut observe = has_active_launch;
        while let Some(event) = self.conn.poll_for_event().map_err(|e| e.to_string())? {
            match event {
                Event::ClientMessage(event) if event.format == 8 => {
                    let begin = event.type_ == self.begin;
                    if !begin && event.type_ != self.cont {
                        continue;
                    }
                    // A complete new sequence may be in this same batch before MapNotify.
                    observe = true;
                    events.push(StartupX11Event::Fragment {
                        sender: event.window,
                        begin,
                        data: event.data.as_data8(),
                    });
                }
                Event::MapNotify(event) if observe && event.event == self.root => {
                    events.extend(self.identity(event.window));
                }
                Event::PropertyNotify(event)
                    if event.window == self.root && event.atom == self.client_list =>
                {
                    let next: HashSet<_> = read_windows(&self.conn, self.root, self.client_list)
                        .into_iter()
                        .collect();
                    if observe {
                        let added: Vec<_> = next.difference(&self.known_clients).copied().collect();
                        events.extend(added.into_iter().filter_map(|window| self.identity(window)));
                    }
                    self.known_clients = next;
                }
                // Covers activation of an existing single-instance application.
                Event::PropertyNotify(event)
                    if observe && event.window == self.root && event.atom == self.active_window =>
                {
                    events.extend(
                        read_windows(&self.conn, self.root, self.active_window)
                            .into_iter()
                            .next()
                            .and_then(|window| self.identity(window)),
                    );
                }
                _ => {}
            }
        }
        Ok(events)
    }

    pub fn pointer_position(&self) -> Option<(i32, i32)> {
        let result = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((i32::from(result.root_x), i32::from(result.root_y)))
    }

    fn startup_id_for(&self, window: Window) -> Option<String> {
        self.conn
            .get_property(false, window, self.startup_id, self.utf8, 0, 256)
            .ok()?
            .reply()
            .ok()
            .and_then(|reply| String::from_utf8(reply.value).ok())
    }

    fn identity(&self, window: Window) -> Option<StartupX11Event> {
        let ready = self
            .conn
            .get_window_attributes(window)
            .ok()?
            .reply()
            .ok()
            .is_some_and(|attributes| window_is_ready(attributes.map_state));
        if !ready {
            return None;
        }
        let startup_id = self.startup_id_for(window).or_else(|| {
            self.conn
                .get_property(false, window, self.client_leader, AtomEnum::WINDOW, 0, 1)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .and_then(|reply| reply.value32().and_then(|mut it| it.next()))
                .and_then(|leader| self.startup_id_for(leader))
        });
        let classes = self
            .conn
            .get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 128)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| {
                reply
                    .value
                    .split(|byte| *byte == 0)
                    .filter_map(|part| std::str::from_utf8(part).ok().map(str::to_owned))
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        (startup_id.is_some() || !classes.is_empty()).then_some(StartupX11Event::WindowIdentity {
            startup_id,
            classes,
        })
    }
}

fn window_is_ready(map_state: MapState) -> bool {
    map_state == MapState::VIEWABLE
}

fn read_windows(conn: &RustConnection, root: Window, atom: Atom) -> Vec<Window> {
    conn.get_property(false, root, atom, AtomEnum::WINDOW, 0, 4096)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| reply.value32().map(|iter| iter.collect::<Vec<_>>()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_completion_requires_a_viewable_window() {
        assert!(window_is_ready(MapState::VIEWABLE));
        assert!(!window_is_ready(MapState::UNMAPPED));
        assert!(!window_is_ready(MapState::UNVIEWABLE));
    }
}

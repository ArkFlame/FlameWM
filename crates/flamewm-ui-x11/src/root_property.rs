//! Narrow root-window CARDINAL publisher for product overlays the WM must see (for example the
//! desktop rubber band, which sits below the WM's shadows). Owns its own X connection; the
//! property is deleted when the publisher drops so a closed desktop never leaves one behind.
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt, PropMode, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

pub struct RootCardinalPublisher {
    conn: RustConnection,
    root: Window,
    property: Atom,
    published: bool,
}

impl RootCardinalPublisher {
    pub fn connect(name: &str) -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let root = conn.setup().roots[screen].root;
        let property = conn
            .intern_atom(false, name.as_bytes())
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?
            .atom;
        // A desktop that died without dropping its publisher leaves the property behind; the WM
        // would then defer shadow re-shading, so a fresh writer starts from a clean root.
        conn.delete_property(root, property)
            .map_err(|e| e.to_string())?;
        conn.flush().map_err(|e| e.to_string())?;
        Ok(Self {
            conn,
            root,
            property,
            published: false,
        })
    }

    /// Replaces the property with `values`, or deletes it for `None`. Flushes immediately so the
    /// WM sees the change without waiting for the next desktop event.
    pub fn set(&mut self, values: Option<&[u32]>) -> Result<(), String> {
        match values {
            Some(values) => {
                self.conn
                    .change_property32(
                        PropMode::REPLACE,
                        self.root,
                        self.property,
                        AtomEnum::CARDINAL,
                        values,
                    )
                    .map_err(|e| e.to_string())?;
                self.published = true;
            }
            None if self.published => {
                self.conn
                    .delete_property(self.root, self.property)
                    .map_err(|e| e.to_string())?;
                self.published = false;
            }
            None => return Ok(()),
        }
        self.conn.flush().map_err(|e| e.to_string())
    }
}

impl Drop for RootCardinalPublisher {
    fn drop(&mut self) {
        if self.published {
            let _ = self.conn.delete_property(self.root, self.property);
            let _ = self.conn.flush();
        }
    }
}

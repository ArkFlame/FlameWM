//! X11 connection, window-tree queries and synthetic pointer input for the canary.

use std::collections::VecDeque;

use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, BUTTON_PRESS_EVENT, ButtonPressEvent, ClientMessageData, ClientMessageEvent,
    ConnectionExt, EventMask, ImageFormat, KeyButMask, MOTION_NOTIFY_EVENT, Motion,
    MotionNotifyEvent, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::contracts::j13_motif_no_decorations;
use crate::output::json_str;

pub(crate) const BUTTON_RELEASE_EVENT: u8 = 5;

pub(crate) struct Canary {
    pub(crate) conn: RustConnection,
    pub(crate) screen: usize,
    pub(crate) root: Window,
}

#[derive(Debug, Clone)]
pub(crate) struct WinInfo {
    pub(crate) id: u32,
    pub(crate) parent: u32,
    pub(crate) x: i16,
    pub(crate) y: i16,
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) map_state: u8,
    pub(crate) name: String,
    pub(crate) class: String,
}

impl WinInfo {
    pub(crate) fn map_name(&self) -> &'static str {
        match self.map_state {
            0 => "unmapped",
            1 => "unviewable",
            2 => "viewable",
            _ => "unknown",
        }
    }
    pub(crate) fn to_json(&self) -> String {
        format!(
            "{{\"id\":{},\"parent\":{},\"x\":{},\"y\":{},\"width\":{},\"height\":{},\"map_state\":\"{}\",\"name\":{},\"class\":{}}}",
            self.id,
            self.parent,
            self.x,
            self.y,
            self.width,
            self.height,
            self.map_name(),
            json_str(&self.name),
            json_str(&self.class),
        )
    }
}

impl Canary {
    pub(crate) fn connect() -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
        let root = conn.setup().roots[screen].root;
        Ok(Self { conn, screen, root })
    }

    pub(crate) fn prop_bytes(&self, window: Window, atom: Atom) -> Vec<u8> {
        self.conn
            .get_property(false, window, atom, AtomEnum::ANY, 0, u32::MAX)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.value)
            .unwrap_or_default()
    }

    pub(crate) fn text_prop(&self, window: Window, names: &[&str]) -> String {
        for name in names {
            if let Some(atom) = self.intern(name) {
                let bytes = self.prop_bytes(window, atom);
                if !bytes.is_empty() {
                    // WM_CLASS is NUL-separated instance\0class\0; use class part.
                    let text = if *name == "WM_CLASS" {
                        bytes
                            .split(|b| *b == 0)
                            .filter(|p| !p.is_empty())
                            .next_back()
                            .map_or(String::new(), |p| String::from_utf8_lossy(p).into_owned())
                    } else {
                        String::from_utf8_lossy(&bytes)
                            .trim_end_matches('\0')
                            .to_owned()
                    };
                    if !text.is_empty() {
                        return text;
                    }
                }
            }
        }
        String::new()
    }

    pub(crate) fn info(&self, window: Window, parent: Window) -> Option<WinInfo> {
        let geom = self.conn.get_geometry(window).ok()?.reply().ok()?;
        let attrs = self.conn.get_window_attributes(window).ok()?.reply().ok()?;
        let name = self.text_prop(window, &["_NET_WM_NAME", "WM_NAME"]);
        let class = self.text_prop(window, &["WM_CLASS"]);
        Some(WinInfo {
            id: window,
            parent,
            x: geom.x,
            y: geom.y,
            width: geom.width,
            height: geom.height,
            map_state: u8::from(attrs.map_state),
            name,
            class,
        })
    }

    pub(crate) fn walk(&self, parent: Window, out: &mut Vec<WinInfo>) {
        let tree = match self.conn.query_tree(parent) {
            Ok(c) => match c.reply() {
                Ok(t) => t,
                Err(_) => return,
            },
            Err(_) => return,
        };
        for child in tree.children {
            if let Some(info) = self.info(child, parent) {
                out.push(info);
            }
            self.walk(child, out);
        }
    }

    pub(crate) fn all(&self) -> Vec<WinInfo> {
        let mut out = Vec::new();
        self.walk(self.root, &mut out);
        out
    }

    pub(crate) fn find(&self, name: &str) -> Vec<WinInfo> {
        let needle = name.to_lowercase();
        self.all()
            .into_iter()
            .filter(|w| {
                w.name.to_lowercase().contains(&needle) || w.class.to_lowercase().contains(&needle)
            })
            .collect()
    }

    pub(crate) fn by_id(&self, id: u32) -> Option<WinInfo> {
        let mut queue = VecDeque::from([self.root]);
        while let Some(parent) = queue.pop_front() {
            let tree = self.conn.query_tree(parent).ok()?.reply().ok()?;
            for child in tree.children {
                if child == id {
                    return self.info(child, parent);
                }
                queue.push_back(child);
            }
        }
        None
    }

    pub(crate) fn sample(
        &self,
        drawable: Window,
        x: i16,
        y: i16,
        w: u16,
        h: u16,
    ) -> Result<Vec<u8>, String> {
        let reply = self
            .conn
            .get_image(ImageFormat::Z_PIXMAP, drawable, x, y, w, h, u32::MAX)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        Ok(reply.data)
    }

    pub(crate) fn pixel_stats(data: &[u8]) -> (bool, u64, u8, u8, u8) {
        if data.is_empty() {
            return (true, 0, 0, 0, 0);
        }
        // Assume 4 bytes/pixel (24/32-bit ZPixmap); fall back to raw mean.
        let mut sum_r: u64 = 0;
        let mut sum_g: u64 = 0;
        let mut sum_b: u64 = 0;
        let mut n: u64 = 0;
        let mut all_zero = true;
        // Try native-endian 32-bit pixels, BGRX order typical for X.
        let mut i = 0;
        while i + 4 <= data.len() {
            let px = u32::from_ne_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
            if px != 0 {
                all_zero = false;
            }
            sum_b += u64::from((px & 0xff) as u8);
            sum_g += u64::from(((px >> 8) & 0xff) as u8);
            sum_r += u64::from(((px >> 16) & 0xff) as u8);
            n += 1;
            i += 4;
        }
        if n == 0 {
            let sum: u64 = data.iter().map(|b| u64::from(*b)).sum();
            all_zero = sum == 0;
            n = data.len() as u64;
            sum_r = sum;
        }
        let mean_r = u8::try_from(sum_r / n.max(1)).unwrap_or(255);
        let mean_g = u8::try_from(sum_g / n.max(1)).unwrap_or(255);
        let mean_b = u8::try_from(sum_b / n.max(1)).unwrap_or(255);
        (all_zero, n, mean_r, mean_g, mean_b)
    }

    pub(crate) fn send_button(
        &self,
        window: Window,
        x: i16,
        y: i16,
        press: bool,
    ) -> Result<(), String> {
        self.send_button_detail(window, x, y, press, 1)
    }

    pub(crate) fn send_button_detail(
        &self,
        window: Window,
        x: i16,
        y: i16,
        press: bool,
        detail: u8,
    ) -> Result<(), String> {
        let (gx, gy) = self.window_origin(window)?;
        let ev = ButtonPressEvent {
            response_type: if press {
                BUTTON_PRESS_EVENT
            } else {
                BUTTON_RELEASE_EVENT
            },
            detail,
            sequence: 0,
            time: CURRENT_TIME,
            root: self.root,
            event: window,
            child: x11rb::NONE,
            root_x: gx.saturating_add(x),
            root_y: gy.saturating_add(y),
            event_x: x,
            event_y: y,
            state: KeyButMask::default(),
            same_screen: true,
        };
        self.send_checked(window, EventMask::BUTTON_PRESS, ev)
    }

    fn send_checked<T: Into<[u8; 32]>>(
        &self,
        dest: Window,
        mask: EventMask,
        ev: T,
    ) -> Result<(), String> {
        self.conn
            .send_event(false, dest, mask, ev)
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())?;
        self.conn.flush().map_err(|e| e.to_string())?;
        Ok(())
    }

    fn window_origin(&self, window: Window) -> Result<(i16, i16), String> {
        let geom = self
            .conn
            .get_geometry(window)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        Ok((geom.x, geom.y))
    }

    pub(crate) fn stacking_order(&self) -> Result<Vec<Window>, String> {
        self.conn
            .query_tree(self.root)
            .map_err(|e| e.to_string())?
            .reply()
            .map(|t| t.children)
            .map_err(|e| e.to_string())
    }

    pub(crate) fn root_tree(&self) -> Option<x11rb::protocol::xproto::QueryTreeReply> {
        self.conn.query_tree(self.root).ok()?.reply().ok()
    }

    pub(crate) fn intern(&self, name: &str) -> Option<Atom> {
        self.conn
            .intern_atom(false, name.as_bytes())
            .ok()?
            .reply()
            .ok()
            .map(|r| r.atom)
    }

    pub(crate) fn active_window(&self) -> Option<Window> {
        let atom = self.intern("_NET_ACTIVE_WINDOW")?;
        let reply = self
            .conn
            .get_property(false, self.root, atom, AtomEnum::WINDOW, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        if reply.value.len() >= 4 {
            let id = u32::from_ne_bytes([
                reply.value[0],
                reply.value[1],
                reply.value[2],
                reply.value[3],
            ]);
            if id != 0 && id != x11rb::NONE {
                return Some(id);
            }
        }
        None
    }

    pub(crate) fn prop_u32(&self, window: Window, name: &str) -> Vec<u32> {
        let Some(atom) = self.intern(name) else {
            return Vec::new();
        };
        let bytes = self.prop_bytes(window, atom);
        bytes
            .chunks_exact(4)
            .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }

    pub(crate) fn motif_no_decor(&self, window: Window) -> bool {
        j13_motif_no_decorations(&self.prop_u32(window, "_MOTIF_WM_HINTS"))
    }

    pub(crate) fn send_active_window(&self, window: Window) -> Result<(), String> {
        let msg_type = self
            .intern("_NET_ACTIVE_WINDOW")
            .ok_or_else(|| "no _NET_ACTIVE_WINDOW atom".to_owned())?;
        let ev = ClientMessageEvent {
            response_type: 33,
            format: 32,
            sequence: 0,
            window,
            type_: msg_type,
            data: ClientMessageData::from([1u32, CURRENT_TIME, 0, 0, 0]),
        };
        self.send_checked(self.root, EventMask::SUBSTRUCTURE_REDIRECT, ev)
    }

    pub(crate) fn root_geometry(&self) -> (u16, u16) {
        let setup = &self.conn.setup().roots[self.screen];
        (setup.width_in_pixels, setup.height_in_pixels)
    }

    pub(crate) fn send_motion(&self, window: Window, x: i16, y: i16) -> Result<(), String> {
        let (gx, gy) = self.window_origin(window)?;
        let ev = MotionNotifyEvent {
            response_type: MOTION_NOTIFY_EVENT,
            detail: Motion::NORMAL,
            sequence: 0,
            time: CURRENT_TIME,
            root: self.root,
            event: window,
            child: x11rb::NONE,
            root_x: gx.saturating_add(x),
            root_y: gy.saturating_add(y),
            event_x: x,
            event_y: y,
            state: KeyButMask::default(),
            same_screen: true,
        };
        self.send_checked(window, EventMask::POINTER_MOTION, ev)
    }
}

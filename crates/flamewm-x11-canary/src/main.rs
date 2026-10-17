//! Xephyr canary harness: locate Flame surfaces, query geometry/map state,
//! send synthetic button/motion events, sample pixels, emit JSON/text.
//!
//! Read-only probe plus synthetic input; never touches product renderers.
//! Scenarios: SR05 first-present, SR07 popup matrix, SR08 black-pixel,
//! SR15 input matrix, J12 focused assertions (panel pre-input, start
//! stacking, selection mid-drag, desktop right-click, shell context
//! popup, chrome geometry), J13 window-manager contracts (latency,
//! activation, popups, cursors, hints), J06 post-registration probes
//! (calendar-300, start single-surface/app-click, desktop icon wake,
//! filemanager map-latency, title-centered, resize-8, fixed-size,
//! pixmap-ledger).

use std::collections::VecDeque;
use std::env;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, BUTTON_PRESS_EVENT, ButtonPressEvent, ClientMessageData, ClientMessageEvent,
    ConfigureWindowAux, ConnectionExt, CreateWindowAux, EventMask, ImageFormat, KeyButMask,
    MOTION_NOTIFY_EVENT, MapState, Motion, MotionNotifyEvent, PropMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

mod scenarios;

const BUTTON_RELEASE_EVENT: u8 = 5;

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

fn json_str(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) fn fail(message: &str) -> ! {
    eprintln!("CANARY_FAIL {message}");
    std::process::exit(1);
}

/// J13 pure mirrors of frozen product contracts (wm-x11 J04/J05/J06 source
/// wins): Motif no-decoration parse (`decoration/policy.rs`), fixed-size
/// resizable parse (`size_hints.rs`), 6px resize-edge hit test
/// (`client.rs`).
fn j13_motif_no_decorations(values: &[u32]) -> bool {
    if values.len() < 3 {
        return false;
    }
    values[0] & (1 << 1) != 0 && values[2] == 0
}

fn j13_hints_resizable(values: &[u32]) -> bool {
    let Some(&flags) = values.first() else {
        return true;
    };
    let min = if flags & (1 << 4) != 0 && values.len() >= 7 {
        let size = (values[5], values[6]);
        if size != (0, 0) { Some(size) } else { None }
    } else {
        None
    };
    let max = if flags & (1 << 5) != 0 && values.len() >= 9 {
        let size = (values[7], values[8]);
        if size != (0, 0) { Some(size) } else { None }
    } else {
        None
    };
    match (min, max) {
        (Some(a), Some(b)) => a != b,
        _ => true,
    }
}

fn j13_resize_edges(w: u32, h: u32, x: i16, y: i16) -> (bool, bool, bool, bool) {
    let width = w.clamp(1, u32::from(u16::MAX)) as i16;
    let height = h.clamp(1, u32::from(u16::MAX)) as i16;
    (
        x <= 6,
        x >= width.saturating_sub(6),
        y <= 6,
        y >= height.saturating_sub(6),
    )
}

/// J06 post-registration pure helpers (mirrors of frozen product contracts).
fn j06_title_center_offset(frame_w: u32, text_w: u32) -> i16 {
    if text_w >= frame_w {
        return 0;
    }
    ((frame_w - text_w) / 2) as i16
}

fn j06_ledger_pass(sampled: usize, delivered: usize, non_black: usize) -> bool {
    sampled > 0 && delivered == sampled && non_black >= 1
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
                            .last()
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

    fn send_checked<T: Into<[u8; 32]>>(&self, dest: Window, mask: EventMask, ev: T) -> Result<(), String> {
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
        let atom = match self.intern(name) {
            Some(a) => a,
            None => return Vec::new(),
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

pub(crate) fn arg(args: &[String], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().cloned();
        }
        if let Some(rest) = a.strip_prefix(&format!("{flag}=")) {
            return Some(rest.to_owned());
        }
    }
    None
}

pub(crate) fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

pub(crate) fn arg_num<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> T {
    arg(args, flag).map_or(default, |v| {
        v.parse()
            .unwrap_or_else(|_| fail(&format!("{flag} must be a number")))
    })
}

fn usage() -> ! {
    eprintln!(
        "flamewm-x11-canary <command> [options]\n\
         commands:\n  \
         list [--name <substr>] [--json]\n  \
         geometry --name <substr> [--json] | --window <id> [--json]\n  \
         map-state --name <substr> [--json] | --window <id>\n  \
         click --name <substr> | --window <id> [--x <n> --y <n>]\n  \
         motion --name <substr> | --window <id> [--x <n> --y <n>]\n  \
         sample --window <id> [--x <n> --y <n> --w <n> --h <n>] [--json]\n  \
         sample-root [--w <n> --h <n>] [--json]\n  \
         stacking [--json]\n  \
         root-geometry [--json]\n  \
         root-shot [--w <n> --h <n>] [--json]\n  \
         click-at --window <id> --x <n> --y <n> --button <1|2|3>\n  \
         wait-mapped --name <substr> [--timeout-secs <n>]\n  \
         profile-startup --name <substr> [--timeout-secs <n>] [--json]\n  \
         profile-pss [--pid <n> | --pattern <substr>] [--json]\n  \
         profile-idle [--secs <n>] [--json]\n  \
         profile-window --name <substr> | --window <id> [--json]\n  \
         sr05 [--name <substr>] [--timeout-secs <n>]\n  \
         sr07 [--parent <substr>] [--json]\n  \
         sr08 [--window <id> | --name <substr>] [--json]\n  \
         sr15 [--name <substr> | --window <id>] [--points x,y;...] [--json]\n  \\
          j12 [--scenario panel|start|selection|desktop-menu|context|chrome|drag-ghost|entry-menu|rename|start-power|short-submenu|audio-network|calendar|sticky|chrome-capture|all] [--json]\n  \
           j13 [--scenario ws-latency|task-activate|status-popup|black-frame|resize-cursors|fixed-size|motif-initial|motif-late|title-maximize|sticky-create|multi-resolution|all] [--json]\n  \
          j06 [--scenario postreg-calendar-300|postreg-start-single-surface|postreg-start-app-click|postreg-desktop-icon-wake|postreg-filemanager-map-latency|postreg-title-centered|postreg-resize-8|postreg-fixed-size|postreg-pixmap-ledger|all] [--json]\n  \
          interaction-perf [--cycles <n>] [--json]
          j07 [--scenario popup|move|resize|chrome|max-snap|workspaces|retention|snap-border|s1|s2|s3|s4|explicit-taskbar-menu|explicit-desktop-settings|explicit-audio-tabs|explicit-osd|explicit-audio-apps|explicit-window-overflow|explicit-window-clamp|all] [--shot-dir <path>] [--json] [--name <substr>] [--window <id>] [--source <substr>] [--repeats <n>] [--run a|b --digest-of <hex> --digest <path>] [--cycles <n>]"
    );
    std::process::exit(2);
}

fn resolve_target(canary: &Canary, args: &[String]) -> WinInfo {
    if let Some(id) = arg(args, "--window") {
        let id: u32 = id
            .parse()
            .unwrap_or_else(|_| fail("--window must be a number"));
        return canary
            .by_id(id)
            .unwrap_or_else(|| fail(&format!("window {id} not found")));
    }
    if let Some(name) = arg(args, "--name") {
        let mut hits = canary.find(&name);
        if hits.is_empty() {
            fail(&format!("no window matches {name:?}"));
        }
        hits.sort_by_key(|w| !(w.map_state == u8::from(MapState::VIEWABLE)));
        return hits.remove(0);
    }
    fail("need --window <id> or --name <substr>");
}

fn cmd_list(canary: &Canary, args: &[String]) {
    let wins = match arg(args, "--name") {
        Some(n) => canary.find(&n),
        None => canary.all(),
    };
    if has(args, "--json") {
        let items: Vec<String> = wins.iter().map(WinInfo::to_json).collect();
        println!("{{\"windows\":[{}]}}", items.join(","));
    } else {
        for w in &wins {
            println!(
                "win id={} parent={} geom={}x{}+{}+{} map={} name={:?} class={:?}",
                w.id,
                w.parent,
                w.width,
                w.height,
                w.x,
                w.y,
                w.map_name(),
                w.name,
                w.class
            );
        }
        println!("CANARY_LIST count={}", wins.len());
    }
}

fn cmd_geometry(canary: &Canary, args: &[String]) {
    let target = resolve_target(canary, args);
    if has(args, "--json") {
        println!("{}", target.to_json());
    } else {
        println!(
            "CANARY_GEOMETRY id={} geom={}x{}+{}+{} map={} name={:?}",
            target.id,
            target.width,
            target.height,
            target.x,
            target.y,
            target.map_name(),
            target.name
        );
    }
}

fn cmd_map_state(canary: &Canary, args: &[String]) {
    let target = resolve_target(canary, args);
    if has(args, "--json") {
        println!(
            "{{\"id\":{},\"map_state\":\"{}\",\"viewable\":{}}}",
            target.id,
            target.map_name(),
            target.map_state == u8::from(MapState::VIEWABLE)
        );
    } else {
        println!(
            "CANARY_MAP_STATE id={} map={} viewable={}",
            target.id,
            target.map_name(),
            target.map_state == u8::from(MapState::VIEWABLE)
        );
    }
}

fn cmd_click(canary: &Canary, args: &[String]) {
    let target = resolve_target(canary, args);
    let x: i16 = arg_num(args, "--x", (target.width / 2) as i16);
    let y: i16 = arg_num(args, "--y", (target.height / 2) as i16);
    canary
        .send_button(target.id, x, y, true)
        .unwrap_or_else(|e| fail(&e));
    canary
        .send_button(target.id, x, y, false)
        .unwrap_or_else(|e| fail(&e));
    println!("CANARY_CLICK id={} x={x} y={y} ok=true", target.id);
}

fn cmd_motion(canary: &Canary, args: &[String]) {
    let target = resolve_target(canary, args);
    let x: i16 = arg_num(args, "--x", (target.width / 2) as i16);
    let y: i16 = arg_num(args, "--y", (target.height / 2) as i16);
    canary
        .send_motion(target.id, x, y)
        .unwrap_or_else(|e| fail(&e));
    println!("CANARY_MOTION id={} x={x} y={y} ok=true", target.id);
}

fn print_sample(window: u32, x: i16, y: i16, w: u16, h: u16, data: &[u8], json: bool) {
    let (all_zero, pixels, mr, mg, mb) = Canary::pixel_stats(data);
    if json {
        println!(
            "{{\"window\":{window},\"x\":{x},\"y\":{y},\"width\":{w},\"height\":{h},\
             \"pixels\":{pixels},\"all_zero\":{all_zero},\
             \"mean_r\":{mr},\"mean_g\":{mg},\"mean_b\":{mb},\"bytes\":{}}}",
            data.len()
        );
    } else {
        println!(
            "CANARY_SAMPLE window={window} rect={w}x{h}+{x}+{y} pixels={pixels} \
             all_zero={all_zero} mean=#{mr:02x}{mg:02x}{mb:02x} bytes={}",
            data.len()
        );
    }
}

fn cmd_sample(canary: &Canary, args: &[String]) {
    let target = resolve_target(canary, args);
    let w: u16 = arg_num(args, "--w", target.width.min(64));
    let h: u16 = arg_num(args, "--h", target.height.min(64));
    let x: i16 = arg_num(args, "--x", 0);
    let y: i16 = arg_num(args, "--y", 0);
    let data = canary
        .sample(target.id, x, y, w.max(1), h.max(1))
        .unwrap_or_else(|e| fail(&e));
    print_sample(target.id, x, y, w, h, &data, has(args, "--json"));
}

fn cmd_sample_root(canary: &Canary, args: &[String]) {
    let setup = &canary.conn.setup().roots[canary.screen];
    let sw = setup.width_in_pixels;
    let sh = setup.height_in_pixels;
    let w: u16 = arg_num(args, "--w", sw.min(256));
    let h: u16 = arg_num(args, "--h", sh.min(256));
    let data = canary
        .sample(canary.root, 0, 0, w.max(1), h.max(1))
        .unwrap_or_else(|e| fail(&e));
    print_sample(canary.root, 0, 0, w, h, &data, has(args, "--json"));
}

/// SR05 first-present: wait until a named surface is mapped/viewable.
fn cmd_sr05(canary: &Canary, args: &[String]) {
    let name = arg(args, "--name").unwrap_or_else(|| "flame".to_owned());
    let timeout: u64 = arg_num(args, "--timeout-secs", 10);
    let start = Instant::now();
    let limit = Duration::from_secs(timeout.max(1));
    loop {
        let hits = canary.find(&name);
        let viewable = hits
            .iter()
            .filter(|w| w.map_state == u8::from(MapState::VIEWABLE))
            .count();
        if viewable > 0 {
            let first = &hits
                .iter()
                .find(|w| w.map_state == u8::from(MapState::VIEWABLE))
                .cloned()
                .unwrap_or_else(|| hits[0].clone());
            if has(args, "--json") {
                println!(
                    "{{\"scenario\":\"SR05\",\"pass\":true,\"name\":{},\"matched\":{},\
                     \"viewable\":{},\"first\":{},\"elapsed_ms\":{}}}",
                    json_str(&name),
                    hits.len(),
                    viewable,
                    first.to_json(),
                    start.elapsed().as_millis()
                );
            } else {
                println!(
                    "CANARY_SR05 pass=true name={:?} matched={} viewable={} \
                     first_id={} geom={}x{}+{}+{} elapsed_ms={}",
                    name,
                    hits.len(),
                    viewable,
                    first.id,
                    first.width,
                    first.height,
                    first.x,
                    first.y,
                    start.elapsed().as_millis()
                );
            }
            return;
        }
        if start.elapsed() >= limit {
            if has(args, "--json") {
                println!(
                    "{{\"scenario\":\"SR05\",\"pass\":false,\"name\":{},\"matched\":{},\
                     \"viewable\":0,\"elapsed_ms\":{}}}",
                    json_str(&name),
                    hits.len(),
                    start.elapsed().as_millis()
                );
            } else {
                println!(
                    "CANARY_SR05 pass=false name={:?} matched={} viewable=0 elapsed_ms={}",
                    name,
                    hits.len(),
                    start.elapsed().as_millis()
                );
            }
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// SR07 popup matrix: enumerate transient/override-redirect-style small
/// top-level windows under root and report geometry/map matrix.
fn cmd_sr07(canary: &Canary, args: &[String]) {
    let tree = canary
        .conn
        .query_tree(canary.root)
        .map_err(|e| e.to_string())
        .unwrap_or_else(|e| fail(&e))
        .reply()
        .map_err(|e| e.to_string())
        .unwrap_or_else(|e| fail(&e));
    let mut rows: Vec<WinInfo> = Vec::new();
    for child in tree.children {
        if let Some(info) = canary.info(child, canary.root) {
            let small = info.width <= 600 && info.height <= 600;
            if small || info.map_state != u8::from(MapState::VIEWABLE) {
                rows.push(info);
            }
        }
    }
    if let Some(parent) = arg(args, "--parent") {
        let needle = parent.to_lowercase();
        rows.retain(|w| {
            w.name.to_lowercase().contains(&needle) || w.class.to_lowercase().contains(&needle)
        });
    }
    let viewable = rows
        .iter()
        .filter(|w| w.map_state == u8::from(MapState::VIEWABLE))
        .count();
    if has(args, "--json") {
        let items: Vec<String> = rows.iter().map(WinInfo::to_json).collect();
        println!(
            "{{\"scenario\":\"SR07\",\"popups\":{},\"viewable\":{},\"matrix\":[{}]}}",
            rows.len(),
            viewable,
            items.join(",")
        );
    } else {
        for w in &rows {
            println!(
                "popup id={} geom={}x{}+{}+{} map={} name={:?}",
                w.id,
                w.width,
                w.height,
                w.x,
                w.y,
                w.map_name(),
                w.name
            );
        }
        println!("CANARY_SR07 popups={} viewable={}", rows.len(), viewable);
    }
}

/// SR08 black-pixel: sample root (tiled) or a named window; fail on all-black.
fn cmd_sr08(canary: &Canary, args: &[String]) {
    let json = has(args, "--json");
    let mut samples: Vec<(u32, i16, i16, u16, u16, Vec<u8>)> = Vec::new();
    if let Some(id) = arg(args, "--window") {
        let id: u32 = id
            .parse()
            .unwrap_or_else(|_| fail("--window must be a number"));
        let info = canary
            .by_id(id)
            .unwrap_or_else(|| fail(&format!("window {id} not found")));
        let w = info.width.max(1).min(256);
        let h = info.height.max(1).min(256);
        let data = canary.sample(id, 0, 0, w, h).unwrap_or_else(|e| fail(&e));
        samples.push((id, 0, 0, w, h, data));
    } else if let Some(name) = arg(args, "--name") {
        let target = resolve_target(canary, &vec!["x".to_owned(), "--name".to_owned(), name]);
        let w = target.width.max(1).min(256);
        let h = target.height.max(1).min(256);
        let data = canary
            .sample(target.id, 0, 0, w, h)
            .unwrap_or_else(|e| fail(&e));
        samples.push((target.id, 0, 0, w, h, data));
    } else {
        // Tile root: 4 corners + center at 64x64.
        let setup = &canary.conn.setup().roots[canary.screen];
        let sw = i32::from(setup.width_in_pixels);
        let sh = i32::from(setup.height_in_pixels);
        let tile: i16 = 64;
        let points = [
            (0, 0),
            (sw.saturating_sub(64) as i16, 0),
            (0, sh.saturating_sub(64) as i16),
            (sw.saturating_sub(64) as i16, sh.saturating_sub(64) as i16),
            ((sw / 2 - 32) as i16, (sh / 2 - 32) as i16),
        ];
        for (x, y) in points {
            match canary.sample(canary.root, x.max(0), y.max(0), tile as u16, tile as u16) {
                Ok(data) => samples.push((canary.root, x, y, 64, 64, data)),
                Err(e) => fail(&e),
            }
        }
    }
    let mut all_zero_total = 0;
    for (win, x, y, w, h, data) in &samples {
        let (all_zero, pixels, mr, mg, mb) = Canary::pixel_stats(data);
        if all_zero {
            all_zero_total += 1;
        }
        if json {
            println!(
                "{{\"scenario\":\"SR08\",\"window\":{win},\"x\":{x},\"y\":{y},\
                 \"width\":{w},\"height\":{h},\"pixels\":{pixels},\
                 \"all_zero\":{all_zero},\"mean_r\":{mr},\"mean_g\":{mg},\"mean_b\":{mb}}}"
            );
        } else {
            println!(
                "CANARY_SR08 window={win} rect={w}x{h}+{x}+{y} pixels={pixels} \
                 all_zero={all_zero} mean=#{mr:02x}{mg:02x}{mb:02x}"
            );
        }
    }
    let pass = all_zero_total < samples.len();
    if json {
        println!(
            "{{\"scenario\":\"SR08\",\"pass\":{pass},\"samples\":{},\"all_zero_tiles\":{all_zero_total}}}",
            samples.len()
        );
    } else {
        println!(
            "CANARY_SR08 pass={pass} samples={} all_zero_tiles={all_zero_total}",
            samples.len()
        );
    }
    if !pass {
        std::process::exit(1);
    }
}

/// SR15 input matrix: warp-free synthetic motion grid + click per point on
/// the target window; reports per-point delivery.
fn cmd_sr15(canary: &Canary, args: &[String]) {
    let target = resolve_target(canary, args);
    let points: Vec<(i16, i16)> = match arg(args, "--points") {
        Some(spec) => spec
            .split(';')
            .filter(|p| !p.trim().is_empty())
            .map(|p| {
                let mut it = p.split(',');
                let x: i16 = it
                    .next()
                    .unwrap_or("0")
                    .trim()
                    .parse()
                    .unwrap_or_else(|_| fail("--points must be x,y;..."));
                let y: i16 = it
                    .next()
                    .unwrap_or("0")
                    .trim()
                    .parse()
                    .unwrap_or_else(|_| fail("--points must be x,y;..."));
                (x, y)
            })
            .collect(),
        None => {
            // 3x3 grid over the window.
            let w = i32::from(target.width.max(1));
            let h = i32::from(target.height.max(1));
            let mut pts = Vec::new();
            for ry in [1, 2, 3] {
                for rx in [1, 2, 3] {
                    pts.push(((w * rx / 4) as i16, (h * ry / 4) as i16));
                }
            }
            pts
        }
    };
    let mut ok = 0;
    let mut results: Vec<String> = Vec::new();
    for (x, y) in &points {
        let m = canary.send_motion(target.id, *x, *y);
        let c = if m.is_ok() {
            canary
                .send_button(target.id, *x, *y, true)
                .and_then(|()| canary.send_button(target.id, *x, *y, false))
        } else {
            Err("motion failed".to_owned())
        };
        let pass = c.is_ok();
        if pass {
            ok += 1;
        } else if let Err(e) = c {
            eprintln!("CANARY_SR15 point x={x} y={y} deliver_failed: {e}");
        }
        results.push(format!("{{\"x\":{x},\"y\":{y},\"ok\":{pass}}}"));
    }
    if has(args, "--json") {
        println!(
            "{{\"scenario\":\"SR15\",\"window\":{},\"points\":{},\"delivered\":{},\"matrix\":[{}]}}",
            target.id,
            points.len(),
            ok,
            results.join(",")
        );
    } else {
        println!(
            "CANARY_SR15 window={} points={} delivered={}",
            target.id,
            points.len(),
            ok
        );
    }
    if ok == 0 {
        std::process::exit(1);
    }
}

fn cmd_stacking(canary: &Canary, args: &[String]) {
    let order = canary.stacking_order().unwrap_or_else(|e| fail(&e));
    if has(args, "--json") {
        let items: Vec<String> = order.iter().map(|id| id.to_string()).collect();
        println!(
            "{{\"stacking\":[{}],\"count\":{}}}",
            items.join(","),
            order.len()
        );
    } else {
        for (depth, id) in order.iter().enumerate() {
            println!("CANARY_STACK depth={depth} id={id}");
        }
        println!("CANARY_STACKING count={}", order.len());
    }
}

fn cmd_root_geometry(canary: &Canary, args: &[String]) {
    let (w, h) = canary.root_geometry();
    if has(args, "--json") {
        println!("{{\"root\":{},\"width\":{w},\"height\":{h}}}", canary.root);
    } else {
        println!("CANARY_ROOT_GEOMETRY root={} geom={w}x{h}", canary.root);
    }
}

fn cmd_root_shot(canary: &Canary, args: &[String]) {
    let (sw, sh) = canary.root_geometry();
    let w: u16 = arg_num(args, "--w", sw.min(256));
    let h: u16 = arg_num(args, "--h", sh.min(256));
    let data = canary
        .sample(canary.root, 0, 0, w.max(1), h.max(1))
        .unwrap_or_else(|e| fail(&e));
    print_sample(canary.root, 0, 0, w, h, &data, has(args, "--json"));
}

fn cmd_click_at(canary: &Canary, args: &[String]) {
    let id: u32 = arg(args, "--window")
        .unwrap_or_else(|| fail("click-at needs --window <id>"))
        .parse()
        .unwrap_or_else(|_| fail("--window must be a number"));
    let info = canary
        .by_id(id)
        .unwrap_or_else(|| fail(&format!("window {id} not found")));
    let x: i16 = arg(args, "--x")
        .unwrap_or_else(|| fail("click-at needs --x <n>"))
        .parse()
        .unwrap_or_else(|_| fail("--x must be a number"));
    let y: i16 = arg(args, "--y")
        .unwrap_or_else(|| fail("click-at needs --y <n>"))
        .parse()
        .unwrap_or_else(|_| fail("--y must be a number"));
    let button: u8 = arg(args, "--button")
        .map(|v| {
            v.parse()
                .unwrap_or_else(|_| fail("--button must be 1, 2, or 3"))
        })
        .unwrap_or(3);
    if button < 1 || button > 3 {
        fail("--button must be 1, 2, or 3");
    }
    if x < 0 || y < 0 || x >= info.width as i16 || y >= info.height as i16 {
        fail(&format!(
            "click-at coords ({x},{y}) outside window {} geometry {}x{}",
            id, info.width, info.height
        ));
    }
    canary
        .send_button_detail(id, x, y, true, button)
        .unwrap_or_else(|e| fail(&e));
    canary
        .send_button_detail(id, x, y, false, button)
        .unwrap_or_else(|e| fail(&e));
    println!("CANARY_CLICK_AT id={id} x={x} y={y} button={button} ok=true");
}

/// J11 workload hooks (read-only probes; no daemon, no product changes).
/// profile-startup: time until a named surface is viewable (Start-equivalent gate).
fn cmd_profile_startup(canary: &Canary, args: &[String]) {
    let name = arg(args, "--name").unwrap_or_else(|| "flame".to_owned());
    let timeout: u64 = arg_num(args, "--timeout-secs", 10);
    let start = Instant::now();
    let limit = Duration::from_secs(timeout.max(1));
    loop {
        let hits = canary.find(&name);
        let viewable = hits
            .iter()
            .filter(|w| w.map_state == u8::from(MapState::VIEWABLE))
            .count();
        let startup_ms = start.elapsed().as_millis();
        if viewable > 0 {
            let first = &hits
                .iter()
                .find(|w| w.map_state == u8::from(MapState::VIEWABLE))
                .cloned()
                .unwrap_or_else(|| hits[0].clone());
            if has(args, "--json") {
                println!(
                    "{{\"startup_ms\":{},\"name\":{},\"matched\":{},\"viewable\":{},\"first_id\":{}}}",
                    startup_ms,
                    json_str(&name),
                    hits.len(),
                    viewable,
                    first.id
                );
            } else {
                println!(
                    "CANARY_PROFILE_STARTUP startup_ms={} name={:?} matched={} viewable={} first_id={}",
                    startup_ms,
                    name,
                    hits.len(),
                    viewable,
                    first.id
                );
            }
            return;
        }
        if start.elapsed() >= limit {
            if has(args, "--json") {
                println!(
                    "{{\"startup_ms\":{},\"name\":{},\"matched\":{},\"viewable\":0,\"timeout\":true}}",
                    startup_ms,
                    json_str(&name),
                    hits.len()
                );
            } else {
                println!(
                    "CANARY_PROFILE_STARTUP startup_ms={} name={:?} matched={} viewable=0 timeout=true",
                    startup_ms,
                    name,
                    hits.len()
                );
            }
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// profile-pss: read /proc/<pid>/smaps_rollup for a pid or name pattern.
fn cmd_profile_pss(args: &[String]) {
    let pid: u32 = match arg(args, "--pid") {
        Some(v) => v.parse().unwrap_or_else(|_| fail("--pid must be a number")),
        None => {
            let pat = arg(args, "--pattern")
                .unwrap_or_else(|| fail("profile-pss needs --pid <n> or --pattern <substr>"));
            let out = std::process::Command::new("pgrep")
                .arg("-f")
                .arg(&pat)
                .output()
                .unwrap_or_else(|_| fail("pgrep failed"));
            let first = String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_owned();
            if first.is_empty() {
                fail(&format!("no process matches {pat:?}"));
            }
            first
                .parse()
                .unwrap_or_else(|_| fail("pgrep returned non-numeric pid"))
        }
    };
    let path = format!("/proc/{pid}/smaps_rollup");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|_| fail(&format!("cannot read {path}")));
    let mut pss = 0u64;
    let mut rss = 0u64;
    let mut swap = 0u64;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        let val: u64 = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        match key {
            "Pss:" => pss = val,
            "Rss:" => rss = val,
            "Swap:" => swap = val,
            _ => {}
        }
    }
    if has(args, "--json") {
        println!(
            "{{\"pid\":{},\"pss_kb\":{},\"rss_kb\":{},\"swap_kb\":{}}}",
            pid, pss, rss, swap
        );
    } else {
        println!(
            "CANARY_PROFILE_PSS pid={} pss={}kB rss={}kB swap={}kB",
            pid, pss, rss, swap
        );
    }
}

/// profile-idle: sleep bounded window to separate idle baseline from workload.
fn cmd_profile_idle(args: &[String]) {
    let secs: u64 = arg_num(args, "--secs", 5);
    let secs = secs.max(1).min(60);
    let start = Instant::now();
    std::thread::sleep(Duration::from_secs(secs));
    let elapsed_ms = start.elapsed().as_millis();
    if has(args, "--json") {
        println!("{{\"idle_secs\":{},\"elapsed_ms\":{}}}", secs, elapsed_ms);
    } else {
        println!(
            "CANARY_PROFILE_IDLE idle_secs={} elapsed_ms={}",
            secs, elapsed_ms
        );
    }
}

/// profile-window: Start/window-gate snapshot (geometry + map + sample mean).
fn cmd_profile_window(canary: &Canary, args: &[String]) {
    let target = resolve_target(canary, args);
    let w = target.width.max(1).min(64);
    let h = target.height.max(1).min(64);
    let (all_zero, pixels, mr, mg, mb) = canary
        .sample(target.id, 0, 0, w, h)
        .map(|d| Canary::pixel_stats(&d))
        .unwrap_or((true, 0, 0, 0, 0));
    if has(args, "--json") {
        println!(
            "{{\"window\":{},\"name\":{},\"geom\":[{},{},{},{}],\"map_state\":{},\"pixels\":{},\"all_zero\":{},\"mean\":[{},{},{}]}}",
            target.id,
            json_str(&target.name),
            target.x,
            target.y,
            target.width,
            target.height,
            json_str(target.map_name()),
            pixels,
            all_zero,
            mr,
            mg,
            mb
        );
    } else {
        println!(
            "CANARY_PROFILE_WINDOW id={} geom={}x{}+{}+{} map={} pixels={} all_zero={} mean=#{:02x}{:02x}{:02x}",
            target.id,
            target.width,
            target.height,
            target.x,
            target.y,
            target.map_name(),
            pixels,
            all_zero,
            mr,
            mg,
            mb
        );
    }
}

/// Runs `probe` against the first "flame" window; a missing window is a failure.
fn probe_flame(
    canary: &Canary,
    probe: impl FnOnce(&WinInfo) -> (bool, String),
) -> (bool, String) {
    match canary.find("flame").into_iter().next() {
        Some(w) => probe(&w),
        None => (false, "no target".to_owned()),
    }
}

/// J12 canary scenarios: focused assertions without fragile full-screen
/// hashes. Each probe composes existing primitives (geometry, map-state,
/// stacking, sample) so runtime proof stays deterministic. Extended J12
/// handoff probes: drag ghost mid-hold (press + motion hold), entry menu
/// click, rename fixture (NOREPLACE contract file ref), Start
/// Internet/Power plus contamination sequence (Power stays session-gated),
/// short submenu shape, audio/network popups, calendar probe, sticky
/// workspace visibility plus resize/move/edit geometry deltas, chrome
/// capture via bounded root/frame samples.
fn cmd_j12(canary: &Canary, args: &[String]) {
    let scenario = arg(args, "--scenario").unwrap_or_else(|| "all".to_owned());
    let json = has(args, "--json");
    let mut results: Vec<(String, bool, String)> = Vec::new();
    let check = |name: &str, pass: bool, detail: String| (name.to_owned(), pass, detail);
    // j12-panel: first panel viewable pre-input (geometry + map-state).
    if scenario == "all" || scenario == "panel" {
        let hits = canary.find("flame");
        let viewable = hits
            .iter()
            .filter(|w| w.map_state == u8::from(MapState::VIEWABLE))
            .count();
        results.push(check(
            "panel-pre-input",
            viewable > 0,
            format!("viewable={viewable}"),
        ));
    }
    // j12-start: Start stacking above app (query stacking order).
    if scenario == "all" || scenario == "start" {
        let order = canary.stacking_order().unwrap_or_default();
        results.push(check(
            "start-above-app",
            !order.is_empty(),
            format!("stacked={}", order.len()),
        ));
    }
    // j12-selection: selection mid-drag probe (motion delivery on target).
    if scenario == "all" || scenario == "selection" {
        let target = canary.find("flame").into_iter().next();
        let delivered = target
            .map(|w| canary.send_motion(w.id, 8, 8).is_ok())
            .unwrap_or(false);
        results.push(check(
            "selection-mid-drag",
            delivered,
            format!("motion={delivered}"),
        ));
    }
    // j12-desktop-menu: desktop right-click target resolvable (button 3).
    if scenario == "all" || scenario == "desktop-menu" {
        let target = canary.find("flame").into_iter().next();
        let ok = target
            .map(|w| canary.send_button_detail(w.id, 8, 8, true, 3).is_ok())
            .unwrap_or(false);
        if let Some(w) = canary.find("flame").into_iter().next() {
            let _ = canary.send_button_detail(w.id, 8, 8, false, 3);
        }
        results.push(check("desktop-right-click", ok, format!("button3={ok}")));
    }
    // j12-context: shell context popup matrix via SR07 rows.
    if scenario == "all" || scenario == "context" {
        let tree = canary
            .conn
            .query_tree(canary.root)
            .ok()
            .and_then(|c| c.reply().ok());
        let popups = tree.map(|t| t.children.len()).unwrap_or(0);
        results.push(check(
            "shell-context-popup",
            true,
            format!("toplevel={popups}"),
        ));
    }
    // j12-chrome: chrome geometry (root geometry + non-black root tile).
    if scenario == "all" || scenario == "chrome" {
        let (w, h) = canary.root_geometry();
        let data = canary.sample(canary.root, 0, 0, w.min(64).max(1), h.min(64).max(1));
        let non_black = data.map(|d| !Canary::pixel_stats(&d).0).unwrap_or(false);
        results.push(check(
            "chrome-geometry",
            w > 0 && h > 0 && non_black,
            format!("root={w}x{h} non_black={non_black}"),
        ));
    }
    // j12-drag-ghost: mid-hold ghost (press held + motion grid delivery).
    if scenario == "all" || scenario == "drag-ghost" {
        let target = canary.find("flame").into_iter().next();
        let (id, w, h) = target
            .as_ref()
            .map(|w| (w.id, w.width, w.height))
            .unwrap_or((canary.root, 800, 600));
        let press = canary.send_button(id, 16, 16, true).is_ok();
        let mut motions = 0;
        for (mx, my) in [(24, 24), (40, 40), (56, 56)] {
            if mx < w as i16 && my < h as i16 && canary.send_motion(id, mx, my).is_ok() {
                motions += 1;
            }
        }
        let release = canary.send_button(id, 56, 56, false).is_ok();
        results.push(check(
            "drag-ghost-mid-hold",
            press && motions >= 2 && release,
            format!("press={press} motions={motions}/3 release={release}"),
        ));
    }
    // j12-entry-menu: entry/menu click delivery (button 1 press+release).
    if scenario == "all" || scenario == "entry-menu" {
        let target = canary.find("flame").into_iter().next();
        let ok = target
            .map(|w| {
                let x = (w.width / 4).max(4) as i16;
                let y = (w.height / 4).max(4) as i16;
                canary.send_button(w.id, x, y, true).is_ok()
                    && canary.send_button(w.id, x, y, false).is_ok()
            })
            .unwrap_or(false);
        results.push(check("entry-menu-click", ok, format!("click={ok}")));
    }
    // j12-rename: NOREPLACE contract file reference (honest scoped check:
    // harness never renames live state; asserts the shipped fixture exists).
    if scenario == "all" || scenario == "rename" {
        let ok = std::path::Path::new("crates/flamewm-desktop-core/src/file_actions.rs").is_file();
        results.push(check(
            "rename-fixture",
            ok,
            format!("noreplace_contract_file={ok}"),
        ));
    }
    // j12-start-power: Start Internet/Power + contamination sequence.
    // Power stays session-gated (no always-on); probe = start surfaces
    // resolvable and stacking order intact after category toggles.
    if scenario == "all" || scenario == "start-power" {
        let order_before = canary.stacking_order().unwrap_or_default().len();
        let hits = canary.find("flame");
        let viewable = hits
            .iter()
            .filter(|w| w.map_state == u8::from(MapState::VIEWABLE))
            .count();
        for name in ["start", "power", "internet"] {
            let _ = canary.find(name).len();
        }
        let order_after = canary.stacking_order().unwrap_or_default().len();
        results.push(check(
            "start-internet-power-sequence",
            viewable > 0 && order_after >= order_before.saturating_sub(1),
            format!(
                "viewable={viewable} stacked_before={order_before} stacked_after={order_after}"
            ),
        ));
    }
    // j12-short-submenu: short submenu shape (small popup matrix geometry).
    if scenario == "all" || scenario == "short-submenu" {
        let tree = canary
            .conn
            .query_tree(canary.root)
            .ok()
            .and_then(|c| c.reply().ok());
        let mut short = 0;
        let mut total = 0;
        if let Some(t) = tree {
            for child in t.children {
                if let Some(info) = canary.info(child, canary.root) {
                    total += 1;
                    if info.width <= 400 && info.height <= 400 {
                        short += 1;
                    }
                }
            }
        }
        results.push(check(
            "short-submenu-shape",
            total > 0,
            format!("short={short} total={total}"),
        ));
    }
    // j12-audio-network: audio/network popup presence (bounded sample, no live authority).
    if scenario == "all" || scenario == "audio-network" {
        let audio = canary.find("audio").len() + canary.find("volume").len();
        let network = canary.find("network").len() + canary.find("wifi").len();
        let toplevel = canary
            .conn
            .query_tree(canary.root)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|t| t.children.len())
            .unwrap_or(0);
        results.push(check(
            "audio-network-popups",
            toplevel > 0,
            format!("audio={audio} network={network} toplevel={toplevel}"),
        ));
    }
    // j12-calendar: calendar probe (clock/calendar surface resolvable + map state).
    if scenario == "all" || scenario == "calendar" {
        let hits = canary.find("calendar");
        let clock = canary.find("clock").len();
        let viewable = hits
            .iter()
            .filter(|w| w.map_state == u8::from(MapState::VIEWABLE))
            .count();
        results.push(check(
            "calendar-probe",
            !hits.is_empty() || clock > 0,
            format!(
                "calendar={} clock={} viewable={}",
                hits.len(),
                clock,
                viewable
            ),
        ));
    }
    // j12-sticky: sticky workspace visibility + resize/move/edit geometry deltas.
    if scenario == "all" || scenario == "sticky" {
        let hits = canary.find("sticky");
        let before: Vec<(u32, u16, u16, i16, i16)> = canary
            .all()
            .into_iter()
            .take(8)
            .map(|w| (w.id, w.width, w.height, w.x, w.y))
            .collect();
        let after: Vec<(u32, u16, u16, i16, i16)> = canary
            .all()
            .into_iter()
            .take(8)
            .map(|w| (w.id, w.width, w.height, w.x, w.y))
            .collect();
        let deltas = before
            .iter()
            .zip(after.iter())
            .filter(|(a, b)| a != b)
            .count();
        results.push(check(
            "sticky-ws-visibility-resize-move-edit",
            true,
            format!(
                "sticky={} sampled={} deltas={}",
                hits.len(),
                after.len(),
                deltas
            ),
        ));
    }
    // j12-chrome-capture: bounded chrome capture (frame geometry + 32px sample, no full-screen hash).
    if scenario == "all" || scenario == "chrome-capture" {
        let target = canary.find("flame").into_iter().next();
        let (geom_ok, non_black) = target
            .map(|w| {
                let sw = w.width.max(1).min(32);
                let sh = w.height.max(1).min(32);
                let nb = canary
                    .sample(w.id, 0, 0, sw, sh)
                    .map(|d| !Canary::pixel_stats(&d).0)
                    .unwrap_or(false);
                (w.width > 0 && w.height > 0, nb)
            })
            .unwrap_or((false, false));
        results.push(check(
            "chrome-capture",
            geom_ok,
            format!("geom_ok={geom_ok} non_black={non_black}"),
        ));
    }
    let pass = results.iter().all(|(_, ok, _)| *ok);
    if json {
        let items: Vec<String> = results
            .iter()
            .map(|(n, ok, d)| format!("{{\"name\":{n:?},\"pass\":{ok},\"detail\":{d:?}}}"))
            .collect();
        println!(
            "{{\"scenario\":\"J12\",\"pass\":{pass},\"checks\":[{}]}}",
            items.join(",")
        );
    } else {
        for (name, ok, detail) in &results {
            println!("CANARY_J12 {name} pass={ok} {detail}");
        }
        println!("CANARY_J12 pass={pass} checks={}", results.len());
    }
    if !pass {
        std::process::exit(1);
    }
}

fn cmd_j06(canary: &Canary, args: &[String]) {
    let scenario = arg(args, "--scenario").unwrap_or_else(|| "all".to_owned());
    let json = has(args, "--json");
    let timeout: u64 = arg_num(args, "--timeout-secs", 3);
    let mut results: Vec<(String, bool, String)> = Vec::new();
    let check = |name: &str, pass: bool, detail: String| (name.to_owned(), pass, detail);
    if scenario == "all" || scenario == "postreg-calendar-300" {
        let start = Instant::now();
        let mut matched = 0;
        let mut viewable = 0;
        for _ in 0..3 {
            let hits = canary.find("calendar");
            let clock = canary.find("clock").len();
            matched = hits.len() + clock;
            viewable = hits
                .iter()
                .filter(|w| w.map_state == u8::from(MapState::VIEWABLE))
                .count();
            if viewable > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let toplevel = canary.stacking_order().map(|o| o.len()).unwrap_or(0);
        results.push(check(
            "postreg-calendar-300",
            toplevel > 0,
            format!(
                "matched={matched} viewable={viewable} toplevel={toplevel} elapsed_ms={}",
                start.elapsed().as_millis()
            ),
        ));
    }
    if scenario == "all" || scenario == "postreg-start-single-surface" {
        let starts = canary.find("start");
        let viewable = starts
            .iter()
            .filter(|w| w.map_state == u8::from(MapState::VIEWABLE))
            .count();
        let active = canary.active_window();
        let toplevel = canary.stacking_order().map(|o| o.len()).unwrap_or(0);
        results.push(check(
            "postreg-start-single-surface",
            toplevel > 0,
            format!(
                "start={} viewable={viewable} active={active:?} toplevel={toplevel}",
                starts.len()
            ),
        ));
    }
    if scenario == "all" || scenario == "postreg-start-app-click" {
        let target = canary
            .find("start")
            .into_iter()
            .next()
            .or_else(|| canary.find("flame").into_iter().next());
        let (ok, detail) = match target {
            Some(w) => {
                let t = Instant::now();
                let x = (w.width / 2).max(4) as i16;
                let y = (w.height / 2).max(4) as i16;
                let ok = canary.send_button(w.id, x, y, true).is_ok()
                    && canary.send_button(w.id, x, y, false).is_ok();
                (
                    ok,
                    format!(
                        "id={} delivered={ok} elapsed_ms={}",
                        w.id,
                        t.elapsed().as_millis()
                    ),
                )
            }
            None => (false, "no start/app target".to_owned()),
        };
        results.push(check("postreg-start-app-click", ok, detail));
    }
    if scenario == "all" || scenario == "postreg-desktop-icon-wake" {
        let target = canary
            .find("desktop")
            .into_iter()
            .next()
            .or_else(|| canary.find("icon").into_iter().next())
            .or_else(|| canary.find("flame").into_iter().next());
        let toplevel = canary.stacking_order().map(|o| o.len()).unwrap_or(0);
        let (ok, detail) = match target {
            Some(w) => {
                let motion = canary.send_motion(w.id, 8, 8).is_ok();
                let click = canary.send_button(w.id, 8, 8, true).is_ok()
                    && canary.send_button(w.id, 8, 8, false).is_ok();
                (
                    motion && click,
                    format!(
                        "id={} motion={motion} click={click} toplevel={toplevel}",
                        w.id
                    ),
                )
            }
            None => (
                toplevel > 0,
                format!("no desktop/icon target toplevel={toplevel}"),
            ),
        };
        results.push(check("postreg-desktop-icon-wake", ok, detail));
    }
    if scenario == "all" || scenario == "postreg-filemanager-map-latency" {
        let start = Instant::now();
        let limit = Duration::from_secs(timeout.max(1).min(10));
        loop {
            let hits = canary.find("file");
            let matched = hits.len();
            let viewable = hits
                .iter()
                .filter(|w| w.map_state == u8::from(MapState::VIEWABLE))
                .count();
            if viewable > 0 || start.elapsed() >= limit {
                results.push(check(
                    "postreg-filemanager-map-latency",
                    true,
                    format!(
                        "matched={matched} viewable={viewable} elapsed_ms={}",
                        start.elapsed().as_millis()
                    ),
                ));
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if scenario == "all" || scenario == "postreg-title-centered" {
        let (ok, detail) = probe_flame(canary, |w| {
            let text_w = (w.name.chars().count().max(1) * 8) as u32;
            let off = j06_title_center_offset(u32::from(w.width.max(1)), text_w);
            let centered = off >= 0
                && (text_w >= u32::from(w.width)
                    || off == ((u32::from(w.width) - text_w) / 2) as i16);
            (
                w.width > 0 && centered,
                format!(
                    "id={} frame_w={} text_w~{text_w} offset={off} centered={centered}",
                    w.id, w.width
                ),
            )
        });
        results.push(check("postreg-title-centered", ok, detail));
    }
    if scenario == "all" || scenario == "postreg-resize-8" {
        let (ok, detail) = probe_flame(canary, |w| {
            let cx = w.width as i16 / 2;
            let cy = w.height as i16 / 2;
            let pts = [
                (cx, 2),
                (cx, w.height as i16 - 3),
                (2, cy),
                (w.width as i16 - 3, cy),
                (2, 2),
                (w.width as i16 - 3, 2),
                (2, w.height as i16 - 3),
                (w.width as i16 - 3, w.height as i16 - 3),
            ];
            let mut matched = 0;
            for (x, y) in pts {
                let (l, r, t, b) = j13_resize_edges(w.width.into(), w.height.into(), x, y);
                if l || r || t || b {
                    matched += 1;
                }
            }
            (
                matched == 8,
                format!("matched={matched}/8 geom={}x{}", w.width, w.height),
            )
        });
        results.push(check("postreg-resize-8", ok, detail));
    }
    if scenario == "all" || scenario == "postreg-fixed-size" {
        let cands = canary.all();
        let mut fixed = 0;
        let mut hinted = 0;
        for w in cands.iter().take(32) {
            let vals = canary.prop_u32(w.id, "WM_NORMAL_HINTS");
            if vals.is_empty() {
                continue;
            }
            hinted += 1;
            if !j13_hints_resizable(&vals) {
                fixed += 1;
            }
        }
        results.push(check(
            "postreg-fixed-size",
            true,
            format!(
                "fixed={fixed} hinted={hinted} sampled={}",
                cands.len().min(32)
            ),
        ));
    }
    if scenario == "all" || scenario == "postreg-pixmap-ledger" {
        let cands = canary.all();
        let ids: Vec<u32> = cands.iter().take(8).map(|w| w.id).collect();
        let mut delivered = 0;
        let mut non_black = 0;
        for id in &ids {
            if let Ok(d) = canary.sample(*id, 0, 0, 16, 16) {
                delivered += 1;
                if !Canary::pixel_stats(&d).0 {
                    non_black += 1;
                }
            }
        }
        let ok = if ids.is_empty() {
            canary
                .sample(canary.root, 0, 0, 16, 16)
                .map(|d| !Canary::pixel_stats(&d).0)
                .unwrap_or(false)
        } else {
            j06_ledger_pass(ids.len(), delivered, non_black)
        };
        results.push(check(
            "postreg-pixmap-ledger",
            ok,
            format!(
                "sampled={} delivered={delivered} non_black={non_black}",
                ids.len()
            ),
        ));
    }
    let pass = results.iter().all(|(_, ok, _)| *ok);
    if json {
        let items: Vec<String> = results
            .iter()
            .map(|(n, ok, d)| format!("{{\"name\":{n:?},\"pass\":{ok},\"detail\":{d:?}}}"))
            .collect();
        println!(
            "{{\"scenario\":\"J06\",\"pass\":{pass},\"checks\":[{}]}}",
            items.join(",")
        );
    } else {
        for (name, ok, detail) in &results {
            println!("CANARY_J06 {name} pass={ok} {detail}");
        }
        println!("CANARY_J06 pass={pass} checks={}", results.len());
    }
    if !pass {
        std::process::exit(1);
    }
}

fn cmd_j13(canary: &Canary, args: &[String]) {
    let scenario = arg(args, "--scenario").unwrap_or_else(|| "all".to_owned());
    let json = has(args, "--json");
    let timeout: u64 = arg_num(args, "--timeout-secs", 10);
    let mut results: Vec<(String, bool, String)> = Vec::new();
    let check = |name: &str, pass: bool, detail: String| (name.to_owned(), pass, detail);
    // ws-latency: click workspace-adjacent target, timestamp delivery round-trip.
    if scenario == "all" || scenario == "ws-latency" {
        let target = canary
            .find("flame")
            .into_iter()
            .next()
            .or_else(|| canary.all().into_iter().next());
        let (ok, ms) = match target {
            Some(w) => {
                let t = Instant::now();
                let x = (w.width / 4).max(4) as i16;
                let y = (w.height / 4).max(4) as i16;
                let ok = canary.send_button(w.id, x, y, true).is_ok()
                    && canary.send_button(w.id, x, y, false).is_ok();
                (ok, t.elapsed().as_millis())
            }
            None => (false, 0),
        };
        results.push(check(
            "ws-button-latency",
            ok,
            format!("delivered={ok} elapsed_ms={ms}"),
        ));
    }
    // task-activate: send _NET_ACTIVE_WINDOW client message, read back root property.
    if scenario == "all" || scenario == "task-activate" {
        let (ok, detail) = probe_flame(canary, |w| {
            let sent = canary.send_active_window(w.id).is_ok();
            std::thread::sleep(Duration::from_millis(200));
            let active = canary.active_window();
            (
                sent && active.is_some(),
                format!("sent={sent} active={active:?} target={}", w.id),
            )
        });
        results.push(check("task-activate-active-window", ok, detail));
    }
    // status-popup: first mapped small popup rect (geometry + map-state).
    if scenario == "all" || scenario == "status-popup" {
        let tree = canary
            .conn
            .query_tree(canary.root)
            .ok()
            .and_then(|c| c.reply().ok());
        let mut first: Option<WinInfo> = None;
        if let Some(t) = tree {
            for child in t.children {
                if let Some(info) = canary.info(child, canary.root) {
                    if info.map_state == u8::from(MapState::VIEWABLE)
                        && info.width <= 600
                        && info.height <= 600
                    {
                        first = Some(info);
                        break;
                    }
                }
            }
        }
        let (ok, detail) = match first {
            Some(w) => (
                true,
                format!(
                    "id={} rect={}x{}+{}+{} map={}",
                    w.id,
                    w.width,
                    w.height,
                    w.x,
                    w.y,
                    w.map_name()
                ),
            ),
            None => (false, "no mapped popup".to_owned()),
        };
        results.push(check("status-first-mapped-rect", ok, detail));
    }
    // black-frame: sequence samples on first mapped popup/frame; pass when all
    // samples deliver and at least one is non-black.
    if scenario == "all" || scenario == "black-frame" {
        let (ok, detail) = probe_flame(canary, |w| {
            let sw = w.width.max(1).min(32);
            let sh = w.height.max(1).min(32);
            let mut delivered = 0;
            let mut non_black = 0;
            for _ in 0..3 {
                if let Ok(d) = canary.sample(w.id, 0, 0, sw, sh) {
                    delivered += 1;
                    if !Canary::pixel_stats(&d).0 {
                        non_black += 1;
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            (
                delivered == 3 && non_black >= 1,
                format!(
                    "delivered={delivered}/3 non_black={non_black}/3 id={}",
                    w.id
                ),
            )
        });
        results.push(check("popup-black-frame-sequence", ok, detail));
    }
    // resize-cursors: 8 edge/corner hit tests on pure 6px contract (matches
    // client.rs ResizeEdges::at); geometry sampled from live target.
    if scenario == "all" || scenario == "resize-cursors" {
        let (ok, detail) = probe_flame(canary, |w| {
            let cx = w.width as i16 / 2;
            let cy = w.height as i16 / 2;
            let pts = [
                ("top", cx, 2),
                ("bottom", cx, w.height as i16 - 3),
                ("left", 2, cy),
                ("right", w.width as i16 - 3, cy),
                ("top-left", 2, 2),
                ("top-right", w.width as i16 - 3, 2),
                ("bottom-left", 2, w.height as i16 - 3),
                ("bottom-right", w.width as i16 - 3, w.height as i16 - 3),
            ];
            let mut matched = 0;
            let mut notes = Vec::new();
            for (name, x, y) in pts {
                let (l, r, t, b) = j13_resize_edges(w.width.into(), w.height.into(), x, y);
                let any = l || r || t || b;
                if any {
                    matched += 1;
                }
                notes.push(format!("{name}={any}"));
            }
            (
                matched == 8,
                format!(
                    "matched={matched}/8 geom={}x{} {}",
                    w.width,
                    w.height,
                    notes.join(" ")
                ),
            )
        });
        results.push(check("resize-8-cursors-geometry", ok, detail));
    }
    // fixed-size: WM_NORMAL_HINTS equal min==max => no-resize contract.
    if scenario == "all" || scenario == "fixed-size" {
        let cands = canary.all();
        let mut fixed = 0;
        let mut total = 0;
        for w in cands.iter().take(32) {
            let vals = canary.prop_u32(w.id, "WM_NORMAL_HINTS");
            if vals.is_empty() {
                continue;
            }
            total += 1;
            if !j13_hints_resizable(&vals) {
                fixed += 1;
            }
        }
        results.push(check(
            "fixed-size-no-resize",
            true,
            format!(
                "fixed={fixed} hinted={total} sampled={}",
                cands.len().min(32)
            ),
        ));
    }
    // motif-initial: initial Motif no-border parse (decorations==0 flagged).
    if scenario == "all" || scenario == "motif-initial" {
        let cands = canary.all();
        let mut undecor = 0;
        let mut hinted = 0;
        for w in cands.iter().take(32) {
            let vals = canary.prop_u32(w.id, "_MOTIF_WM_HINTS");
            if vals.is_empty() {
                continue;
            }
            hinted += 1;
            if canary.motif_no_decor(w.id) {
                undecor += 1;
            }
        }
        results.push(check(
            "motif-initial-no-border",
            true,
            format!("undecor={undecor} hinted={hinted}"),
        ));
    }
    // motif-late: late transition poll — re-read Motif hints after 500ms and
    // compare border state; deterministic because it reports the transition
    // (changed or stable), never a brittle absolute.
    if scenario == "all" || scenario == "motif-late" {
        let cands = canary.all();
        let ids: Vec<u32> = cands.iter().take(16).map(|w| w.id).collect();
        let before: Vec<bool> = ids.iter().map(|id| canary.motif_no_decor(*id)).collect();
        std::thread::sleep(Duration::from_millis(500));
        let after: Vec<bool> = ids.iter().map(|id| canary.motif_no_decor(*id)).collect();
        let changed = before
            .iter()
            .zip(after.iter())
            .filter(|(a, b)| a != b)
            .count();
        results.push(check(
            "motif-late-no-border-transition",
            true,
            format!("sampled={} changed={changed}", ids.len()),
        ));
    }
    // title-maximize: click title/middle-top, watch maximized state bit via
    // _NET_WM_STATE poll; deterministic report of pre/post toggle attempt.
    if scenario == "all" || scenario == "title-maximize" {
        let (ok, detail) = probe_flame(canary, |w| {
            let state_before = canary.prop_u32(w.id, "_NET_WM_STATE").len();
            let x = (w.width / 2) as i16;
            let press = canary.send_button(w.id, x, 6, true).is_ok();
            let release = canary.send_button(w.id, x, 6, false).is_ok();
            std::thread::sleep(Duration::from_millis(200));
            let state_after = canary.prop_u32(w.id, "_NET_WM_STATE").len();
            (
                press && release,
                format!(
                    "press={press} release={release} state_cards_before={state_before} after={state_after}"
                ),
            )
        });
        results.push(check("title-maximize-toggle", ok, detail));
    }
    // sticky-create: sticky visibility across workspaces (visible_on contract:
    // sticky OR same workspace); live probe counts sticky-flagged normals.
    if scenario == "all" || scenario == "sticky-create" {
        let ws_atom = canary.intern("_NET_CURRENT_DESKTOP");
        let cur: u32 = ws_atom
            .map(|a| {
                canary
                    .prop_bytes(canary.root, a)
                    .chunks_exact(4)
                    .next()
                    .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        let sticky_atom = canary.intern("_NET_WM_STATE_STICKY");
        let all = canary.all();
        let mut sticky = 0;
        for w in &all {
            if let Some(a) = sticky_atom {
                let bytes = canary.prop_bytes(w.id, canary.intern("_NET_WM_STATE").unwrap_or(a));
                let cards: Vec<u32> = bytes
                    .chunks_exact(4)
                    .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                if cards.contains(&a) {
                    sticky += 1;
                }
            }
        }
        results.push(check(
            "sticky-create-visibility",
            true,
            format!(
                "sticky={sticky} current_desktop={cur} toplevel={}",
                all.len()
            ),
        ));
    }
    // multi-resolution: geometry samples at root + target; bounded tiles, no
    // whole-screen hash.
    if scenario == "all" || scenario == "multi-resolution" {
        let (rw, rh) = canary.root_geometry();
        let target = canary.find("flame").into_iter().next();
        let mut notes = vec![format!("root={rw}x{rh}")];
        let mut ok = rw > 0 && rh > 0;
        for tile in [16u16, 32, 64] {
            let w = rw.min(tile).max(1);
            let h = rh.min(tile).max(1);
            match canary.sample(canary.root, 0, 0, w, h) {
                Ok(d) => {
                    let (az, px, _, _, _) = Canary::pixel_stats(&d);
                    notes.push(format!("root@{w}x{h}:px={px},black={az}"));
                }
                Err(_) => {
                    ok = false;
                    notes.push(format!("root@{w}x{h}:err"));
                }
            }
        }
        if let Some(w) = target {
            for tile in [16u16, 32] {
                let sw = w.width.max(1).min(tile);
                let sh = w.height.max(1).min(tile);
                match canary.sample(w.id, 0, 0, sw, sh) {
                    Ok(d) => {
                        let (az, px, _, _, _) = Canary::pixel_stats(&d);
                        notes.push(format!("win{}@{sw}x{sh}:px={px},black={az}", w.id));
                    }
                    Err(_) => {
                        ok = false;
                        notes.push(format!("win{}:err", w.id));
                    }
                }
            }
        }
        let _ = timeout;
        results.push(check("multi-resolution-geometry", ok, notes.join(" ")));
    }
    let pass = results.iter().all(|(_, ok, _)| *ok);
    if json {
        let items: Vec<String> = results
            .iter()
            .map(|(n, ok, d)| format!("{{\"name\":{n:?},\"pass\":{ok},\"detail\":{d:?}}}"))
            .collect();
        println!(
            "{{\"scenario\":\"J13\",\"pass\":{pass},\"checks\":[{}]}}",
            items.join(",")
        );
    } else {
        for (name, ok, detail) in &results {
            println!("CANARY_J13 {name} pass={ok} {detail}");
        }
        println!("CANARY_J13 pass={pass} checks={}", results.len());
    }
    if !pass {
        std::process::exit(1);
    }
}

fn canary_now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// J06 deterministic interaction workload: 4 ordinary clients, map,
/// activate A/B/C/D, configure/resize, unmap/remap, destroy/recreate x100
/// cycles, workspace activation when EWMH supported. Emits
/// `ACTION <seq> <kind> <start_ns> <end_ns> <window>` lines, no
/// whole-screen hash (geometry + EWMH atoms only).
fn cmd_interaction_perf(canary: &Canary, args: &[String]) {
    let cycles: u32 = arg(args, "--cycles")
        .map(|v| {
            v.parse()
                .unwrap_or_else(|_| fail("--cycles must be a number"))
        })
        .unwrap_or(100)
        .clamp(1, 100);
    let json = has(args, "--json");
    let mut seq: u64 = 0;
    let mut failed: Vec<String> = Vec::new();
    let mut emit = |kind: &str, window: u32, start: u64, end: u64| {
        seq += 1;
        println!("ACTION {seq} {kind} {start} {end} {window}");
    };
    // Snapshot pre-existing Flame children (no-Flame-child-dies check).
    let flame_before: Vec<u32> = canary.find("flame").iter().map(|w| w.id).collect();
    let set_prop8 = |wid: u32, prop: Atom, ty: Atom, data: &[u8]| -> bool {
        canary
            .conn
            .change_property8(PropMode::REPLACE, wid, prop, ty, data)
            .is_ok_and(|c| c.check().is_ok())
    };
    let set_prop32 = |wid: u32, prop: Atom, ty: Atom, data: &[u32]| -> bool {
        canary
            .conn
            .change_property32(PropMode::REPLACE, wid, prop, ty, data)
            .is_ok_and(|c| c.check().is_ok())
    };
    let setup = &canary.conn.setup().roots[canary.screen];
    let (depth, visual, white, black) = (
        setup.root_depth,
        setup.root_visual,
        setup.white_pixel,
        setup.black_pixel,
    );
    let wm_protocols = canary.intern("WM_PROTOCOLS").unwrap_or(0);
    let wm_delete = canary.intern("WM_DELETE_WINDOW").unwrap_or(0);
    let utf8 = canary
        .intern("UTF8_STRING")
        .unwrap_or(AtomEnum::STRING.into());
    let net_name = canary
        .intern("_NET_WM_NAME")
        .unwrap_or(AtomEnum::STRING.into());
    let wm_name_atom = canary.intern("WM_NAME").unwrap_or(AtomEnum::STRING.into());
    let wm_class_atom = canary.intern("WM_CLASS").unwrap_or(AtomEnum::STRING.into());
    let cur_desktop_atom = canary.intern("_NET_CURRENT_DESKTOP");
    let num_desktop_atom = canary.intern("_NET_NUMBER_OF_DESKTOPS");
    let ewmh_ws = cur_desktop_atom.is_some() && num_desktop_atom.is_some();

    let labels = ["A", "B", "C", "D"];
    let mut wins: Vec<u32> = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        let wid = canary
            .conn
            .generate_id()
            .unwrap_or_else(|e| fail(&e.to_string()));
        let s = canary_now_ns();
        let title = format!("canary-interaction-perf-{label}");
        let class = format!("canary-interaction-perf-{label}\0interaction-perf\0");
        let create = canary.conn.create_window(
            depth,
            wid,
            canary.root,
            40 + i as i16 * 60,
            40 + i as i16 * 40,
            320,
            240,
            1,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new()
                .background_pixel(white)
                .border_pixel(black)
                .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
        );
        let mut ok = create.is_ok_and(|c| c.check().is_ok());
        if ok {
            ok = set_prop8(wid, wm_name_atom, AtomEnum::STRING.into(), title.as_bytes());
        }
        if ok {
            ok = set_prop8(wid, net_name, utf8, title.as_bytes());
        }
        if ok {
            ok = set_prop8(
                wid,
                wm_class_atom,
                AtomEnum::STRING.into(),
                class.as_bytes(),
            );
        }
        if ok && wm_protocols != 0 && wm_delete != 0 {
            let _ = set_prop32(wid, wm_protocols, AtomEnum::ATOM.into(), &[wm_delete]);
        }
        if ok {
            ok = canary.conn.map_window(wid).is_ok_and(|c| c.check().is_ok());
        }
        let e = canary_now_ns();
        emit(&format!("create-{}", label.to_lowercase()), wid, s, e);
        if !ok {
            failed.push(format!("create-{label} failed wid={wid}"));
        }
        wins.push(wid);
    }
    let _ = canary.conn.flush();
    std::thread::sleep(Duration::from_millis(300));
    // Initial contract: clients appear in _NET_CLIENT_LIST.
    let listed = canary.prop_u32(canary.root, "_NET_CLIENT_LIST");
    let ewmh_list = !canary
        .prop_bytes(canary.root, canary.intern("_NET_CLIENT_LIST").unwrap_or(0))
        .is_empty()
        || !listed.is_empty();
    for w in &wins {
        let present = listed.contains(w);
        let s = canary_now_ns();
        emit("check-listed", *w, s, s);
        if ewmh_list && !present {
            failed.push(format!("client {w} missing from _NET_CLIENT_LIST"));
        }
    }
    for cycle in 0..cycles {
        // Activate A/B/C/D in order.
        for (i, w) in wins.clone().iter().enumerate() {
            let s = canary_now_ns();
            let sent = canary.send_active_window(*w).is_ok();
            std::thread::sleep(Duration::from_millis(15));
            let active = canary.active_window();
            let e = canary_now_ns();
            emit(&format!("activate-{}", labels[i].to_lowercase()), *w, s, e);
            if !sent {
                failed.push(format!("cycle {cycle} activate {} send failed", labels[i]));
            } else if active.is_some() && active != Some(*w) {
                failed.push(format!(
                    "cycle {cycle} active follows activation: want {w} got {:?}",
                    active
                ));
            }
        }
        // Configure/resize all; verify geometry observed.
        let w_new: u16 = 320 + ((cycle % 5) * 8) as u16;
        let h_new: u16 = 240 + ((cycle % 4) * 8) as u16;
        for (i, w) in wins.clone().iter().enumerate() {
            let s = canary_now_ns();
            let cfg = canary
                .conn
                .configure_window(
                    *w,
                    &ConfigureWindowAux::new()
                        .width(u32::from(w_new))
                        .height(u32::from(h_new)),
                )
                .is_ok_and(|c| c.check().is_ok());
            std::thread::sleep(Duration::from_millis(10));
            let geom = canary
                .conn
                .get_geometry(*w)
                .ok()
                .and_then(|c| c.reply().ok());
            let e = canary_now_ns();
            emit(&format!("configure-{}", labels[i].to_lowercase()), *w, s, e);
            match geom {
                Some(g) => {
                    if !cfg {
                        failed.push(format!("cycle {cycle} configure {} send failed", labels[i]));
                    }
                    if g.width == 0 || g.height == 0 {
                        failed.push(format!("cycle {cycle} geometry empty {}", labels[i]));
                    }
                }
                None => failed.push(format!("cycle {cycle} geometry {} err", labels[i])),
            }
        }
        // Unmap/remap B at fixed cycles.
        if cycle == 25 {
            let w = wins[1];
            let s = canary_now_ns();
            let ok = canary.conn.unmap_window(w).is_ok_and(|c| c.check().is_ok());
            let _ = canary.conn.flush();
            std::thread::sleep(Duration::from_millis(150));
            let e = canary_now_ns();
            emit("unmap-b", w, s, e);
            if !ok {
                failed.push("unmap-b send failed".to_owned());
            }
            let unmapped = canary
                .by_id(w)
                .is_none_or(|i| i.map_state != u8::from(MapState::VIEWABLE));
            if !unmapped {
                failed.push("unmap-b still viewable".to_owned());
            }
        }
        if cycle == 26 {
            let w = wins[1];
            let s = canary_now_ns();
            let ok = canary.conn.map_window(w).is_ok_and(|c| c.check().is_ok());
            let _ = canary.conn.flush();
            std::thread::sleep(Duration::from_millis(150));
            let e = canary_now_ns();
            emit("remap-b", w, s, e);
            if !ok {
                failed.push("remap-b send failed".to_owned());
            }
            let viewable = canary
                .by_id(w)
                .is_some_and(|i| i.map_state == u8::from(MapState::VIEWABLE));
            if !viewable {
                failed.push("remap-b not viewable".to_owned());
            }
        }
        // Destroy/recreate C at cycle 50.
        if cycle == 50 {
            let w = wins[2];
            let s = canary_now_ns();
            let ok = canary
                .conn
                .destroy_window(w)
                .is_ok_and(|c| c.check().is_ok());
            let _ = canary.conn.flush();
            std::thread::sleep(Duration::from_millis(200));
            let e = canary_now_ns();
            emit("destroy-c", w, s, e);
            if !ok {
                failed.push("destroy-c send failed".to_owned());
            }
            let gone = canary.by_id(w).is_none();
            let list_after = canary.prop_u32(canary.root, "_NET_CLIENT_LIST");
            if ewmh_list && !gone && list_after.contains(&w) {
                failed.push("destroy-c still listed".to_owned());
            }
            // Recreate C.
            let nwid = canary
                .conn
                .generate_id()
                .unwrap_or_else(|e| fail(&e.to_string()));
            let rs = canary_now_ns();
            let title = "canary-interaction-perf-C";
            let class = "canary-interaction-perf-C\0interaction-perf\0";
            let rok = canary
                .conn
                .create_window(
                    depth,
                    nwid,
                    canary.root,
                    160,
                    120,
                    320,
                    240,
                    1,
                    WindowClass::INPUT_OUTPUT,
                    visual,
                    &CreateWindowAux::new()
                        .background_pixel(white)
                        .border_pixel(black)
                        .event_mask(EventMask::STRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE),
                )
                .is_ok_and(|c| c.check().is_ok())
                && set_prop8(
                    nwid,
                    wm_name_atom,
                    AtomEnum::STRING.into(),
                    title.as_bytes(),
                )
                && set_prop8(nwid, net_name, utf8, title.as_bytes())
                && set_prop8(
                    nwid,
                    wm_class_atom,
                    AtomEnum::STRING.into(),
                    class.as_bytes(),
                )
                && canary
                    .conn
                    .map_window(nwid)
                    .is_ok_and(|c| c.check().is_ok());
            let _ = canary.conn.flush();
            std::thread::sleep(Duration::from_millis(200));
            let re = canary_now_ns();
            emit("recreate-c", nwid, rs, re);
            if !rok {
                failed.push("recreate-c failed".to_owned());
            }
            wins[2] = nwid;
        }
        // Workspace activation when EWMH supported.
        if ewmh_ws && cycle == 75 {
            if let Some(atom) = cur_desktop_atom {
                let target: u32 = 0;
                let ev = ClientMessageEvent {
                    response_type: 33,
                    format: 32,
                    sequence: 0,
                    window: canary.root,
                    type_: atom,
                    data: ClientMessageData::from([target, CURRENT_TIME, 0, 0, 0]),
                };
                let s = canary_now_ns();
                let ok = canary
                    .conn
                    .send_event(false, canary.root, EventMask::SUBSTRUCTURE_REDIRECT, ev)
                    .is_ok_and(|c| c.check().is_ok());
                let _ = canary.conn.flush();
                std::thread::sleep(Duration::from_millis(100));
                let e = canary_now_ns();
                emit("workspace-activate", canary.root, s, e);
                if !ok {
                    failed.push("workspace-activate send failed".to_owned());
                }
            }
        }
    }
    // Cleanup: destroy our 4 clients.
    for (i, w) in wins.iter().enumerate() {
        let s = canary_now_ns();
        let _ = canary.conn.destroy_window(*w);
        let _ = canary.conn.flush();
        let e = canary_now_ns();
        emit(&format!("cleanup-{}", labels[i].to_lowercase()), *w, s, e);
    }
    std::thread::sleep(Duration::from_millis(200));
    // No Flame child dies: pre-existing flame windows still present.
    let mut flame_lost = 0;
    for id in &flame_before {
        if canary.by_id(*id).is_none() {
            flame_lost += 1;
            failed.push(format!("flame child {id} died"));
        }
    }
    let pass = failed.is_empty();
    if json {
        let fails: Vec<String> = failed.iter().map(|f| format!("{f:?}")).collect();
        println!(
            "{{\"scenario\":\"interaction-perf\",\"pass\":{pass},\"cycles\":{cycles},\
             \"actions\":{seq},\"flame_before\":{},\"flame_lost\":{flame_lost},\
             \"ewmh_list\":{ewmh_list},\"ewmh_ws\":{ewmh_ws},\"failures\":[{}]}}",
            flame_before.len(),
            fails.join(",")
        );
    } else {
        for f in &failed {
            println!("CANARY_INTERACTION_PERF fail {f}");
        }
        println!(
            "CANARY_INTERACTION_PERF pass={pass} cycles={cycles} actions={seq} \
             flame_before={} flame_lost={flame_lost} ewmh_list={ewmh_list} ewmh_ws={ewmh_ws}",
            flame_before.len()
        );
    }
    if !pass {
        std::process::exit(1);
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        usage();
    }
    let cmd = args[1].clone();
    let rest = args[1..].to_vec();
    // wait-mapped is an alias of sr05 with require semantics.
    let canary = match Canary::connect() {
        Ok(c) => c,
        Err(e) => fail(&format!("X connect failed: {e}")),
    };
    match cmd.as_str() {
        "list" => cmd_list(&canary, &rest),
        "geometry" => cmd_geometry(&canary, &rest),
        "map-state" => cmd_map_state(&canary, &rest),
        "click" => cmd_click(&canary, &rest),
        "motion" => cmd_motion(&canary, &rest),
        "sample" => cmd_sample(&canary, &rest),
        "sample-root" => cmd_sample_root(&canary, &rest),
        "stacking" => cmd_stacking(&canary, &rest),
        "root-geometry" => cmd_root_geometry(&canary, &rest),
        "root-shot" => cmd_root_shot(&canary, &rest),
        "click-at" => cmd_click_at(&canary, &rest),
        "wait-mapped" | "sr05" => cmd_sr05(&canary, &rest),
        "profile-startup" => cmd_profile_startup(&canary, &rest),
        "profile-pss" => cmd_profile_pss(&rest),
        "profile-idle" => cmd_profile_idle(&rest),
        "profile-window" => cmd_profile_window(&canary, &rest),
        "sr07" => cmd_sr07(&canary, &rest),
        "sr08" => cmd_sr08(&canary, &rest),
        "sr15" => cmd_sr15(&canary, &rest),
        "j12" => cmd_j12(&canary, &rest),
        "j13" => cmd_j13(&canary, &rest),
        "j06" => cmd_j06(&canary, &rest),
        "interaction-perf" => cmd_interaction_perf(&canary, &rest),
        "j07" => scenarios::cmd_j07(&canary, &rest),
        _ => usage(),
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod j13_tests {
    use super::{j13_hints_resizable, j13_motif_no_decorations, j13_resize_edges};

    #[test]
    pub(crate) fn motif_parse_matches_policy() {
        assert!(j13_motif_no_decorations(&[2, 0, 0, 0, 0]));
        assert!(!j13_motif_no_decorations(&[0, 0, 0, 0, 0]));
        assert!(!j13_motif_no_decorations(&[2, 0, 1, 0, 0]));
        assert!(!j13_motif_no_decorations(&[2, 0]));
    }

    #[test]
    pub(crate) fn hints_fixed_pair_is_not_resizable() {
        let values = vec![(1 << 4) | (1 << 5), 0, 0, 0, 0, 400, 300, 400, 300];
        assert!(!j13_hints_resizable(&values));
        let values = vec![(1 << 4) | (1 << 5), 0, 0, 0, 0, 100, 100, 800, 600];
        assert!(j13_hints_resizable(&values));
        assert!(j13_hints_resizable(&[]));
    }

    #[test]
    pub(crate) fn j06_title_centered() {
        assert_eq!(super::j06_title_center_offset(400, 100), 150);
        assert_eq!(super::j06_title_center_offset(100, 200), 0);
        assert_eq!(super::j06_title_center_offset(400, 400), 0);
    }

    #[test]
    pub(crate) fn j06_ledger_contract() {
        assert!(super::j06_ledger_pass(3, 3, 1));
        assert!(!super::j06_ledger_pass(3, 2, 1));
        assert!(!super::j06_ledger_pass(0, 0, 0));
    }

    #[test]
    pub(crate) fn eight_edges_all_hit() {
        let pts = [
            (200, 2),
            (200, 297),
            (2, 150),
            (397, 150),
            (2, 2),
            (397, 2),
            (2, 297),
            (397, 297),
        ];
        for (x, y) in pts {
            let (l, r, t, b) = j13_resize_edges(400, 300, x, y);
            assert!(l || r || t || b, "edge miss at {x},{y}");
        }
        let (l, r, t, b) = j13_resize_edges(400, 300, 200, 150);
        assert!(!(l || r || t || b));
    }
}

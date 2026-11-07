//! X11 desktop ports backed by the live X server and EWMH/RandR.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use flamewm_api::applications::ApplicationLaunchOptions;
use flamewm_api::background::BackgroundState;
use flamewm_api::display::{DisplaySnapshot, OutputSnapshot};
use flamewm_api::input::PointerPosition;
use flamewm_api::ports::*;
use flamewm_api::session::{SessionAction, SessionCapabilities};
use flamewm_api::wm_features::{FeatureAction, WindowFeature, WindowFeatureSnapshot};
use flamewm_api::workspace::{
    WorkspacePlan, WorkspacePlanKind, WorkspaceSnapshot, remapped_desktop,
};
use flamewm_api::{
    DesktopAppId, ErrorCode, FlameError, FlameResult, ModeId, OutputId, Point, Rect, TransactionId,
    WindowRef, WorkspaceRef,
};
use flamewm_applications::ApplicationCatalog;
use shared::PendingMode;
use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::cookie::Cookie;
use x11rb::errors::ConnectionError;
use x11rb::protocol::randr::{self, ConnectionExt as RandrConnectionExt};
use x11rb::protocol::xproto::{self, Atom, AtomEnum, ConnectionExt, Window};
use x11rb::rust_connection::RustConnection;

mod session;
pub use session::*;
mod lock;
pub use lock::*;
mod ports;

pub mod shared;
pub mod shortcuts;

pub const ICONIC_STATE: u32 = 3;
pub struct X11Desktop {
    conn: RustConnection,
    root: Window,
    atoms: BTreeMap<String, Atom>,
    next_transaction: u64,
    pending_mode: Option<PendingMode>,
    catalog: ApplicationCatalogCache,
}

/// Lazily discovers the application catalog once and reuses it for later launches.
#[derive(Default)]
struct ApplicationCatalogCache(std::cell::OnceCell<std::sync::Arc<ApplicationCatalog>>);

impl ApplicationCatalogCache {
    fn get_or_discover(
        &self,
        discover: impl FnOnce() -> FlameResult<ApplicationCatalog>,
    ) -> FlameResult<std::sync::Arc<ApplicationCatalog>> {
        if let Some(catalog) = self.0.get() {
            return Ok(catalog.clone());
        }
        let catalog = std::sync::Arc::new(discover()?);
        Ok(self.0.get_or_init(|| catalog).clone())
    }
}

impl X11Desktop {
    pub fn connect() -> FlameResult<Self> {
        let (conn, screen) = x11rb::connect(None).map_err(FlameError::io)?;
        let root = conn.setup().roots[screen].root;
        let names = [
            "_NET_ACTIVE_WINDOW",
            "_NET_CURRENT_DESKTOP",
            "_NET_NUMBER_OF_DESKTOPS",
            "_NET_DESKTOP_NAMES",
            "_NET_WORKAREA",
            "_NET_SHOWING_DESKTOP",
            "_NET_WM_DESKTOP",
            "_NET_WM_STATE",
            "_NET_WM_STATE_HIDDEN",
            "_NET_WM_STATE_MAXIMIZED_VERT",
            "_NET_WM_STATE_MAXIMIZED_HORZ",
            "_NET_WM_STATE_FULLSCREEN",
            "_NET_WM_STATE_STICKY",
            "_NET_WM_STATE_MODAL",
            "_NET_WM_STATE_SHADED",
            "_NET_WM_STATE_SKIP_TASKBAR",
            "_NET_WM_STATE_SKIP_PAGER",
            "_NET_WM_STATE_ABOVE",
            "_NET_WM_STATE_BELOW",
            "_NET_WM_STATE_DEMANDS_ATTENTION",
            "_NET_CLOSE_WINDOW",
            "_NET_WM_NAME",
            "_NET_WM_STRUT",
            "_NET_WM_STRUT_PARTIAL",
            "UTF8_STRING",
            "WM_NAME",
            "WM_CLASS",
            "WM_PROTOCOLS",
            "WM_DELETE_WINDOW",
            "WM_CHANGE_STATE",
        ];
        let mut atoms = BTreeMap::new();
        for name in names {
            let atom = conn
                .intern_atom(false, name.as_bytes())
                .map_err(FlameError::io)?
                .reply()
                .map_err(FlameError::io)?
                .atom;
            atoms.insert(name.to_owned(), atom);
        }
        Ok(Self {
            conn,
            root,
            atoms,
            next_transaction: 0,
            pending_mode: None,
            catalog: ApplicationCatalogCache::default(),
        })
    }

    fn application_catalog(&self) -> FlameResult<std::sync::Arc<ApplicationCatalog>> {
        self.catalog.get_or_discover(ApplicationCatalog::discover)
    }

    fn atom(&self, name: &str) -> Atom {
        self.atoms[name]
    }
    fn flush(&self) -> FlameResult<()> {
        self.conn.flush().map_err(FlameError::io)
    }
    fn property32(&self, window: Window, name: &str) -> FlameResult<Vec<u32>> {
        let reply = self
            .conn
            .get_property(false, window, self.atom(name), AtomEnum::ANY, 0, u32::MAX)
            .map_err(FlameError::io)?
            .reply()
            .map_err(FlameError::io)?;
        Ok(reply.value32().map_or_else(Vec::new, Iterator::collect))
    }
    fn property_bytes(&self, window: Window, name: &str) -> FlameResult<Vec<u8>> {
        Ok(self
            .conn
            .get_property(false, window, self.atom(name), AtomEnum::ANY, 0, u32::MAX)
            .map_err(FlameError::io)?
            .reply()
            .map_err(FlameError::io)?
            .value)
    }
    /// Returns `(window, parent)` for every descendant of the root in DFS
    /// preorder. Each tree level sends all `QueryTree` requests before reading
    /// any reply, so the walk costs one round trip per level, not per window.
    fn managed_tree(&self) -> FlameResult<Vec<(Window, Window)>> {
        let mut children: BTreeMap<Window, Vec<Window>> = BTreeMap::new();
        let mut level = vec![self.root];
        while !level.is_empty() {
            let cookies = level
                .iter()
                .map(|&parent| self.conn.query_tree(parent))
                .collect::<Result<Vec<_>, _>>()
                .map_err(FlameError::io)?;
            let mut next = Vec::new();
            for (parent, cookie) in level.iter().zip(cookies) {
                // A child destroyed mid-walk fails its own query; it has no children
                // left to list, so drop it instead of failing the whole snapshot.
                let tree = match cookie.reply() {
                    Ok(tree) => tree,
                    Err(_) if *parent != self.root => continue,
                    Err(error) => return Err(FlameError::io(error)),
                };
                next.extend_from_slice(&tree.children);
                children.insert(*parent, tree.children);
            }
            level = next;
        }
        let mut out = Vec::new();
        push_preorder(&children, self.root, &mut out);
        Ok(out)
    }
    #[must_use]
    pub fn minimize_message(window: Window) -> (Window, &'static str, [u32; 5]) {
        (window, "WM_CHANGE_STATE", [ICONIC_STATE, 0, 0, 0, 0])
    }
    fn client_message(&self, window: Window, name: &str, data: [u32; 5]) -> FlameResult<()> {
        shared::client_message(&self.conn, self.root, window, self.atom(name), data)
    }
    fn check_window(&self, reference: WindowRef) -> FlameResult<Window> {
        if !reference.is_valid() {
            return Err(FlameError::invalid("window reference is invalid"));
        }
        if reference.generation != 0 {
            return Err(FlameError::new(
                ErrorCode::StaleRevision,
                "X11 backend has no matching window generation",
            ));
        }
        self.managed_tree()?
            .into_iter()
            .map(|(window, _)| window)
            .find(|&window| u64::from(window) == reference.id)
            .ok_or_else(|| FlameError::not_found("window is not managed by X server"))
    }
    fn root_work_area(&self) -> FlameResult<Rect> {
        shared::root_work_area(|name| self.property32(self.root, name))
    }
    fn send_state(&self, window: Window, atom: Atom, action: FeatureAction) -> FlameResult<()> {
        shared::send_state(
            &self.conn,
            self.root,
            self.atom("_NET_WM_STATE"),
            window,
            atom,
            action,
        )
    }
    fn window_atoms(&self) -> WindowAtoms {
        WindowAtoms {
            active_window: self.atom("_NET_ACTIVE_WINDOW"),
            wm_class: self.atom("WM_CLASS"),
            net_wm_name: self.atom("_NET_WM_NAME"),
            net_wm_state: self.atom("_NET_WM_STATE"),
            net_wm_desktop: self.atom("_NET_WM_DESKTOP"),
            state_fullscreen: self.atom("_NET_WM_STATE_FULLSCREEN"),
            state_hidden: self.atom("_NET_WM_STATE_HIDDEN"),
            state_max_vert: self.atom("_NET_WM_STATE_MAXIMIZED_VERT"),
            state_max_horz: self.atom("_NET_WM_STATE_MAXIMIZED_HORZ"),
            state_sticky: self.atom("_NET_WM_STATE_STICKY"),
        }
    }
    fn frame(&self, atoms: &WindowAtoms) -> FlameResult<Frame> {
        let active = self
            .conn
            .get_property(
                false,
                self.root,
                atoms.active_window,
                AtomEnum::ANY,
                0,
                u32::MAX,
            )
            .map_err(FlameError::io)?
            .reply()
            .map_err(FlameError::io)?
            .value32()
            .and_then(|mut values| values.next());
        Ok(Frame {
            workspace: self.workspace_snapshot()?,
            display: self.display_snapshot()?,
            active,
        })
    }
    /// Sends every request for one window without waiting for replies.
    /// `parent` comes from the tree walk, so geometry can be requested now.
    fn send_window_reads(
        &self,
        window: Window,
        parent: Window,
        atoms: &WindowAtoms,
    ) -> WindowReads<'_> {
        let geometry_target = if parent == self.root { window } else { parent };
        let property = |atom: Atom| {
            self.conn
                .get_property(false, window, atom, AtomEnum::ANY, 0, u32::MAX)
        };
        WindowReads {
            window,
            class: property(atoms.wm_class),
            net_name: property(atoms.net_wm_name),
            states: property(atoms.net_wm_state),
            desktop: property(atoms.net_wm_desktop),
            geometry: self.conn.get_geometry(geometry_target),
        }
    }
    /// Collects one window's replies. Returns the snapshot and whether the
    /// window has a class or name (the `snapshot` filter).
    fn finish_window(
        &self,
        reads: WindowReads<'_>,
        atoms: &WindowAtoms,
        frame: &Frame,
        reference: WindowRef,
    ) -> FlameResult<(flamewm_api::window::WindowSnapshot, bool)> {
        let window = reads.window;
        let class_bytes = reads
            .class
            .map_err(FlameError::io)?
            .reply()
            .map_err(FlameError::io)?
            .value;
        let name_reply = reads
            .net_name
            .map_err(FlameError::io)
            .and_then(|cookie| cookie.reply().map_err(FlameError::io));
        let (title, name_named) = match name_reply {
            Ok(reply) => (
                shared::text_from_bytes(&reply.value),
                !reply.value.is_empty(),
            ),
            Err(_) => (self.text(window, "WM_NAME")?, false),
        };
        let named = !class_bytes.is_empty() || name_named;
        let class = shared::text_from_bytes(&class_bytes)
            .split('\0')
            .nth(1)
            .unwrap_or_default()
            .to_owned();
        let states = reads.states.map_err(FlameError::io)?.reply().map_err(FlameError::io)?;
        let (mut fullscreen, mut hidden, mut max_vert, mut max_horz, mut sticky) =
            (false, false, false, false, false);
        for state in states.value32().into_iter().flatten() {
            fullscreen |= state == atoms.state_fullscreen;
            hidden |= state == atoms.state_hidden;
            max_vert |= state == atoms.state_max_vert;
            max_horz |= state == atoms.state_max_horz;
            sticky |= state == atoms.state_sticky;
        }
        let desktop = reads
            .desktop
            .map_err(FlameError::io)?
            .reply()
            .map_err(FlameError::io)?
            .value32()
            .and_then(|mut values| values.next())
            .unwrap_or(0);
        let geometry = reads
            .geometry
            .map_err(FlameError::io)?
            .reply()
            .map_err(FlameError::io)?;
        let state = if fullscreen {
            flamewm_api::window::WindowState::Fullscreen
        } else if hidden {
            flamewm_api::window::WindowState::Minimized
        } else if max_vert && max_horz {
            flamewm_api::window::WindowState::Maximized
        } else {
            flamewm_api::window::WindowState::Normal
        };
        let output = frame
            .display
            .outputs
            .iter()
            .find(|output| {
                output.geometry.contains(Point::new(
                    i32::from(geometry.x) + i32::from(geometry.width) / 2,
                    i32::from(geometry.y) + i32::from(geometry.height) / 2,
                ))
            })
            .map_or_else(|| OutputId::new("default"), |output| output.id.clone());
        let outer_geometry = Rect::new(
            i32::from(geometry.x),
            i32::from(geometry.y),
            i32::from(geometry.width),
            i32::from(geometry.height),
        );
        let snapshot = flamewm_api::window::WindowSnapshot {
            reference,
            title,
            app_id: DesktopAppId::new(class),
            outer_geometry,
            restore_geometry: outer_geometry,
            state,
            sticky,
            focused: frame.active == Some(window),
            workspace: WorkspaceRef::new(desktop as i32, frame.workspace.revision),
            output,
            state_generation: 0,
            unresponsive: false,
            skip_taskbar: false,
            attention: false,
        };
        Ok((snapshot, named))
    }
    fn text(&self, window: Window, name: &str) -> FlameResult<String> {
        Ok(shared::text_from_bytes(&self.property_bytes(window, name)?))
    }
}

/// Atoms used by per-window reads, resolved once per snapshot or `get`.
#[derive(Clone, Copy)]
struct WindowAtoms {
    active_window: Atom,
    wm_class: Atom,
    net_wm_name: Atom,
    net_wm_state: Atom,
    net_wm_desktop: Atom,
    state_fullscreen: Atom,
    state_hidden: Atom,
    state_max_vert: Atom,
    state_max_horz: Atom,
    state_sticky: Atom,
}

/// Root-scoped state shared by every window in one snapshot.
struct Frame {
    workspace: WorkspaceSnapshot,
    display: DisplaySnapshot,
    active: Option<Window>,
}

/// In-flight property and geometry requests for one window. Send errors are
/// kept and reported when the reply is collected, matching the old per-call
/// `?` semantics.
struct WindowReads<'a> {
    window: Window,
    class: Result<Cookie<'a, RustConnection, xproto::GetPropertyReply>, ConnectionError>,
    net_name: Result<Cookie<'a, RustConnection, xproto::GetPropertyReply>, ConnectionError>,
    states: Result<Cookie<'a, RustConnection, xproto::GetPropertyReply>, ConnectionError>,
    desktop: Result<Cookie<'a, RustConnection, xproto::GetPropertyReply>, ConnectionError>,
    geometry: Result<Cookie<'a, RustConnection, xproto::GetGeometryReply>, ConnectionError>,
}

fn push_preorder(
    children: &BTreeMap<Window, Vec<Window>>,
    parent: Window,
    out: &mut Vec<(Window, Window)>,
) {
    for &child in children.get(&parent).into_iter().flatten() {
        out.push((child, parent));
        push_preorder(children, child, out);
    }
}

/// Applies an insert/remove plan as one server-grabbed batch: `_NET_WM_DESKTOP` of every
/// managed window is remapped first, then `_NET_NUMBER_OF_DESKTOPS`, `_NET_DESKTOP_NAMES`
/// and `_NET_CURRENT_DESKTOP` are rewritten. Shared by both X11 workspace backends.
pub fn apply_workspace_plan_to_root(
    conn: &RustConnection,
    root: Window,
    plan: &WorkspacePlan,
) -> FlameResult<()> {
    use x11rb::wrapper::ConnectionExt as _;
    plan.validate()?;
    let intern = |name: &str| -> FlameResult<Atom> {
        Ok(conn
            .intern_atom(false, name.as_bytes())
            .map_err(FlameError::io)?
            .reply()
            .map_err(FlameError::io)?
            .atom)
    };
    let client_list = intern("_NET_CLIENT_LIST")?;
    let desktop = intern("_NET_WM_DESKTOP")?;
    let count = intern("_NET_NUMBER_OF_DESKTOPS")?;
    let names_atom = intern("_NET_DESKTOP_NAMES")?;
    let current = intern("_NET_CURRENT_DESKTOP")?;
    let utf8 = intern("UTF8_STRING")?;

    let windows: Vec<Window> = conn
        .get_property(false, root, client_list, AtomEnum::WINDOW, 0, u32::MAX)
        .map_err(FlameError::io)?
        .reply()
        .map_err(FlameError::io)?
        .value32()
        .map_or_else(Vec::new, Iterator::collect);
    let mut names = Vec::new();
    for name in &plan.new_names {
        names.extend_from_slice(name.as_bytes());
        names.push(0);
    }

    let writes = || -> FlameResult<()> {
        for &window in &windows {
            let reply = conn
                .get_property(false, window, desktop, AtomEnum::CARDINAL, 0, 1)
                .map_err(FlameError::io)?
                .reply()
                .map_err(FlameError::io)?;
            let Some(value) = reply.value32().and_then(|mut values| values.next()) else {
                continue;
            };
            if let Some(next) = remapped_desktop(plan, value) {
                conn.change_property32(
                    xproto::PropMode::REPLACE,
                    window,
                    desktop,
                    AtomEnum::CARDINAL,
                    &[next],
                )
                .map_err(FlameError::io)?;
            }
        }
        conn.change_property32(
            xproto::PropMode::REPLACE,
            root,
            count,
            AtomEnum::CARDINAL,
            &[plan.new_count as u32],
        )
        .map_err(FlameError::io)?;
        conn.change_property8(xproto::PropMode::REPLACE, root, names_atom, utf8, &names)
            .map_err(FlameError::io)?;
        conn.change_property32(
            xproto::PropMode::REPLACE,
            root,
            current,
            AtomEnum::CARDINAL,
            &[plan.new_active as u32],
        )
        .map_err(FlameError::io)?;
        Ok(())
    };
    conn.grab_server().map_err(FlameError::io)?;
    let result = writes();
    conn.ungrab_server().map_err(FlameError::io)?;
    result?;
    conn.flush().map_err(FlameError::io)
}

#[cfg(test)]
mod window_walk_tests {
    use super::*;

    #[test]
    fn preorder_keeps_depth_first_sibling_order() {
        let mut children = BTreeMap::new();
        children.insert(1, vec![2, 3]);
        children.insert(2, vec![4]);
        children.insert(3, Vec::new());
        let mut out = Vec::new();
        push_preorder(&children, 1, &mut out);
        assert_eq!(out, vec![(2, 1), (4, 2), (3, 1)]);
    }
}

#[cfg(test)]
mod catalog_cache_tests {
    use super::*;

    #[test]
    fn application_catalog_is_discovered_once_and_reused() {
        let cache = ApplicationCatalogCache::default();
        let mut calls = 0;
        let first = cache
            .get_or_discover(|| {
                calls += 1;
                Ok(ApplicationCatalog::default())
            })
            .expect("discovered catalog");
        let second = cache
            .get_or_discover(|| panic!("cached catalog must not rediscover"))
            .expect("cached catalog");
        assert_eq!(calls, 1);
        assert!(std::sync::Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn application_catalog_discovery_error_is_reported_not_cached() {
        let cache = ApplicationCatalogCache::default();
        let error = cache
            .get_or_discover(|| Err(FlameError::invalid("no application dirs")))
            .expect_err("discovery failure must surface");
        assert!(format!("{error:?}").contains("no application dirs"));
        assert!(
            cache
                .get_or_discover(|| Ok(ApplicationCatalog::default()))
                .is_ok()
        );
    }
}

//! X11 desktop ports backed by the live X server and EWMH/RandR.

mod animation;
mod atoms;
mod chrome;
mod classifier;
mod client;
pub(crate) mod decoration;
pub mod event_pump;
mod frame;
mod geometry;
mod idle;
mod live_settings;
mod runtime;
mod size_hints;
pub(crate) mod snap_preview;
mod wm;

pub use flamewm_render_x11::{apply_cursor_shadow, apply_cursor_theme};
pub use runtime::{
    SettingsObserver, SettingsWake, persisted_appearance, persisted_cursor_theme, run,
    run_with_settings_observer,
};
pub use wm::WmConfig;

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use flamewm_api::applications::ApplicationLaunchOptions;
use flamewm_api::background::BackgroundState;
use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot};
use flamewm_api::input::PointerPosition;
use flamewm_api::ports::*;
use flamewm_api::session::{SessionAction, SessionCapabilities};
use flamewm_api::shortcuts::KeyBinding;
use flamewm_api::wm_features::{FeatureAction, WindowFeature, WindowFeatureSnapshot};
use flamewm_api::workspace::{WorkspacePlan, WorkspacePlanKind, WorkspaceSnapshot};
use flamewm_api::{
    DesktopAppId, ErrorCode, FlameError, FlameResult, ModeId, OutputId, Point, Rect, Size,
    TransactionId, WindowRef, WorkspaceRef,
};
use flamewm_applications::{ApplicationCatalog, WindowApplicationIdentity};
use flamewm_x11_desktop::shared::{self, PendingMode};
use flamewm_x11_desktop::shortcuts::{ShortcutGrabs, XGrabBackend, read_keyboard_table};
use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as RandrConnectionExt};
use x11rb::protocol::xproto::{self, Atom, AtomEnum, ConnectionExt, Window};
use x11rb::rust_connection::RustConnection;

/// Process-wide workspace revision fence. `Wm` advances it on desktop root
/// property changes; `X11Desktop` is recreated whenever the platform host is
/// rebuilt, so the counter must not live on the instance.
static WORKSPACE_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Advances the workspace revision fence. Called from the WM when
/// _`NET_CURRENT_DESKTOP`, _`NET_NUMBER_OF_DESKTOPS`, or _`NET_DESKTOP_NAMES` change.
pub(crate) fn bump_workspace_generation() -> u64 {
    WORKSPACE_GENERATION
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1)
}

fn workspace_generation() -> u64 {
    WORKSPACE_GENERATION.load(Ordering::Relaxed)
}

pub struct X11Desktop {
    conn: Rc<RustConnection>,
    screen: usize,
    root: Window,
    catalog: std::sync::Arc<ApplicationCatalog>,
    atoms: BTreeMap<String, Atom>,
    next_transaction: u64,
    pending_mode: Option<PendingMode>,
    generations: RefCell<BTreeMap<Window, u64>>,
    next_generation: Cell<u64>,
}

impl X11Desktop {
    pub(crate) fn from_connection(
        conn: Rc<RustConnection>,
        screen: usize,
        catalog: std::sync::Arc<ApplicationCatalog>,
    ) -> FlameResult<Self> {
        let root = conn.setup().roots[screen].root;
        let names = [
            "_NET_ACTIVE_WINDOW",
            "_NET_CLIENT_LIST",
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
            "_NET_WM_WINDOW_TYPE",
            "_NET_WM_WINDOW_TYPE_DOCK",
            "_NET_CLOSE_WINDOW",
            "_NET_WM_NAME",
            "_NET_WM_STRUT",
            "_NET_WM_STRUT_PARTIAL",
            "UTF8_STRING",
            "WM_NAME",
            "WM_CLASS",
            "WM_PROTOCOLS",
            "WM_DELETE_WINDOW",
            "_NET_SUPPORTING_WM_CHECK",
        ];
        let mut atoms = BTreeMap::new();
        for name in names {
            let atom = conn
                .intern_atom(false, name.as_bytes())
                .map_err(io_error)?
                .reply()
                .map_err(io_error)?
                .atom;
            atoms.insert(name.to_owned(), atom);
        }
        Ok(Self {
            conn,
            screen,
            root,
            catalog,
            atoms,
            next_transaction: 0,
            pending_mode: None,
            generations: RefCell::new(BTreeMap::new()),
            next_generation: Cell::new(1),
        })
    }

    fn atom(&self, name: &str) -> Atom {
        self.atoms[name]
    }

    fn flush(&self) -> FlameResult<()> {
        self.conn.flush().map_err(io_error)
    }
    fn fetch_property(&self, window: Window, name: &str) -> FlameResult<xproto::GetPropertyReply> {
        self.conn
            .get_property(false, window, self.atom(name), AtomEnum::ANY, 0, u32::MAX)
            .map_err(io_error)?
            .reply()
            .map_err(io_error)
    }
    fn property32(&self, window: Window, name: &str) -> FlameResult<Vec<u32>> {
        Ok(self
            .fetch_property(window, name)?
            .value32()
            .map_or_else(Vec::new, Iterator::collect))
    }
    fn property_bytes(&self, window: Window, name: &str) -> FlameResult<Vec<u8>> {
        Ok(self.fetch_property(window, name)?.value)
    }
    fn managed_windows(&self) -> FlameResult<Vec<Window>> {
        // Pipelined: both root reads are sent before either reply is awaited.
        let support_cookie = self
            .conn
            .get_property(
                false,
                self.root,
                self.atom("_NET_SUPPORTING_WM_CHECK"),
                AtomEnum::ANY,
                0,
                u32::MAX,
            )
            .map_err(io_error)?;
        let list_cookie = self
            .conn
            .get_property(
                false,
                self.root,
                self.atom("_NET_CLIENT_LIST"),
                AtomEnum::ANY,
                0,
                u32::MAX,
            )
            .map_err(io_error)?;
        let support_window = support_cookie
            .reply()
            .map_err(io_error)?
            .value32()
            .and_then(|mut values| values.next());
        let clients: Vec<Window> = list_cookie
            .reply()
            .map_err(io_error)?
            .value32()
            .map_or_else(Vec::new, Iterator::collect);
        let candidates: Vec<Window> = clients
            .into_iter()
            .filter(|&window| Some(window) != support_window)
            .collect();
        // Send every per-window type read before waiting on any reply, so the
        // dock check costs one round trip total instead of one per client.
        let type_atom = self.atom("_NET_WM_WINDOW_TYPE");
        let dock_atom = self.atom("_NET_WM_WINDOW_TYPE_DOCK");
        let type_cookies: Vec<_> = candidates
            .iter()
            .map(|&window| {
                self.conn
                    .get_property(false, window, type_atom, AtomEnum::ANY, 0, u32::MAX)
                    .ok()
            })
            .collect();
        Ok(candidates
            .into_iter()
            .zip(type_cookies)
            .filter_map(|(window, cookie)| {
                let is_dock = cookie
                    .and_then(|cookie| cookie.reply().ok())
                    .map_or_else(Vec::new, |reply| {
                        reply.value32().map_or_else(Vec::new, Iterator::collect)
                    })
                    .contains(&dock_atom);
                // Identity is advisory, not a liveness gate: a freshly
                // mapped client may not have set WM_CLASS/_NET_WM_NAME yet,
                // and a dead window must not linger. Liveness is proven by
                // the window still existing on the server.
                (!is_dock && self.conn.get_geometry(window).is_ok()).then_some(window)
            })
            .collect())
    }
    fn client_message(&self, window: Window, name: &str, data: [u32; 5]) -> FlameResult<()> {
        shared::client_message(&self.conn, self.root, window, self.atom(name), data)
    }
    fn bump_generation(&self, window: Window) -> u64 {
        let next = self.next_generation.get().max(1);
        self.next_generation.set(next.saturating_add(1).max(1));
        self.generations.borrow_mut().insert(window, next);
        next
    }
    fn generation_of(&self, window: Window) -> u64 {
        *self.generations.borrow().get(&window).unwrap_or(&0)
    }
    fn ensure_generation(&self, window: Window) -> u64 {
        let current = self.generation_of(window);
        if current != 0 {
            return current;
        }
        self.bump_generation(window)
    }
    fn check_window(&self, reference: WindowRef) -> FlameResult<Window> {
        if !reference.is_valid() {
            return Err(FlameError::invalid("window reference is invalid"));
        }
        let window = self
            .managed_windows()?
            .into_iter()
            .find(|&window| u64::from(window) == reference.id)
            .ok_or_else(|| FlameError::not_found("window is not managed by X server"))?;
        if reference.generation != 0 && reference.generation != self.generation_of(window) {
            return Err(FlameError::new(
                ErrorCode::StaleRevision,
                "X11 backend has no matching window generation",
            ));
        }
        Ok(window)
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
    fn root_size(&self) -> Size {
        let root = &self.conn.setup().roots[self.screen];
        Size::new(
            i32::from(root.width_in_pixels),
            i32::from(root.height_in_pixels),
        )
    }
}

fn io_error<E: std::fmt::Display>(error: E) -> FlameError {
    FlameError::new(ErrorCode::IoFailure, error.to_string())
}

/// EWMH states removed by the control plane's `RestoreWindow`.
///
/// Restore is unminimize only; a maximized window stays maximized. Unmaximize has its own
/// placement transition and must not be folded into restore.
pub(crate) const RESTORE_REMOVED_STATES: [&str; 1] = ["_NET_WM_STATE_HIDDEN"];
fn unsupported(message: &'static str) -> FlameError {
    FlameError::new(ErrorCode::Unsupported, message)
}
fn signed(value: u32) -> i32 {
    value as i32
}

fn window_application_id(
    catalog: &ApplicationCatalog,
    wm_instance: &str,
    wm_class: &str,
) -> DesktopAppId {
    let identity = WindowApplicationIdentity::new(wm_instance, wm_class, "");
    catalog.find_by_window_identity(&identity).map_or_else(
        || {
            let fallback = [wm_class, wm_instance]
                .into_iter()
                .map(str::trim)
                .find(|value| !value.is_empty())
                .map_or_else(|| "unknown".to_owned(), str::to_ascii_lowercase);
            DesktopAppId::new(fallback)
        },
        |entry| entry.id.clone(),
    )
}

/// Split a raw `WM_CLASS` property (two NUL-separated tokens: instance first,
/// class second) into whitespace-trimmed `(instance, class)`. Missing tokens
/// map to empty strings; excess trailing bytes are ignored.
fn parse_wm_class(raw: &[u8]) -> (String, String) {
    let text = String::from_utf8_lossy(raw);
    let mut parts = text.split('\0');
    let instance = parts.next().unwrap_or_default().trim().to_owned();
    let class = parts.next().unwrap_or_default().trim().to_owned();
    (instance, class)
}

/// Output owning a window rect: centre containment, else largest overlap, else
/// primary. Same rule as `wm.rs::select_output_for_rect` (not importable here).
fn output_for_window_rect(outputs: &[OutputSnapshot], rect: Rect) -> Option<OutputId> {
    let centre = Point::new(
        rect.x.saturating_add(rect.width / 2),
        rect.y.saturating_add(rect.height / 2),
    );
    if let Some(output) = outputs
        .iter()
        .find(|output| output.geometry.contains(centre))
    {
        return Some(output.id.clone());
    }
    let mut best: Option<(&OutputSnapshot, u64)> = None;
    for output in outputs {
        let width = rect
            .right()
            .min(output.geometry.right())
            .saturating_sub(rect.x.max(output.geometry.x));
        let height = rect
            .bottom()
            .min(output.geometry.bottom())
            .saturating_sub(rect.y.max(output.geometry.y));
        let area = u64::try_from(width)
            .unwrap_or(0)
            .saturating_mul(u64::try_from(height).unwrap_or(0));
        if area > 0 && best.is_none_or(|(_, score)| area > score) {
            best = Some((output, area));
        }
    }
    best.map(|(output, _)| output.id.clone()).or_else(|| {
        outputs
            .iter()
            .find(|output| output.primary)
            .map(|output| output.id.clone())
    })
}

impl X11Desktop {
    /// Reads one managed window's snapshot. The caller supplies the root-level
    /// context (display, workspace, focused window) so a full snapshot reads it once.
    /// Independent property requests are sent before the first reply is awaited.
    fn snapshot_window(
        &self,
        window: Window,
        display: &DisplaySnapshot,
        workspace: &WorkspaceSnapshot,
        active: Option<Window>,
    ) -> FlameResult<flamewm_api::window::WindowSnapshot> {
        let tree_cookie = self.conn.query_tree(window).map_err(io_error)?;
        let states_cookie = self
            .conn
            .get_property(
                false,
                window,
                self.atom("_NET_WM_STATE"),
                AtomEnum::ANY,
                0,
                u32::MAX,
            )
            .map_err(io_error)?;
        let desktop_cookie = self
            .conn
            .get_property(
                false,
                window,
                self.atom("_NET_WM_DESKTOP"),
                AtomEnum::ANY,
                0,
                u32::MAX,
            )
            .map_err(io_error)?;
        let name_cookie = self
            .conn
            .get_property(
                false,
                window,
                self.atom("_NET_WM_NAME"),
                AtomEnum::ANY,
                0,
                u32::MAX,
            )
            .ok();
        let class_cookie = self
            .conn
            .get_property(
                false,
                window,
                self.atom("WM_CLASS"),
                AtomEnum::ANY,
                0,
                u32::MAX,
            )
            .map_err(io_error)?;
        let parent = tree_cookie.reply().map_err(io_error)?.parent;
        let geometry_window = if parent == self.root { window } else { parent };
        let geometry_cookie = self.conn.get_geometry(geometry_window).map_err(io_error)?;
        let states: Vec<u32> = states_cookie
            .reply()
            .map_err(io_error)?
            .value32()
            .map_or_else(Vec::new, Iterator::collect);
        let desktop = desktop_cookie
            .reply()
            .map_err(io_error)?
            .value32()
            .and_then(|mut values| values.next())
            .unwrap_or(0);
        let title_bytes = match name_cookie.and_then(|cookie| cookie.reply().ok()) {
            Some(reply) => reply.value,
            None => self.property_bytes(window, "WM_NAME")?,
        };
        let title = shared::text_from_bytes(&title_bytes);
        let wm_class_bytes = class_cookie.reply().map_err(io_error)?.value;
        let (wm_instance, wm_class) = parse_wm_class(&wm_class_bytes);
        let geometry = geometry_cookie.reply().map_err(io_error)?;
        let outer = Rect::new(
            i32::from(geometry.x),
            i32::from(geometry.y),
            i32::from(geometry.width),
            i32::from(geometry.height),
        );
        let output = output_for_window_rect(&display.outputs, outer)
            .unwrap_or_else(|| OutputId::new("default"));
        let has = |name: &str| states.contains(&self.atom(name));
        let state = if has("_NET_WM_STATE_FULLSCREEN") {
            flamewm_api::window::WindowState::Fullscreen
        } else if has("_NET_WM_STATE_HIDDEN") {
            flamewm_api::window::WindowState::Minimized
        } else if has("_NET_WM_STATE_MAXIMIZED_VERT") && has("_NET_WM_STATE_MAXIMIZED_HORZ") {
            flamewm_api::window::WindowState::Maximized
        } else {
            flamewm_api::window::WindowState::Normal
        };
        Ok(flamewm_api::window::WindowSnapshot {
            reference: WindowRef::new(u64::from(window), self.ensure_generation(window)),
            title,
            app_id: window_application_id(&self.catalog, &wm_instance, &wm_class),
            outer_geometry: outer,
            restore_geometry: outer,
            state,
            sticky: has("_NET_WM_STATE_STICKY"),
            focused: active == Some(window),
            workspace: WorkspaceRef::new(signed(desktop) as i32, workspace.revision),
            output,
            state_generation: self.generation_of(window),
        })
    }
}

impl WindowPort for X11Desktop {
    fn get(&self, reference: WindowRef) -> FlameResult<flamewm_api::window::WindowSnapshot> {
        let window = self.check_window(reference)?;
        let display = self.display_snapshot()?;
        let workspace = self.workspace_snapshot()?;
        let active = self
            .property32(self.root, "_NET_ACTIVE_WINDOW")?
            .first()
            .copied();
        self.snapshot_window(window, &display, &workspace, active)
    }
    fn snapshot(&self) -> FlameResult<Vec<flamewm_api::window::WindowSnapshot>> {
        let windows = self.managed_windows()?;
        // Root-level context is read once per snapshot, not once per window (the old
        // per-window get() re-scanned managed_windows and re-read the root each time).
        let (Ok(display), Ok(workspace), Ok(active)) = (
            self.display_snapshot(),
            self.workspace_snapshot(),
            self.property32(self.root, "_NET_ACTIVE_WINDOW"),
        ) else {
            return Ok(Vec::new());
        };
        let active = active.first().copied();
        Ok(windows
            .into_iter()
            .filter_map(|window| {
                self.snapshot_window(window, &display, &workspace, active)
                    .ok()
            })
            .collect())
    }
    fn activate(&mut self, w: WindowRef) -> FlameResult<()> {
        let window = self.check_window(w)?;
        self.bump_generation(window);
        self.client_message(window, "_NET_ACTIVE_WINDOW", [2, CURRENT_TIME, 0, 0, 0])
    }
    fn minimize(&mut self, w: WindowRef) -> FlameResult<()> {
        // EWMH state route: ADD _NET_WM_STATE_HIDDEN so the in-WM minimize
        // path runs with ignore_unmap protection and stays managed. Raw
        // unmap would trigger handle_unmap -> unmanage (self-destruction).
        let window = self.check_window(w)?;
        self.bump_generation(window);
        self.send_state(
            window,
            self.atom("_NET_WM_STATE_HIDDEN"),
            FeatureAction::Add,
        )
    }
    fn maximize(&mut self, w: WindowRef) -> FlameResult<()> {
        let window = self.check_window(w)?;
        self.bump_generation(window);
        for name in [
            "_NET_WM_STATE_MAXIMIZED_VERT",
            "_NET_WM_STATE_MAXIMIZED_HORZ",
        ] {
            self.send_state(window, self.atom(name), FeatureAction::Add)?;
        }
        Ok(())
    }
    fn restore(&mut self, w: WindowRef) -> FlameResult<()> {
        let window = self.check_window(w)?;
        self.bump_generation(window);
        // Restore means unminimize. Maximize is an independent placement and must survive a
        // minimize/restore cycle: removing the MAXIMIZED atoms here would return the window
        // demaximized (taskbar click on a minimized maximized window).
        for name in RESTORE_REMOVED_STATES {
            self.send_state(window, self.atom(name), FeatureAction::Remove)?;
        }
        Ok(())
    }
    fn close(&mut self, w: WindowRef) -> FlameResult<()> {
        let window = self.check_window(w)?;
        self.bump_generation(window);
        self.generations.borrow_mut().remove(&window);
        self.client_message(window, "_NET_CLOSE_WINDOW", [CURRENT_TIME, 2, 0, 0, 0])
    }
    fn set_outer_geometry(&mut self, w: WindowRef, geometry: Rect) -> FlameResult<()> {
        let geometry = shared::window_geometry_aux(geometry)?;
        let window = self.check_window(w)?;
        self.bump_generation(window);
        self.conn
            .configure_window(window, &geometry)
            .map_err(io_error)?
            .check()
            .map_err(io_error)?;
        self.flush()
    }
    fn work_area(&self, _w: WindowRef) -> FlameResult<Rect> {
        self.root_work_area()
    }
    fn output(&self, w: WindowRef) -> FlameResult<OutputId> {
        Ok(self.get(w)?.output)
    }
    fn feature_snapshot(&self, w: WindowRef) -> FlameResult<WindowFeatureSnapshot> {
        let states = self.property32(self.check_window(w)?, "_NET_WM_STATE")?;
        Ok(shared::feature_snapshot(&states, |name| self.atom(name)))
    }
    fn apply_window_feature(
        &mut self,
        w: WindowRef,
        feature: WindowFeature,
        action: FeatureAction,
    ) -> FlameResult<()> {
        let atom = match feature {
            WindowFeature::Sticky => "_NET_WM_STATE_STICKY",
            WindowFeature::Modal => "_NET_WM_STATE_MODAL",
            WindowFeature::Shaded => "_NET_WM_STATE_SHADED",
            WindowFeature::SkipTaskbar => "_NET_WM_STATE_SKIP_TASKBAR",
            WindowFeature::SkipPager => "_NET_WM_STATE_SKIP_PAGER",
            WindowFeature::Above => "_NET_WM_STATE_ABOVE",
            WindowFeature::Below => "_NET_WM_STATE_BELOW",
            WindowFeature::DemandsAttention => "_NET_WM_STATE_DEMANDS_ATTENTION",
        };
        let window = self.check_window(w)?;
        self.bump_generation(window);
        self.send_state(window, self.atom(atom), action)
    }
}

impl WorkspacePort for X11Desktop {
    fn workspace_snapshot(&self) -> FlameResult<WorkspaceSnapshot> {
        let count = self
            .property32(self.root, "_NET_NUMBER_OF_DESKTOPS")?
            .first()
            .copied()
            .unwrap_or(1) as usize;
        let active_index = self
            .property32(self.root, "_NET_CURRENT_DESKTOP")?
            .first()
            .copied()
            .unwrap_or(0) as usize;
        let bytes = self.property_bytes(self.root, "_NET_DESKTOP_NAMES")?;
        Ok(shared::workspace_snapshot(
            count,
            active_index,
            &bytes,
            workspace_generation(),
        ))
    }
    fn activate_workspace(&mut self, index: usize, expected_revision: u64) -> FlameResult<()> {
        // Only the desktop count is needed here; reading the full snapshot
        // costs three blocking GetProperty round trips. The revision is checked
        // against the workspace generation counter, not a second snapshot fetch.
        let count = self
            .property32(self.root, "_NET_NUMBER_OF_DESKTOPS")?
            .first()
            .copied()
            .unwrap_or(1)
            .max(1) as usize;
        if expected_revision != workspace_generation() {
            return Err(FlameError::stale("workspace revision changed"));
        }
        if index >= count {
            return Err(FlameError::invalid("workspace index is outside topology"));
        }
        self.client_message(
            self.root,
            "_NET_CURRENT_DESKTOP",
            [index as u32, CURRENT_TIME, 0, 0, 0],
        )?;
        Ok(())
    }
    fn move_window_to_workspace(&mut self, w: WindowRef, target: usize) -> FlameResult<()> {
        let current = self.workspace_snapshot()?;
        if target >= current.count {
            return Err(FlameError::invalid("workspace index is outside topology"));
        }
        let window = self.check_window(w)?;
        self.bump_generation(window);
        self.client_message(
            window,
            "_NET_WM_DESKTOP",
            [target as u32, CURRENT_TIME, 0, 0, 0],
        )
    }
    fn switch_workspace_with_window(
        &mut self,
        _w: WindowRef,
        _target: usize,
        _revision: u64,
    ) -> FlameResult<()> {
        Err(unsupported(
            "X11 EWMH has no atomic workspace-and-window transaction",
        ))
    }
    fn apply_workspace_plan(&mut self, plan: &WorkspacePlan) -> FlameResult<()> {
        plan.validate()?;
        if matches!(plan.kind, WorkspacePlanKind::Activate) {
            return self.activate_workspace(plan.new_active, plan.expected_revision);
        }
        // The WM observes these root writes as an external change and adopts the
        // topology, bumping the workspace generation once (see events.rs).
        flamewm_x11_desktop::apply_workspace_plan_to_root(&self.conn, self.root, plan)
    }
}

impl InputPort for X11Desktop {
    fn root_pointer(&self) -> FlameResult<PointerPosition> {
        shared::root_pointer(&self.conn, self.root, || self.display_snapshot())
    }
}

impl ApplicationPort for X11Desktop {
    fn launch(
        &mut self,
        app: &DesktopAppId,
        options: &ApplicationLaunchOptions,
    ) -> FlameResult<()> {
        self.catalog.launch(app, options)
    }
    fn launch_uri(&mut self, uri: &str) -> FlameResult<()> {
        Command::new("xdg-open")
            .arg(uri)
            .spawn()
            .map(|_| ())
            .map_err(io_error)
    }
}

impl SessionPort for X11Desktop {
    fn session_capabilities(&self) -> SessionCapabilities {
        SessionCapabilities {
            lock: Path::new("/usr/bin/loginctl").exists(),
            logout: flamewm_x11_desktop::session_logout_supported(),
            suspend: Path::new("/usr/bin/systemctl").exists(),
            reboot: Path::new("/usr/bin/systemctl").exists(),
            shutdown: Path::new("/usr/bin/systemctl").exists(),
        }
    }
    fn perform_session_action(&mut self, action: SessionAction) -> FlameResult<()> {
        let (program, args): (&str, &[&str]) = match action {
            SessionAction::Lock => return flamewm_x11_desktop::start_lock_screen(),
            SessionAction::LockPreview => return flamewm_x11_desktop::start_lock_preview(),
            SessionAction::ScreensaverPreview => {
                return flamewm_x11_desktop::start_screensaver_preview();
            }
            SessionAction::Logout => {
                if !flamewm_x11_desktop::session_logout_supported() {
                    return Err(FlameError::new(
                        ErrorCode::Unavailable,
                        "logout requires a managed FlameWM session",
                    ));
                }
                flamewm_x11_desktop::request_session_logout();
                return Ok(());
            }
            SessionAction::Suspend => ("systemctl", &["suspend"]),
            SessionAction::Reboot => ("systemctl", &["reboot"]),
            SessionAction::Shutdown => ("systemctl", &["poweroff"]),
        };
        Command::new(program)
            .args(args)
            .status()
            .map_err(io_error)?
            .success()
            .then_some(())
            .ok_or_else(|| {
                FlameError::new(ErrorCode::PermissionDenied, "session action command failed")
            })
    }
}

/// Process-wide passive-grab state for Settings hotkeys. `X11Desktop` is
/// rebuilt per event, so the grab set must not live on the instance.
static SHORTCUT_GRABS: Mutex<ShortcutGrabs> = Mutex::new(ShortcutGrabs::new());

fn shortcut_grabs() -> MutexGuard<'static, ShortcutGrabs> {
    SHORTCUT_GRABS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Resolves a `KeyPress` (keycode and modifier state) to the active action id.
pub(crate) fn shortcut_action_for_key(keycode: u8, state: u16) -> Option<String> {
    let matched = shortcut_grabs()
        .action_for_key(keycode, state)
        .map(str::to_owned);
    if wm::key_diag_enabled() {
        // Same masking as ShortcutGrabs::action_for_key: NumLock/CapsLock ignored.
        let lock_bits =
            flamewm_x11_desktop::shortcuts::MOD_LOCK | flamewm_x11_desktop::shortcuts::MOD_NUM_LOCK;
        eprintln!(
            "flamewm: shortcut lookup keycode={} masked_state={:04x} grab_matched={} action={}",
            keycode,
            state & !lock_bits,
            matched.is_some(),
            matched.as_deref().unwrap_or("none")
        );
    }
    matched
}

impl ShortcutPort for X11Desktop {
    fn prepare_shortcuts(&mut self, desired: &BTreeMap<String, KeyBinding>) -> FlameResult<()> {
        let table = read_keyboard_table(&*self.conn)?;
        shortcut_grabs().prepare(desired, &table)
    }
    fn commit_shortcuts(&mut self) -> FlameResult<()> {
        let mut backend = XGrabBackend::new(&*self.conn, self.root);
        shortcut_grabs().commit(&mut backend)
    }
    fn rollback_shortcuts(&mut self) {
        let mut backend = XGrabBackend::new(&*self.conn, self.root);
        shortcut_grabs().rollback(&mut backend);
    }
}

/// Diagnostic only (`FLAMEWM_KEY_DIAG=1)`: one line per committed startup binding
/// (the effective set, persisted hotkeys over defaults) with the keysym and
/// every keycode the server maps that keysym to. The grab itself is performed by
/// `PlatformHost::commit_startup_shortcuts` through `ShortcutPort`.
pub(crate) fn log_committed_startup_shortcuts(
    port: &X11Desktop,
    bindings: &BTreeMap<String, KeyBinding>,
) {
    if !wm::key_diag_enabled() {
        return;
    }
    let table = match read_keyboard_table(&*port.conn) {
        Ok(table) => table,
        Err(error) => {
            eprintln!(
                "flamewm: shortcut commit diag: keyboard table unreadable: {}",
                error.message
            );
            return;
        }
    };
    for (action, binding) in bindings {
        let Some(value) = binding.0.as_deref() else {
            eprintln!("flamewm: shortcut committed action={action} binding=unbound");
            continue;
        };
        match flamewm_x11_desktop::shortcuts::parse_binding(value) {
            Ok(chord) => {
                let keycodes = flamewm_x11_desktop::shortcuts::KeycodeLookup::keycodes_for(
                    &table,
                    chord.keysym,
                );
                eprintln!(
                    "flamewm: shortcut committed action={action} binding={value} keysym=0x{:04x} keycodes={keycodes:?}",
                    chord.keysym
                );
            }
            Err(error) => eprintln!(
                "flamewm: shortcut committed action={action} binding={value} parse_error={}",
                error.message
            ),
        }
    }
}

impl MainLoopPort for X11Desktop {
    fn add_poll(
        &mut self,
        _fd: i32,
        _events: FdEvents,
        _callback: FdCallback,
    ) -> FlameResult<FdHandle> {
        Err(unsupported("desktop host owns X11 event loop"))
    }
    fn remove_poll(&mut self, _handle: FdHandle) {}
    fn add_timer(
        &mut self,
        _delay_ms: u64,
        _callback: TimerCallback,
        _repeat: bool,
    ) -> FlameResult<TimerHandle> {
        Err(unsupported("desktop host owns timers"))
    }
    fn remove_timer(&mut self, _handle: TimerHandle) {}
    fn defer(&mut self, _callback: TimerCallback) -> FlameResult<TimerHandle> {
        Err(unsupported("desktop host owns deferred callbacks"))
    }
}
const BACKGROUND_UNSUPPORTED: &str = "X11 desktop adapter does not own wallpaper rendering";
const TRAY_UNSUPPORTED: &str = "X11 desktop adapter does not implement system tray ownership";

impl BackgroundPort for X11Desktop {
    fn project_background(&mut self, _state: &BackgroundState) -> FlameResult<()> {
        Err(unsupported(BACKGROUND_UNSUPPORTED))
    }
    fn reload_background(&mut self) -> FlameResult<()> {
        Err(unsupported(BACKGROUND_UNSUPPORTED))
    }
}
impl TrayPort for X11Desktop {
    fn set_tray_owner(&mut self, _output: &OutputId) -> FlameResult<()> {
        Err(unsupported(TRAY_UNSUPPORTED))
    }
    fn clear_tray_owner(&mut self) -> FlameResult<()> {
        Err(unsupported(TRAY_UNSUPPORTED))
    }
    fn tray_owner(&self) -> FlameResult<Option<OutputId>> {
        Err(unsupported(TRAY_UNSUPPORTED))
    }
}
impl WorkAreaPort for X11Desktop {
    fn base_work_areas(&self) -> FlameResult<Vec<(OutputId, Rect)>> {
        let work_area = self.root_work_area()?;
        let outputs = self.display_snapshot()?;
        Ok(shared::base_work_areas(work_area, &outputs.outputs))
    }
    fn apply_flame_reservations(&mut self, _reservations: &[(OutputId, Rect)]) -> FlameResult<()> {
        Err(unsupported(
            "X11 desktop adapter does not own work-area reservations",
        ))
    }
    fn request_recompute(&mut self) {}
}

// Only a single RandR output may use the drawable root as a fallback.
// Disabled outputs on multi-monitor hardware must remain geometryless.
fn single_output_root_geometry(
    single_output: bool,
    root_size: Size,
    modes: &[DisplayMode],
) -> (Rect, ModeId) {
    if !single_output || !root_size.is_valid() {
        return (Rect::default(), ModeId(0));
    }
    let mode = modes
        .iter()
        .find(|mode| mode.resolution == root_size)
        .map_or(ModeId(0), |mode| mode.id);
    (Rect::from_parts(Point::new(0, 0), root_size), mode)
}

impl DisplayPort for X11Desktop {
    fn display_snapshot(&self) -> FlameResult<DisplaySnapshot> {
        let resources = self
            .conn
            .randr_get_screen_resources_current(self.root)
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?;
        // Stable physical output identity: never infer primary from RandR
        // resource enumeration order (it can change after hotplug/rotation).
        let primary_output = self
            .conn
            .randr_get_output_primary(self.root)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map_or(0, |reply| reply.output);
        // A multi-output server can have connected but disabled CRTCs. Only a
        // genuinely single RandR output may use Xephyr's CRTC=0 fallback.
        let single_randr_output = resources.outputs.len() == 1;
        let mut outputs = Vec::new();
        for output in resources.outputs {
            let info = self
                .conn
                .randr_get_output_info(output, resources.config_timestamp)
                .map_err(io_error)?
                .reply()
                .map_err(io_error)?;
            if u8::from(info.connection) != u8::from(randr::Connection::CONNECTED) {
                continue;
            }
            let modes = shared::display_modes(&info.modes, &resources.modes);
            let crtc = info.crtc;
            let (geometry, current_mode) = if crtc != 0 {
                let c = self
                    .conn
                    .randr_get_crtc_info(crtc, resources.config_timestamp)
                    .map_err(io_error)?
                    .reply()
                    .map_err(io_error)?;
                (
                    Rect::new(
                        i32::from(c.x),
                        i32::from(c.y),
                        i32::from(c.width),
                        i32::from(c.height),
                    ),
                    ModeId(u64::from(c.mode)),
                )
            } else {
                let root_size = self.root_size();
                // A lone Xephyr/legacy output can draw into the root even
                // when RandR has no matching mode. Keep its real geometry and
                // use mode 0 only to denote an unavailable mode switch.
                single_output_root_geometry(single_randr_output, root_size, &modes)
            };
            // Do not turn an attached-but-disabled RandR output into an
            // imaginary root-sized monitor. Its CRTC is zero and its geometry
            // is invalid; the UI may show it as disabled, but panel/workarea
            // ownership MUST NOT treat it as another active display. Xephyr's
            // single-output CRTC=0 compatibility is handled above only when
            // a reported mode exactly matches the real root resolution.
            outputs.push(OutputSnapshot {
                id: OutputId::new(String::from_utf8_lossy(&info.name).into_owned()),
                connector: String::from_utf8_lossy(&info.name).into_owned(),
                edid_identity: String::new(),
                connected: true,
                primary: output == primary_output,
                geometry,
                current_mode,
                modes,
                shell_scale_percent: 100,
            });
        }
        if outputs.is_empty() {
            let root_size = self.root_size();
            if root_size.is_valid() {
                outputs.push(OutputSnapshot {
                    id: OutputId::new("default"),
                    connector: String::from("default"),
                    edid_identity: String::new(),
                    connected: true,
                    primary: true,
                    geometry: Rect::from_parts(Point::new(0, 0), root_size),
                    current_mode: ModeId(0),
                    modes: Vec::new(),
                    shell_scale_percent: 100,
                });
            }
        }
        // Some nested and legacy X servers do not implement RandR primary.
        // In that case stabilize fallback by connector, not enumeration order.
        if !outputs
            .iter()
            .any(|output| output.primary && output.geometry.is_valid())
        {
            for output in &mut outputs {
                output.primary = false;
            }
            outputs.sort_by(|a, b| a.connector.cmp(&b.connector));
            if let Some(output) = outputs.iter_mut().find(|output| output.geometry.is_valid()) {
                output.primary = true;
            }
        }
        Ok(DisplaySnapshot {
            generation: u64::from(resources.config_timestamp),
            outputs,
            pending: None,
        })
    }
    fn apply_mode(&mut self, output_name: &OutputId, mode: ModeId) -> FlameResult<TransactionId> {
        shared::apply_mode(
            &self.conn,
            self.root,
            &mut self.pending_mode,
            &mut self.next_transaction,
            output_name,
            mode,
        )
    }
    fn keep_mode(&mut self, transaction: TransactionId) -> FlameResult<()> {
        if self.pending_mode.map(|p| p.transaction) != Some(transaction) {
            return Err(FlameError::not_found("RandR mode transaction not found"));
        }
        self.pending_mode = None;
        Ok(())
    }
    fn revert_mode(&mut self, transaction: TransactionId) -> FlameResult<()> {
        let pending = self
            .pending_mode
            .ok_or_else(|| FlameError::not_found("RandR mode transaction not found"))?;
        if pending.transaction != transaction {
            return Err(FlameError::not_found("RandR mode transaction not found"));
        }
        let resources = self
            .conn
            .randr_get_screen_resources_current(self.root)
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?;
        let status = self
            .conn
            .randr_set_crtc_config(
                pending.crtc,
                CURRENT_TIME,
                resources.config_timestamp,
                pending.old_x,
                pending.old_y,
                pending.old_mode,
                pending.old_rotation,
                &[pending.output],
            )
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?
            .status;
        if u8::from(status) != u8::from(randr::SetConfig::SUCCESS) {
            return Err(FlameError::new(
                ErrorCode::EngineRejected,
                "RandR rejected mode rollback",
            ));
        }
        self.pending_mode = None;
        self.flush()
    }
}

#[cfg(test)]
mod restore_state_tests {
    use super::RESTORE_REMOVED_STATES;

    #[test]
    fn restore_unminimizes_without_clearing_maximize() {
        assert_eq!(RESTORE_REMOVED_STATES, ["_NET_WM_STATE_HIDDEN"]);
        assert!(!RESTORE_REMOVED_STATES.contains(&"_NET_WM_STATE_MAXIMIZED_VERT"));
        assert!(!RESTORE_REMOVED_STATES.contains(&"_NET_WM_STATE_MAXIMIZED_HORZ"));
    }
}

#[cfg(test)]
mod output_derivation_tests {
    use super::{DisplayMode, OutputId, OutputSnapshot, Rect, Size, output_for_window_rect};
    use flamewm_api::ModeId;

    fn output(name: &str, geometry: Rect, primary: bool) -> OutputSnapshot {
        OutputSnapshot {
            id: OutputId::new(name),
            connector: name.to_owned(),
            edid_identity: String::new(),
            connected: true,
            primary,
            geometry,
            current_mode: ModeId(1),
            modes: Vec::new(),
            shell_scale_percent: 100,
        }
    }

    fn two_outputs() -> Vec<OutputSnapshot> {
        vec![
            output("A", Rect::new(0, 0, 1000, 800), true),
            output("B", Rect::new(1000, 0, 1000, 800), false),
        ]
    }

    #[test]
    fn centre_containment_picks_the_output_under_the_window_centre() {
        let outputs = two_outputs();
        let on_b = Rect::new(1100, 100, 200, 200);
        assert_eq!(
            output_for_window_rect(&outputs, on_b),
            Some(OutputId::new("B"))
        );
        // Straddling the seam: centre at x=1000 belongs to B.
        let seam = Rect::new(900, 100, 200, 200);
        assert_eq!(
            output_for_window_rect(&outputs, seam),
            Some(OutputId::new("B"))
        );
    }

    #[test]
    fn negative_origin_output_is_found_by_centre() {
        let outputs = vec![
            output("LEFT", Rect::new(-1920, 0, 1920, 1080), false),
            output("A", Rect::new(0, 0, 1920, 1080), true),
        ];
        let window = Rect::new(-1800, 100, 400, 300);
        assert_eq!(
            output_for_window_rect(&outputs, window),
            Some(OutputId::new("LEFT"))
        );
        let spanning = Rect::new(-100, 100, 400, 300);
        assert_eq!(
            output_for_window_rect(&outputs, spanning),
            Some(OutputId::new("A"))
        );
    }

    #[test]
    fn off_screen_window_falls_back_to_largest_overlap_then_primary() {
        let outputs = two_outputs();
        // Centre (2100, 800) is off both outputs; overlap with B is 100x100.
        let mostly_b = Rect::new(1900, 700, 400, 200);
        assert_eq!(
            output_for_window_rect(&outputs, mostly_b),
            Some(OutputId::new("B"))
        );
        // No overlap at all: primary output wins.
        let nowhere = Rect::new(5000, 5000, 100, 100);
        assert_eq!(
            output_for_window_rect(&outputs, nowhere),
            Some(OutputId::new("A"))
        );
    }

    #[test]
    fn lone_xephyr_output_without_mode_keeps_its_root_geometry() {
        let size = Size::new(1350, 641);
        assert_eq!(
            super::single_output_root_geometry(true, size, &[]),
            (Rect::new(0, 0, 1350, 641), ModeId(0))
        );
        assert_eq!(
            super::single_output_root_geometry(false, size, &[]),
            (Rect::default(), ModeId(0))
        );
        assert_eq!(
            super::single_output_root_geometry(true, Size::new(0, 0), &[]),
            (Rect::default(), ModeId(0))
        );
    }

    #[test]
    fn lone_xephyr_output_uses_real_mode_when_advertised() {
        let mode = DisplayMode {
            id: ModeId(42),
            resolution: Size::new(1350, 641),
            refresh_millihz: 60_000,
            preferred: true,
        };
        assert_eq!(
            super::single_output_root_geometry(true, Size::new(1350, 641), &[mode]),
            (Rect::new(0, 0, 1350, 641), ModeId(42))
        );
    }

    #[test]
    fn no_outputs_yields_none_so_caller_keeps_its_fallback() {
        assert_eq!(output_for_window_rect(&[], Rect::new(0, 0, 10, 10)), None);
    }
}

#[cfg(test)]
mod application_identity_tests {
    use super::{ApplicationCatalog, DesktopAppId, parse_wm_class, window_application_id};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn wm_class_property_preserves_instance_and_class_order() {
        assert_eq!(
            parse_wm_class(b"firefox\0Firefox\0"),
            ("firefox".to_owned(), "Firefox".to_owned())
        );
    }

    #[test]
    fn unmatched_identity_uses_normalized_class_then_instance_fallback() {
        let catalog = ApplicationCatalog::default();
        assert_eq!(
            window_application_id(&catalog, "  MyInstance  ", "  MyClass  "),
            DesktopAppId::new("myclass")
        );
        assert_eq!(
            window_application_id(&catalog, "  MyInstance  ", "  "),
            DesktopAppId::new("myinstance")
        );
        assert_eq!(
            window_application_id(&catalog, "", ""),
            DesktopAppId::new("unknown")
        );
    }

    #[test]
    fn matched_window_identity_uses_canonical_desktop_entry_id() {
        let root = std::env::temp_dir().join(format!(
            "flamewm-window-identity-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock after epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create desktop fixture directory");
        std::fs::write(
            root.join("org.example.Editor.desktop"),
            "[Desktop Entry]\nType=Application\nName=Editor\nExec=editor\nStartupWMClass=EditorClass\n",
        )
        .expect("write desktop fixture");
        let catalog = ApplicationCatalog::from_dirs(std::slice::from_ref(&root))
            .expect("load desktop fixture");

        assert_eq!(
            window_application_id(&catalog, "editor-instance", "EditorClass"),
            DesktopAppId::new("org.example.Editor.desktop")
        );
        std::fs::remove_dir_all(root).expect("remove desktop fixture directory");
    }
}

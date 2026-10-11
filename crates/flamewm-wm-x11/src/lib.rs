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
mod runtime;
mod size_hints;
pub(crate) mod snap_preview;
mod wm;

pub use flamewm_render_x11::apply_cursor_theme;
pub use runtime::{
    SettingsObserver, SettingsWake, persisted_cursor_theme, run, run_with_settings_observer,
};
pub use wm::WmConfig;

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::rc::Rc;

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
use flamewm_x11_desktop::shortcuts::{ShortcutGrabs, XGrabBackend, read_keyboard_table};
use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as RandrConnectionExt};
use x11rb::protocol::xproto::{
    self, Atom, AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt, EventMask, Window,
};
use x11rb::rust_connection::RustConnection;

#[derive(Debug, Clone, Copy)]
struct PendingMode {
    transaction: TransactionId,
    crtc: randr::Crtc,
    output: randr::Output,
    old_mode: randr::Mode,
    old_x: i16,
    old_y: i16,
    old_rotation: randr::Rotation,
}

/// Process-wide workspace revision fence. `Wm` advances it on desktop root
/// property changes; `X11Desktop` is rebuilt per event, so the counter must
/// not live on the instance.
static WORKSPACE_GENERATION: AtomicU64 = AtomicU64::new(1);

/// Advances the workspace revision fence. Called from the WM when
/// _NET_CURRENT_DESKTOP, _NET_NUMBER_OF_DESKTOPS, or _NET_DESKTOP_NAMES change.
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
    fn property32(&self, window: Window, name: &str) -> FlameResult<Vec<u32>> {
        let reply = self
            .conn
            .get_property(false, window, self.atom(name), AtomEnum::ANY, 0, u32::MAX)
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?;
        Ok(reply.value32().map_or_else(Vec::new, Iterator::collect))
    }
    fn property_bytes(&self, window: Window, name: &str) -> FlameResult<Vec<u8>> {
        Ok(self
            .conn
            .get_property(false, window, self.atom(name), AtomEnum::ANY, 0, u32::MAX)
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?
            .value)
    }
    fn managed_windows(&self) -> FlameResult<Vec<Window>> {
        let support_window = self
            .property32(self.root, "_NET_SUPPORTING_WM_CHECK")?
            .first()
            .copied();
        Ok(self
            .property32(self.root, "_NET_CLIENT_LIST")?
            .into_iter()
            .map(|window| window as Window)
            .filter(|&window| Some(window) != support_window)
            .filter(|&window| {
                let is_dock = self
                    .property32(window, "_NET_WM_WINDOW_TYPE")
                    .map(|types| types.contains(&self.atom("_NET_WM_WINDOW_TYPE_DOCK")))
                    .unwrap_or(false);
                // Identity is advisory, not a liveness gate: a freshly
                // mapped client may not have set WM_CLASS/_NET_WM_NAME yet,
                // and a dead window must not linger. Liveness is proven by
                // the window still existing on the server.
                if is_dock {
                    return false;
                }
                self.conn.get_geometry(window).is_ok()
            })
            .collect())
    }
    fn client_message(&self, window: Window, name: &str, data: [u32; 5]) -> FlameResult<()> {
        let event = ClientMessageEvent {
            response_type: x11rb::protocol::xproto::CLIENT_MESSAGE_EVENT,
            format: 32,
            sequence: 0,
            window,
            type_: self.atom(name),
            data: ClientMessageData::from(data),
        };
        self.conn
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )
            .map_err(io_error)?
            .check()
            .map_err(io_error)?;
        self.flush()
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
        let desktop = self
            .property32(self.root, "_NET_CURRENT_DESKTOP")?
            .first()
            .copied()
            .unwrap_or(0) as usize;
        let values = self.property32(self.root, "_NET_WORKAREA")?;
        let start = desktop
            .checked_mul(4)
            .ok_or_else(|| FlameError::unavailable("root _NET_WORKAREA is invalid"))?;
        if values.len() < start.saturating_add(4) {
            return Err(FlameError::unavailable("root _NET_WORKAREA is unavailable"));
        }
        let area = Rect::new(
            signed(values[start]),
            signed(values[start + 1]),
            values[start + 2] as i32,
            values[start + 3] as i32,
        );
        area.is_valid()
            .then_some(area)
            .ok_or_else(|| FlameError::unavailable("root _NET_WORKAREA is invalid"))
    }
    fn send_state(&self, window: Window, atom: Atom, action: FeatureAction) -> FlameResult<()> {
        let action = match action {
            FeatureAction::Remove => 0,
            FeatureAction::Add => 1,
            FeatureAction::Toggle => 2,
        };
        self.client_message(window, "_NET_WM_STATE", [action, atom, 0, 1, 0])
    }
    fn text(&self, window: Window, name: &str) -> FlameResult<String> {
        Ok(String::from_utf8_lossy(&self.property_bytes(window, name)?)
            .trim_end_matches('\0')
            .to_owned())
    }
}

fn io_error<E: std::fmt::Display>(error: E) -> FlameError {
    FlameError::new(ErrorCode::IoFailure, error.to_string())
}
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
    catalog
        .find_by_window_identity(&identity)
        .map(|entry| entry.id.clone())
        .unwrap_or_else(|| {
            let fallback = [wm_class, wm_instance]
                .into_iter()
                .map(str::trim)
                .find(|value| !value.is_empty())
                .map(str::to_ascii_lowercase)
                .unwrap_or_else(|| "unknown".to_owned());
            DesktopAppId::new(fallback)
        })
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

impl WindowPort for X11Desktop {
    fn get(&self, reference: WindowRef) -> FlameResult<flamewm_api::window::WindowSnapshot> {
        let window = self.check_window(reference)?;
        let tree = self
            .conn
            .query_tree(window)
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?;
        let geometry = if tree.parent != self.root {
            self.conn
                .get_geometry(tree.parent)
                .map_err(io_error)?
                .reply()
                .map_err(io_error)?
        } else {
            self.conn
                .get_geometry(window)
                .map_err(io_error)?
                .reply()
                .map_err(io_error)?
        };
        let states = self.property32(window, "_NET_WM_STATE")?;
        let desktop = self
            .property32(window, "_NET_WM_DESKTOP")?
            .first()
            .copied()
            .unwrap_or(0);
        let workspace = self.workspace_snapshot()?;
        let output = self
            .display_snapshot()?
            .outputs
            .into_iter()
            .find(|output| {
                output.geometry.contains(Point::new(
                    i32::from(geometry.x) + i32::from(geometry.width) / 2,
                    i32::from(geometry.y) + i32::from(geometry.height) / 2,
                ))
            })
            .map(|output| output.id)
            .unwrap_or_else(|| OutputId::new("default"));
        let title = self
            .text(window, "_NET_WM_NAME")
            .or_else(|_| self.text(window, "WM_NAME"))?;
        let (wm_instance, wm_class) = parse_wm_class(&self.property_bytes(window, "WM_CLASS")?);
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
            outer_geometry: Rect::new(
                i32::from(geometry.x),
                i32::from(geometry.y),
                i32::from(geometry.width),
                i32::from(geometry.height),
            ),
            restore_geometry: Rect::new(
                i32::from(geometry.x),
                i32::from(geometry.y),
                i32::from(geometry.width),
                i32::from(geometry.height),
            ),
            state,
            sticky: has("_NET_WM_STATE_STICKY"),
            focused: self
                .property32(self.root, "_NET_ACTIVE_WINDOW")?
                .first()
                .copied()
                == Some(window),
            workspace: WorkspaceRef::new(signed(desktop) as i32, workspace.revision),
            output,
            state_generation: self.generation_of(window),
        })
    }
    fn snapshot(&self) -> FlameResult<Vec<flamewm_api::window::WindowSnapshot>> {
        Ok(self
            .managed_windows()?
            .into_iter()
            .filter_map(|window| self.get(WindowRef::new(u64::from(window), 0)).ok())
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
        self.send_state(
            window,
            self.atom("_NET_WM_STATE_MAXIMIZED_VERT"),
            FeatureAction::Add,
        )?;
        self.send_state(
            window,
            self.atom("_NET_WM_STATE_MAXIMIZED_HORZ"),
            FeatureAction::Add,
        )
    }
    fn restore(&mut self, w: WindowRef) -> FlameResult<()> {
        let window = self.check_window(w)?;
        self.bump_generation(window);
        self.send_state(
            window,
            self.atom("_NET_WM_STATE_HIDDEN"),
            FeatureAction::Remove,
        )?;
        self.send_state(
            window,
            self.atom("_NET_WM_STATE_MAXIMIZED_VERT"),
            FeatureAction::Remove,
        )?;
        self.send_state(
            window,
            self.atom("_NET_WM_STATE_MAXIMIZED_HORZ"),
            FeatureAction::Remove,
        )
    }
    fn close(&mut self, w: WindowRef) -> FlameResult<()> {
        let window = self.check_window(w)?;
        self.bump_generation(window);
        self.generations.borrow_mut().remove(&window);
        self.client_message(window, "_NET_CLOSE_WINDOW", [CURRENT_TIME, 2, 0, 0, 0])
    }
    fn set_outer_geometry(&mut self, w: WindowRef, geometry: Rect) -> FlameResult<()> {
        if !geometry.is_valid() {
            return Err(FlameError::invalid("window geometry is invalid"));
        }
        let window = self.check_window(w)?;
        self.bump_generation(window);
        self.conn
            .configure_window(
                window,
                &xproto::ConfigureWindowAux::new()
                    .x(geometry.x)
                    .y(geometry.y)
                    .width(
                        u32::try_from(geometry.width)
                            .map_err(|_| FlameError::invalid("window width is invalid"))?,
                    )
                    .height(
                        u32::try_from(geometry.height)
                            .map_err(|_| FlameError::invalid("window height is invalid"))?,
                    ),
            )
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
        let has = |n: &str| states.contains(&self.atom(n));
        Ok(WindowFeatureSnapshot {
            sticky: has("_NET_WM_STATE_STICKY"),
            modal: has("_NET_WM_STATE_MODAL"),
            shaded: has("_NET_WM_STATE_SHADED"),
            skip_taskbar: has("_NET_WM_STATE_SKIP_TASKBAR"),
            skip_pager: has("_NET_WM_STATE_SKIP_PAGER"),
            above: has("_NET_WM_STATE_ABOVE"),
            below: has("_NET_WM_STATE_BELOW"),
            demands_attention: has("_NET_WM_STATE_DEMANDS_ATTENTION"),
        })
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
        let names = bytes
            .split(|b| *b == 0)
            .filter(|v| !v.is_empty())
            .map(|v| String::from_utf8_lossy(v).into_owned())
            .collect();
        Ok(WorkspaceSnapshot {
            revision: workspace_generation(),
            count: count.max(1),
            active_index: active_index.min(count.max(1) - 1),
            last_index: None,
            names,
        })
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
        if !matches!(plan.kind, WorkspacePlanKind::Activate) {
            return Err(unsupported(
                "X11 EWMH cannot atomically apply workspace topology plans",
            ));
        }
        self.activate_workspace(plan.new_active, plan.expected_revision)
    }
}

impl InputPort for X11Desktop {
    fn root_pointer(&self) -> FlameResult<PointerPosition> {
        let reply = self
            .conn
            .query_pointer(self.root)
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?;
        Ok(PointerPosition {
            root: Point::new(i32::from(reply.root_x), i32::from(reply.root_y)),
            output: self
                .display_snapshot()?
                .outputs
                .into_iter()
                .find(|o| {
                    o.geometry
                        .contains(Point::new(i32::from(reply.root_x), i32::from(reply.root_y)))
                })
                .map(|o| o.id),
        })
    }
}

impl ApplicationPort for X11Desktop {
    fn launch(
        &mut self,
        app: &DesktopAppId,
        options: &ApplicationLaunchOptions,
    ) -> FlameResult<()> {
        self.catalog.launch(app, options).map(|_| ())
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
            logout: Path::new("/usr/bin/loginctl").exists(),
            suspend: Path::new("/usr/bin/systemctl").exists(),
            reboot: Path::new("/usr/bin/systemctl").exists(),
            shutdown: Path::new("/usr/bin/systemctl").exists(),
        }
    }
    fn perform_session_action(&mut self, action: SessionAction) -> FlameResult<()> {
        let (program, args): (&str, &[&str]) = match action {
            SessionAction::Lock => ("loginctl", &["lock-session"]),
            SessionAction::Logout => ("loginctl", &["terminate-session", "self"]),
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

/// Resolves a KeyPress (keycode and modifier state) to the active action id.
pub(crate) fn shortcut_action_for_key(keycode: u8, state: u16) -> Option<String> {
    let matched = shortcut_grabs()
        .action_for_key(keycode, state)
        .map(str::to_owned);
    if wm::key_diag_enabled() {
        // Same masking as ShortcutGrabs::action_for_key: NumLock/CapsLock ignored.
        let lock_bits = flamewm_x11_desktop::shortcuts::MOD_LOCK
            | flamewm_x11_desktop::shortcuts::MOD_NUM_LOCK;
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

/// Diagnostic only (FLAMEWM_KEY_DIAG=1): one line per committed startup binding
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
            eprintln!("flamewm: shortcut commit diag: keyboard table unreadable: {}", error.message);
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
impl BackgroundPort for X11Desktop {
    fn project_background(&mut self, _state: &BackgroundState) -> FlameResult<()> {
        Err(unsupported(
            "X11 desktop adapter does not own wallpaper rendering",
        ))
    }
    fn reload_background(&mut self) -> FlameResult<()> {
        Err(unsupported(
            "X11 desktop adapter does not own wallpaper rendering",
        ))
    }
}
impl TrayPort for X11Desktop {
    fn set_tray_owner(&mut self, _output: &OutputId) -> FlameResult<()> {
        Err(unsupported(
            "X11 desktop adapter does not implement system tray ownership",
        ))
    }
    fn clear_tray_owner(&mut self) -> FlameResult<()> {
        Err(unsupported(
            "X11 desktop adapter does not implement system tray ownership",
        ))
    }
    fn tray_owner(&self) -> FlameResult<Option<OutputId>> {
        Err(unsupported(
            "X11 desktop adapter does not implement system tray ownership",
        ))
    }
}
impl WorkAreaPort for X11Desktop {
    fn base_work_areas(&self) -> FlameResult<Vec<(OutputId, Rect)>> {
        let work_area = self.root_work_area()?;
        Ok(self
            .display_snapshot()?
            .outputs
            .into_iter()
            .filter_map(|output| {
                output
                    .geometry
                    .intersection(work_area)
                    .map(|area| (output.id, area))
            })
            .collect())
    }
    fn apply_flame_reservations(&mut self, _reservations: &[(OutputId, Rect)]) -> FlameResult<()> {
        Err(unsupported(
            "X11 desktop adapter does not own work-area reservations",
        ))
    }
    fn request_recompute(&mut self) {}
}

impl DisplayPort for X11Desktop {
    fn display_snapshot(&self) -> FlameResult<DisplaySnapshot> {
        let resources = self
            .conn
            .randr_get_screen_resources_current(self.root)
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?;
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
            let modes = info
                .modes
                .iter()
                .filter_map(|id| resources.modes.iter().find(|m| m.id == *id))
                .map(|m| DisplayMode {
                    id: ModeId(u64::from(m.id)),
                    resolution: Size::new(i32::from(m.width), i32::from(m.height)),
                    refresh_millihz: if m.htotal == 0 || m.vtotal == 0 {
                        0
                    } else {
                        ((m.dot_clock as u64 * 1000) / u64::from(m.htotal) / u64::from(m.vtotal))
                            as i32
                    },
                    preferred: info.modes.first() == Some(&m.id),
                })
                .collect::<Vec<_>>();
            let crtc = info.crtc;
            let (mut geometry, current_mode) = if crtc != 0 {
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
                let root = &self.conn.setup().roots[self.screen];
                let root_size = Size::new(
                    i32::from(root.width_in_pixels),
                    i32::from(root.height_in_pixels),
                );
                // Xephyr exposes its active output with CRTC zero. Prefer the
                // advertised mode when it matches the valid root geometry.
                match modes.iter().find(|mode| mode.resolution == root_size) {
                    Some(mode) if root_size.is_valid() => {
                        (Rect::from_parts(Point::new(0, 0), root_size), mode.id)
                    }
                    _ => (Rect::default(), ModeId(0)),
                }
            };
            if !geometry.is_valid() {
                let root = &self.conn.setup().roots[self.screen];
                let root_size = Size::new(
                    i32::from(root.width_in_pixels),
                    i32::from(root.height_in_pixels),
                );
                if root_size.is_valid() {
                    geometry = Rect::from_parts(Point::new(0, 0), root_size);
                }
            }
            outputs.push(OutputSnapshot {
                id: OutputId::new(String::from_utf8_lossy(&info.name).into_owned()),
                connector: String::from_utf8_lossy(&info.name).into_owned(),
                edid_identity: String::new(),
                connected: true,
                primary: false,
                geometry,
                current_mode,
                modes,
                shell_scale_percent: 100,
            });
        }
        if outputs.is_empty() {
            let root = &self.conn.setup().roots[self.screen];
            let root_size = Size::new(
                i32::from(root.width_in_pixels),
                i32::from(root.height_in_pixels),
            );
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
        Ok(DisplaySnapshot {
            generation: u64::from(resources.config_timestamp),
            outputs,
            pending: None,
        })
    }
    fn apply_mode(&mut self, output_name: &OutputId, mode: ModeId) -> FlameResult<TransactionId> {
        if self.pending_mode.is_some() {
            return Err(FlameError::new(
                ErrorCode::Busy,
                "RandR mode transaction already pending",
            ));
        }
        let resources = self
            .conn
            .randr_get_screen_resources_current(self.root)
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?;
        for output in resources.outputs {
            let info = self
                .conn
                .randr_get_output_info(output, resources.config_timestamp)
                .map_err(io_error)?
                .reply()
                .map_err(io_error)?;
            if u8::from(info.connection) != u8::from(randr::Connection::CONNECTED)
                || String::from_utf8_lossy(&info.name) != output_name.as_str()
            {
                continue;
            }
            let crtc = info.crtc;
            if crtc == 0 || !info.modes.contains(&(mode.0 as u32)) {
                return Err(FlameError::new(
                    ErrorCode::InvalidArgument,
                    "RandR output or mode is not active",
                ));
            }
            let old = self
                .conn
                .randr_get_crtc_info(crtc, resources.config_timestamp)
                .map_err(io_error)?
                .reply()
                .map_err(io_error)?;
            let reply = self
                .conn
                .randr_set_crtc_config(
                    crtc,
                    CURRENT_TIME,
                    resources.config_timestamp,
                    old.x,
                    old.y,
                    mode.0 as u32,
                    old.rotation,
                    &old.outputs,
                )
                .map_err(io_error)?
                .reply()
                .map_err(io_error)?;
            if u8::from(reply.status) != u8::from(randr::SetConfig::SUCCESS) {
                return Err(FlameError::new(
                    ErrorCode::EngineRejected,
                    "RandR rejected mode change",
                ));
            }
            self.next_transaction = self.next_transaction.saturating_add(1).max(1);
            let transaction = TransactionId(self.next_transaction);
            self.pending_mode = Some(PendingMode {
                transaction,
                crtc,
                output,
                old_mode: old.mode,
                old_x: old.x,
                old_y: old.y,
                old_rotation: old.rotation,
            });
            self.flush()?;
            return Ok(transaction);
        }
        Err(FlameError::not_found("RandR output not found"))
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

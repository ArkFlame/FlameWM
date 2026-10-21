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

pub mod shared;
pub mod shortcuts;

pub const ICONIC_STATE: u32 = 3;
const SESSION_SUPERVISOR_PID: &str = "FLAMEWM_SESSION_SUPERVISOR_PID";
const SESSION_LOGOUT_OWNER: &str = "FLAMEWM_SESSION_LOGOUT_OWNER";

static SESSION_LOGOUT_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn session_logout_supported() -> bool {
    session_logout_owner_matches(
        std::env::var_os(SESSION_LOGOUT_OWNER).as_deref(),
        std::env::var_os(SESSION_SUPERVISOR_PID).as_deref(),
        rustix::process::getppid(),
    )
}

pub fn request_session_logout() {
    SESSION_LOGOUT_REQUESTED.store(true, Ordering::Release);
}

pub fn take_session_logout_request() -> bool {
    SESSION_LOGOUT_REQUESTED.swap(false, Ordering::AcqRel)
}

fn supervisor_pid_matches(value: Option<&OsStr>, parent: Option<rustix::process::Pid>) -> bool {
    let Some(value) = value.and_then(|value| value.to_str()) else {
        return false;
    };
    value
        .parse::<i32>()
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|pid| Some(pid) == parent)
}

fn session_logout_owner_matches(
    owner: Option<&OsStr>,
    supervisor: Option<&OsStr>,
    parent: Option<rustix::process::Pid>,
) -> bool {
    owner == Some(OsStr::new("1")) && supervisor_pid_matches(supervisor, parent)
}

pub struct X11Desktop {
    conn: RustConnection,
    root: Window,
    atoms: BTreeMap<String, Atom>,
    next_transaction: u64,
    pending_mode: Option<PendingMode>,
    catalog: Option<std::sync::Arc<ApplicationCatalog>>,
}

impl X11Desktop {
    pub fn connect() -> FlameResult<Self> {
        let (conn, screen) = x11rb::connect(None).map_err(io_error)?;
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
                .map_err(io_error)?
                .reply()
                .map_err(io_error)?
                .atom;
            atoms.insert(name.to_owned(), atom);
        }
        Ok(Self {
            conn,
            root,
            atoms,
            next_transaction: 0,
            pending_mode: None,
            catalog: None,
        })
    }

    #[must_use]
    pub fn with_application_catalog(mut self, catalog: std::sync::Arc<ApplicationCatalog>) -> Self {
        self.catalog = Some(catalog);
        self
    }

    pub fn set_application_catalog(&mut self, catalog: std::sync::Arc<ApplicationCatalog>) {
        self.catalog = Some(catalog);
    }

    fn application_catalog(&self) -> FlameResult<std::sync::Arc<ApplicationCatalog>> {
        self.catalog.clone().ok_or_else(|| {
            FlameError::new(
                ErrorCode::Unavailable,
                "application catalog is not injected",
            )
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
                .map_err(io_error)?;
            let mut next = Vec::new();
            for (parent, cookie) in level.iter().zip(cookies) {
                // A child destroyed mid-walk fails its own query; it has no children
                // left to list, so drop it instead of failing the whole snapshot.
                let tree = match cookie.reply() {
                    Ok(tree) => tree,
                    Err(_) if *parent != self.root => continue,
                    Err(error) => return Err(io_error(error)),
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
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?
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
        let geometry_target = if parent != self.root { parent } else { window };
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
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?
            .value;
        let name_reply = reads
            .net_name
            .map_err(io_error)
            .and_then(|cookie| cookie.reply().map_err(io_error));
        let (title, name_named) = match name_reply {
            Ok(reply) => (shared::text_from_bytes(&reply.value), !reply.value.is_empty()),
            Err(_) => (self.text(window, "WM_NAME")?, false),
        };
        let named = !class_bytes.is_empty() || name_named;
        let class = shared::text_from_bytes(&class_bytes)
            .split('\0')
            .nth(1)
            .unwrap_or_default()
            .to_owned();
        let states = reads
            .states
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?;
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
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?
            .value32()
            .and_then(|mut values| values.next())
            .unwrap_or(0);
        let geometry = reads
            .geometry
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?;
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
            .map(|output| output.id.clone())
            .unwrap_or_else(|| OutputId::new("default"));
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

fn io_error<E: std::fmt::Display>(error: E) -> FlameError {
    FlameError::new(ErrorCode::IoFailure, error.to_string())
}

const LOCK_SCREEN_BINARY: &str = "flamewm-lock";

/// What the Lock session action runs.
#[derive(Debug, PartialEq, Eq)]
enum LockProgram {
    Screen(PathBuf),
    Loginctl,
}

/// Prefers `flamewm-lock` next to the running executable, then on `PATH`,
/// and falls back to `loginctl lock-session` when no executable is found.
fn resolve_lock_program(exe_dir: Option<&Path>, path_var: Option<&OsStr>) -> LockProgram {
    let on_path = path_var
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(LOCK_SCREEN_BINARY));
    exe_dir
        .map(|dir| dir.join(LOCK_SCREEN_BINARY))
        .into_iter()
        .chain(on_path)
        .find(|candidate| {
            candidate
                .metadata()
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
        .map_or(LockProgram::Loginctl, LockProgram::Screen)
}

/// Delay before a crashed lock screen is respawned.
const LOCK_RESPAWN_DELAY: Duration = Duration::from_millis(500);
/// Respawns allowed inside [`LOCK_RESPAWN_WINDOW`] before supervision gives up.
const LOCK_RESPAWN_CAP: usize = 5;
const LOCK_RESPAWN_WINDOW: Duration = Duration::from_secs(30);

/// What the lock supervisor does after the lock process exits.
#[derive(Debug, PartialEq, Eq)]
enum LockSupervision {
    Respawn { delay: Duration },
    Stop,
}

/// A successful exit means the user unlocked, so supervision ends. Any other
/// exit (non-zero status or a signal such as SIGKILL, which drops the grabs)
/// respawns the lock until the crash-loop cap is reached.
fn decide(exit_status: ExitStatus, recent_restarts: usize) -> LockSupervision {
    if exit_status.success() || recent_restarts >= LOCK_RESPAWN_CAP {
        LockSupervision::Stop
    } else {
        LockSupervision::Respawn {
            delay: LOCK_RESPAWN_DELAY,
        }
    }
}

fn spawn_lock_process(program: &Path, args: &[&str]) -> std::io::Result<Child> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
}

/// Owns the lock child for the lifetime of the lock screen: respawns it per
/// [`decide`] and returns once supervision stops.
fn supervise_lock(program: PathBuf, mut child: Child) {
    let mut restarts: VecDeque<Instant> = VecDeque::new();
    loop {
        let status = match child.wait() {
            Ok(status) => status,
            Err(error) => {
                eprintln!("flamewm: lock screen wait failed: {error}");
                return;
            }
        };
        let now = Instant::now();
        while restarts
            .front()
            .is_some_and(|at| now.duration_since(*at) > LOCK_RESPAWN_WINDOW)
        {
            restarts.pop_front();
        }
        match decide(status, restarts.len()) {
            LockSupervision::Stop => {
                if !status.success() {
                    eprintln!(
                        "flamewm: lock screen exited ({status}) and crash-loop cap reached; not respawning"
                    );
                }
                return;
            }
            LockSupervision::Respawn { delay } => {
                thread::sleep(delay);
                restarts.push_back(Instant::now());
                match spawn_lock_process(&program, &[]) {
                    Ok(next) => child = next,
                    Err(error) => {
                        eprintln!("flamewm: lock screen respawn failed: {error}");
                        return;
                    }
                }
            }
        }
    }
}

/// Starts the lock screen. The lock binary is spawned detached; a supervisor
/// thread respawns it if it crashes so the screen cannot silently unlock.
pub fn start_lock_screen() -> FlameResult<()> {
    match current_lock_program() {
        LockProgram::Screen(program) => {
            let child = spawn_lock_process(&program, &[]).map_err(io_error)?;
            thread::Builder::new()
                .name("flamewm-lock-supervisor".to_owned())
                .spawn(move || supervise_lock(program, child))
                .map(|_| ())
                .map_err(io_error)
        }
        LockProgram::Loginctl => Command::new("loginctl")
            .arg("lock-session")
            .status()
            .map_err(io_error)?
            .success()
            .then_some(())
            .ok_or_else(|| {
                FlameError::new(ErrorCode::PermissionDenied, "session action command failed")
            }),
    }
}

fn current_lock_program() -> LockProgram {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let path_var = std::env::var_os("PATH");
    resolve_lock_program(exe_dir.as_deref(), path_var.as_deref())
}

/// Argument vector for the preview lock: it takes no grabs and exits on any input.
const LOCK_PREVIEW_ARGS: &[&str] = &["--preview"];

/// Argument vector for the screensaver preview: ribbons only, no grabs, no PAM, exits on input.
const SCREENSAVER_PREVIEW_ARGS: &[&str] = &["--preview-screensaver"];

/// The single live lock preview child. Reaped with `try_wait` before each spawn decision.
static LOCK_PREVIEW: Mutex<Option<Child>> = Mutex::new(None);

/// The single live screensaver preview child, tracked separately from the lock preview.
static SCREENSAVER_PREVIEW: Mutex<Option<Child>> = Mutex::new(None);

/// Spawns a preview unless `slot` still holds a running child. An exited child is reaped first.
fn spawn_preview_once(
    slot: &mut Option<Child>,
    program: &Path,
    args: &[&str],
) -> std::io::Result<()> {
    if let Some(child) = slot.as_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            return Ok(());
        }
    }
    *slot = Some(spawn_lock_process(program, args)?);
    Ok(())
}

/// Starts one preview of `slot`'s kind. At most one preview of each kind runs at a time.
fn start_preview(slot: &Mutex<Option<Child>>, args: &[&str]) -> FlameResult<()> {
    match current_lock_program() {
        LockProgram::Screen(program) => {
            let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
            spawn_preview_once(&mut slot, &program, args).map_err(io_error)
        }
        LockProgram::Loginctl => Err(FlameError::new(
            ErrorCode::NotFound,
            "flamewm-lock binary not found; lock preview unavailable",
        )),
    }
}

/// Starts the lock screen in preview mode. At most one preview runs at a time.
pub fn start_lock_preview() -> FlameResult<()> {
    start_preview(&LOCK_PREVIEW, LOCK_PREVIEW_ARGS)
}

/// Starts the screensaver preview. At most one preview runs at a time.
pub fn start_screensaver_preview() -> FlameResult<()> {
    start_preview(&SCREENSAVER_PREVIEW, SCREENSAVER_PREVIEW_ARGS)
}

#[cfg(test)]
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("flamewm-lock-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[cfg(test)]
mod lock_preview_tests {
    use super::*;

    /// Fake lock binary: logs its argv once per launch, then runs until killed.
    fn fake_lock(dir: &Path) -> PathBuf {
        let program = dir.join(LOCK_SCREEN_BINARY);
        let body = format!(
            "#!/bin/sh\necho \"$@\" >> '{}'\nexec sleep 30\n",
            dir.join("launches.log").display()
        );
        std::fs::write(&program, body).expect("write fake lock");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake lock");
        program
    }

    fn launches(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("launches.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn wait_for_launches(dir: &Path, count: usize) {
        // 10 s: the fake lock is a shell script and the workspace suite runs under heavy load.
        for _ in 0..1000 {
            if launches(dir).len() >= count {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("expected {count} launches, saw {:?}", launches(dir));
    }

    /// Spawns the preview, retrying `ETXTBSY`: exec of a just-written script races with a fork
    /// in a parallel test thread that still holds the write fd.
    fn spawn_retrying(slot: &mut Option<Child>, program: &Path) -> std::io::Result<()> {
        spawn_retrying_with(slot, program, LOCK_PREVIEW_ARGS)
    }

    fn spawn_retrying_with(
        slot: &mut Option<Child>,
        program: &Path,
        args: &[&str],
    ) -> std::io::Result<()> {
        let mut last = None;
        for _ in 0..100 {
            match spawn_preview_once(slot, program, args) {
                Err(error) if error.raw_os_error() == Some(26) => {
                    last = Some(error);
                    std::thread::sleep(Duration::from_millis(20));
                }
                other => return other,
            }
        }
        Err(last.expect("retried at least once"))
    }

    #[test]
    fn preview_argument_vector_is_exactly_preview_flag() {
        assert_eq!(LOCK_PREVIEW_ARGS, ["--preview"]);
    }

    #[test]
    fn screensaver_preview_argument_vector_is_exactly_its_flag() {
        assert_eq!(SCREENSAVER_PREVIEW_ARGS, ["--preview-screensaver"]);
    }

    #[test]
    fn screensaver_preview_spawns_once_with_only_its_flag() {
        let dir = temp_dir("screensaver-argv");
        let program = fake_lock(&dir);
        let mut slot = None;
        spawn_retrying_with(&mut slot, &program, SCREENSAVER_PREVIEW_ARGS).expect("spawn");
        spawn_preview_once(&mut slot, &program, SCREENSAVER_PREVIEW_ARGS).expect("second spawn");
        wait_for_launches(&dir, 1);
        assert_eq!(launches(&dir), ["--preview-screensaver"]);
        let _ = slot.expect("child tracked").kill();
    }

    #[test]
    fn preview_spawns_fake_lock_with_only_the_preview_flag() {
        let dir = temp_dir("argv");
        let program = fake_lock(&dir);
        let mut slot = None;
        spawn_retrying(&mut slot, &program).expect("spawn preview");
        wait_for_launches(&dir, 1);
        assert_eq!(launches(&dir), ["--preview"]);
        let _ = slot.expect("child tracked").kill();
    }

    #[test]
    fn second_preview_is_refused_while_first_is_running() {
        let dir = temp_dir("single");
        let program = fake_lock(&dir);
        let mut slot = None;
        spawn_retrying(&mut slot, &program).expect("first preview");
        spawn_retrying(&mut slot, &program).expect("second call is a no-op");
        wait_for_launches(&dir, 1);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            launches(&dir).len(),
            1,
            "a second preview spawned while one runs"
        );
        let _ = slot.expect("child tracked").kill();
    }

    #[test]
    fn exited_preview_is_reaped_and_a_new_one_starts() {
        let dir = temp_dir("reap");
        let program = fake_lock(&dir);
        let mut slot = None;
        spawn_retrying(&mut slot, &program).expect("first preview");
        wait_for_launches(&dir, 1);
        let child = slot.as_mut().expect("child tracked");
        child.kill().expect("kill preview");
        child.wait().expect("reap preview");
        spawn_retrying(&mut slot, &program).expect("preview after exit");
        wait_for_launches(&dir, 2);
        let _ = slot.expect("child tracked").kill();
    }
}

#[cfg(test)]
mod lock_resolution_tests {
    use super::*;

    fn write_binary(dir: &Path, mode: u32) {
        let path = dir.join(LOCK_SCREEN_BINARY);
        std::fs::write(&path, b"#!/bin/sh\n").expect("write binary");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("chmod binary");
    }

    #[test]
    fn falls_back_to_loginctl_when_binary_absent() {
        let exe = temp_dir("absent-exe");
        let path = temp_dir("absent-path");
        let path_var = std::env::join_paths([&path]).expect("join");
        assert_eq!(
            resolve_lock_program(Some(&exe), Some(&path_var)),
            LockProgram::Loginctl
        );
        assert_eq!(resolve_lock_program(None, None), LockProgram::Loginctl);
    }

    #[test]
    fn prefers_sibling_of_current_executable() {
        let exe = temp_dir("sibling-exe");
        let path = temp_dir("sibling-path");
        write_binary(&exe, 0o755);
        write_binary(&path, 0o755);
        let path_var = std::env::join_paths([&path]).expect("join");
        assert_eq!(
            resolve_lock_program(Some(&exe), Some(&path_var)),
            LockProgram::Screen(exe.join(LOCK_SCREEN_BINARY))
        );
    }

    #[test]
    fn finds_binary_on_path_when_not_beside_executable() {
        let exe = temp_dir("path-exe");
        let path = temp_dir("path-dir");
        write_binary(&path, 0o755);
        let path_var = std::env::join_paths([&exe, &path]).expect("join");
        assert_eq!(
            resolve_lock_program(Some(&exe), Some(&path_var)),
            LockProgram::Screen(path.join(LOCK_SCREEN_BINARY))
        );
    }

    #[test]
    fn ignores_non_executable_binary() {
        let exe = temp_dir("noexec-exe");
        write_binary(&exe, 0o644);
        assert_eq!(
            resolve_lock_program(Some(&exe), None),
            LockProgram::Loginctl
        );
    }
}
fn unsupported(message: &'static str) -> FlameError {
    FlameError::new(ErrorCode::Unsupported, message)
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
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?
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
        .map_err(io_error)?
        .reply()
        .map_err(io_error)?
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
                .map_err(io_error)?
                .reply()
                .map_err(io_error)?;
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
                .map_err(io_error)?;
            }
        }
        conn.change_property32(
            xproto::PropMode::REPLACE,
            root,
            count,
            AtomEnum::CARDINAL,
            &[plan.new_count as u32],
        )
        .map_err(io_error)?;
        conn.change_property8(xproto::PropMode::REPLACE, root, names_atom, utf8, &names)
            .map_err(io_error)?;
        conn.change_property32(
            xproto::PropMode::REPLACE,
            root,
            current,
            AtomEnum::CARDINAL,
            &[plan.new_active as u32],
        )
        .map_err(io_error)?;
        Ok(())
    };
    conn.grab_server().map_err(io_error)?;
    let result = writes();
    conn.ungrab_server().map_err(io_error)?;
    result?;
    conn.flush().map_err(io_error)
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
        let atoms = self.window_atoms();
        let reads = self.send_window_reads(window, tree.parent, &atoms);
        let frame = self.frame(&atoms)?;
        Ok(self.finish_window(reads, &atoms, &frame, reference)?.0)
    }
    fn snapshot(&self) -> FlameResult<Vec<flamewm_api::window::WindowSnapshot>> {
        let tree = self.managed_tree()?;
        let atoms = self.window_atoms();
        let reads: Vec<WindowReads<'_>> = tree
            .iter()
            .map(|&(window, parent)| self.send_window_reads(window, parent, &atoms))
            .collect();
        // A failed shared read drops every window, as the per-window `get` did.
        let Ok(frame) = self.frame(&atoms) else {
            return Ok(Vec::new());
        };
        Ok(reads
            .into_iter()
            .filter_map(|reads| {
                let reference = WindowRef::new(u64::from(reads.window), 0);
                self.finish_window(reads, &atoms, &frame, reference)
                    .ok()
                    .filter(|(_, named)| *named)
                    .map(|(window, _)| window)
            })
            .collect())
    }
    fn activate(&mut self, w: WindowRef) -> FlameResult<()> {
        self.client_message(
            self.check_window(w)?,
            "_NET_ACTIVE_WINDOW",
            [2, CURRENT_TIME, 0, 0, 0],
        )
    }
    fn minimize(&mut self, w: WindowRef) -> FlameResult<()> {
        let window = self.check_window(w)?;
        let (target, name, data) = Self::minimize_message(window);
        self.client_message(target, name, data)
    }
    fn maximize(&mut self, w: WindowRef) -> FlameResult<()> {
        let window = self.check_window(w)?;
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
        self.client_message(
            self.check_window(w)?,
            "_NET_CLOSE_WINDOW",
            [CURRENT_TIME, 2, 0, 0, 0],
        )
    }
    fn set_outer_geometry(&mut self, w: WindowRef, geometry: Rect) -> FlameResult<()> {
        let geometry = shared::window_geometry_aux(geometry)?;
        self.conn
            .configure_window(self.check_window(w)?, &geometry)
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
        self.send_state(self.check_window(w)?, self.atom(atom), action)
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
        Ok(shared::workspace_snapshot(count, active_index, &bytes, 0))
    }
    fn activate_workspace(&mut self, index: usize, expected_revision: u64) -> FlameResult<()> {
        let current = self.workspace_snapshot()?;
        if expected_revision != current.revision {
            return Err(FlameError::stale("workspace revision changed"));
        }
        if index >= current.count {
            return Err(FlameError::invalid("workspace index is outside topology"));
        }
        self.client_message(
            self.root,
            "_NET_CURRENT_DESKTOP",
            [index as u32, CURRENT_TIME, 0, 0, 0],
        )
    }
    fn move_window_to_workspace(&mut self, w: WindowRef, target: usize) -> FlameResult<()> {
        let current = self.workspace_snapshot()?;
        if target >= current.count {
            return Err(FlameError::invalid("workspace index is outside topology"));
        }
        self.client_message(
            self.check_window(w)?,
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
        apply_workspace_plan_to_root(&self.conn, self.root, plan)
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
        let catalog = self.application_catalog()?;
        catalog.launch(app, options).map(|_| ())
    }
    fn launch_uri(&mut self, uri: &str) -> FlameResult<()> {
        flamewm_applications::open_target(std::ffi::OsStr::new(uri))
    }
}

impl SessionPort for X11Desktop {
    fn session_capabilities(&self) -> SessionCapabilities {
        SessionCapabilities {
            lock: Path::new("/usr/bin/loginctl").exists(),
            logout: session_logout_supported(),
            suspend: Path::new("/usr/bin/systemctl").exists(),
            reboot: Path::new("/usr/bin/systemctl").exists(),
            shutdown: Path::new("/usr/bin/systemctl").exists(),
        }
    }
    fn perform_session_action(&mut self, action: SessionAction) -> FlameResult<()> {
        let (program, args): (&str, &[&str]) = match action {
            SessionAction::Lock => return start_lock_screen(),
            SessionAction::LockPreview => return start_lock_preview(),
            SessionAction::ScreensaverPreview => return start_screensaver_preview(),
            SessionAction::Logout => {
                if !session_logout_supported() {
                    return Err(FlameError::new(
                        ErrorCode::Unavailable,
                        "logout requires a managed FlameWM session",
                    ));
                }
                request_session_logout();
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
            .map(|reply| reply.output)
            .unwrap_or(0);
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
                (Rect::default(), ModeId(0))
            };
            let modes = shared::display_modes(&info.modes, &resources.modes);
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
        // Some nested and legacy X servers do not implement RandR primary.
        // In that case stabilize fallback by connector, not enumeration order.
        if !outputs.iter().any(|output| output.primary) {
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
        // The product coordinator retires the transaction before this call, so the adapter must
        // retire it too even when RandR refuses or fails the rollback; otherwise apply_mode
        // returns Busy for every later mode change.
        self.pending_mode = None;
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
        self.flush()
    }
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
mod lock_supervision_tests {
    use std::os::unix::process::ExitStatusExt;

    use super::*;

    /// Raw wait status: exit code lives in bits 8..16, a signal in bits 0..7.
    fn exited(code: i32) -> ExitStatus {
        ExitStatus::from_raw(code << 8)
    }

    fn killed(signal: i32) -> ExitStatus {
        ExitStatus::from_raw(signal)
    }

    #[test]
    fn successful_exit_unlocks_and_ends_supervision() {
        assert_eq!(decide(exited(0), 0), LockSupervision::Stop);
        assert_eq!(
            decide(exited(0), LOCK_RESPAWN_CAP - 1),
            LockSupervision::Stop
        );
    }

    #[test]
    fn nonzero_exit_and_sigkill_respawn_after_backoff() {
        let respawn = LockSupervision::Respawn {
            delay: Duration::from_millis(500),
        };
        assert_eq!(decide(exited(1), 0), respawn);
        assert_eq!(decide(killed(9), 0), respawn);
        assert_eq!(decide(killed(9), LOCK_RESPAWN_CAP - 1), respawn);
    }

    #[test]
    fn crash_loop_cap_stops_supervision() {
        assert_eq!(decide(killed(9), LOCK_RESPAWN_CAP), LockSupervision::Stop);
        assert_eq!(
            decide(exited(1), LOCK_RESPAWN_CAP + 3),
            LockSupervision::Stop
        );
    }
}

#[cfg(test)]
mod session_logout_tests {
    use std::ffi::OsStr;

    use rustix::process::Pid;

    use super::{
        request_session_logout, session_logout_owner_matches, take_session_logout_request,
    };

    #[test]
    fn logout_requires_the_current_supervisor_process() {
        let supervisor = Pid::from_raw(1234).unwrap();
        assert!(session_logout_owner_matches(
            Some(OsStr::new("1")),
            Some(OsStr::new("1234")),
            Some(supervisor)
        ));
        assert!(!session_logout_owner_matches(
            None,
            Some(OsStr::new("1235")),
            Some(supervisor)
        ));
        assert!(!session_logout_owner_matches(
            Some(OsStr::new("1")),
            Some(OsStr::new("not-a-pid")),
            Some(supervisor)
        ));
        assert!(!session_logout_owner_matches(
            Some(OsStr::new("1")),
            Some(OsStr::new("1235")),
            Some(supervisor)
        ));
    }

    #[test]
    fn repeated_logout_requests_collapse_to_one_clean_exit_intent() {
        let _ = take_session_logout_request();
        request_session_logout();
        request_session_logout();
        assert!(take_session_logout_request());
        assert!(!take_session_logout_request());
    }
}

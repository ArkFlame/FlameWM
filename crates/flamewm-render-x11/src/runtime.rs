use super::*;
use crate::xlib::*;

pub fn run(document: RuntimeDocument, config: X11Config) -> Result<(), String> {
    run_with_handler(document, config, |action| {
        println!("FLAMEWM_RENDER_ACTION {action}")
    })
}

/// Return default X screen root dimensions.
pub fn root_geometry() -> Result<(u32, u32), String> {
    unsafe {
        let display = XOpenDisplay(ptr::null());
        if display.is_null() {
            return Err("XOpenDisplay failed; DISPLAY is unset or unreachable".to_string());
        }
        let screen = XDefaultScreen(display);
        let width = XDisplayWidth(display, screen).max(0) as u32;
        let height = XDisplayHeight(display, screen).max(0) as u32;
        XCloseDisplay(display);
        if width == 0 || height == 0 {
            return Err("X11 root geometry is empty".to_string());
        }
        Ok((width, height))
    }
}

pub fn run_with_controller_events<F>(
    document: RuntimeDocument,
    config: X11Config,
    mut on_event: F,
) -> Result<(), String>
where
    F: FnMut(&ControllerEvent, &mut RuntimeDocument) -> Result<(), String>,
{
    run_with_controller(document, config, move |event, document| {
        on_event(&ControllerEvent::Action(event.clone()), document)
    })
}

/// Requested outer geometry (root-relative position and client size) for the
/// single app window. Applied by the X11 event loop on its next reactor wake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Single-slot geometry request shared between the reactor callback (writer)
/// and the X11 event loop (taker). Latest request wins; `None` means no-op.
pub type WindowGeometrySlot = std::rc::Rc<std::cell::Cell<Option<WindowGeometry>>>;

/// One root press observed while the host has armed `RawPressWatch`. Presses
/// are only observed: they are never grabbed, replayed or consumed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RawPress {
    /// Root-space position in logical px.
    pub x: f32,
    pub y: f32,
    pub button: u32,
    pub time_ms: u64,
}

/// Host request for the root press observer, written by the host's callbacks
/// and read by the X11 event loop on each turn. `true` arms the observer.
pub type RawPressWatch = std::rc::Rc<std::cell::Cell<bool>>;

pub fn run_with_controller_events_with_reactor<F, R>(
    document: RuntimeDocument,
    config: X11Config,
    reactor: &mut Reactor,
    on_event: F,
    on_reactor: R,
) -> Result<(), String>
where
    F: FnMut(&ControllerEvent, &mut RuntimeDocument) -> Result<(), String>,
    R: FnMut(&mut RuntimeDocument) -> Result<(), String>,
{
    run_with_controller_events_with_reactor_geometry(
        document,
        config,
        reactor,
        WindowGeometrySlot::default(),
        RawPressWatch::default(),
        on_event,
        |_, _| Ok(()),
        on_reactor,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_with_controller_events_with_reactor_geometry<F, P, R>(
    mut document: RuntimeDocument,
    config: X11Config,
    reactor: &mut Reactor,
    geometry: WindowGeometrySlot,
    press_watch: RawPressWatch,
    mut on_event: F,
    mut on_press: P,
    mut on_reactor: R,
) -> Result<(), String>
where
    F: FnMut(&ControllerEvent, &mut RuntimeDocument) -> Result<(), String>,
    P: FnMut(&RawPress, &mut RuntimeDocument) -> Result<(), String>,
    R: FnMut(&mut RuntimeDocument) -> Result<(), String>,
{
    unsafe {
        let display = XOpenDisplay(ptr::null());
        if display.is_null() {
            return Err("XOpenDisplay failed; DISPLAY is unset or unreachable".to_string());
        }
        let mut app = match X11App::new(display, &config) {
            Ok(app) => app,
            Err(error) => {
                XCloseDisplay(display);
                return Err(error);
            }
        };
        let x_fd = XConnectionNumber(display);
        let token =
            match reactor.register_raw_fd_with_action(x_fd, calloop::Interest::READ, |_, _| {
                FdAction::Continue
            }) {
                Ok(token) => token,
                Err(error) => {
                    let deferred_native = app.take_deferred_native_libraries();
                    drop(app);
                    XCloseDisplay(display);
                    drop(deferred_native);
                    return Err(error.to_string());
                }
            };
        let result = app.event_loop_with_reactor(
            &mut document,
            &mut |event, document| on_event(&ControllerEvent::Action(event.clone()), document),
            reactor,
            &mut on_reactor,
            &geometry,
            &press_watch,
            &mut on_press,
        );
        let remove_result = reactor.remove(token).map_err(|error| error.to_string());
        let deferred_native = app.take_deferred_native_libraries();
        drop(app);
        XCloseDisplay(display);
        drop(deferred_native);
        remove_result?;
        result
    }
}

pub fn run_with_handler<F>(
    document: RuntimeDocument,
    config: X11Config,
    mut on_action: F,
) -> Result<(), String>
where
    F: FnMut(&str),
{
    run_with_controller(document, config, move |event, _document| {
        if event.phase == ActionPhase::Release && event.inside {
            on_action(&event.action);
        }
        Ok(())
    })
}

pub fn run_with_controller<F>(
    mut document: RuntimeDocument,
    config: X11Config,
    mut on_action: F,
) -> Result<(), String>
where
    F: FnMut(&ActionEvent, &mut RuntimeDocument) -> Result<(), String>,
{
    unsafe {
        let display = XOpenDisplay(ptr::null());
        if display.is_null() {
            return Err("XOpenDisplay failed; DISPLAY is unset or unreachable".to_string());
        }
        let mut app = match X11App::new(display, &config) {
            Ok(app) => app,
            Err(error) => {
                XCloseDisplay(display);
                return Err(error);
            }
        };
        let result = app.event_loop(&mut document, &mut on_action);
        let deferred_native = app.take_deferred_native_libraries();
        drop(app);
        XCloseDisplay(display);
        drop(deferred_native);
        result
    }
}

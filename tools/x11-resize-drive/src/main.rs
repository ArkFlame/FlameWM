//! Drive one interactive X11 resize with XTEST and report it on one line.
//!
//! Reference evidence (never a dependency, never loaded or executed):
//! - `.vendor/xfwm4-master/src/events.c` / `moveresize.c`: a corner window button press starts a
//!   resize; the drag is computed from continuously updated root pointer coordinates.
//! - xdotool semantics: synthetic absolute motion plus button press/release through XTEST.
//! Active owner evidence:
//! - `crates/flamewm-wm-x11/src/frame/layout.rs`: eight resize zones, 16 px bottom-right grip.
//! - `crates/flamewm-wm-x11/src/wm/drag_modifier.rs`: Ctrl + drag modifier + button 3 resizes;
//!   the modifier is Alt when `FLAMEWM_DRAG_MODIFIER=alt`.
//! Choice: default to button 1 on the frame bottom-right grip; `--mode alt-button3` instead holds
//! Alt_L + Control_L (FlameWM's Alt resize binding) and presses button 3.

use std::env;
use std::error::Error;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConnectionExt as XprotoExt, KEY_PRESS_EVENT,
    KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT, Window,
};
use x11rb::protocol::xtest::ConnectionExt as XtestExt;
use x11rb::rust_connection::RustConnection;

/// Standard X11 keysyms (`Alt_L`, `Control_L`) used to locate keycodes via the keyboard mapping.
const XK_CONTROL_L: u32 = 0xFFE3;
const XK_ALT_L: u32 = 0xFFE9;
/// Stay inside the 16 px bottom-right corner grip instead of landing exactly on the border.
const CORNER_INSET: i32 = 4;
const DEFAULT_STEPS: u32 = 600;
const DEFAULT_DURATION_MS: u64 = 1000;
const DEFAULT_DELTA: (i32, i32) = (120, 80);
/// XTEST `deviceid` 0 means the core pointer/keyboard.
const CORE_DEVICE: u8 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Corner,
    AltButton3,
}

const USAGE: &str = "usage: x11-resize-drive <WM_NAME> [--display DISPLAY] [--mode corner|alt-button3] [--steps N] [--duration-ms MS] [--delta X,Y]";

fn main() -> ExitCode {
    match run() {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("x11-resize-drive: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_u32(flag: &str, value: &str) -> Result<u32, Box<dyn Error>> {
    value
        .parse::<u32>()
        .map_err(|_| format!("{flag} expects a positive integer, got {value:?}").into())
}

fn parse_delta(value: &str) -> Result<(i32, i32), Box<dyn Error>> {
    let (x, y) = value
        .split_once(',')
        .ok_or_else(|| format!("--delta expects X,Y, got {value:?}"))?;
    Ok((x.trim().parse()?, y.trim().parse()?))
}

fn run() -> Result<String, Box<dyn Error>> {
    let mut title: Option<String> = None;
    let mut display: Option<String> = None;
    let mut mode = Mode::Corner;
    let mut steps = DEFAULT_STEPS;
    let mut duration_ms = DEFAULT_DURATION_MS;
    let mut delta = DEFAULT_DELTA;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(String::new());
            }
            "--display" => display = Some(args.next().ok_or("--display needs a value")?),
            "--mode" => {
                let value = args.next().ok_or("--mode needs a value")?;
                mode = match value.as_str() {
                    "corner" => Mode::Corner,
                    "alt-button3" => Mode::AltButton3,
                    _ => return Err(format!("unknown --mode {value:?}").into()),
                };
            }
            "--steps" => steps = parse_u32("--steps", &args.next().ok_or("--steps needs a value")?)?,
            "--duration-ms" => {
                duration_ms = u64::from(parse_u32(
                    "--duration-ms",
                    &args.next().ok_or("--duration-ms needs a value")?,
                )?);
            }
            "--delta" => delta = parse_delta(&args.next().ok_or("--delta needs a value")?)?,
            other if other.starts_with('-') => return Err(format!("unknown flag {other:?}").into()),
            other if title.is_none() => title = Some(other.to_string()),
            other => return Err(format!("unexpected argument {other:?}").into()),
        }
    }
    let title = title.ok_or(USAGE)?;
    if steps == 0 {
        return Err("--steps must be at least 1".into());
    }

    // `None` selects the DISPLAY environment variable, as required by the brief.
    let (connection, _) = x11rb::connect(display.as_deref())?;
    let root = connection.setup().roots[0].root;
    let client = find_window(&connection, &title)?
        .ok_or_else(|| format!("no top-level window named {title:?}"))?;
    wait_framed(&connection, client, root)?;
    let (frame_x, frame_y, width, height) = frame_rect(&connection, client)?;
    let start_x = clamp_coord(i32::from(frame_x) + i32::from(width) - CORNER_INSET);
    let start_y = clamp_coord(i32::from(frame_y) + i32::from(height) - CORNER_INSET);

    let mut held_keys: Vec<u8> = Vec::new();
    if mode == Mode::AltButton3 {
        // FlameWM's resize binding is Ctrl + drag modifier + button 3; this crate selects the
        // Alt variant, so hold both Control_L and Alt_L before the button press.
        for keysym in [XK_CONTROL_L, XK_ALT_L] {
            let code = keycode_for_keysym(&connection, keysym)?
                .ok_or_else(|| format!("no keycode for keysym 0x{keysym:X}"))?;
            fake(&connection, KEY_PRESS_EVENT, code, root, 0, 0)?;
            held_keys.push(code);
        }
    }

    let button = match mode {
        Mode::Corner => 1,
        Mode::AltButton3 => 3,
    };
    fake(&connection, MOTION_NOTIFY_EVENT, 0, root, start_x, start_y)?;
    fake(&connection, BUTTON_PRESS_EVENT, button, root, start_x, start_y)?;

    let started = Instant::now();
    let drive_error = move_steps(
        &connection,
        root,
        start_x,
        start_y,
        delta,
        steps,
        Duration::from_millis(u64::from(duration_ms)),
        started,
    )
    .err();

    // Always release the button and any held modifiers, even when a motion was rejected.
    let end_x = clamp_coord(i32::from(start_x) + delta.0);
    let end_y = clamp_coord(i32::from(start_y) + delta.1);
    fake(
        &connection,
        BUTTON_RELEASE_EVENT,
        button,
        root,
        end_x,
        end_y,
    )?;
    for code in held_keys.iter().rev() {
        fake(&connection, KEY_RELEASE_EVENT, *code, root, 0, 0)?;
    }
    if let Some(error) = drive_error {
        return Err(error);
    }

    let elapsed_ms = started.elapsed().as_millis();
    Ok(format!("RESIZE-DRIVE steps={steps} elapsed_ms={elapsed_ms}"))
}

fn fake(
    connection: &RustConnection,
    event_type: u8,
    detail: u8,
    root: Window,
    x: i16,
    y: i16,
) -> Result<(), Box<dyn Error>> {
    connection
        .xtest_fake_input(event_type, detail, 0, root, x, y, CORE_DEVICE)?
        .check()?;
    Ok(())
}

fn move_steps(
    connection: &RustConnection,
    root: Window,
    start_x: i16,
    start_y: i16,
    (delta_x, delta_y): (i32, i32),
    steps: u32,
    duration: Duration,
    started: Instant,
) -> Result<(), Box<dyn Error>> {
    for step in 1..=steps {
        let ratio = i32::try_from(step)?;
        let count = i32::try_from(steps)?;
        let x = clamp_coord(i32::from(start_x) + delta_x * ratio / count);
        let y = clamp_coord(i32::from(start_y) + delta_y * ratio / count);
        fake(connection, MOTION_NOTIFY_EVENT, 0, root, x, y)?;
        // Even pacing over the configured duration keeps the synthetic drag bounded; the
        // nanosecond deadline gives sub-millisecond (1 kHz class) scheduling resolution.
        let due = started + duration.mul_f64(ratio as f64 / count as f64);
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        }
    }
    Ok(())
}

fn clamp_coord(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

fn atom(connection: &RustConnection, name: &str) -> Result<u32, Box<dyn Error>> {
    Ok(connection.intern_atom(false, name.as_bytes())?.reply()?.atom)
}

/// Find a direct child of the root whose `_NET_WM_NAME` or `WM_NAME` equals `title`.
fn find_window(connection: &RustConnection, title: &str) -> Result<Option<Window>, Box<dyn Error>> {
    let root = connection.setup().roots[0].root;
    let net_name = atom(connection, "_NET_WM_NAME")?;
    let wm_name = atom(connection, "WM_NAME")?;
    for window in connection.query_tree(root)?.reply()?.children {
        for property in [net_name, wm_name] {
            // Type 0 (ANY) accepts UTF8_STRING and legacy STRING alike.
            let value = connection
                .get_property(false, window, property, 0u32, 0, 256)?
                .reply()?
                .value;
            if value == title.as_bytes() {
                return Ok(Some(window));
            }
        }
    }
    Ok(None)
}

/// Return the reparented frame's root-relative rectangle, or the client's own when unframed.
/// Wait (bounded, 5 s) until the window manager has reparented the client into a mapped frame.
/// The title can exist on the server well before the WM frames it, and a press before that lands
/// on the root window instead of the resize grip.
fn wait_framed(
    connection: &RustConnection,
    client: Window,
    root: Window,
) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let parent = connection.query_tree(client)?.reply()?.parent;
        if parent != root && parent != 0 {
            let attributes = connection.get_window_attributes(parent)?.reply()?;
            let geometry = connection.get_geometry(parent)?.reply()?;
            if attributes.map_state == x11rb::protocol::xproto::MapState::VIEWABLE
                && geometry.width > 1
                && geometry.height > 1
            {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err("window was never framed and mapped by the window manager".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn frame_rect(connection: &RustConnection, client: Window) -> Result<(i16, i16, u16, u16), Box<dyn Error>> {
    let root = connection.setup().roots[0].root;
    let parent = connection.query_tree(client)?.reply()?.parent;
    let subject = if parent != root && parent != 0 { parent } else { client };
    let geometry = connection.get_geometry(subject)?.reply()?;
    Ok((geometry.x, geometry.y, geometry.width, geometry.height))
}

/// Locate a keycode by scanning the core keyboard mapping for `keysym`.
fn keycode_for_keysym(connection: &RustConnection, keysym: u32) -> Result<Option<u8>, Box<dyn Error>> {
    let setup = connection.setup();
    let first = setup.min_keycode;
    let count = setup.max_keycode - setup.min_keycode + 1;
    let mapping = connection.get_keyboard_mapping(first, count)?.reply()?;
    let per_keycode = usize::from(mapping.keysyms_per_keycode);
    if per_keycode == 0 {
        return Ok(None);
    }
    for (index, &candidate) in mapping.keysyms.iter().enumerate() {
        if candidate == keysym {
            let code = usize::from(first) + index / per_keycode;
            if code <= usize::from(u8::MAX) {
                return Ok(Some(code as u8));
            }
        }
    }
    Ok(None)
}

//! Keyboard and pointer input: shortcut dispatch, pointer focus, move/resize grabs.

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
    KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::desktop::WindowSurfaceType;
use smithay::desktop::utils::under_from_surface_tree;
use smithay::input::keyboard::xkb;
use smithay::input::keyboard::{FilterResult, ModifiersState, XkbConfig};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use flamewm_api::settings::{DEFAULT_KEYBOARD_LAYOUT, KEYBOARD_LAYOUTS};
use flamewm_api::shortcuts::{
    ACTION_FULLSCREEN_ESCAPE, ACTION_LAUNCHER, ACTION_SCREENSHOT_FULL, ACTION_SCREENSHOT_REGION,
    ACTION_SCREENSHOT_WINDOW, ACTION_SESSION_LOCK, ACTION_SHOW_DESKTOP,
    ACTION_SHOW_DESKTOP_CTRL_ALT, ACTION_TOGGLE_START_MENU, ACTION_WINDOW_CLOSE,
    ACTION_WINDOW_CYCLE_NEXT, ACTION_WINDOW_CYCLE_PREV, ACTION_WINDOW_FLOAT,
    ACTION_WINDOW_FULLSCREEN, ACTION_WINDOW_MAXIMIZE, ACTION_WINDOW_MINIMIZE,
    ACTION_WINDOW_RESIZE_GROW, ACTION_WINDOW_RESIZE_SHRINK, ACTION_WINDOW_SWITCH, focus_direction,
    move_direction, workspace_activate_index, workspace_direction, workspace_move_index,
};
use flamewm_platform::shortcuts::{
    ACTION_BRIGHTNESS_DOWN, ACTION_BRIGHTNESS_UP, ACTION_OPEN_ASSISTANT_CTRL_ALT,
    ACTION_OPEN_ASSISTANT_SUPER, ACTION_OPEN_FILES_CTRL_ALT, ACTION_OPEN_FILES_SUPER,
    ACTION_OPEN_FINDER, ACTION_OPEN_FINDER_CTRL_ALT, ACTION_OPEN_TERMINAL_CTRL_ALT,
    ACTION_OPEN_TERMINAL_SUPER, ACTION_VOLUME_DOWN, ACTION_VOLUME_MUTE, ACTION_VOLUME_UP,
};
use flamewm_platform::system::MediaAudioKey;

use crate::model::{
    Button, Hit, Mode, Mods, Rect, ResizeEdge, TITLEBAR_HEIGHT, WindowId, resize_edge_at,
};
use crate::state::{Grab, State, next_serial};
use flamewm_window_core::{SnapTarget, keep_visible};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// One media-key action the compositor routes through the platform system service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaKey {
    Brightness(bool),
    Audio(MediaAudioKey),
}

/// Maps a shortcut action id to its media key, so the registry binding and the dispatch table
/// cannot drift apart. Every other action yields `None`.
fn media_key(action: &str) -> Option<MediaKey> {
    match action {
        ACTION_BRIGHTNESS_UP => Some(MediaKey::Brightness(true)),
        ACTION_BRIGHTNESS_DOWN => Some(MediaKey::Brightness(false)),
        ACTION_VOLUME_UP => Some(MediaKey::Audio(MediaAudioKey::VolumeUp)),
        ACTION_VOLUME_DOWN => Some(MediaKey::Audio(MediaAudioKey::VolumeDown)),
        ACTION_VOLUME_MUTE => Some(MediaKey::Audio(MediaAudioKey::VolumeMute)),
        _ => None,
    }
}

fn mods_of(state: &ModifiersState) -> Mods {
    Mods {
        ctrl: state.ctrl,
        alt: state.alt,
        shift: state.shift,
        logo: state.logo,
    }
}

/// Keysym names that are modifier keys themselves.
fn is_super_key(name: &str) -> bool {
    name == "super_l" || name == "super_r"
}

/// Keysym names to look up for one key event, in priority order: the unshifted symbol first,
/// then the symbol the active level produces. `Ctrl+Alt` is level 3 on several layouts, where
/// `Ctrl+Alt+d` would otherwise be reported under a different keysym and the chord would miss.
fn name_candidates<'a>(raw: &'a str, modified: &'a str) -> impl Iterator<Item = &'a str> {
    std::iter::once(raw)
        .chain(std::iter::once(modified))
        .filter(|name| !name.is_empty())
}

mod buttons;
mod keyboard;
mod keyboard_tile;
mod move_feedback;
mod pointer;
mod settings;
mod shortcuts;
mod snap;
#[cfg(test)]
mod tests;
pub use self::buttons::TitleClick;
pub use self::keyboard::saved_xkb_config;
pub use self::settings::{saved_focus_follows_mouse, saved_model_settings};
use self::move_feedback::*;
use self::snap::*;

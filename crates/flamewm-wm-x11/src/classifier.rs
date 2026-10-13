use x11rb::protocol::xproto::Atom;

use crate::atoms::Atoms;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    Desktop,
    Dock,
    Popup,
    Utility,
    Normal,
}

pub fn classify(atoms: &Atoms, types: &[Atom], override_redirect: bool) -> WindowKind {
    if override_redirect
        || types.iter().any(|atom| {
            *atom == atoms.net_wm_window_type_popup_menu
                || *atom == atoms.net_wm_window_type_dropdown_menu
                || *atom == atoms.net_wm_window_type_tooltip
                || *atom == atoms.net_wm_window_type_notification
                // EWMH: splash windows are unframed and client-placed.
                || *atom == atoms.net_wm_window_type_splash
        })
    {
        return WindowKind::Popup;
    }
    if types.contains(&atoms.net_wm_window_type_desktop) {
        return WindowKind::Desktop;
    }
    if types.contains(&atoms.net_wm_window_type_dock) {
        return WindowKind::Dock;
    }
    if types.contains(&atoms.net_wm_window_type_utility) {
        return WindowKind::Utility;
    }
    WindowKind::Normal
}

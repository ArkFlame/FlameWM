//! Live Xcursor state for one X11App window. The app registers this owner in the
//! thread-local cursor theme registry, so `apply_cursor_theme` re-defines the
//! current cursor kind on the window after a `cursorTheme` change.

use std::cell::RefCell;
use std::rc::Rc;

use flamewm_render_core::CursorKind;

use crate::external_decoration::{CursorThemeOwner, register_cursor_theme_owner};
use crate::native::cursor::NativeCursorSession;
use crate::xlib::Window;

/// Cursor operations needed for a live theme swap.
trait ThemeSession {
    fn reload(&mut self, theme: &str, size: Option<u32>) -> Result<(), String>;
    fn define(&mut self, window: Window, kind: CursorKind) -> bool;
}

impl ThemeSession for NativeCursorSession {
    fn reload(&mut self, theme: &str, size: Option<u32>) -> Result<(), String> {
        unsafe { self.reload_theme(theme, size) }
    }

    fn define(&mut self, window: Window, kind: CursorKind) -> bool {
        unsafe { NativeCursorSession::define(self, window, kind) }.is_some()
    }
}

/// Reloads the theme (which rolls back on failure), then re-defines `kind` on
/// `window`. A failed reload skips the redefine and keeps the previous theme.
fn reload_then_redefine<S: ThemeSession>(
    session: &mut S,
    window: Window,
    kind: CursorKind,
    theme: &str,
    size: Option<u32>,
) -> Result<(), String> {
    session.reload(theme, size)?;
    if session.define(window, kind) {
        Ok(())
    } else {
        Err(format!("cursor redefine failed for window {window:#x}"))
    }
}

pub(crate) struct SurfaceCursor {
    pub(super) session: NativeCursorSession,
    pub(super) window: Window,
    /// Kind last defined on `window`; re-defined after a theme reload.
    pub(super) kind: CursorKind,
}

impl SurfaceCursor {
    /// Defines `kind` on the window and remembers it for the next theme reload.
    pub(crate) fn define(&mut self, kind: CursorKind) -> bool {
        let defined = unsafe { self.session.define(self.window, kind) }.is_some();
        if defined {
            self.kind = kind;
        }
        defined
    }

    pub(crate) fn into_session(self) -> NativeCursorSession {
        self.session
    }
}

impl CursorThemeOwner for SurfaceCursor {
    fn reload_cursor_theme(&mut self, theme: &str, size: Option<u32>) -> Result<(), String> {
        reload_then_redefine(&mut self.session, self.window, self.kind, theme, size)
    }
}

/// Wraps the surface cursor for live theme reloads. Returns the shared handle the
/// app keeps; the registry holds only a weak reference.
pub(crate) fn share_surface_cursor(
    cursor: SurfaceCursor,
    in_use: Option<String>,
) -> Rc<RefCell<SurfaceCursor>> {
    let shared = Rc::new(RefCell::new(cursor));
    let owner: Rc<RefCell<dyn CursorThemeOwner>> = shared.clone();
    register_cursor_theme_owner(&owner, in_use);
    shared
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeSession {
        reloads: Vec<String>,
        sizes: Vec<Option<u32>>,
        defines: Vec<(Window, CursorKind)>,
        fail_reload: bool,
        fail_define: bool,
    }

    impl ThemeSession for FakeSession {
        fn reload(&mut self, theme: &str, size: Option<u32>) -> Result<(), String> {
            self.sizes.push(size);
            self.reloads.push(theme.to_owned());
            if self.fail_reload {
                Err(format!("reload {theme} failed"))
            } else {
                Ok(())
            }
        }

        fn define(&mut self, window: Window, kind: CursorKind) -> bool {
            self.defines.push((window, kind));
            !self.fail_define
        }
    }

    #[test]
    fn reload_applies_theme_and_redefines_current_kind() {
        let mut session = FakeSession::default();
        let result = reload_then_redefine(&mut session, 0x10, CursorKind::Move, "Breeze", None);
        assert!(result.is_ok());
        assert_eq!(session.reloads, ["Breeze"]);
        assert_eq!(session.defines, [(0x10, CursorKind::Move)]);
    }

    #[test]
    fn reload_forwards_requested_size_and_redefines_at_new_size() {
        let mut session = FakeSession::default();
        let result =
            reload_then_redefine(&mut session, 0x10, CursorKind::Default, "Breeze", Some(48));
        assert!(result.is_ok());
        assert_eq!(session.sizes, [Some(48)]);
        assert_eq!(session.defines, [(0x10, CursorKind::Default)]);
    }

    #[test]
    fn failed_reload_keeps_previous_theme_without_redefine() {
        let mut session = FakeSession {
            fail_reload: true,
            ..FakeSession::default()
        };
        let result = reload_then_redefine(&mut session, 0x10, CursorKind::Pointer, "Missing", None);
        assert_eq!(result, Err("reload Missing failed".to_owned()));
        assert!(session.defines.is_empty());
    }

    #[test]
    fn failed_redefine_is_reported_with_window() {
        let mut session = FakeSession {
            fail_define: true,
            ..FakeSession::default()
        };
        let error =
            reload_then_redefine(&mut session, 0x10, CursorKind::Text, "Breeze", None).unwrap_err();
        assert!(error.contains("0x10"));
    }
}

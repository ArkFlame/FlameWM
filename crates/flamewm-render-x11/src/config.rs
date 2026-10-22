use super::*;
#[derive(Clone, Debug)]
pub struct X11Config {
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub title: String,
    pub role: X11WindowRole,
    /// Optional ICCCM WM_CLASS identity. `None` leaves WM_CLASS unset.
    pub identity: Option<X11WindowIdentity>,
    /// Whole-window pseudo-transparency (None = no window snapshot).
    pub backdrop: Option<crate::backdrop::WindowBackdrop>,
    /// XDND opt-in. `Some` advertises XdndAware and enables file drop-target and drag-source
    /// handling; the slot carries an outgoing drag request from the UI to the event loop.
    pub file_drag: Option<FileDragSlot>,
}

/// Reserved action name for a settled window size. The event carries the size in `x` (width)
/// and `y` (height), logical px. Delivered once per settled size, never per intermediate step.
pub const WINDOW_RESIZE_ACTION: &str = "__flamewm.window.resize";

/// Reserved action names for XDND drop-target events. `x`/`y` carry the window-relative
/// pointer position. The drop payload is percent-encoded `text/uri-list` in `text`.
pub const FILE_DRAG_ENTER_ACTION: &str = "__flamewm.window.drag.enter";
pub const FILE_DRAG_OVER_ACTION: &str = "__flamewm.window.drag.over";
pub const FILE_DRAG_LEAVE_ACTION: &str = "__flamewm.window.drag.leave";
pub const FILE_DRAG_DROP_ACTION: &str = "__flamewm.window.drag.drop";

/// Outgoing file-drag request: written by the UI, taken by the X11 event loop after the event
/// that requested it. `Some(paths)` starts a modal XDND drag with absolute `paths`.
pub type FileDragSlot =
    std::rc::Rc<std::cell::RefCell<Option<Vec<std::path::PathBuf>>>>;

/// ICCCM WM_CLASS pair (instance, class) plus EWMH UTF-8 name is taken from the title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct X11WindowIdentity {
    pub instance: String,
    pub class: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum X11WindowRole {
    Normal,
    Desktop,
    Dock,
    PopupMenu,
    DropdownMenu,
    Overlay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceRole {
    Normal,
    Desktop,
    Dock,
    PopupMenu,
    DropdownMenu,
    Overlay,
}

impl From<SurfaceRole> for X11WindowRole {
    fn from(role: SurfaceRole) -> Self {
        match role {
            SurfaceRole::Normal => Self::Normal,
            SurfaceRole::Desktop => Self::Desktop,
            SurfaceRole::Dock => Self::Dock,
            SurfaceRole::PopupMenu => Self::PopupMenu,
            SurfaceRole::DropdownMenu => Self::DropdownMenu,
            SurfaceRole::Overlay => Self::Overlay,
        }
    }
}

/// Input participation of a surface. PassThrough surfaces never take
/// focus or pointer grabs and get an empty XShape input region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SurfaceInputMode {
    #[default]
    Interactive,
    PassThrough,
}

impl SurfaceInputMode {
    pub fn allows_grab(self) -> bool {
        matches!(self, Self::Interactive)
    }

    pub fn allows_focus(self) -> bool {
        matches!(self, Self::Interactive)
    }
}

#[derive(Clone, Debug)]
pub struct SurfaceConfig {
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub role: SurfaceRole,
    pub input: SurfaceInputMode,
    pub initially_visible: bool,
    pub x: i32,
    pub y: i32,
}

/// Pure policy: overlay surfaces are never WM-managed, never in the
/// workarea/taskbar, and default to pass-through input.
pub fn overlay_excluded_from_wm(role: SurfaceRole) -> bool {
    matches!(role, SurfaceRole::Overlay)
}

/// Shared popup policy: transient menus/overlays are override-redirect and
/// must map+raise atomically before any grab (XMapWindow+XRaiseWindow+flush
/// or XMapRaised). Ordinary Normal/Desktop/Dock surfaces map only.
pub fn surface_role_needs_popup_raise(role: SurfaceRole) -> bool {
    matches!(
        role,
        SurfaceRole::PopupMenu | SurfaceRole::DropdownMenu | SurfaceRole::Overlay
    )
}

/// X11-role twin of [`surface_role_needs_popup_raise`] for render-owned code.
pub fn x11_role_needs_popup_raise(role: X11WindowRole) -> bool {
    matches!(
        role,
        X11WindowRole::PopupMenu | X11WindowRole::DropdownMenu | X11WindowRole::Overlay
    )
}

pub fn overlay_window_type_name(role: SurfaceRole) -> &'static str {
    match role {
        SurfaceRole::Normal => "_NET_WM_WINDOW_TYPE_NORMAL",
        SurfaceRole::Desktop => "_NET_WM_WINDOW_TYPE_DESKTOP",
        SurfaceRole::Dock => "_NET_WM_WINDOW_TYPE_DOCK",
        SurfaceRole::PopupMenu => "_NET_WM_WINDOW_TYPE_POPUP_MENU",
        SurfaceRole::DropdownMenu => "_NET_WM_WINDOW_TYPE_DROPDOWN_MENU",
        SurfaceRole::Overlay => "_NET_WM_WINDOW_TYPE_NOTIFICATION",
    }
}

impl Default for SurfaceConfig {
    fn default() -> Self {
        Self {
            width: 1350,
            height: 641,
            title: "FlameWM Render 0.0.9".to_string(),
            role: SurfaceRole::Normal,
            input: SurfaceInputMode::Interactive,
            initially_visible: true,
            x: 0,
            y: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceId(pub(crate) u64);

/// Pure EWMH dock strut model (C11 native).
///
/// Root-coordinate edge reservation computed from the root extent and the
/// panel rect. Exactly one edge is reserved: bottom-touch gives
/// `bottom = root_h - panel.y` with `bottom_start_x = panel.x` and
/// `bottom_end_x = panel.right - 1` (others 0); top/left/right are
/// equivalent; a panel touching no root edge yields no strut (`None`).
/// Guards exclude full-span rects (e.g. a full-height window with `y == 0`
/// is not a bottom strut). Reference: `.vendor/.../wmtaskbar.cc`
/// `updateWMHints` (thickness = height on the touched edge) and
/// `wmclient.cc` `getNetWMStrutPartial` (12-long partial layout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DockStrut {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
    pub left_start_y: u32,
    pub left_end_y: u32,
    pub right_start_y: u32,
    pub right_end_y: u32,
    pub top_start_x: u32,
    pub top_end_x: u32,
    pub bottom_start_x: u32,
    pub bottom_end_x: u32,
}

/// Root edge a dock surface reserves. Explicit so a floating bar that touches no root edge
/// still reserves space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockEdge {
    Top,
    Bottom,
    Left,
    Right,
}

impl DockStrut {
    const NONE: Self = Self {
        left: 0,
        right: 0,
        top: 0,
        bottom: 0,
        left_start_y: 0,
        left_end_y: 0,
        right_start_y: 0,
        right_end_y: 0,
        top_start_x: 0,
        top_end_x: 0,
        bottom_start_x: 0,
        bottom_end_x: 0,
    };

    /// Reserves `edge` with extent = distance from that root edge to the rect (bar thickness
    /// plus any floating inset). Classic bars yield the same values as `from_root_and_rect`.
    pub fn from_edge_rect(
        root_w: u32,
        root_h: u32,
        edge: DockEdge,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    ) -> Option<Self> {
        if w == 0 || h == 0 {
            return None;
        }
        let (root_w, root_h) = (root_w as i64, root_h as i64);
        let (x, y, w, h) = (x as i64, y as i64, w as i64, h as i64);
        let mut s = Self::NONE;
        match edge {
            DockEdge::Bottom => {
                let extent = root_h - y;
                if extent <= 0 {
                    return None;
                }
                s.bottom = extent as u32;
                s.bottom_start_x = x.max(0) as u32;
                s.bottom_end_x = (x + w - 1).max(0) as u32;
            }
            DockEdge::Top => {
                let extent = y + h;
                if extent <= 0 {
                    return None;
                }
                s.top = extent as u32;
                s.top_start_x = x.max(0) as u32;
                s.top_end_x = (x + w - 1).max(0) as u32;
            }
            DockEdge::Left => {
                let extent = x + w;
                if extent <= 0 {
                    return None;
                }
                s.left = extent as u32;
                s.left_start_y = y.max(0) as u32;
                s.left_end_y = (y + h - 1).max(0) as u32;
            }
            DockEdge::Right => {
                let extent = root_w - x;
                if extent <= 0 {
                    return None;
                }
                s.right = extent as u32;
                s.right_start_y = y.max(0) as u32;
                s.right_end_y = (y + h - 1).max(0) as u32;
            }
        }
        Some(s)
    }

    pub fn from_root_and_rect(
        root_w: u32,
        root_h: u32,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
    ) -> Option<Self> {
        if w == 0 || h == 0 {
            return None;
        }
        let root_w = root_w as i64;
        let root_h = root_h as i64;
        let x = x as i64;
        let y = y as i64;
        let w = w as i64;
        let h = h as i64;
        // Bottom edge wins over left/right corners; the `y > 0` guard keeps
        // full-height rects (y == 0) from misclassifying as bottom struts.
        if y + h == root_h && y > 0 {
            let mut s = Self::NONE;
            s.bottom = (root_h - y).max(0) as u32;
            s.bottom_start_x = x.max(0) as u32;
            s.bottom_end_x = (x + w - 1).max(0) as u32;
            return Some(s);
        }
        if y == 0 && y + h < root_h {
            let mut s = Self::NONE;
            s.top = h.max(0) as u32;
            s.top_start_x = x.max(0) as u32;
            s.top_end_x = (x + w - 1).max(0) as u32;
            return Some(s);
        }
        if x == 0 && x + w < root_w {
            let mut s = Self::NONE;
            s.left = w.max(0) as u32;
            s.left_start_y = y.max(0) as u32;
            s.left_end_y = (y + h - 1).max(0) as u32;
            return Some(s);
        }
        if x + w == root_w && x > 0 {
            let mut s = Self::NONE;
            s.right = (root_w - x).max(0) as u32;
            s.right_start_y = y.max(0) as u32;
            s.right_end_y = (y + h - 1).max(0) as u32;
            return Some(s);
        }
        None
    }

    /// 12-long `_NET_WM_STRUT_PARTIAL` layout (authority): left, right,
    /// top, bottom, left_start_y, left_end_y, right_start_y, right_end_y,
    /// top_start_x, top_end_x, bottom_start_x, bottom_end_x.
    pub fn partial12(&self) -> [u64; 12] {
        [
            self.left as u64,
            self.right as u64,
            self.top as u64,
            self.bottom as u64,
            self.left_start_y as u64,
            self.left_end_y as u64,
            self.right_start_y as u64,
            self.right_end_y as u64,
            self.top_start_x as u64,
            self.top_end_x as u64,
            self.bottom_start_x as u64,
            self.bottom_end_x as u64,
        ]
    }

    /// 4-long `_NET_WM_STRUT` layout: left, right, top, bottom.
    pub fn strut4(&self) -> [u64; 4] {
        [
            self.left as u64,
            self.right as u64,
            self.top as u64,
            self.bottom as u64,
        ]
    }
}

#[cfg(test)]
mod dock_strut_tests {
    use super::*;

    #[test]
    fn bottom_44px_panel_reserves_bottom_with_x_range() {
        let s = DockStrut::from_root_and_rect(1920, 1080, 0, 1036, 1920, 44)
            .expect("bottom panel reserves");
        assert_eq!(s.bottom, 44);
        assert_eq!((s.bottom_start_x, s.bottom_end_x), (0, 1919));
        assert_eq!((s.left, s.right, s.top), (0, 0, 0));
        assert_eq!(s.strut4(), [0, 0, 0, 44]);
        let p = s.partial12();
        assert_eq!(&p[0..4], &[0, 0, 0, 44]);
        assert_eq!((p[10], p[11]), (0, 1919));
    }

    #[test]
    fn top_left_right_reserve_their_edge_only() {
        let top = DockStrut::from_root_and_rect(1920, 1080, 100, 0, 800, 30).unwrap();
        assert_eq!(top.top, 30);
        assert_eq!((top.top_start_x, top.top_end_x), (100, 899));
        assert_eq!((top.left, top.right, top.bottom), (0, 0, 0));
        let left = DockStrut::from_root_and_rect(1920, 1080, 0, 100, 48, 880).unwrap();
        assert_eq!(left.left, 48);
        assert_eq!((left.left_start_y, left.left_end_y), (100, 979));
        let right = DockStrut::from_root_and_rect(1920, 1080, 1872, 100, 48, 880).unwrap();
        assert_eq!(right.right, 48);
        assert_eq!((right.right_start_y, right.right_end_y), (100, 979));
    }

    #[test]
    fn floating_rect_reserves_nothing() {
        assert!(DockStrut::from_root_and_rect(1920, 1080, 400, 300, 800, 600).is_none());
        assert!(DockStrut::from_root_and_rect(1920, 1080, 0, 0, 1920, 1080).is_none());
    }

    #[test]
    fn explicit_edge_classic_matches_inferred_strut() {
        let classic = DockStrut::from_root_and_rect(1920, 1080, 0, 1036, 1920, 44);
        assert_eq!(
            DockStrut::from_edge_rect(1920, 1080, DockEdge::Bottom, 0, 1036, 1920, 44),
            classic
        );
        let left = DockStrut::from_root_and_rect(1920, 1080, 0, 100, 48, 880);
        assert_eq!(
            DockStrut::from_edge_rect(1920, 1080, DockEdge::Left, 0, 100, 48, 880),
            left
        );
    }

    #[test]
    fn explicit_edge_floating_bar_reserves_thickness_plus_inset() {
        // Bar inset 8px from every side: 44px thick, anchored bottom.
        let s = DockStrut::from_edge_rect(1920, 1080, DockEdge::Bottom, 8, 1028, 1904, 44)
            .expect("floating bottom reserves");
        assert_eq!(s.strut4(), [0, 0, 0, 52]);
        assert_eq!((s.bottom_start_x, s.bottom_end_x), (8, 1911));
        let top = DockStrut::from_edge_rect(1920, 1080, DockEdge::Top, 8, 8, 1904, 44).unwrap();
        assert_eq!(top.strut4(), [0, 0, 52, 0]);
        let right =
            DockStrut::from_edge_rect(1920, 1080, DockEdge::Right, 1868, 8, 44, 1064).unwrap();
        assert_eq!(right.strut4(), [0, 52, 0, 0]);
        assert!(
            DockStrut::from_edge_rect(1920, 1080, DockEdge::Bottom, 0, 1080, 1920, 44).is_none()
        );
    }
}

impl SurfaceId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceControllerEvent {
    pub surface: SurfaceId,
    pub event: ActionEvent,
}

impl Default for X11Config {
    fn default() -> Self {
        Self {
            width: 1350,
            height: 641,
            x: 0,
            y: 0,
            title: "FlameWM Render 0.0.9".to_string(),
            role: X11WindowRole::Normal,
            identity: None,
            backdrop: None,
            file_drag: None,
        }
    }
}

impl From<SurfaceConfig> for X11Config {
    fn from(config: SurfaceConfig) -> Self {
        Self {
            width: config.width,
            height: config.height,
            x: config.x,
            y: config.y,
            title: config.title,
            role: config.role.into(),
            identity: None,
            backdrop: None,
            file_drag: None,
        }
    }
}

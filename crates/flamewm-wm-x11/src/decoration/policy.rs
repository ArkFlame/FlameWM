//! Pure decoration-policy decision (no X calls, no rendering).
//!
//! J12 ruling: no verified GTK CSD atom exists in source/reference, so no
//! CSD indicator is invented here. `ClientDecorated` is reserved and
//! currently unreachable; it exists so a future verified hint can map to
//! "managed without Flame frame" without a redesign. `_GTK_FRAME_EXTENTS`
//! alone is never a decoration signal, and `WM_CLASS` is never consulted.
//!

/// Who draws window borders/titlebar for a managed client.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DecorationPolicy {
    /// Flame creates and paints its frame; full EWMH action set applies.
    #[default]
    ServerDecorated,
    /// Reserved: client draws its own decorations. Managed without a Flame
    /// frame. Unreachable until a verified CSD indicator lands.
    #[cfg(test)]
    ClientDecorated,
    /// No Flame frame (override-redirect, non-normal EWMH type, or an
    /// explicit Motif `decorations == 0` no-decoration request).
    #[cfg(test)]
    Undecorated,
}

#[cfg(test)]
impl DecorationPolicy {
    #[must_use]
    pub const fn has_flame_frame(self) -> bool {
        matches!(self, Self::ServerDecorated)
    }
}

/// Inputs to the pure decision. All values are already-read hints.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DecorationRequest {
    /// ICCCM `override_redirect` attribute.
    pub override_redirect: bool,
    /// True when the window carries no EWMH type or only
    /// `_NET_WM_WINDOW_TYPE_NORMAL`.
    pub is_normal_type: bool,
    /// True when `_MOTIF_WM_HINTS` carries `flags & (1 << 1) != 0`
    /// with `decorations == 0` (explicit no-decoration request).
    pub motif_no_decorations: bool,
}

/// Pure mapping. Order matters: override-redirect wins, then non-normal
/// types, then the Motif no-decoration request. Anything else keeps the
/// server frame.
#[cfg(test)]
#[must_use]
pub const fn decide(request: DecorationRequest) -> DecorationPolicy {
    if request.override_redirect || !request.is_normal_type || request.motif_no_decorations {
        DecorationPolicy::Undecorated
    } else {
        DecorationPolicy::ServerDecorated
    }
}

/// ICCCM `WM_NORMAL_HINTS` flag bits (subset used here). Canonical
/// definitions live in [`crate::size_hints`]; re-exported here so existing
/// `policy::` test sites keep compiling.
#[cfg(test)]
pub use crate::size_hints::{HINT_P_MAX_SIZE, HINT_P_MIN_SIZE};

#[cfg(test)]
use crate::size_hints::ClientSizeHints;

/// Pure resizable test from already-read `WM_NORMAL_HINTS` cardinals.
///
/// `values[0]` is flags; `PMinSize` occupies indices 5..=6 and `PMaxSize`
/// indices 7..=8 when their bits are set. A window is fixed-size only when
/// both are present and equal (and non-degenerate); every other
/// hint-free or partial state stays resizable.
#[cfg(test)]
#[must_use]
pub fn normal_hints_resizable(values: &[u32]) -> bool {
    ClientSizeHints::parse(values).resizable
}

/// Parse raw `_MOTIF_WM_HINTS` cardinals (`flags, functions, decorations,
/// input_mode, status`). Returns true only for an explicit no-decoration
/// request: decorations flag present (`flags & (1 << 1)`) and
/// `decorations == 0`.
///
/// Convention: Motif WM hints (`MWM_HINTS_DECORATIONS` = 1 << 1, and
/// `MWM_DECOR_*` bits where `MWM_DECOR_ALL` = 1, `BORDER` = 2, `RESIZEH` = 4,
/// `TITLE` = 8). Used by Qt frameless windows, OnlyOffice, and Electron.
#[cfg(test)]
#[must_use]
pub fn motif_no_decorations(values: &[u32]) -> bool {
    motif_decorations_field(values) == Some(0)
}

/// Motif `MWM_HINTS_DECORATIONS` flag bit.
pub const MOTIF_HINTS_DECORATIONS: u32 = 1 << 1;
/// Motif `MWM_DECOR_TITLE` bit: a title bar is requested.
#[cfg(test)]
pub const MOTIF_DECOR_TITLE: u32 = 1 << 3;

/// The Motif `decorations` field when `MWM_HINTS_DECORATIONS` is set in
/// `flags`; `None` when the client expressed no decoration opinion.
#[must_use]
pub fn motif_decorations_field(values: &[u32]) -> Option<u32> {
    match values {
        [flags, _, decorations, ..] if flags & MOTIF_HINTS_DECORATIONS != 0 => Some(*decorations),
        _ => None,
    }
}

/// Who draws the window frame (titlebar, buttons, border, shadow) for a
/// managed or candidate client. Typed owner for the CSD question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decoration {
    /// Flame draws the frame and titlebar; full EWMH action set applies.
    Server,
    /// Client draws its own frame (GTK CSD). The client owns the invisible
    /// shadow/resize extents; Flame adds no frame.
    ClientSide,
    /// No Flame frame at all (menus, docks, desktop, override-redirect,
    /// or an explicit Motif `decorations == 0`).
    None,
}

/// Already-read inputs for [`decoration_policy`]. No X calls happen here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecorationHints {
    /// ICCCM `override_redirect` attribute.
    pub override_redirect: bool,
    /// Classified EWMH window kind (`WindowKind`), derived from
    /// `_NET_WM_WINDOW_TYPE`.
    pub kind: crate::classifier::WindowKind,
    /// Motif `decorations` field when `MWM_HINTS_DECORATIONS` is set.
    pub motif_decorations: Option<u32>,
    /// `_GTK_FRAME_EXTENTS` (left, right, top, bottom) when present.
    pub gtk_frame_extents: Option<[u32; 4]>,
}

/// Parse `_GTK_FRAME_EXTENTS` (CARDINAL[4]: left, right, top, bottom).
///
/// Convention: GTK3/GTK4 set this on CSD toplevels (Firefox, GNOME apps)
/// to report client-drawn shadow/resize borders. Values are invisible
/// extents, not a request for a server frame.
#[must_use]
pub fn gtk_frame_extents(values: &[u32]) -> Option<[u32; 4]> {
    match values {
        [l, r, t, b, ..] => Some([*l, *r, *t, *b]),
        _ => None,
    }
}

/// Typed decoration decision. Order: override-redirect, then no-frame
/// window kinds (menu/tooltip/desktop/dock), then explicit Motif no-decor,
/// then GTK CSD extents, else server frame.
///
/// Motif title-only (`MWM_DECOR_TITLE` without `MWM_DECOR_ALL`/border) keeps
/// the server titlebar and therefore maps to `Server`. A Motif
/// `decorations` value of zero with the flag set is the only Motif case that
/// removes the frame. `WM_CLASS` is never consulted.
#[must_use]
pub fn decoration_policy(hints: &DecorationHints) -> Decoration {
    use crate::classifier::WindowKind;
    if hints.override_redirect
        || matches!(hints.kind, WindowKind::Popup | WindowKind::Desktop | WindowKind::Dock)
        || hints.motif_decorations == Some(0)
    {
        return Decoration::None;
    }
    if hints
        .gtk_frame_extents
        .is_some_and(|extents| extents.iter().any(|edge| *edge != 0))
    {
        return Decoration::ClientSide;
    }
    Decoration::Server
}

#[cfg(test)]
mod csd_tests {
    use super::*;
    use crate::classifier::WindowKind;

    fn hints(kind: WindowKind) -> DecorationHints {
        DecorationHints { override_redirect: false, kind, motif_decorations: None, gtk_frame_extents: None }
    }

    fn motif_hints(values: &[u32]) -> DecorationHints {
        DecorationHints { motif_decorations: Some(values[2]), ..hints(WindowKind::Normal) }
    }

    #[test]
    fn motif_decorations_zero_removes_frame() {
        // flags = DECORATIONS, functions = 0, decorations = 0
        let values = [MOTIF_HINTS_DECORATIONS, 0, 0, 0, 0];
        assert!(motif_no_decorations(&values));
        assert_eq!(decoration_policy(&motif_hints(&values)), Decoration::None);
    }

    #[test]
    fn motif_title_only_keeps_server_titlebar() {
        // decorations = TITLE only: not a no-decoration request.
        let values = [MOTIF_HINTS_DECORATIONS, 0, MOTIF_DECOR_TITLE, 0, 0];
        assert!(!motif_no_decorations(&values));
        assert_eq!(decoration_policy(&motif_hints(&values)), Decoration::Server);
    }

    #[test]
    fn motif_nonzero_decorations_is_not_a_no_frame_request() {
        // Negative control: decorations = ALL (1) must not hide the frame.
        let values = [MOTIF_HINTS_DECORATIONS, 0, 1, 0, 0];
        assert!(!motif_no_decorations(&values));
        assert_eq!(decoration_policy(&motif_hints(&values)), Decoration::Server);
    }

    #[test]
    fn motif_without_decorations_flag_is_ignored() {
        // Negative control: decorations field of 0 without the flag bit.
        let values = [0, 0, 0, 0, 0];
        assert!(!motif_no_decorations(&values));
    }

    #[test]
    fn gtk_frame_extents_nonzero_is_client_side() {
        let extents = gtk_frame_extents(&[20, 20, 20, 20]).expect("four values");
        let mut h = hints(WindowKind::Normal);
        h.gtk_frame_extents = Some(extents);
        assert_eq!(decoration_policy(&h), Decoration::ClientSide);
    }

    #[test]
    fn gtk_frame_extents_all_zero_stays_server() {
        // Negative control: zero extents are not a CSD signal.
        let mut h = hints(WindowKind::Normal);
        h.gtk_frame_extents = gtk_frame_extents(&[0, 0, 0, 0]);
        assert_eq!(decoration_policy(&h), Decoration::Server);
    }

    #[test]
    fn gtk_frame_extents_short_is_unparsed() {
        assert_eq!(gtk_frame_extents(&[20, 20, 20]), None);
    }

    #[test]
    fn motif_zero_beats_gtk_client_side() {
        let mut h = hints(WindowKind::Normal);
        h.motif_decorations = Some(0);
        h.gtk_frame_extents = Some([8, 8, 8, 8]);
        assert_eq!(decoration_policy(&h), Decoration::None);
    }

    #[test]
    fn non_frame_kinds_have_no_frame() {
        for kind in [WindowKind::Popup, WindowKind::Desktop, WindowKind::Dock] {
            assert_eq!(decoration_policy(&hints(kind)), Decoration::None, "{kind:?}");
        }
    }

    #[test]
    fn override_redirect_has_no_frame() {
        let mut h = hints(WindowKind::Normal);
        h.override_redirect = true;
        assert_eq!(decoration_policy(&h), Decoration::None);
    }

    #[test]
    fn utility_and_normal_keep_server_frame() {
        // Negative control: utility windows are not stripped of their frame.
        assert_eq!(decoration_policy(&hints(WindowKind::Utility)), Decoration::Server);
        assert_eq!(decoration_policy(&hints(WindowKind::Normal)), Decoration::Server);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_redirect_is_undecorated() {
        let policy = decide(DecorationRequest { override_redirect: true, is_normal_type: true, motif_no_decorations: false });
        assert_eq!(policy, DecorationPolicy::Undecorated);
        assert!(!policy.has_flame_frame());
    }

    #[test]
    fn non_normal_type_is_undecorated() {
        let policy = decide(DecorationRequest { override_redirect: false, is_normal_type: false, motif_no_decorations: false });
        assert_eq!(policy, DecorationPolicy::Undecorated);
    }

    #[test]
    fn motif_zero_decorations_is_undecorated() {
        let policy = decide(DecorationRequest { override_redirect: false, is_normal_type: true, motif_no_decorations: true });
        assert_eq!(policy, DecorationPolicy::Undecorated);
    }

    #[test]
    fn normal_hint_free_is_server_decorated() {
        let policy = decide(DecorationRequest { override_redirect: false, is_normal_type: true, motif_no_decorations: false });
        assert_eq!(policy, DecorationPolicy::ServerDecorated);
        assert!(policy.has_flame_frame());
    }

    #[test]
    fn client_decorated_variant_has_no_frame() {
        assert!(!DecorationPolicy::ClientDecorated.has_flame_frame());
    }

    #[test]
    fn empty_hints_stay_resizable() {
        assert!(normal_hints_resizable(&[]));
        assert!(normal_hints_resizable(&[0]));
    }

    #[test]
    fn equal_min_max_is_fixed() {
        let values = vec![
            HINT_P_MIN_SIZE | HINT_P_MAX_SIZE,
            0,
            0,
            0,
            0,
            400,
            300,
            400,
            300,
        ];
        assert!(!normal_hints_resizable(&values));
    }

    #[test]
    fn unequal_min_max_stays_resizable() {
        let values = vec![
            HINT_P_MIN_SIZE | HINT_P_MAX_SIZE,
            0,
            0,
            0,
            0,
            100,
            100,
            800,
            600,
        ];
        assert!(normal_hints_resizable(&values));
    }

    #[test]
    fn motif_parse_requires_flag_and_zero() {
        assert!(motif_no_decorations(&[2, 0, 0, 0, 0]));
        assert!(!motif_no_decorations(&[0, 0, 0, 0, 0]));
        assert!(!motif_no_decorations(&[2, 0, 1, 0, 0]));
        assert!(!motif_no_decorations(&[2, 0]));
    }
}

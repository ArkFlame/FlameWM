//! Desktop-name and workspace-metadata helpers.

/// Workspace root for the canonical indexed `IconResolver` (packaged icon
/// assets live under `<workspace>/assets`). Pure path probe for resolver
/// construction; no icon decode happens here.
pub(crate) fn flamewm_workspace_root() -> std::path::PathBuf {
    crate::frame::titlebar_backdrop::packaged_root()
}

/// True when a `_NET_CURRENT_DESKTOP` value differs from the WM's own current
/// workspace, i.e. the change came from another client rather than our publish.
pub(super) fn desktop_change_is_external(published: Option<u32>, own_current: usize) -> bool {
    published.is_none_or(|value| value as usize != own_current)
}

/// Payload of `_NET_DESKTOP_NAMES` for `count` workspaces: NUL-terminated names.
pub(super) fn desktop_names_payload(count: usize) -> Vec<u8> {
    (1..=count)
        .flat_map(|index| format!("Workspace {index}\0").into_bytes())
        .collect()
}

/// True when `_NET_NUMBER_OF_DESKTOPS` or `_NET_DESKTOP_NAMES` differ from what
/// we last published; an identical echo of our own write must not advance the fence.
pub(super) fn workspace_metadata_is_external(
    count: Option<u32>,
    names: &[u8],
    own_count: usize,
) -> bool {
    count.is_none_or(|value| value as usize != own_count) || !desktop_names_match(names, own_count)
}

/// True when `names` equals `desktop_names_payload(count)`, checked without building the
/// payload: this runs on every root desktop-metadata `PropertyNotify`.
pub(super) fn desktop_names_match(names: &[u8], count: usize) -> bool {
    use std::io::Write;
    let mut rest = names;
    for index in 1..=count {
        let mut buf = [0u8; 32];
        let mut cursor: &mut [u8] = &mut buf;
        // "Workspace " (10) + at most 20 digits + NUL fits in 32 bytes.
        if write!(cursor, "Workspace {index}\0").is_err() {
            return false;
        }
        let written = 32 - cursor.len();
        match rest.strip_prefix(&buf[..written]) {
            Some(next) => rest = next,
            None => return false,
        }
    }
    rest.is_empty()
}

/// Mapping a frame is necessary only when workspace membership changes its
/// actual visibility. In particular, a minimized frame remains unmapped when
/// moved, and a hidden frame must not accumulate unmatched `ignore_unmap` counts.
pub(super) fn frame_visibility_delta(before: bool, after: bool) -> Option<bool> {
    (before != after).then_some(after)
}

/// Windows need re-publishing only when a client's visibility flipped across a
/// workspace switch or the focused window changed; a plain workspace change
/// emits workspace state only.
pub(super) fn workspace_switch_touches_windows(
    visibility_changed: bool,
    focus_changed: bool,
) -> bool {
    visibility_changed || focus_changed
}

//! X11 helper bodies shared by `X11Desktop` in this crate and in `flamewm-wm-x11`.
//!
//! Each owner keeps its own connection, root, atom table, property readers and
//! `pending_mode` state; the owner passes those in as parameters.

use flamewm_api::display::{DisplayMode, DisplaySnapshot, OutputSnapshot};
use flamewm_api::input::PointerPosition;
use flamewm_api::wm_features::{FeatureAction, WindowFeatureSnapshot};
use flamewm_api::workspace::WorkspaceSnapshot;
use flamewm_api::{
    ErrorCode, FlameError, FlameResult, ModeId, OutputId, Point, Rect, Size, TransactionId,
};
use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as RandrConnectionExt};
use x11rb::protocol::xproto::{
    self, Atom, ClientMessageData, ClientMessageEvent, ConfigureWindowAux, ConnectionExt,
    EventMask, Window,
};
use x11rb::rust_connection::RustConnection;

/// An applied-but-not-yet-kept RandR mode change, with the state needed to revert it.
#[derive(Debug, Clone, Copy)]
pub struct PendingMode {
    pub transaction: TransactionId,
    pub crtc: randr::Crtc,
    pub output: randr::Output,
    pub old_mode: randr::Mode,
    pub old_x: i16,
    pub old_y: i16,
    pub old_rotation: randr::Rotation,
}

fn io_error<E: std::fmt::Display>(error: E) -> FlameError {
    FlameError::new(ErrorCode::IoFailure, error.to_string())
}

fn signed(value: u32) -> i32 {
    value as i32
}

pub fn window_geometry_aux(geometry: Rect) -> FlameResult<ConfigureWindowAux> {
    if !geometry.is_valid() {
        return Err(FlameError::invalid("window geometry is invalid"));
    }
    Ok(ConfigureWindowAux::new()
        .x(geometry.x)
        .y(geometry.y)
        .width(
            u32::try_from(geometry.width)
                .map_err(|_| FlameError::invalid("window width is invalid"))?,
        )
        .height(
            u32::try_from(geometry.height)
                .map_err(|_| FlameError::invalid("window height is invalid"))?,
        ))
}

pub fn feature_snapshot(states: &[u32], atom: impl Fn(&str) -> Atom) -> WindowFeatureSnapshot {
    let has = |name| states.contains(&atom(name));
    WindowFeatureSnapshot {
        sticky: has("_NET_WM_STATE_STICKY"),
        modal: has("_NET_WM_STATE_MODAL"),
        shaded: has("_NET_WM_STATE_SHADED"),
        skip_taskbar: has("_NET_WM_STATE_SKIP_TASKBAR"),
        skip_pager: has("_NET_WM_STATE_SKIP_PAGER"),
        above: has("_NET_WM_STATE_ABOVE"),
        below: has("_NET_WM_STATE_BELOW"),
        demands_attention: has("_NET_WM_STATE_DEMANDS_ATTENTION"),
    }
}

pub fn workspace_snapshot(
    count: usize,
    active_index: usize,
    name_bytes: &[u8],
    revision: u64,
) -> WorkspaceSnapshot {
    let count = count.max(1);
    let names = name_bytes
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect();
    WorkspaceSnapshot {
        revision,
        count,
        active_index: active_index.min(count - 1),
        last_index: None,
        names,
    }
}

pub fn base_work_areas(work_area: Rect, outputs: &[OutputSnapshot]) -> Vec<(OutputId, Rect)> {
    outputs
        .iter()
        .filter_map(|output| {
            output
                .geometry
                .intersection(work_area)
                .map(|area| (output.id.clone(), area))
        })
        .collect()
}

pub fn display_modes(
    output_modes: &[randr::Mode],
    resource_modes: &[randr::ModeInfo],
) -> Vec<DisplayMode> {
    output_modes
        .iter()
        .filter_map(|id| resource_modes.iter().find(|mode| mode.id == *id))
        .map(|mode| DisplayMode {
            id: ModeId(u64::from(mode.id)),
            resolution: Size::new(i32::from(mode.width), i32::from(mode.height)),
            refresh_millihz: if mode.htotal == 0 || mode.vtotal == 0 {
                0
            } else {
                ((mode.dot_clock as u64 * 1000) / u64::from(mode.htotal) / u64::from(mode.vtotal))
                    as i32
            },
            preferred: output_modes.first() == Some(&mode.id),
        })
        .collect()
}

pub fn client_message(
    conn: &RustConnection,
    root: Window,
    window: Window,
    type_: Atom,
    data: [u32; 5],
) -> FlameResult<()> {
    let event = ClientMessageEvent {
        response_type: xproto::CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window,
        type_,
        data: ClientMessageData::from(data),
    };
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )
    .map_err(io_error)?
    .check()
    .map_err(io_error)?;
    conn.flush().map_err(io_error)
}

pub fn send_state(
    conn: &RustConnection,
    root: Window,
    net_wm_state: Atom,
    window: Window,
    atom: Atom,
    action: FeatureAction,
) -> FlameResult<()> {
    let action = match action {
        FeatureAction::Remove => 0,
        FeatureAction::Add => 1,
        FeatureAction::Toggle => 2,
    };
    client_message(conn, root, window, net_wm_state, [action, atom, 0, 1, 0])
}

/// `property32` reads a root-window property as 32-bit words for the owner's connection.
pub fn root_work_area(property32: impl Fn(&str) -> FlameResult<Vec<u32>>) -> FlameResult<Rect> {
    let invalid = || FlameError::unavailable("root _NET_WORKAREA is invalid");
    let desktop = property32("_NET_CURRENT_DESKTOP")?
        .first()
        .copied()
        .unwrap_or(0) as usize;
    let values = property32("_NET_WORKAREA")?;
    let start = desktop.checked_mul(4).ok_or_else(invalid)?;
    if values.len() < start.saturating_add(4) {
        return Err(FlameError::unavailable("root _NET_WORKAREA is unavailable"));
    }
    let area = Rect::new(
        signed(values[start]),
        signed(values[start + 1]),
        values[start + 2] as i32,
        values[start + 3] as i32,
    );
    area.is_valid().then_some(area).ok_or_else(invalid)
}

pub fn text_from_bytes(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches('\0')
        .to_owned()
}

pub fn root_pointer(
    conn: &RustConnection,
    root: Window,
    outputs: impl FnOnce() -> FlameResult<DisplaySnapshot>,
) -> FlameResult<PointerPosition> {
    let reply = conn
        .query_pointer(root)
        .map_err(io_error)?
        .reply()
        .map_err(io_error)?;
    let point = Point::new(i32::from(reply.root_x), i32::from(reply.root_y));
    Ok(PointerPosition {
        root: point,
        output: outputs()?
            .outputs
            .into_iter()
            .find(|o| o.geometry.contains(point))
            .map(|o| o.id),
    })
}

pub fn apply_mode(
    conn: &RustConnection,
    root: Window,
    pending_mode: &mut Option<PendingMode>,
    next_transaction: &mut u64,
    output_name: &OutputId,
    mode: ModeId,
) -> FlameResult<TransactionId> {
    if pending_mode.is_some() {
        return Err(FlameError::new(
            ErrorCode::Busy,
            "RandR mode transaction already pending",
        ));
    }
    let resources = conn
        .randr_get_screen_resources_current(root)
        .map_err(io_error)?
        .reply()
        .map_err(io_error)?;
    for output in resources.outputs {
        let info = conn
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
        let old = conn
            .randr_get_crtc_info(crtc, resources.config_timestamp)
            .map_err(io_error)?
            .reply()
            .map_err(io_error)?;
        let reply = conn
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
        *next_transaction = next_transaction.saturating_add(1).max(1);
        let transaction = TransactionId(*next_transaction);
        *pending_mode = Some(PendingMode {
            transaction,
            crtc,
            output,
            old_mode: old.mode,
            old_x: old.x,
            old_y: old.y,
            old_rotation: old.rotation,
        });
        conn.flush().map_err(io_error)?;
        return Ok(transaction);
    }
    Err(FlameError::not_found("RandR output not found"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(id: &str, geometry: Rect) -> OutputSnapshot {
        OutputSnapshot {
            id: OutputId::new(id),
            connector: id.to_owned(),
            edid_identity: String::new(),
            connected: true,
            primary: false,
            geometry,
            current_mode: ModeId(0),
            modes: Vec::new(),
            shell_scale_percent: 100,
        }
    }

    #[test]
    fn shared_window_geometry_conversion_validates_before_x_request() {
        let aux = window_geometry_aux(Rect::new(-4, 9, 640, 480)).unwrap();
        assert_eq!(aux.x, Some(-4));
        assert_eq!(aux.y, Some(9));
        assert_eq!(aux.width, Some(640));
        assert_eq!(aux.height, Some(480));
        assert!(window_geometry_aux(Rect::new(0, 0, 0, 10)).is_err());
        assert!(window_geometry_aux(Rect::new(0, 0, -1, 10)).is_err());
    }

    #[test]
    fn shared_feature_projection_maps_each_ewmh_atom_independently() {
        let names = [
            "_NET_WM_STATE_STICKY",
            "_NET_WM_STATE_SHADED",
            "_NET_WM_STATE_SKIP_PAGER",
            "_NET_WM_STATE_ABOVE",
            "_NET_WM_STATE_DEMANDS_ATTENTION",
        ];
        let atoms = names
            .iter()
            .enumerate()
            .map(|(i, name)| (*name, i as Atom + 10))
            .collect::<std::collections::BTreeMap<_, _>>();
        let states = [10, 11, 12, 13, 14];
        let snapshot = feature_snapshot(&states, |name| atoms.get(name).copied().unwrap_or(0));
        assert!(snapshot.sticky);
        assert!(!snapshot.modal);
        assert!(snapshot.shaded);
        assert!(!snapshot.skip_taskbar);
        assert!(snapshot.skip_pager);
        assert!(snapshot.above);
        assert!(!snapshot.below);
        assert!(snapshot.demands_attention);
        assert_eq!(
            feature_snapshot(&[], |_| 0),
            WindowFeatureSnapshot::default()
        );
    }

    #[test]
    fn workspace_projection_clamps_topology_and_preserves_revision() {
        let snapshot = workspace_snapshot(0, 7, b"one\0\0two\xff\0", 42);
        assert_eq!(snapshot.revision, 42);
        assert_eq!(snapshot.count, 1);
        assert_eq!(snapshot.active_index, 0);
        assert_eq!(snapshot.names, vec!["one".to_owned(), "two�".to_owned()]);
        let snapshot = workspace_snapshot(4, 8, b"", 9);
        assert_eq!(snapshot.active_index, 3);
        assert!(snapshot.names.is_empty());
    }

    #[test]
    fn work_area_projection_keeps_order_and_only_intersections() {
        let outputs = [
            output("left", Rect::new(-100, 0, 100, 100)),
            output("right", Rect::new(0, 0, 100, 100)),
            output("disabled", Rect::default()),
        ];
        assert_eq!(
            base_work_areas(Rect::new(-50, 20, 100, 50), &outputs),
            vec![
                (OutputId::new("left"), Rect::new(-50, 20, 50, 50)),
                (OutputId::new("right"), Rect::new(0, 20, 50, 50)),
            ]
        );
    }

    #[test]
    fn randr_mode_projection_preserves_server_order_refresh_and_preference() {
        let modes = [
            randr::ModeInfo {
                id: 7,
                width: 1920,
                height: 1080,
                dot_clock: 148_500_000,
                htotal: 2200,
                vtotal: 1125,
                ..Default::default()
            },
            randr::ModeInfo {
                id: 8,
                width: 800,
                height: 600,
                dot_clock: 40_000_000,
                htotal: 0,
                vtotal: 628,
                ..Default::default()
            },
        ];
        let projected = display_modes(&[8, 7, 99], &modes);
        assert_eq!(projected.len(), 2);
        assert_eq!(projected[0].id, ModeId(8));
        assert_eq!(projected[0].refresh_millihz, 0);
        assert!(projected[0].preferred);
        assert_eq!(projected[1].id, ModeId(7));
        assert_eq!(projected[1].refresh_millihz, 60_000);
        assert!(!projected[1].preferred);
    }
}

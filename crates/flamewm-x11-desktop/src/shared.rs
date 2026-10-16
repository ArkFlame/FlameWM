//! X11 helper bodies shared by `X11Desktop` in this crate and in `flamewm-wm-x11`.
//!
//! Each owner keeps its own connection, root, atom table, property readers and
//! `pending_mode` state; the owner passes those in as parameters.

use flamewm_api::display::DisplaySnapshot;
use flamewm_api::input::PointerPosition;
use flamewm_api::wm_features::FeatureAction;
use flamewm_api::{
    ErrorCode, FlameError, FlameResult, ModeId, OutputId, Point, Rect, TransactionId,
};
use x11rb::CURRENT_TIME;
use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as RandrConnectionExt};
use x11rb::protocol::xproto::{
    self, Atom, ClientMessageData, ClientMessageEvent, ConnectionExt, EventMask, Window,
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
    let desktop = property32("_NET_CURRENT_DESKTOP")?
        .first()
        .copied()
        .unwrap_or(0) as usize;
    let values = property32("_NET_WORKAREA")?;
    let start = desktop
        .checked_mul(4)
        .ok_or_else(|| FlameError::unavailable("root _NET_WORKAREA is invalid"))?;
    if values.len() < start.saturating_add(4) {
        return Err(FlameError::unavailable("root _NET_WORKAREA is unavailable"));
    }
    let area = Rect::new(
        signed(values[start]),
        signed(values[start + 1]),
        values[start + 2] as i32,
        values[start + 3] as i32,
    );
    area.is_valid()
        .then_some(area)
        .ok_or_else(|| FlameError::unavailable("root _NET_WORKAREA is invalid"))
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

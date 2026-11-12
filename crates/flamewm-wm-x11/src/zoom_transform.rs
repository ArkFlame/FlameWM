//! Magnifier output on X11: one `RandR` CRTC transform per scanning crtc.
//!
//! [`crate::zoom`] owns the level and the follow mode and computes the sampled viewport; this module
//! sends it to the server. A transform maps each output pixel to a framebuffer pixel, so a 2x
//! magnifier is `src = viewport.origin + dst / 2`: scale 0.5 with the translate set to the viewport
//! origin, which keeps the pointer's source pixel on the output pixel that shows it.
//!
//! Direction (`RandR` `SetCrtcTransform`, the `RandR` protocol spec, and the Xorg `randr` transform that
//! applies the matrix as destination -> source through pixman): the request matrix maps output
//! coordinates to framebuffer coordinates. The derivation is checked by a unit test below. The
//! spec text was not re-read in this tree, and no X server here honours the request (see the proof
//! limit in the commit message), so the on-screen direction is not runtime-proven.
//!
//! Server state: the transform lives in the X server, not in this process.
//! - A graceful exit or an early error return drops [`ResetOnExit`], which restores identity.
//!   The X11 loop has no signal handler, so no other exit path exists.
//! - A WM crash leaves the last transform in place; the first WM turn applies the current level
//!   (identity at start), which clears it.
//!
//! Precedent: `blue_light.rs` (crtc walk and per-crtc error handling); the viewport math mirrors
//! `wm-wayland` zoom.rs `viewport` (choice only, not shared: that crate is Smithay-facing).

use std::cell::Cell;
use std::rc::Rc;

use flamewm_api::settings::ZOOM_LEVEL_MIN;
use x11rb::cookie::VoidCookie;
use x11rb::errors::ReplyError;
use x11rb::protocol::randr::ConnectionExt as RandrConnectionExt;
use x11rb::protocol::render::{Fixed, Transform};
use x11rb::protocol::xproto::{ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

use crate::zoom::{self, Viewport};

/// One unit in 16.16 fixed point.
const ONE: Fixed = 65_536;
/// `RandR`'s standard filter name for smooth scaling.
const FILTER: &[u8] = b"bilinear";

thread_local! {
    /// Matrix last sent to the server; an unchanged viewport costs no request.
    static SENT: Cell<Option<[Fixed; 9]>> = const { Cell::new(None) };
    /// Viewport origin last sent, with the zoom it was taken at: `Push` holds it across turns.
    static LAST: Cell<Option<(f64, (f64, f64))>> = const { Cell::new(None) };
}

/// The row-major 3x3 matrix (16.16) that maps output pixels to framebuffer pixels for `view`.
pub(crate) fn magnify_matrix(view: Viewport, size: (f64, f64)) -> [Fixed; 9] {
    let fixed = |value: f64| (value * f64::from(ONE)).round() as Fixed;
    [
        fixed(view.w / size.0),
        0,
        fixed(view.x),
        0,
        fixed(view.h / size.1),
        fixed(view.y),
        0,
        0,
        ONE,
    ]
}

fn to_transform(m: [Fixed; 9]) -> Transform {
    Transform {
        matrix11: m[0],
        matrix12: m[1],
        matrix13: m[2],
        matrix21: m[3],
        matrix22: m[4],
        matrix23: m[5],
        matrix31: m[6],
        matrix32: m[7],
        matrix33: m[8],
    }
}

/// Sends the viewport for `zoom` and the committed follow mode to every scanning crtc of `root`.
/// A no-op when the resulting matrix is already on the server.
pub(crate) fn apply(conn: &RustConnection, root: Window, zoom: f64) -> Result<bool, ReplyError> {
    let geometry = conn.get_geometry(root)?.reply()?;
    let size = (f64::from(geometry.width), f64::from(geometry.height));
    // The pointer only steers a magnified view; at 1.0 the viewport is the whole output.
    let pointer = if zoom > ZOOM_LEVEL_MIN {
        let at = conn.query_pointer(root)?.reply()?;
        (f64::from(at.root_x), f64::from(at.root_y))
    } else {
        (0.0, 0.0)
    };
    let previous = LAST
        .with(Cell::get)
        .filter(|(last_zoom, _)| *last_zoom == zoom)
        .map(|(_, origin)| origin);
    let view = zoom::viewport(zoom::follow(), size, zoom, pointer, previous);
    LAST.with(|slot| slot.set(Some((zoom, (view.x, view.y)))));
    let matrix = magnify_matrix(view, size);
    if SENT.with(Cell::get) == Some(matrix) {
        return Ok(false);
    }
    let crtcs = crate::blue_light::crtcs_for_root(conn, root)?;
    SENT.with(|slot| slot.set(Some(matrix)));
    for crtc in crtcs {
        let set = conn
            .randr_set_crtc_transform(crtc, to_transform(matrix), FILTER, &[])
            .map_err(ReplyError::ConnectionError)
            .and_then(VoidCookie::check);
        if let Err(error) = set {
            // A server without transform support leaves the picture unscaled; the state still holds.
            eprintln!("flamewm: zoom transform not applied to crtc {crtc}: {error}");
        }
    }
    Ok(true)
}

/// Forgets the last send so the next apply walks the crtcs again (output topology changed).
pub(crate) fn invalidate() {
    SENT.with(|slot| slot.set(None));
    LAST.with(|slot| slot.set(None));
}

/// Restores the identity transform on every crtc of `root` (level 1.0 is the whole output).
fn restore(conn: &RustConnection, root: Window) {
    invalidate();
    if let Err(error) = apply(conn, root, ZOOM_LEVEL_MIN) {
        eprintln!("flamewm: zoom transform reset failed: {error}");
    }
}

/// Restores identity when dropped. The X11 loop is left only through its return path, so holding
/// this guard for the whole run covers graceful exit, early `?` errors and unwinding.
pub(crate) struct ResetOnExit {
    conn: Rc<RustConnection>,
    root: Window,
}

impl ResetOnExit {
    pub(crate) fn new(conn: Rc<RustConnection>, root: Window) -> Self {
        Self { conn, root }
    }
}

impl Drop for ResetOnExit {
    fn drop(&mut self) {
        restore(&self.conn, self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: (f64, f64) = (800.0, 600.0);

    #[test]
    fn whole_output_viewport_is_the_identity_matrix() {
        let whole = zoom::viewport(
            zoom::FollowMode::Centred,
            OUTPUT,
            ZOOM_LEVEL_MIN,
            (0.0, 0.0),
            None,
        );
        assert_eq!(
            magnify_matrix(whole, OUTPUT),
            [ONE, 0, 0, 0, ONE, 0, 0, 0, ONE]
        );
    }

    /// `RandR` maps output -> framebuffer: the output centre must sample the pointer's source pixel.
    #[test]
    fn pointer_centred_magnifier_maps_the_pointer_to_the_output_centre() {
        let pointer = (400.0, 300.0);
        let view = zoom::viewport(zoom::FollowMode::Centred, OUTPUT, 2.0, pointer, None);
        let m = magnify_matrix(view, OUTPUT);
        // src.x = m[0] * dst.x + m[2]; the output centre (400, 300) must sample the pointer (400, 300).
        assert_eq!(m[0] * 400 + m[2], 400 * ONE);
        assert_eq!(m[4] * 300 + m[5], 300 * ONE);
        // Scale is 1/zoom: one output pixel spans half a framebuffer pixel.
        assert_eq!((m[0], m[4]), (ONE / 2, ONE / 2));
    }

    /// At the output edge the clamped viewport still maps the first output pixel to source 0.
    #[test]
    fn edge_pointer_clamps_the_viewport_inside_the_output() {
        let view = zoom::viewport(zoom::FollowMode::Centred, OUTPUT, 2.0, (0.0, 0.0), None);
        let m = magnify_matrix(view, OUTPUT);
        assert_eq!((m[2], m[5]), (0, 0));
        // The far corner samples the last source column, never beyond the framebuffer.
        assert!(m[0] * 800 + m[2] <= 800 * ONE);
    }
}

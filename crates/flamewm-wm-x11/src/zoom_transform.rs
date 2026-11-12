//! Magnifier output on X11: one `RandR` CRTC transform per scanning crtc.
//!
//! [`crate::zoom`] owns the level; this module makes it visible. Each crtc gets a 16.16 transform
//! that magnifies about the screen centre, so the server scans out the enlarged region. Level 1.0
//! is the identity. Precedent: `blue_light.rs` (same crtc walk and per-crtc error handling).
//!
//! Server state: the transform lives in the X server, not in this process. Two consequences:
//! - A WM crash leaves the last transform in place. The first WM turn applies the current level
//!   (identity at start), which clears a stale transform.
//! - A graceful exit does not restore identity. shortcut: no shutdown hook on the X11 loop;
//!   revisit when one exists.
//!
//! Direction: the transform maps output (destination) coordinates to framebuffer (source) ones,
//! `src = centre + (dst - centre) / level`. This follows the RandR/pixman convention and is not
//! verified on hardware. Xvfb rejects `SetCrtcTransform` with `BadValue`, so this path is unit-tested
//! for the matrix only.

use std::cell::Cell;

use x11rb::cookie::VoidCookie;
use x11rb::errors::ReplyError;
use x11rb::protocol::randr::ConnectionExt as RandrConnectionExt;
use x11rb::protocol::render::{Fixed, Transform};
use x11rb::protocol::xproto::{ConnectionExt as _, Window};
use x11rb::rust_connection::RustConnection;

/// One unit in 16.16 fixed point.
const ONE: Fixed = 65_536;
/// `RandR`'s standard filter name for smooth scaling.
const FILTER: &[u8] = b"bilinear";

thread_local! {
    /// Level last sent to the server; an unchanged level costs no round trip.
    static APPLIED: Cell<Option<f64>> = const { Cell::new(None) };
}

/// The 16.16 transform that magnifies about `centre` by `level` (destination -> source).
pub(crate) fn magnify_transform(level: f64, centre: (f64, f64)) -> Transform {
    let scale = (f64::from(ONE) / level).round() as Fixed;
    let offset = |c: f64| (c * f64::from(ONE - scale)).round() as Fixed;
    Transform {
        matrix11: scale,
        matrix12: 0,
        matrix13: offset(centre.0),
        matrix21: 0,
        matrix22: scale,
        matrix23: offset(centre.1),
        matrix31: 0,
        matrix32: 0,
        matrix33: ONE,
    }
}

/// Sends `level` to every scanning crtc of `root`; a no-op when that level is already applied.
pub(crate) fn apply(conn: &RustConnection, root: Window, level: f64) -> Result<bool, ReplyError> {
    if APPLIED.with(Cell::get) == Some(level) {
        return Ok(false);
    }
    let geometry = conn.get_geometry(root)?.reply()?;
    let centre = (
        f64::from(geometry.width) / 2.0,
        f64::from(geometry.height) / 2.0,
    );
    let transform = magnify_transform(level, centre);
    let crtcs = crate::blue_light::crtcs_for_root(conn, root)?;
    APPLIED.with(|slot| slot.set(Some(level)));
    for crtc in crtcs {
        let set = conn
            .randr_set_crtc_transform(crtc, transform, FILTER, &[])
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
    APPLIED.with(|slot| slot.set(None));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_level_is_the_identity_transform() {
        let t = magnify_transform(1.0, (800.0, 450.0));
        assert_eq!(
            [
                t.matrix11, t.matrix12, t.matrix13, t.matrix21, t.matrix22, t.matrix23, t.matrix31,
                t.matrix32, t.matrix33
            ],
            [ONE, 0, 0, 0, ONE, 0, 0, 0, ONE]
        );
    }

    /// The centre is a fixed point of the magnifier, and the map is linear with slope 1/level.
    #[test]
    fn magnifier_keeps_the_centre_fixed_and_scales_offsets() {
        let t = magnify_transform(2.0, (800.0, 450.0));
        assert_eq!((t.matrix11, t.matrix22), (ONE / 2, ONE / 2));
        // src(centre) = (1/2) * centre + offset must equal centre.
        assert_eq!(t.matrix11 * 800 + t.matrix13, 800 * ONE);
        assert_eq!(t.matrix22 * 450 + t.matrix23, 450 * ONE);
        // A destination 100 px right of the centre samples 50 px right of it.
        assert_eq!(t.matrix11 * 900 + t.matrix13, 850 * ONE);
    }
}

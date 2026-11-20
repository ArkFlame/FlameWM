//! X11 per-output scale via `RandR`: pure planning and a thin apply step.
//!
//! The scale percent persisted in
//! `flamewm_platform::settings::product::per_output_scale` is a logical-to-physical
//! multiplier (100 = 1.0). For a non-1.0 scale we plan a `RandR` output transform that
//! scales the framebuffer up to fill the physical mode, shrink the framebuffer (and its
//! panning region) to the resulting logical size, and apply the result through `x11rb`.
//!
//! Planning is pure data with no native handles, so the arithmetic is unit-tested without
//! an X server. [`apply_scale`] is the only function that touches the connection.

use std::collections::BTreeMap;

use flamewm_api::{ErrorCode, FlameError, FlameResult};
use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as RandrConnectionExt};
use x11rb::protocol::render::{self, Fixed};
use x11rb::protocol::xproto::Window;
use x11rb::CURRENT_TIME;

/// 16.16 fixed-point value for `1.0` (the third diagonal element of the matrix).
const FIXED_ONE: Fixed = 1 << 16;

/// Pure `RandR` scale plan for one output. Contains no native handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScalePlan {
    /// Logical framebuffer size (physical size divided by the scale, rounded), at least 1x1.
    pub logical_width: u16,
    pub logical_height: u16,
    /// 3x3 16.16 fixed-point transform matrix; scales content up by the chosen factor.
    pub transform: [[Fixed; 3]; 3],
}

impl ScalePlan {
    /// Converts the planned matrix into the `Render` transform used by `x11rb`.
    fn render_transform(self) -> render::Transform {
        let [[m11, m12, m13], [m21, m22, m23], [m31, m32, m33]] = self.transform;
        render::Transform {
            matrix11: m11,
            matrix12: m12,
            matrix13: m13,
            matrix21: m21,
            matrix22: m22,
            matrix23: m23,
            matrix31: m31,
            matrix32: m32,
            matrix33: m33,
        }
    }
}

/// Plans the `RandR` transform for `output` from the persisted per-output scale map.
///
/// Returns `None` when the output has no saved scale, the saved value is 100% (1.0), or
/// the value is outside the supported 100..=200 range in 5% steps.
#[must_use]
pub fn plan_output_scale(
    per_output_scale: &BTreeMap<String, u16>,
    output: &str,
    physical_width: u16,
    physical_height: u16,
) -> Option<ScalePlan> {
    let percent = *per_output_scale.get(output)?;
    plan_scale(percent, physical_width, physical_height)
}

/// Pure scale math: builds the transform and logical framebuffer for a valid percent.
///
/// A 100% scale (1.0) is a no-op and returns `None`, as do percentages outside the
/// supported 100..=200 range in 5% steps.
#[must_use]
pub fn plan_scale(percent: u16, physical_width: u16, physical_height: u16) -> Option<ScalePlan> {
    if percent == 100 || !(100..=200).contains(&percent) || percent % 5 != 0 {
        return None;
    }
    let scale = f64::from(percent) / 100.0;
    let scale_fixed = (scale * f64::from(FIXED_ONE)).round() as Fixed;
    Some(ScalePlan {
        logical_width: logical_dimension(physical_width, scale),
        logical_height: logical_dimension(physical_height, scale),
        transform: [[scale_fixed, 0, 0], [0, scale_fixed, 0], [0, 0, FIXED_ONE]],
    })
}

fn logical_dimension(physical: u16, scale: f64) -> u16 {
    let logical = (f64::from(physical) / scale).round();
    logical.clamp(1.0, f64::from(u16::MAX)) as u16
}

/// Applies `plan` to `crtc`: disable panning, install the scale transform, resize the
/// framebuffer to the logical size, then lock panning to that logical region.
///
/// Panning is disabled first because `RandR` requires it before changing a CRTC transform.
/// The framebuffer resize is global to the X screen; coordinating different scales across
/// multiple outputs is the caller's responsibility, not this helper's.
pub fn apply_scale<C: Connection>(
    conn: &C,
    root: Window,
    crtc: randr::Crtc,
    plan: &ScalePlan,
) -> FlameResult<()> {
    let config_timestamp = conn
        .randr_get_screen_resources_current(root)
        .map_err(FlameError::io)?
        .reply()
        .map_err(FlameError::io)?
        .config_timestamp;

    // `RandR` will not accept a transform while panning is active on the CRTC.
    let _ = conn
        .randr_set_panning(crtc, config_timestamp, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
        .map_err(FlameError::io)?
        .reply()
        .map_err(FlameError::io)?;

    conn.randr_set_crtc_transform(crtc, plan.render_transform(), b"bilinear", &[])
        .map_err(FlameError::io)?
        .check()
        .map_err(FlameError::io)?;

    conn.randr_set_screen_size(root, plan.logical_width, plan.logical_height, 0, 0)
        .map_err(FlameError::io)?
        .check()
        .map_err(FlameError::io)?;

    let status = conn
        .randr_set_panning(
            crtc,
            CURRENT_TIME,
            0,
            0,
            plan.logical_width,
            plan.logical_height,
            0,
            0,
            plan.logical_width,
            plan.logical_height,
            0,
            0,
            0,
            0,
        )
        .map_err(FlameError::io)?
        .reply()
        .map_err(FlameError::io)?
        .status;
    if u8::from(status) != u8::from(randr::SetConfig::SUCCESS) {
        return Err(FlameError::new(
            ErrorCode::EngineRejected,
            "RandR rejected scale panning",
        ));
    }
    conn.flush().map_err(FlameError::io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unity_and_unsupported_scales_are_no_ops() {
        assert_eq!(plan_scale(100, 1920, 1080), None);
        assert_eq!(plan_scale(99, 1920, 1080), None);
        assert_eq!(plan_scale(201, 1920, 1080), None);
        assert_eq!(plan_scale(103, 1920, 1080), None);
        assert_eq!(plan_scale(0, 1920, 1080), None);
    }

    #[test]
    fn scale_150_shrinks_framebuffer_and_builds_a_diagonal_transform() {
        let plan = plan_scale(150, 1920, 1080).unwrap();
        assert_eq!(plan.logical_width, 1280);
        assert_eq!(plan.logical_height, 720);
        assert_eq!(plan.transform[0], [98304, 0, 0]); // 1.5 * 2^16
        assert_eq!(plan.transform[1], [0, 98304, 0]);
        assert_eq!(plan.transform[2], [0, 0, FIXED_ONE]);
    }

    #[test]
    fn scale_125_rounds_logical_dimensions_to_nearest_pixel() {
        let plan = plan_scale(125, 1920, 1080).unwrap();
        assert_eq!(plan.logical_width, 1536);
        assert_eq!(plan.logical_height, 864);
        assert_eq!(plan.transform[0][0], 81920); // 1.25 * 2^16
    }

    #[test]
    fn fractional_scale_rounds_fixed_point_and_dimensions() {
        let plan = plan_scale(110, 1920, 1080).unwrap();
        assert_eq!(plan.logical_width, 1745); // 1920 / 1.1 = 1745.45
        assert_eq!(plan.logical_height, 982); // 1080 / 1.1 = 981.82
        assert_eq!(plan.transform[0][0], 72090); // 1.1 * 2^16 = 72089.6
        assert_eq!(plan.transform[1][1], 72090);
    }

    #[test]
    fn tiny_modes_never_produce_a_zero_logical_dimension() {
        let plan = plan_scale(200, 1, 1).unwrap();
        assert_eq!(plan.logical_width, 1);
        assert_eq!(plan.logical_height, 1);
    }

    #[test]
    fn plan_output_scale_reads_only_the_named_output() {
        let scales = BTreeMap::from([
            ("eDP-1".to_owned(), 150_u16),
            ("HDMI-1".to_owned(), 100_u16),
        ]);
        assert_eq!(
            plan_output_scale(&scales, "eDP-1", 1920, 1080),
            plan_scale(150, 1920, 1080)
        );
        assert_eq!(plan_output_scale(&scales, "HDMI-1", 1920, 1080), None);
        assert_eq!(plan_output_scale(&scales, "DP-9", 1920, 1080), None);
    }
}

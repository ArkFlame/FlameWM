//! Lowers a whole frame to GPU pixel ops for `renderAcceleration=gpu`.
//!
//! A frame qualifies only when every command is a rounded fill, a stroke or a scrollbar track on
//! whole device pixels. Anything else (text, images, shadows, backdrops, clips, scrollbar thumbs,
//! fractional geometry) keeps the XRender path for the whole frame, so the two painters never mix.

use flamewm_render_core::{PaintCommand, PixelOp, Rect, RoundedRect};

use super::primitives::scale_rect;

/// Pixel ops for `commands` at device `scale`, or `None` when the frame must take the XRender path.
pub(crate) fn frame_ops(commands: &[PaintCommand], scale: f32) -> Option<Vec<PixelOp>> {
    let mut ops = Vec::with_capacity(commands.len());
    for command in commands {
        match *command {
            PaintCommand::FillRect {
                rect,
                color,
                radius,
            } => ops.push(PixelOp::FillRoundedRect {
                outer: device_rect(rect, radius, scale)?,
                hole: None,
                color: [color.r, color.g, color.b, color.a],
            }),
            PaintCommand::StrokeRect {
                rect,
                color,
                width,
                radius,
            } => {
                let outer = device_rect(rect, radius, scale)?;
                let thickness = whole(width * scale)?;
                ops.push(PixelOp::FillRoundedRect {
                    outer,
                    hole: Some(outer.inset(thickness)),
                    color: [color.r, color.g, color.b, color.a],
                });
            }
            // The XRender painter draws nothing for a track, so the GPU frame skips it too.
            PaintCommand::ScrollbarTrack { .. } => {}
            _ => return None,
        }
    }
    Some(ops)
}

/// The device-pixel rounded rectangle for `rect`, or `None` when it is fractional or empty.
fn device_rect(rect: Rect, radius: f32, scale: f32) -> Option<RoundedRect> {
    let rect = scale_rect(rect, scale);
    let outer = RoundedRect {
        x: whole(rect.x)? as i32,
        y: whole(rect.y)? as i32,
        width: whole(rect.width)?,
        height: whole(rect.height)?,
        radius: whole(radius * scale)?,
    };
    (outer.width > 0 && outer.height > 0).then(|| outer.normalized())
}

/// `value` as a whole non-negative number, or `None` when it has a fraction.
fn whole(value: f32) -> Option<u32> {
    (value >= 0.0 && value.fract() == 0.0 && value <= u32::MAX as f32).then_some(value as u32)
}

#[cfg(test)]
mod tests {
    use flamewm_render_core::Color;

    use super::*;

    fn fill(x: f32, width: f32, radius: f32) -> PaintCommand {
        PaintCommand::FillRect {
            rect: Rect {
                x,
                y: 4.0,
                width,
                height: 20.0,
            },
            color: Color {
                r: 1,
                g: 2,
                b: 3,
                a: 200,
            },
            radius,
        }
    }

    #[test]
    fn whole_pixel_fills_and_strokes_lower_to_pixel_ops() {
        let stroke = PaintCommand::StrokeRect {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 40.0,
                height: 30.0,
            },
            color: Color {
                r: 9,
                g: 9,
                b: 9,
                a: 255,
            },
            width: 2.0,
            radius: 8.0,
        };
        let ops = frame_ops(&[fill(10.0, 50.0, 6.0), stroke], 1.0).expect("supported frame");
        assert_eq!(ops.len(), 2);
        let PixelOp::FillRoundedRect { hole, .. } = ops[1] else {
            panic!("stroke lowers to a fill with a hole");
        };
        assert_eq!(hole.map(|hole| hole.width), Some(36));
    }

    #[test]
    fn text_images_backdrops_clips_and_thumbs_keep_the_xrender_path() {
        let text = PaintCommand::Text {
            x: 0.0,
            y: 0.0,
            color: Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            },
            size: 12.0,
            weight: 400,
            text: "x".into(),
        };
        assert!(frame_ops(&[fill(0.0, 10.0, 0.0), text], 1.0).is_none());
        assert!(frame_ops(&[PaintCommand::PopClip], 1.0).is_none());
        let thumb = PaintCommand::ScrollbarThumb {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 4.0,
                height: 4.0,
            },
            horizontal: false,
        };
        assert!(frame_ops(&[thumb], 1.0).is_none());
    }

    #[test]
    fn fractional_device_geometry_keeps_the_xrender_path() {
        assert!(frame_ops(&[fill(10.5, 50.0, 6.0)], 1.0).is_none());
        assert!(
            frame_ops(&[fill(10.0, 51.0, 6.0)], 1.5).is_none(),
            "76.5 device px wide"
        );
        assert!(
            frame_ops(&[fill(10.0, 50.0, 6.0)], 1.5).is_some(),
            "whole at 1.5x"
        );
        assert!(frame_ops(&[fill(10.0, 50.0, 6.0)], 2.0).is_some());
    }
}

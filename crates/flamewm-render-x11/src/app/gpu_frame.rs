//! Lowers a whole frame to GPU pixel ops for `renderAcceleration=gpu`.
//!
//! A frame qualifies when every command is a rounded fill, a stroke, a scrollbar track on whole
//! device pixels, a text run, a text shadow or an image. Text and shadows reach the GPU as the
//! Xft A8 glyph coverage the XRender path already draws (shadows blurred by the same code), and
//! images as the same premultiplied pixels their XRender blit uses. Node backdrops, clips,
//! scrollbar thumbs and fractional geometry keep the XRender path for the whole frame, so the two
//! painters never mix.

use flamewm_render_core::{PaintCommand, PixelOp, Rect, RoundedRect};

use super::primitives::scale_rect;

/// Pixels for one text, text shadow or image command, gathered before the frame is lowered. The
/// variants keep the same order as the commands that need them.
pub(crate) enum FrameSource {
    /// Coverage at device pixel (`x`, `y`) tinted by straight `color`. Text uses the Xft A8 mask;
    /// a text shadow uses its blurred alpha with the colour's alpha folded in and `color[3] == 255`.
    Mask {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        alpha: Vec<u8>,
        color: [u8; 4],
    },
    /// Premultiplied RGBA8 at device pixel (`x`, `y`), the image's XRender blit pixels.
    Image {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}

/// Whether the GPU frame may run: `renderAcceleration=gpu`, a scene with a backbuffer, and no
/// backdrop or clip the XRender path blends. Pure so the CPU gate is testable without X.
#[must_use]
pub(crate) fn lowering_allowed(
    gpu: bool,
    scene_ready: bool,
    backdrops: bool,
    clipped: bool,
) -> bool {
    gpu && scene_ready && !backdrops && !clipped
}

/// The first command kind the GPU frame cannot lower, or `None` when every kind qualifies. Checked
/// before any glyph or image work, so an unsupported frame costs nothing.
#[must_use]
pub(crate) fn unsupported_kind(commands: &[PaintCommand]) -> Option<&'static str> {
    commands.iter().find_map(|command| match command {
        PaintCommand::FillRect { .. }
        | PaintCommand::StrokeRect { .. }
        | PaintCommand::ScrollbarTrack { .. }
        | PaintCommand::Text { .. }
        | PaintCommand::TextShadow { .. }
        | PaintCommand::Image { .. } => None,
        PaintCommand::NodeBackdrop { .. } => Some("NodeBackdrop"),
        PaintCommand::PushClip { .. } => Some("PushClip"),
        PaintCommand::PopClip => Some("PopClip"),
        PaintCommand::ScrollbarThumb { .. } => Some("ScrollbarThumb"),
    })
}

/// Pixel ops for `commands` at device `scale`, taking the pixels of each text, text shadow and
/// image command from `sources` in command order. `None` when the frame must take the XRender path,
/// including when the sources and commands do not line up exactly.
pub(crate) fn frame_ops<'a>(
    commands: &[PaintCommand],
    scale: f32,
    sources: &'a [FrameSource],
) -> Option<Vec<PixelOp<'a>>> {
    let mut ops = Vec::with_capacity(commands.len());
    let mut sources = sources.iter();
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
            PaintCommand::Text { .. } | PaintCommand::TextShadow { .. } => {
                match sources.next()? {
                    FrameSource::Mask {
                        x,
                        y,
                        width,
                        height,
                        alpha,
                        color,
                    } => ops.push(PixelOp::BlendMask {
                        x: *x,
                        y: *y,
                        width: *width,
                        height: *height,
                        mask: alpha.as_slice(),
                        color: *color,
                    }),
                    FrameSource::Image { .. } => return None,
                }
            }
            PaintCommand::Image { .. } => match sources.next()? {
                FrameSource::Image {
                    x,
                    y,
                    width,
                    height,
                    rgba,
                } => ops.push(PixelOp::BlendImage {
                    x: *x,
                    y: *y,
                    width: *width,
                    height: *height,
                    rgba: rgba.as_slice(),
                }),
                FrameSource::Mask { .. } => return None,
            },
            _ => return None,
        }
    }
    // A leftover source means a command was skipped: refuse rather than paint a different frame.
    sources.next().is_none().then_some(ops)
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

    fn text() -> PaintCommand {
        PaintCommand::Text {
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
        }
    }

    fn mask_source() -> FrameSource {
        FrameSource::Mask {
            x: 3,
            y: 5,
            width: 2,
            height: 1,
            alpha: vec![255, 0],
            color: [9, 8, 7, 255],
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
        let ops = frame_ops(&[fill(10.0, 50.0, 6.0), stroke], 1.0, &[]).expect("supported frame");
        assert_eq!(ops.len(), 2);
        let PixelOp::FillRoundedRect { hole, .. } = ops[1] else {
            panic!("stroke lowers to a fill with a hole");
        };
        assert_eq!(hole.map(|hole| hole.width), Some(36));
    }

    #[test]
    fn text_and_images_lower_with_their_sources_in_command_order() {
        let image = PaintCommand::Image {
            rect: Rect {
                x: 1.0,
                y: 1.0,
                width: 2.0,
                height: 1.0,
            },
            asset: 0,
            node: 0,
            revision: 0,
            treatment: flamewm_render_core::ImageTreatment::Original,
            tint: None,
        };
        let sources = [
            mask_source(),
            FrameSource::Image {
                x: 1,
                y: 1,
                width: 2,
                height: 1,
                rgba: vec![0; 8],
            },
        ];
        let ops = frame_ops(&[text(), image], 1.0, &sources).expect("real frame shape");
        assert!(matches!(ops[0], PixelOp::BlendMask { x: 3, width: 2, .. }));
        assert!(matches!(ops[1], PixelOp::BlendImage { x: 1, width: 2, .. }));
    }

    #[test]
    fn a_text_command_without_its_source_keeps_the_xrender_path() {
        assert!(frame_ops(&[fill(0.0, 10.0, 0.0), text()], 1.0, &[]).is_none());
        let image = FrameSource::Image {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            rgba: vec![0; 4],
        };
        assert!(
            frame_ops(&[text()], 1.0, std::slice::from_ref(&image)).is_none(),
            "a mask slot cannot take an image"
        );
        assert!(
            frame_ops(&[], 1.0, &[mask_source()]).is_none(),
            "an unused source means the frame and the sources disagree"
        );
    }

    #[test]
    fn backdrops_clips_and_thumbs_keep_the_xrender_path() {
        assert_eq!(unsupported_kind(&[fill(0.0, 10.0, 0.0), text()]), None);
        assert_eq!(unsupported_kind(&[PaintCommand::PopClip]), Some("PopClip"));
        let thumb = PaintCommand::ScrollbarThumb {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 4.0,
                height: 4.0,
            },
            horizontal: false,
        };
        assert_eq!(unsupported_kind(&[thumb]), Some("ScrollbarThumb"));
        assert!(frame_ops(&[PaintCommand::PopClip], 1.0, &[]).is_none());
    }

    #[test]
    fn cpu_mode_never_lowers_a_frame() {
        assert!(!lowering_allowed(false, true, false, false));
        assert!(!lowering_allowed(true, false, false, false), "no scene");
        assert!(!lowering_allowed(true, true, true, false), "backdrop");
        assert!(!lowering_allowed(true, true, false, true), "clip");
        assert!(lowering_allowed(true, true, false, false));
    }

    #[test]
    fn fractional_device_geometry_keeps_the_xrender_path() {
        assert!(frame_ops(&[fill(10.5, 50.0, 6.0)], 1.0, &[]).is_none());
        assert!(
            frame_ops(&[fill(10.0, 51.0, 6.0)], 1.5, &[]).is_none(),
            "76.5 device px wide"
        );
        assert!(
            frame_ops(&[fill(10.0, 50.0, 6.0)], 1.5, &[]).is_some(),
            "whole at 1.5x"
        );
        assert!(frame_ops(&[fill(10.0, 50.0, 6.0)], 2.0, &[]).is_some());
    }
}

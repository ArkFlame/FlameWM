//! Lowers a whole frame to GPU pixel ops for `renderAcceleration=gpu`.
//!
//! A frame qualifies when every command is a rounded fill, a stroke, a scrollbar track or thumb, a
//! text run, a text shadow, an image, or a rectangular clip. Clips are resolved here exactly as the
//! XRender painter resolves them (`visible_under`): a fill or thumb cut by a clip becomes its visible
//! rectangle with the corner radius dropped, a hidden primitive is skipped, an image is drawn only
//! when wholly visible, and glyph masks are cropped to the Xft clip bounds. A stroke cut by a clip
//! has no exact pixel-op form yet, so that frame keeps the XRender path. A `NodeBackdrop` lowers to
//! a `BackdropBlur` op that blurs the pixels already painted under it on the GPU, in command order;
//! the surface snapshot starts the frame as a `BlendImage`. The two painters never mix.

use flamewm_render_core::{PaintCommand, PixelOp, Rect};

use super::clip::stack_bounds;

impl FrameSource {
    /// Borrowed view for the shared lowering, which does not copy frame pixels.
    fn borrow(&self) -> flamewm_render_core::lower::FrameSource<'_> {
        match self {
            Self::Mask {
                x,
                y,
                width,
                height,
                alpha,
                color,
            } => flamewm_render_core::lower::FrameSource::Mask {
                x: *x,
                y: *y,
                width: *width,
                height: *height,
                alpha,
                color: *color,
            },
            Self::Image {
                x,
                y,
                width,
                height,
                rgba,
            } => flamewm_render_core::lower::FrameSource::Image {
                x: *x,
                y: *y,
                width: *width,
                height: *height,
                rgba,
            },
        }
    }
}

/// Pixels for one text, text shadow or image command, gathered before the frame is lowered. The
/// variants keep the same order as the commands that need them.
pub(crate) enum FrameSource {
    /// Coverage at device pixel (`x`, `y`) tinted by straight `color`. Text uses the Xft A8 mask;
    /// a text shadow uses its blurred alpha with the colour's alpha folded in and `color[3] == 255`.
    /// A zero-sized mask paints nothing.
    Mask {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        alpha: Vec<u8>,
        color: [u8; 4],
    },
    /// Premultiplied RGBA8 at device pixel (`x`, `y`), the image's XRender blit pixels. A zero-sized
    /// image paints nothing.
    Image {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}

/// Counts one frame that took the XRender path for `reason`, under a static profiler label, so a
/// profile report shows every fallback reason per scene. Unknown reasons count as `other`.
pub(crate) fn count_fallback(reason: &str) {
    let label = match reason {
        "StrokeClipped" => "render.gpu_frame.fallback.StrokeClipped",
        "FractionalGeometry" => "render.gpu_frame.fallback.FractionalGeometry",
        "SourceMismatch" => "render.gpu_frame.fallback.SourceMismatch",
        "BackdropBytes" => "render.gpu_frame.fallback.BackdropBytes",
        "WindowBackdrop" => "render.gpu_frame.fallback.WindowBackdrop",
        "BackdropSize" => "render.gpu_frame.fallback.BackdropSize",
        "source-failed" => "render.gpu_frame.fallback.source-failed",
        "gpu-batch" => "render.gpu_frame.fallback.gpu-batch",
        _ => "render.gpu_frame.fallback.other",
    };
    flamewm_profiler::CounterPoint::new(label).increment();
}

/// Whether the GPU frame may run: `renderAcceleration=gpu` and a scene with a backbuffer. Pure so
/// the CPU gate is testable without X; backdrops are lowered or named by [`frame_ops`].
#[must_use]
pub(crate) fn lowering_allowed(gpu: bool, scene_ready: bool) -> bool {
    gpu && scene_ready
}

/// Pixel ops for `commands` at device `scale` on a `scene` sized frame, taking the pixels of each
/// text, text shadow and image command from `sources` in command order. `backdrop` is the retained
/// surface snapshot (width, height, premultiplied RGBA8), composited first as the XRender painter
/// does. `Err` names why the frame must take the XRender path, including when the sources and
/// commands do not line up exactly.
pub(crate) fn frame_ops<'a>(
    commands: &[PaintCommand],
    scale: f32,
    sources: &'a [FrameSource],
    scene: (u32, u32),
    backdrop: Option<(u32, u32, &'a [u8])>,
) -> Result<Vec<PixelOp<'a>>, &'static str> {
    let borrowed: Vec<flamewm_render_core::lower::FrameSource<'a>> =
        sources.iter().map(FrameSource::borrow).collect();
    flamewm_render_core::lower::lower_commands(commands, scale, &borrowed, scene, backdrop)
        .map_err(|error| error.as_str())
}

/// The integer rect the Xft draw clips glyphs to under `stack`, as the XRender text path rounds it.
/// `None` when no clip applies; `Some(None)` when the stack shares no area and nothing may draw.
#[must_use]
pub(crate) fn glyph_clip(stack: &[Rect]) -> Option<Option<(i32, i32, i32, i32)>> {
    if stack.is_empty() {
        return None;
    }
    Some(stack_bounds(stack).map(|b| {
        let x0 = b.x.round() as i32;
        let y0 = b.y.round() as i32;
        let x1 = x0 + b.width.round().max(0.0) as i32;
        let y1 = y0 + b.height.round().max(0.0) as i32;
        (x0, y0, x1, y1)
    }))
}

/// The device bounds `[min_x, max_x) x [min_y, max_y)` a text shadow keeps under `stack`, as the
/// XRender shadow blit cuts them (`floor` of each clip's origin, `ceil` of its far edge).
#[must_use]
pub(crate) fn shadow_clip(stack: &[Rect]) -> (i32, i32, i32, i32) {
    let (mut min_x, mut min_y) = (i32::MIN, i32::MIN);
    let (mut max_x, mut max_y) = (i32::MAX, i32::MAX);
    for clip in stack {
        min_x = min_x.max(clip.x.floor() as i32);
        min_y = min_y.max(clip.y.floor() as i32);
        max_x = max_x.min((clip.x + clip.width).ceil() as i32);
        max_y = max_y.min((clip.y + clip.height).ceil() as i32);
    }
    (min_x, min_y, max_x, max_y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_render_core::Color;

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

    fn clip(x: f32, y: f32, width: f32, height: f32) -> PaintCommand {
        PaintCommand::PushClip {
            rect: Rect {
                x,
                y,
                width,
                height,
            },
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
        let ops = frame_ops(&[fill(10.0, 50.0, 6.0), stroke], 1.0, &[], SCENE, None)
            .expect("supported frame");
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
        let ops =
            frame_ops(&[text(), image], 1.0, &sources, SCENE, None).expect("real frame shape");
        assert!(matches!(ops[0], PixelOp::BlendMask { x: 3, width: 2, .. }));
        assert!(matches!(ops[1], PixelOp::BlendImage { x: 1, width: 2, .. }));
    }

    #[test]
    fn a_text_command_without_its_source_keeps_the_xrender_path() {
        assert!(frame_ops(&[fill(0.0, 10.0, 0.0), text()], 1.0, &[], SCENE, None).is_err());
        let image = FrameSource::Image {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            rgba: vec![0; 4],
        };
        assert!(
            frame_ops(&[text()], 1.0, std::slice::from_ref(&image), SCENE, None).is_err(),
            "a mask slot cannot take an image"
        );
        assert!(
            frame_ops(&[], 1.0, &[mask_source()], SCENE, None).is_err(),
            "an unused source means the frame and the sources disagree"
        );
    }

    const SCENE: (u32, u32) = (1000, 1000);

    fn node(x: f32, y: f32, size: f32, radius: f32, blur_percent: u8, opacity: u8) -> PaintCommand {
        PaintCommand::NodeBackdrop {
            rect: Rect {
                x,
                y,
                width: size,
                height: size,
            },
            radius,
            blur_percent,
            opacity,
            node: 1,
        }
    }

    #[test]
    fn node_backdrop_lowers_to_a_blur_of_its_clip_at_device_geometry() {
        let ops = frame_ops(&[node(0.0, 0.0, 10.0, 4.0, 50, 128)], 1.0, &[], SCENE, None)
            .expect("unclipped node backdrop lowers");
        assert!(matches!(
            ops.as_slice(),
            [PixelOp::BackdropBlur {
                clip: (0, 0, 10, 10),
                node: (0, 0, 10, 10),
                corner: 4,
                radius,
            }] if *radius == 12.0
        ));
        let ops = frame_ops(
            &[node(10.0, 10.0, 10.0, 4.0, 50, 128)],
            1.5,
            &[],
            SCENE,
            None,
        )
        .expect("scaled node backdrop lowers");
        assert!(matches!(
            ops.as_slice(),
            [PixelOp::BackdropBlur {
                clip: (15, 15, 15, 15),
                node: (15, 15, 15, 15),
                corner: 6,
                ..
            }]
        ));
    }

    #[test]
    fn node_backdrop_keeps_command_order_around_fills() {
        let commands = [
            fill(0.0, 10.0, 0.0),
            node(0.0, 0.0, 10.0, 4.0, 50, 128),
            fill(20.0, 10.0, 0.0),
        ];
        let ops = frame_ops(&commands, 1.0, &[], SCENE, None).expect("mixed frame lowers");
        assert!(matches!(
            ops.as_slice(),
            [
                PixelOp::FillRoundedRect { .. },
                PixelOp::BackdropBlur { .. },
                PixelOp::FillRoundedRect { .. }
            ]
        ));
    }

    #[test]
    fn node_backdrop_that_paints_nothing_lowers_to_no_op() {
        let no_blur = [node(0.0, 0.0, 10.0, 4.0, 0, 128)];
        let no_opacity = [node(0.0, 0.0, 10.0, 4.0, 50, 0)];
        let off_scene = [node(2000.0, 0.0, 10.0, 4.0, 50, 128)];
        for commands in [&no_blur[..], &no_opacity[..], &off_scene[..]] {
            assert!(
                frame_ops(commands, 1.0, &[], SCENE, None)
                    .expect("lowers")
                    .is_empty()
            );
        }
    }

    #[test]
    fn node_backdrop_ignores_the_clip_stack_as_the_xrender_blit_does() {
        let cut = [
            clip(0.0, 0.0, 50.0, 50.0),
            node(0.0, 0.0, 10.0, 4.0, 50, 128),
        ];
        let ops = frame_ops(&cut, 1.0, &[], SCENE, None).expect("lowers under a clip");
        assert!(matches!(ops.as_slice(), [PixelOp::BackdropBlur { .. }]));
    }

    #[test]
    fn surface_snapshot_composites_first_and_must_match_the_scene() {
        let rgba = [1_u8, 2, 3, 255, 4, 5, 6, 255];
        let ops = frame_ops(&[], 1.0, &[], (2, 1), Some((2, 1, &rgba))).expect("snapshot lowers");
        assert!(matches!(
            ops.as_slice(),
            [PixelOp::BlendImage { x: 0, y: 0, width: 2, height: 1, rgba: r }] if *r == rgba
        ));
        assert_eq!(
            frame_ops(&[], 1.0, &[], (2, 2), Some((2, 1, &rgba))).err(),
            Some("BackdropSize")
        );
    }

    #[test]
    fn clipped_fills_and_thumbs_cut_to_the_visible_part_with_a_square_corner() {
        // Half of a rounded fill sits below a clip ending at y=12: it becomes the visible rect.
        let commands = [clip(0.0, 0.0, 100.0, 12.0), fill(10.0, 50.0, 6.0)];
        let ops = frame_ops(&commands, 1.0, &[], SCENE, None).expect("clipped fill lowers");
        let PixelOp::FillRoundedRect { outer, .. } = ops[0] else {
            panic!("fill lowers");
        };
        assert_eq!(
            (outer.x, outer.y, outer.width, outer.height),
            (10, 4, 50, 8)
        );
        assert_eq!(outer.radius, 0, "a cut fill drops its corner radius");

        // Entirely outside the clip: nothing is drawn, the frame still lowers.
        let hidden = [clip(0.0, 0.0, 5.0, 5.0), fill(10.0, 50.0, 6.0)];
        assert_eq!(
            frame_ops(&hidden, 1.0, &[], SCENE, None)
                .expect("hidden fill")
                .len(),
            0
        );
    }

    #[test]
    fn a_stroke_cut_by_a_clip_lowers_to_a_ring_clipped_to_the_visible_pixels() {
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
        let ops = frame_ops(
            &[clip(0.5, 0.0, 20.0, 20.5), stroke.clone()],
            1.0,
            &[],
            SCENE,
            None,
        )
        .expect("cut stroke lowers");
        // Columns from ceil(0.5) = 1 to floor(20.5) = 20, rows from 0 to floor(20.5) = 20.
        assert!(matches!(
            ops.as_slice(),
            [PixelOp::FillRoundedRectClipped {
                clip: (1, 0, 19, 20),
                ..
            }]
        ));
        assert_eq!(
            frame_ops(&[clip(0.0, 0.0, 0.5, 0.5), stroke], 1.0, &[], SCENE, None)
                .expect("no whole pixel left")
                .len(),
            0,
            "a clip with no whole pixel hides the stroke"
        );
    }

    #[test]
    fn glyph_clip_rounds_the_stack_bounds_like_the_text_path() {
        assert_eq!(glyph_clip(&[]), None);
        assert_eq!(
            glyph_clip(&[Rect {
                x: 1.4,
                y: 2.6,
                width: 10.2,
                height: 5.0,
            }]),
            Some(Some((1, 3, 11, 8)))
        );
        let apart = [
            Rect {
                x: 0.0,
                y: 0.0,
                width: 5.0,
                height: 5.0,
            },
            Rect {
                x: 10.0,
                y: 0.0,
                width: 5.0,
                height: 5.0,
            },
        ];
        assert_eq!(glyph_clip(&apart), Some(None));
    }

    #[test]
    fn cpu_mode_never_lowers_a_frame() {
        assert!(!lowering_allowed(false, true));
        assert!(!lowering_allowed(true, false), "no scene");
        assert!(lowering_allowed(true, true));
    }

    #[test]
    fn fractional_device_geometry_keeps_the_xrender_path() {
        assert!(frame_ops(&[fill(10.5, 50.0, 6.0)], 1.0, &[], SCENE, None).is_err());
        assert!(
            frame_ops(&[fill(10.0, 51.0, 6.0)], 1.5, &[], SCENE, None).is_err(),
            "76.5 device px wide"
        );
        assert!(
            frame_ops(&[fill(10.0, 50.0, 6.0)], 1.5, &[], SCENE, None).is_ok(),
            "whole at 1.5x"
        );
        assert!(frame_ops(&[fill(10.0, 50.0, 6.0)], 2.0, &[], SCENE, None).is_ok());
    }

    /// The launch badge icon as the frame sees it: alpha 0 with RGB 000000, a half-alpha edge
    /// (straight 200,50,50) and an opaque centre, premultiplied as `scale_and_premultiply` leaves
    /// them. Composited over the surface snapshot, the badge must show the backdrop where alpha
    /// is 0, blend the edge toward the backdrop, and never print black.
    #[test]
    fn launch_icon_edges_blend_over_the_backdrop_and_never_print_black() {
        let icon = FrameSource::Image {
            x: 0,
            y: 0,
            width: 3,
            height: 1,
            rgba: vec![0, 0, 0, 0, 100, 25, 25, 128, 255, 255, 255, 255],
        };
        let backdrop = [30_u8, 60, 90, 255].repeat(3);
        let command = PaintCommand::Image {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 3.0,
                height: 1.0,
            },
            asset: 0,
            node: 0,
            revision: 0,
            treatment: flamewm_render_core::ImageTreatment::Original,
            tint: None,
        };
        let commands = [command];
        let sources = [icon];
        let ops = frame_ops(&commands, 1.0, &sources, (3, 1), Some((3, 1, &backdrop)))
            .expect("badge frame lowers");
        let mut scene = vec![0_u8; 12];
        crate::accel::paint_batch(&mut scene, 3, 1, &ops).expect("badge composites");
        assert_eq!(&scene[0..4], &[30, 60, 90, 255], "alpha 0 keeps the backdrop");
        assert_eq!(scene[7], 255, "the edge stays opaque over the backdrop");
        for (got, want) in scene[4..7].iter().zip([115_u8, 55, 70]) {
            assert!(got.abs_diff(want) <= 2, "edge {got} != blend {want}");
        }
        assert_eq!(&scene[8..12], &[255, 255, 255, 255], "opaque centre");
    }
}

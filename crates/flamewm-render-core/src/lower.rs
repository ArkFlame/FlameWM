//! Backend-neutral lowering of `PaintCommand`s into `PixelOp`s.
//!
//! This started as the GPU fast path inside `flamewm-render-x11`, where it existed only to feed
//! that backend's GL batch. Nothing in the lowering touches X11: it is clip-stack math over
//! backend-neutral rects. It lives here so any renderer, including a Wayland shared-memory
//! presenter, can paint the same frame from the same commands.

use crate::{Color, PaintCommand, PixelOp, Rect, RoundedRect};

/// Why a command list could not be lowered to whole pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LowerError {
    /// A rect or radius does not land on whole device pixels.
    Fractional,
    /// A text or image command had no prepared source, or the source order drifted.
    SourceMismatch,
    /// The backdrop is not the size of the scene it paints.
    BackdropSize,
}

impl LowerError {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fractional => "FractionalGeometry",
            Self::SourceMismatch => "SourceMismatch",
            Self::BackdropSize => "BackdropSize",
        }
    }
}

/// One prepared pixel source for a text or image command, in command order.
#[derive(Clone, Copy, Debug)]
pub enum FrameSource<'a> {
    /// An 8-bit coverage mask at device pixel (`x`, `y`), tinted by `color`.
    Mask {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        alpha: &'a [u8],
        color: [u8; 4],
    },
    /// Premultiplied RGBA8 pixels at device pixel (`x`, `y`).
    Image {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        rgba: &'a [u8],
    },
}

/// Largest blur radius in device pixels for a 100% blur percent.
pub const MAX_BLUR_RADIUS_PX: f32 = 24.0;

/// Radius for a blur percent. Blur only applies below 100% opacity, so 100% never blurs.
#[must_use]
pub fn blur_radius_px(opacity_percent: u8, blur_percent: u8) -> f32 {
    if opacity_percent >= 100 {
        return 0.0;
    }
    f32::from(blur_percent.min(100)) / 100.0 * MAX_BLUR_RADIUS_PX
}

/// Visible part of one primitive under the active clip stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Visible {
    /// Entirely inside every clip: draw unchanged.
    Whole,
    /// Entirely outside at least one clip: draw nothing.
    Hidden,
    /// Cut by a clip: draw only this device rect.
    Part(Rect),
}

/// Sub-pixel slack for edge equality: a child that fills its clip exactly can differ by float
/// rounding after device scaling. Treating that as a partial cut would drop its corner radius.
const EDGE_EPSILON: f32 = 0.01;

pub fn same_rect(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() <= EDGE_EPSILON
        && (a.y - b.y).abs() <= EDGE_EPSILON
        && (a.width - b.width).abs() <= EDGE_EPSILON
        && (a.height - b.height).abs() <= EDGE_EPSILON
}

/// Overlap of two rects, or `None` when they do not share area.
pub fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let x1 = a.x.max(b.x);
    let y1 = a.y.max(b.y);
    let x2 = (a.x + a.width).min(b.x + b.width);
    let y2 = (a.y + a.height).min(b.y + b.height);
    (x2 > x1 && y2 > y1).then_some(Rect {
        x: x1,
        y: y1,
        width: x2 - x1,
        height: y2 - y1,
    })
}

/// Intersection of every clip on the stack.
pub fn stack_bounds(stack: &[Rect]) -> Option<Rect> {
    let mut iter = stack.iter().copied();
    let first = iter.next()?;
    iter.try_fold(first, intersect)
}

/// Classifies `rect` against the clip stack. An empty stack leaves it whole.
pub fn visible_under(rect: Rect, stack: &[Rect]) -> Visible {
    if stack.is_empty() {
        return Visible::Whole;
    }
    let Some(bounds) = stack_bounds(stack) else {
        return Visible::Hidden;
    };
    match intersect(rect, bounds) {
        None => Visible::Hidden,
        Some(part) if same_rect(part, rect) => Visible::Whole,
        Some(part) => Visible::Part(part),
    }
}

/// Scales a logical rect into device space.
#[must_use]
pub fn scale_rect(rect: Rect, scale: f32) -> Rect {
    Rect {
        x: rect.x * scale,
        y: rect.y * scale,
        width: rect.width * scale,
        height: rect.height * scale,
    }
}

fn whole(value: f32) -> Option<u32> {
    (value >= 0.0 && value.fract() == 0.0 && value <= u32::MAX as f32).then_some(value as u32)
}

fn device_rect(rect: Rect, radius: f32) -> Option<RoundedRect> {
    // `whole` admits up to u32::MAX, which does not fit the i32 device origin: refuse it rather
    // than let `as i32` wrap the origin to a negative coordinate.
    let outer = RoundedRect {
        x: i32::try_from(whole(rect.x)?).ok()?,
        y: i32::try_from(whole(rect.y)?).ok()?,
        width: whole(rect.width)?,
        height: whole(rect.height)?,
        radius: whole(radius)?,
    };
    (outer.width > 0 && outer.height > 0).then(|| outer.normalized())
}

fn rgba(color: Color) -> [u8; 4] {
    [color.r, color.g, color.b, color.a]
}

/// The rect and radius a fill paints under `stack`: `None` when hidden, unchanged when whole,
/// and the visible part with a square corner when cut.
#[must_use]
pub fn visible_fill(rect: Rect, radius: f32, stack: &[Rect]) -> Option<(Rect, f32)> {
    match visible_under(rect, stack) {
        Visible::Hidden => None,
        Visible::Whole => Some((rect, radius)),
        Visible::Part(part) => Some((part, 0.0)),
    }
}

fn stroke_op(
    rect: Rect,
    radius: f32,
    width: f32,
    color: Color,
    scale: f32,
    stack: &[Rect],
) -> Result<Option<PixelOp<'static>>, LowerError> {
    let device = scale_rect(rect, scale);
    let visible = match visible_under(device, stack) {
        Visible::Hidden => return Ok(None),
        Visible::Whole => None,
        Visible::Part(part) => Some(part),
    };
    let outer = device_rect(device, radius * scale).ok_or(LowerError::Fractional)?;
    let thickness = whole(width * scale).ok_or(LowerError::Fractional)?;
    let (outer, hole, color) = (outer, Some(outer.inset(thickness)), rgba(color));
    let Some(part) = visible else {
        return Ok(Some(PixelOp::FillRoundedRect { outer, hole, color }));
    };
    // A clipped ring keeps only whole pixels inside `part`, exactly as the X11 painter cuts it:
    // from the first pixel at or after its left/top edge to the last one before its right/bottom.
    let (left, top) = (part.x.ceil() as i32, part.y.ceil() as i32);
    let right = (part.x + part.width).floor() as i32;
    let bottom = (part.y + part.height).floor() as i32;
    if right <= left || bottom <= top {
        return Ok(None);
    }
    Ok(Some(PixelOp::FillRoundedRectClipped {
        outer,
        hole,
        color,
        clip: (left, top, (right - left) as u32, (bottom - top) as u32),
    }))
}

/// Device rect of a node backdrop, clipped to the scene.
pub fn node_backdrop_clip(
    rect: (i32, i32, u32, u32),
    scene: (u32, u32),
) -> Option<(i32, i32, u32, u32)> {
    let (x, y, width, height) = rect;
    let left = i64::from(x).max(0);
    let top = i64::from(y).max(0);
    let right = (i64::from(x) + i64::from(width)).min(i64::from(scene.0));
    let bottom = (i64::from(y) + i64::from(height)).min(i64::from(scene.1));
    if right <= left || bottom <= top {
        return None;
    }
    Some((
        left as i32,
        top as i32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

fn node_device_rect(rect: Rect, scale: f32) -> (i32, i32, u32, u32) {
    let rect = scale_rect(rect, scale);
    (
        rect.x.round() as i32,
        rect.y.round() as i32,
        rect.width.round().max(1.0) as u32,
        rect.height.round().max(1.0) as u32,
    )
}

/// Lower `commands` into normalized ops for a `scene` of (`width`, `height`) device pixels.
///
/// `sources` must hold one entry per `Text`, `TextShadow` and `Image` command, in order; the
/// lowering refuses a list whose source order drifts rather than paint a different frame.
///
/// # Errors
/// Returns [`LowerError`] when a rect is fractional or a source is missing or out of order.
pub fn lower_commands<'a>(
    commands: &[PaintCommand],
    scale: f32,
    sources: &[FrameSource<'a>],
    scene: (u32, u32),
    backdrop: Option<(u32, u32, &'a [u8])>,
) -> Result<Vec<PixelOp<'a>>, LowerError> {
    let mut ops = Vec::with_capacity(commands.len() + 1);
    if let Some((width, height, rgba)) = backdrop {
        if (width, height) != scene {
            return Err(LowerError::BackdropSize);
        }
        ops.push(PixelOp::BlendImage {
            x: 0,
            y: 0,
            width,
            height,
            rgba,
        });
    }
    let mut stack: Vec<Rect> = Vec::new();
    let mut sources = sources.iter().copied();
    for command in commands {
        match *command {
            PaintCommand::PushClip { rect } => stack.push(scale_rect(rect, scale)),
            PaintCommand::PopClip => {
                stack.pop();
            }
            PaintCommand::FillRect {
                rect,
                color,
                radius,
            } => {
                let Some((rect, radius)) =
                    visible_fill(scale_rect(rect, scale), radius * scale, &stack)
                else {
                    continue;
                };
                ops.push(PixelOp::FillRoundedRect {
                    outer: device_rect(rect, radius).ok_or(LowerError::Fractional)?,
                    hole: None,
                    color: rgba(color),
                });
            }
            PaintCommand::StrokeRect {
                rect,
                color,
                width,
                radius,
            } => {
                if let Some(op) = stroke_op(rect, radius, width, color, scale, &stack)? {
                    ops.push(op);
                }
            }
            // The XRender painter draws nothing for a track, so this paints nothing either.
            PaintCommand::ScrollbarTrack { .. } => {}
            PaintCommand::ScrollbarThumb { rect, horizontal } => {
                let mut thumb = rect;
                if horizontal {
                    thumb.y += thumb.height - 8.0;
                    thumb.height = 6.0;
                } else {
                    thumb.x += thumb.width - 8.0;
                    thumb.width = 6.0;
                }
                let Some((rect, radius)) =
                    visible_fill(scale_rect(thumb, scale), 3.0 * scale, &stack)
                else {
                    continue;
                };
                ops.push(PixelOp::FillRoundedRect {
                    outer: device_rect(rect, radius).ok_or(LowerError::Fractional)?,
                    hole: None,
                    color: [255, 255, 255, 0x48],
                });
            }
            PaintCommand::Text { .. } | PaintCommand::TextShadow { .. } => {
                match sources.next().ok_or(LowerError::SourceMismatch)? {
                    FrameSource::Mask {
                        x,
                        y,
                        width,
                        height,
                        alpha,
                        color,
                    } => ops.push(PixelOp::BlendMask {
                        x,
                        y,
                        width,
                        height,
                        mask: alpha,
                        color,
                    }),
                    FrameSource::Image { .. } => return Err(LowerError::SourceMismatch),
                }
            }
            PaintCommand::Image { .. } => match sources.next().ok_or(LowerError::SourceMismatch)? {
                FrameSource::Image {
                    x,
                    y,
                    width,
                    height,
                    rgba,
                } => ops.push(PixelOp::BlendImage {
                    x,
                    y,
                    width,
                    height,
                    rgba,
                }),
                FrameSource::Mask { .. } => return Err(LowerError::SourceMismatch),
            },
            PaintCommand::NodeBackdrop {
                rect,
                radius,
                blur_percent,
                opacity,
                ..
            } => {
                if blur_percent == 0 || opacity == 0 {
                    continue;
                }
                let device = node_device_rect(rect, scale);
                let Some(clip) = node_backdrop_clip(device, scene) else {
                    continue;
                };
                ops.push(PixelOp::BackdropBlur {
                    clip,
                    node: device,
                    corner: (radius * scale).round().max(0.0) as u32,
                    radius: blur_radius_px(0, blur_percent),
                });
            }
        }
    }
    if sources.next().is_some() {
        return Err(LowerError::SourceMismatch);
    }
    Ok(ops)
}

/// Snaps every rect, radius and stroke width in `commands` to whole device pixels.
///
/// The X11 painter resolves fractional geometry directly; a software presenter rasterizes whole
/// pixels, so it snaps first instead of refusing the frame. Sizes keep at least one pixel so a
/// hairline stays visible.
#[must_use]
pub fn snap_commands(commands: &[PaintCommand], scale: f32) -> Vec<PaintCommand> {
    fn snap(value: f32, scale: f32) -> f32 {
        (value * scale).round() / scale
    }
    fn snap_rect(rect: Rect, scale: f32) -> Rect {
        let x = snap(rect.x, scale);
        let y = snap(rect.y, scale);
        // Snap the far edge, not the width, so neighbours meet instead of drifting apart.
        let right = snap(rect.x + rect.width, scale);
        let bottom = snap(rect.y + rect.height, scale);
        Rect {
            x,
            y,
            width: (right - x).max(1.0 / scale),
            height: (bottom - y).max(1.0 / scale),
        }
    }
    commands
        .iter()
        .map(|command| match *command {
            PaintCommand::FillRect {
                rect,
                color,
                radius,
            } => PaintCommand::FillRect {
                rect: snap_rect(rect, scale),
                color,
                radius: snap(radius, scale),
            },
            PaintCommand::StrokeRect {
                rect,
                color,
                width,
                radius,
            } => PaintCommand::StrokeRect {
                rect: snap_rect(rect, scale),
                color,
                width: snap(width, scale).max(1.0 / scale),
                radius: snap(radius, scale),
            },
            PaintCommand::ScrollbarThumb { rect, horizontal } => PaintCommand::ScrollbarThumb {
                rect: snap_rect(rect, scale),
                horizontal,
            },
            PaintCommand::ScrollbarTrack { rect, horizontal } => PaintCommand::ScrollbarTrack {
                rect: snap_rect(rect, scale),
                horizontal,
            },
            PaintCommand::NodeBackdrop {
                rect,
                radius,
                blur_percent,
                opacity,
                node,
            } => PaintCommand::NodeBackdrop {
                rect: snap_rect(rect, scale),
                radius: snap(radius, scale),
                blur_percent,
                opacity,
                node,
            },
            PaintCommand::Text {
                x,
                y,
                color,
                size,
                weight,
                ref text,
            } => PaintCommand::Text {
                x: snap(x, scale),
                y: snap(y, scale),
                color,
                size,
                weight,
                text: text.clone(),
            },
            PaintCommand::TextShadow {
                x,
                y,
                color,
                size,
                weight,
                blur_radius,
                ref text,
            } => PaintCommand::TextShadow {
                x: snap(x, scale),
                y: snap(y, scale),
                color,
                size,
                weight,
                blur_radius,
                text: text.clone(),
            },
            PaintCommand::Image {
                rect,
                asset,
                node,
                revision,
                treatment,
                tint,
            } => PaintCommand::Image {
                rect: snap_rect(rect, scale),
                asset,
                node,
                revision,
                treatment,
                tint,
            },
            PaintCommand::PushClip { rect } => PaintCommand::PushClip {
                rect: snap_rect(rect, scale),
            },
            PaintCommand::PopClip => PaintCommand::PopClip,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn color(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r, g, b, a }
    }

    #[test]
    fn a_fill_becomes_one_rounded_rect() {
        let commands = [PaintCommand::FillRect {
            rect: rect(1.0, 2.0, 3.0, 4.0),
            color: color(1, 2, 3, 255),
            radius: 2.0,
        }];
        let ops = lower_commands(&commands, 1.0, &[], (10, 10), None).expect("lower");
        assert_eq!(ops.len(), 1);
        match ops[0] {
            PixelOp::FillRoundedRect {
                outer,
                hole,
                color: got,
            } => {
                assert_eq!((outer.x, outer.y, outer.width, outer.height), (1, 2, 3, 4));
                assert_eq!(hole, None);
                assert_eq!(got, [1, 2, 3, 255]);
            }
            _ => panic!("expected a fill"),
        }
    }

    #[test]
    fn a_device_origin_past_i32_is_refused_not_wrapped() {
        let commands = [PaintCommand::FillRect {
            rect: rect(3_000_000_000.0, 0.0, 10.0, 10.0),
            color: color(1, 2, 3, 255),
            radius: 0.0,
        }];
        assert_eq!(
            lower_commands(&commands, 1.0, &[], (10, 10), None).unwrap_err(),
            LowerError::Fractional
        );
    }

    #[test]
    fn a_clip_cuts_a_fill_and_square_its_corners() {
        let commands = [
            PaintCommand::PushClip {
                rect: rect(0.0, 0.0, 5.0, 10.0),
            },
            PaintCommand::FillRect {
                rect: rect(0.0, 0.0, 10.0, 10.0),
                color: color(9, 9, 9, 255),
                radius: 4.0,
            },
        ];
        let ops = lower_commands(&commands, 1.0, &[], (10, 10), None).expect("lower");
        match ops[0] {
            PixelOp::FillRoundedRect { outer, .. } => {
                assert_eq!(outer.width, 5, "clipped to the push-clip width");
                assert_eq!(outer.radius, 0, "a cut fill loses its radius");
            }
            _ => panic!("expected a fill"),
        }
    }

    #[test]
    fn a_hidden_fill_produces_no_op() {
        let commands = [
            PaintCommand::PushClip {
                rect: rect(0.0, 0.0, 2.0, 2.0),
            },
            PaintCommand::FillRect {
                rect: rect(50.0, 50.0, 4.0, 4.0),
                color: color(1, 1, 1, 255),
                radius: 0.0,
            },
        ];
        let ops = lower_commands(&commands, 1.0, &[], (100, 100), None).expect("lower");
        assert!(ops.is_empty(), "a fully clipped fill paints nothing");
    }

    #[test]
    fn a_missing_source_is_refused() {
        let commands = [PaintCommand::Text {
            x: 0.0,
            y: 0.0,
            color: color(255, 255, 255, 255),
            size: 12.0,
            weight: 400,
            text: "hi".into(),
        }];
        assert_eq!(
            lower_commands(&commands, 1.0, &[], (10, 10), None).unwrap_err(),
            LowerError::SourceMismatch
        );
    }

    #[test]
    fn an_unused_source_is_refused() {
        let mask = FrameSource::Mask {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            alpha: &[255],
            color: [255, 255, 255, 255],
        };
        assert_eq!(
            lower_commands(&[], 1.0, &[mask], (1, 1), None).unwrap_err(),
            LowerError::SourceMismatch
        );
    }

    #[test]
    fn a_text_source_becomes_a_tinted_mask() {
        let commands = [PaintCommand::Text {
            x: 0.0,
            y: 0.0,
            color: color(10, 20, 30, 40),
            size: 12.0,
            weight: 400,
            text: "hi".into(),
        }];
        let alpha = [128u8];
        let mask = FrameSource::Mask {
            x: 2,
            y: 3,
            width: 1,
            height: 1,
            alpha: &alpha,
            color: [10, 20, 30, 40],
        };
        let sources = [mask];
        let ops = lower_commands(&commands, 1.0, &sources, (10, 10), None).expect("lower");
        match ops[0] {
            PixelOp::BlendMask {
                x, y, mask, color, ..
            } => {
                assert_eq!((x, y), (2, 3));
                assert_eq!(mask, &[128]);
                assert_eq!(color, [10, 20, 30, 40]);
            }
            _ => panic!("expected a mask blend"),
        }
    }

    #[test]
    fn a_backdrop_precedes_the_commands() {
        let backdrop = [0u8; 4];
        let commands = [PaintCommand::FillRect {
            rect: rect(0.0, 0.0, 1.0, 1.0),
            color: color(1, 1, 1, 255),
            radius: 0.0,
        }];
        let ops =
            lower_commands(&commands, 1.0, &[], (1, 1), Some((1, 1, &backdrop))).expect("lower");
        assert!(matches!(ops[0], PixelOp::BlendImage { .. }));
        assert_eq!(ops.len(), 2);
    }

    #[test]
    fn snapping_makes_a_fractional_frame_lower() {
        // A real UI document produces fractional rects; the X11 painter resolves them, and a
        // software presenter snaps instead of refusing the frame.
        let commands = [PaintCommand::FillRect {
            rect: rect(494.83594, 463.9087, 251.98288, 31.04137),
            color: color(1, 1, 1, 255),
            radius: 999.0,
        }];
        assert_eq!(
            lower_commands(&commands, 1.0, &[], (1280, 800), None).unwrap_err(),
            LowerError::Fractional,
            "the raw frame is fractional"
        );
        let snapped = snap_commands(&commands, 1.0);
        let ops =
            lower_commands(&snapped, 1.0, &[], (1280, 800), None).expect("a snapped frame lowers");
        match ops[0] {
            PixelOp::FillRoundedRect { outer, .. } => {
                assert_eq!((outer.x, outer.y), (495, 464));
                assert_eq!((outer.width, outer.height), (252, 31));
            }
            _ => panic!("expected a fill"),
        }
    }

    #[test]
    fn snapping_keeps_a_hairline_visible() {
        let commands = [PaintCommand::StrokeRect {
            rect: rect(10.0, 10.0, 40.0, 20.0),
            color: color(1, 1, 1, 255),
            width: 0.3,
            radius: 0.0,
        }];
        let snapped = snap_commands(&commands, 1.0);
        let ops = lower_commands(&snapped, 1.0, &[], (100, 100), None).expect("lowers");
        assert_eq!(ops.len(), 1, "a hairline must still paint");
    }

    #[test]
    fn a_mismatched_backdrop_is_refused() {
        let backdrop = [0u8; 4];
        assert_eq!(
            lower_commands(&[], 1.0, &[], (2, 2), Some((1, 1, &backdrop))).unwrap_err(),
            LowerError::BackdropSize
        );
    }
}

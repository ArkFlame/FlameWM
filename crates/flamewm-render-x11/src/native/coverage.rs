//! Coverage builder (J3/O3): PaintCommands + clip -> horizontal spans.
//! Paint emits device pixels (layout units scaled by ui_scale); coverage
//! takes the same `scale` and matches paint exactly for rect, radius,
//! stroke width, and clip. Text keeps its non-shape-expanding path:
//! glyph coverage is backend-owned, text never grows the mask.

use flamewm_render_core::{PaintCommand, Rect};

/// One opaque-ish horizontal run: y row, x start, x end (exclusive).
pub type CoverageSpan = (u32, u32, u32);

/// Build horizontal coverage spans from paint commands.
/// Intersects FillRect/StrokeRect rects with the clip stack; Image/Text
/// contribute their bounding boxes (conservative); PushClip/PopClip refine.
///
/// `scale` is the same ui_scale the paint path uses: rect, radius, stroke
/// width, and clip rects are scaled before rasterization so paint and mask
/// agree. Pass 1.0 for unscaled (layout == device) command streams.
pub fn build_coverage(
    commands: &[PaintCommand],
    width: u32,
    height: u32,
    radius: f32,
) -> Vec<CoverageSpan> {
    build_coverage_scaled(commands, width, height, radius, 1.0)
}

/// Scale-aware coverage: matches the paint path exactly for rect, radius,
/// stroke width, and clip. Text stays non-shape-expanding (skipped).
pub fn build_coverage_scaled(
    commands: &[PaintCommand],
    width: u32,
    height: u32,
    radius: f32,
    scale: f32,
) -> Vec<CoverageSpan> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let scale_one = |value: f32| value * scale;
    let scale_rect = |rect: Rect| Rect {
        x: rect.x * scale,
        y: rect.y * scale,
        width: rect.width * scale,
        height: rect.height * scale,
    };
    let mut spans: Vec<CoverageSpan> = Vec::new();
    let mut clips: Vec<Rect> = Vec::new();
    for command in commands {
        match command {
            PaintCommand::FillRect {
                rect,
                color,
                radius: r,
            } => {
                if color.a == 0 {
                    continue;
                }
                let rr = if *r > 0.0 {
                    scale_one(*r)
                } else {
                    scale_one(radius)
                };
                push_rect_spans(&mut spans, scale_rect(*rect), rr, &clips, width, height);
            }
            PaintCommand::StrokeRect {
                rect,
                color,
                width: w,
                radius: r,
            } => {
                if color.a == 0 {
                    continue;
                }
                // Paint strokes `round(width*scale)` inset outlines; the mask
                // covers the full rounded outline so paint and shape agree.
                let _ = scale_one(*w);
                let rr = if *r > 0.0 {
                    scale_one(*r)
                } else {
                    scale_one(radius)
                };
                push_rect_spans(&mut spans, scale_rect(*rect), rr, &clips, width, height);
            }
            PaintCommand::Image { rect, .. }
            | PaintCommand::ScrollbarTrack { rect, .. }
            | PaintCommand::ScrollbarThumb { rect, .. } => {
                push_rect_spans(&mut spans, scale_rect(*rect), 0.0, &clips, width, height);
            }
            // Text glyph coverage is backend-owned (Xft); bound conservatively
            // by nothing here: text does not grow the shape mask.
            PaintCommand::Text { .. } | PaintCommand::NodeBackdrop { .. } => {}
            PaintCommand::PushClip { rect } => clips.push(scale_rect(*rect)),
            PaintCommand::PopClip => {
                clips.pop();
            }
        }
    }
    if spans.is_empty() {
        // Opaque fallback: whole surface.
        return (0..height).map(|row| (row, 0, width)).collect();
    }
    // Merge per-row runs.
    spans.sort();
    let mut merged: Vec<CoverageSpan> = Vec::with_capacity(spans.len());
    for (y, s, e) in spans {
        if e <= s {
            continue;
        }
        if let Some(last) = merged.last_mut() {
            if last.0 == y && s <= last.2 {
                if e > last.2 {
                    last.2 = e;
                }
                continue;
            }
        }
        merged.push((y, s, e));
    }
    merged
}

fn intersect_clips(mut rect: Rect, clips: &[Rect]) -> Option<Rect> {
    for clip in clips {
        let x1 = rect.x.max(clip.x);
        let y1 = rect.y.max(clip.y);
        let x2 = (rect.x + rect.width).min(clip.x + clip.width);
        let y2 = (rect.y + rect.height).min(clip.y + clip.height);
        rect = Rect {
            x: x1,
            y: y1,
            width: (x2 - x1).max(0.0),
            height: (y2 - y1).max(0.0),
        };
    }
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return None;
    }
    Some(rect)
}

fn push_rect_spans(
    out: &mut Vec<CoverageSpan>,
    rect: Rect,
    radius: f32,
    clips: &[Rect],
    width: u32,
    height: u32,
) {
    let Some(rect) = intersect_clips(rect, clips) else {
        return;
    };
    let x0 = rect.x.round().max(0.0) as u32;
    let y0 = rect.y.round().max(0.0) as u32;
    let x1 = (rect.x + rect.width).round().clamp(0.0, width as f32) as u32;
    let y1 = (rect.y + rect.height).round().clamp(0.0, height as f32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let w = x1 - x0;
    let h = y1 - y0;
    let r = (radius.round().max(0.0) as u32).min(w / 2).min(h / 2);
    for row in y0..y1 {
        let local = row - y0;
        let inset = if r <= 1 {
            0
        } else {
            let corner = local.min(h - 1 - local);
            if corner >= r {
                0
            } else {
                let rf = f64::from(r);
                let dy = rf - (f64::from(corner) + 0.5);
                let dx = (rf * rf - dy * dy).max(0.0).sqrt();
                (rf - dx).floor().clamp(0.0, rf) as u32
            }
        };
        let s = (x0 + inset).min(x1);
        let e = x1.saturating_sub(inset).max(s);
        if e > s {
            out.push((row, s, e));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_render_core::Color;

    fn fill(x: f32, y: f32, w: f32, h: f32, a: u8, radius: f32) -> PaintCommand {
        PaintCommand::FillRect {
            rect: Rect {
                x,
                y,
                width: w,
                height: h,
            },
            color: Color {
                r: 255,
                g: 0,
                b: 0,
                a,
            },
            radius,
        }
    }

    fn text_at(x: f32, y: f32) -> PaintCommand {
        PaintCommand::Text {
            x,
            y,
            color: Color {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            },
            size: 13.0,
            weight: 400,
            text: "hi".to_string(),
        }
    }

    fn image_at(x: f32, y: f32, w: f32, h: f32) -> PaintCommand {
        PaintCommand::Image {
            rect: Rect {
                x,
                y,
                width: w,
                height: h,
            },
            asset: 0,
            node: 0,
            revision: 0,
            treatment: flamewm_render_core::ImageTreatment::Original,
            tint: None,
        }
    }

    #[test]
    fn transparent_paint_contributes_no_coverage() {
        // Only an alpha-0 spare: falls back to whole surface? No: empty means
        // opaque fallback produces full spans — verify transparent is skipped
        // by mixing with one visible rect.
        let cmds = vec![
            fill(0.0, 0.0, 8.0, 8.0, 0, 0.0),
            fill(2.0, 2.0, 4.0, 4.0, 255, 0.0),
        ];
        let spans = build_coverage(&cmds, 8, 8, 0.0);
        assert!(spans.iter().all(|(y, _, _)| (2..6).contains(y)));
    }

    #[test]
    fn clip_restricts_coverage() {
        let cmds = vec![
            PaintCommand::PushClip {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 4.0,
                    height: 8.0,
                },
            },
            fill(0.0, 0.0, 8.0, 8.0, 255, 0.0),
            PaintCommand::PopClip,
        ];
        let spans = build_coverage(&cmds, 8, 8, 0.0);
        assert_eq!(spans.len(), 8);
        assert!(spans.iter().all(|(_, s, e)| (*s, *e) == (0, 4)));
    }

    #[test]
    fn rounded_coverage_narrows_corners() {
        let cmds = vec![fill(0.0, 0.0, 8.0, 8.0, 255, 4.0)];
        let spans = build_coverage(&cmds, 8, 8, 0.0);
        let row0 = spans.iter().find(|(y, _, _)| *y == 0).unwrap();
        assert_eq!((*row0).1, 2);
        assert_eq!((*row0).2, 6);
        let row3 = spans.iter().find(|(y, _, _)| *y == 3).unwrap();
        assert_eq!((row3.1, row3.2), (0, 8));
    }

    #[test]
    fn rounded_fill_and_stroke_have_no_square_corners() {
        let fill_cmds = vec![fill(0.0, 0.0, 8.0, 8.0, 255, 4.0)];
        let fill_spans = build_coverage(&fill_cmds, 8, 8, 0.0);
        let fill_row0 = fill_spans.iter().find(|(y, _, _)| *y == 0).unwrap();
        assert_eq!((fill_row0.1, fill_row0.2), (2, 6));
        let stroke_cmds = vec![PaintCommand::StrokeRect {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 8.0,
                height: 8.0,
            },
            color: Color {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            },
            width: 1.0,
            radius: 4.0,
        }];
        let stroke_spans = build_coverage(&stroke_cmds, 8, 8, 0.0);
        let stroke_row0 = stroke_spans.iter().find(|(y, _, _)| *y == 0).unwrap();
        assert_eq!((stroke_row0.1, stroke_row0.2), (2, 6));
        // radius 0 stays rectangular.
        let plain = vec![fill(0.0, 0.0, 8.0, 8.0, 255, 0.0)];
        let plain_spans = build_coverage(&plain, 8, 8, 0.0);
        let plain_row0 = plain_spans.iter().find(|(y, _, _)| *y == 0).unwrap();
        assert_eq!((plain_row0.1, plain_row0.2), (0, 8));
    }

    #[test]
    fn scale_one_matches_unscaled_entry_point() {
        let cmds = vec![
            fill(0.0, 0.0, 8.0, 8.0, 255, 4.0),
            PaintCommand::PushClip {
                rect: Rect {
                    x: 1.0,
                    y: 1.0,
                    width: 6.0,
                    height: 6.0,
                },
            },
            image_at(0.0, 0.0, 8.0, 8.0),
            PaintCommand::PopClip,
            text_at(2.0, 2.0),
        ];
        assert_eq!(
            build_coverage_scaled(&cmds, 8, 8, 0.0, 1.0),
            build_coverage(&cmds, 8, 8, 0.0)
        );
    }

    #[test]
    fn scaled_parity_matches_prescaled_paint_stream() {
        // Layout-space rect at scale 1.25 == same rect pre-scaled at 1.0.
        let layout_cmds = vec![fill(0.0, 0.0, 8.0, 8.0, 255, 4.0)];
        let scaled = build_coverage_scaled(&layout_cmds, 10, 10, 0.0, 1.25);
        let prescaled = build_coverage(&[fill(0.0, 0.0, 10.0, 10.0, 255, 5.0)], 10, 10, 0.0);
        assert_eq!(scaled, prescaled);
    }

    #[test]
    fn scaled_stroke_parity_at_1_5() {
        let layout_cmds = vec![PaintCommand::StrokeRect {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 8.0,
                height: 8.0,
            },
            color: Color {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            },
            width: 2.0,
            radius: 2.0,
        }];
        let scaled = build_coverage_scaled(&layout_cmds, 12, 12, 0.0, 1.5);
        let prescaled = build_coverage(
            &[PaintCommand::StrokeRect {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 12.0,
                    height: 12.0,
                },
                color: Color {
                    r: 255,
                    g: 0,
                    b: 0,
                    a: 255,
                },
                width: 3.0,
                radius: 3.0,
            }],
            12,
            12,
            0.0,
        );
        assert_eq!(scaled, prescaled);
    }

    #[test]
    fn scaled_clipped_image_parity_at_2x() {
        let layout_cmds = vec![
            PaintCommand::PushClip {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 4.0,
                    height: 8.0,
                },
            },
            image_at(0.0, 0.0, 8.0, 8.0),
            PaintCommand::PopClip,
        ];
        let scaled = build_coverage_scaled(&layout_cmds, 16, 16, 0.0, 2.0);
        let prescaled = build_coverage(
            &[
                PaintCommand::PushClip {
                    rect: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 8.0,
                        height: 16.0,
                    },
                },
                image_at(0.0, 0.0, 16.0, 16.0),
                PaintCommand::PopClip,
            ],
            16,
            16,
            0.0,
        );
        assert_eq!(scaled, prescaled);
        assert!(scaled.iter().all(|(_, s, e)| (*s, *e) == (0, 8)));
    }

    #[test]
    fn text_never_expands_shape() {
        let cmds = vec![text_at(0.0, 0.0)];
        let spans = build_coverage_scaled(&cmds, 8, 8, 0.0, 2.0);
        // Text-only stream falls back to the opaque whole-surface mask,
        // identical with and without scale: no glyph expansion either way.
        assert_eq!(spans, build_coverage(&cmds, 8, 8, 0.0));
        assert_eq!(spans.len(), 8);
    }
}

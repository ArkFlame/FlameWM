//! Backend-neutral lock frame painter.
//!
//! `flamewm-render-x11` turns the lock document into pixels through `XRender` and `Xft`. This module
//! does the same job with no X server at all: it lowers the document's paint commands to whole
//! pixel ops and rasterizes them into a straight RGBA8 frame, which is exactly what a Wayland
//! shared-memory buffer holds. The Wayland presenter is therefore buffer sizing and surface
//! plumbing; the pixels come from here.

use flamewm_render_core::glyphs;
use flamewm_render_core::lower::{FrameSource, lower_commands, snap_commands};
use flamewm_render_core::soft::SoftPixels;
use flamewm_render_core::{
    InteractionState, LayoutEngine, PaintCommand, RuntimeDocument, build_paint_commands,
};

/// A painted frame, ready to copy into a shared memory buffer.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Straight (non-premultiplied) RGBA8, row-major.
    pub pixels: Vec<u8>,
}

/// A source prepared for lowering, owned so the borrowed views can outlive the preparation step.
enum Prepared {
    Mask {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        coverage: Vec<u8>,
        color: [u8; 4],
    },
    Image {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
}

impl Prepared {
    fn as_source(&self) -> FrameSource<'_> {
        match self {
            Self::Mask {
                x,
                y,
                width,
                height,
                coverage,
                color,
            } => FrameSource::Mask {
                x: *x,
                y: *y,
                width: *width,
                height: *height,
                alpha: coverage,
                color: *color,
            },
            Self::Image {
                x,
                y,
                width,
                height,
                rgba,
            } => FrameSource::Image {
                x: *x,
                y: *y,
                width: *width,
                height: *height,
                rgba,
            },
        }
    }
}

/// The coverage mask for one text run, at the pen position the painter uses.
fn glyph_source(text: &str, size: f32, x: f32, y: f32, color: [u8; 4]) -> Prepared {
    let (mask_x, mask_y, width, height, coverage) = match glyphs::text_mask(text, size, x, y) {
        Some(mask) => (mask.x, mask.y, mask.width, mask.height, mask.coverage),
        // No installed font: an empty mask keeps the source order aligned with the commands.
        None => (0, 0, 0, 0, Vec::new()),
    };
    Prepared::Mask {
        x: mask_x,
        y: mask_y,
        width,
        height,
        coverage,
        color,
    }
}

/// One source per `Text`, `TextShadow` and `Image` command, in command order.
fn prepare_sources(commands: &[PaintCommand], document: &RuntimeDocument) -> Vec<Prepared> {
    let mut sources = Vec::new();
    for command in commands {
        match command {
            PaintCommand::Text {
                x,
                y,
                color,
                size,
                text,
                ..
            }
            | PaintCommand::TextShadow {
                x,
                y,
                color,
                size,
                text,
                ..
            } => sources.push(glyph_source(
                text,
                *size,
                *x,
                *y,
                [color.r, color.g, color.b, color.a],
            )),
            PaintCommand::Image { rect, node, .. } => {
                // The document already holds the decoded pixels for this node.
                let (width, height, rgba) = match document.image_for_node(*node) {
                    Some((_asset, image_width, image_height, _revision, pixels)) => {
                        (image_width, image_height, pixels.to_vec())
                    }
                    None => (0, 0, Vec::new()),
                };
                sources.push(Prepared::Image {
                    x: rect.x.round() as i32,
                    y: rect.y.round() as i32,
                    width,
                    height,
                    rgba,
                });
            }
            _ => {}
        }
    }
    sources
}

/// Lays out `document` for a `width` x `height` output and paints it into an RGBA8 frame.
///
/// The frame is fully repainted, so it can be handed to a compositor as the whole lock surface.
///
/// # Errors
/// Returns `Err` when a command cannot be expressed as whole pixels even after snapping, or when
/// an op fails. An error means no frame was produced; nothing partial is returned.
pub fn paint_document(
    document: &RuntimeDocument,
    width: u32,
    height: u32,
    scale: f32,
) -> Result<Frame, String> {
    if width == 0 || height == 0 {
        return Err("lock frame has no area".to_string());
    }
    let layout = LayoutEngine::compute(
        document,
        width as f32,
        height as f32,
        InteractionState::default(),
    );
    let commands = build_paint_commands(document, &layout, InteractionState::default());
    // The X11 painter resolves fractional geometry directly; a shm frame holds whole pixels.
    let commands = snap_commands(&commands, scale);

    let prepared = prepare_sources(&commands, document);
    let sources: Vec<FrameSource<'_>> = prepared.iter().map(Prepared::as_source).collect();
    let ops = lower_commands(&commands, scale, &sources, (width, height), None)
        .map_err(|error| format!("lower lock frame: {}", error.as_str()))?;

    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    SoftPixels::new()
        .paint(&mut pixels, width, height, &ops)
        .map_err(|error| format!("paint lock frame: {error}"))?;
    Ok(Frame {
        width,
        height,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_render_core::{CompiledDocument, CompiledNode, NodeKind, RuntimeDocument, Style};
    use std::sync::Arc;

    /// The smallest valid document: one root node with a solid fill.
    fn root_fill(color: [u8; 4]) -> RuntimeDocument {
        let node = CompiledNode {
            kind: NodeKind::Element,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: "root".into(),
            action: "".into(),
            text: "".into(),
            image: None,
            style: Arc::new(Style {
                background: flamewm_render_core::ColorValue::Literal(flamewm_render_core::Color {
                    r: color[0],
                    g: color[1],
                    b: color[2],
                    a: color[3],
                }),
                ..Style::default()
            }),
            hover_style: None,
            active_style: None,
            focus_style: None,
        };
        RuntimeDocument::new(CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![node],
        })
        .expect("valid document")
    }

    #[test]
    fn an_empty_output_is_refused() {
        let document = root_fill([1, 2, 3, 255]);
        assert!(paint_document(&document, 0, 100, 1.0).is_err());
        assert!(paint_document(&document, 100, 0, 1.0).is_err());
    }

    #[test]
    fn a_frame_has_one_rgba_quad_per_pixel() {
        let document = root_fill([1, 2, 3, 255]);
        let frame = paint_document(&document, 64, 48, 1.0).expect("paint");
        assert_eq!(frame.width, 64);
        assert_eq!(frame.height, 48);
        assert_eq!(frame.pixels.len(), 64 * 48 * 4);
    }

    #[test]
    fn a_root_fill_reaches_the_frame() {
        let document = root_fill([12, 200, 40, 255]);
        let frame = paint_document(&document, 8, 8, 1.0).expect("paint");
        assert_eq!(&frame.pixels[0..4], &[12, 200, 40, 255]);
    }
}

//! Shared recording `UiDocumentAccess` double for projection tests.
//!
//! Records the writes tests assert on and accepts every other write without effect.

use std::collections::HashMap;

use flamewm_render_core::Overflow;
#[cfg(test)]
use flamewm_render_core::{CompiledNode, NodeKind, Style};
use flamewm_ui_core::style::UiLayer;

use crate::{RuntimeImage, UiColor, UiDocumentAccess};
use crate::{UiDocument, UiTemplate};
use flamewm_render_core::glyphs::{TextMask, text_mask};
use flamewm_render_core::lower::{FrameSource, lower_commands};
use flamewm_render_core::soft::SoftPixels;
use flamewm_render_core::{InteractionState, LayoutEngine, PaintCommand};

/// Fixture node with empty action/text and no style; callers override fields with `..`.
#[cfg(test)]
pub(crate) fn compiled_node(id: &str, kind: NodeKind) -> CompiledNode {
    CompiledNode {
        kind,
        parent: None,
        first_child: None,
        next_sibling: None,
        id: id.to_string().into(),
        action: std::borrow::Cow::Borrowed(""),
        text: std::borrow::Cow::Borrowed(""),
        image: None,
        style: Style::default().into(),
        hover_style: None,
        active_style: None,
        focus_style: None,
    }
}

#[derive(Default)]
pub struct RecordingUiDoc {
    pub texts: HashMap<String, String>,
    pub visible: HashMap<String, bool>,
    pub toggles: HashMap<String, bool>,
    pub backgrounds: HashMap<String, UiColor>,
    pub borders: HashMap<String, UiColor>,
    pub foregrounds: HashMap<String, UiColor>,
    /// Foregrounds bound to a colour custom property, by node id.
    pub foreground_variables: HashMap<String, String>,
    pub variables: HashMap<String, UiColor>,
    pub positions: HashMap<String, (f32, f32)>,
    pub sizes: HashMap<String, (f32, f32)>,
    pub images: HashMap<String, (u32, u32)>,
    pub radii: HashMap<String, f32>,
    pub opacities: HashMap<String, f32>,
    /// Ids whose background was cleared, in call order. Also removes the entry from `backgrounds`.
    pub cleared: Vec<String>,
}

impl UiDocumentAccess for RecordingUiDoc {
    fn text(&mut self, id: &str, text: String) -> Result<(), String> {
        self.texts.insert(id.to_owned(), text);
        Ok(())
    }
    fn image_rgb8(&mut self, id: &str, image: RuntimeImage) -> Result<(), String> {
        self.images
            .insert(id.to_owned(), (image.width, image.height));
        Ok(())
    }
    fn image_rgba8(&mut self, id: &str, image: RuntimeImage) -> Result<(), String> {
        self.images
            .insert(id.to_owned(), (image.width, image.height));
        Ok(())
    }
    fn image_reset(&mut self, id: &str) -> Result<(), String> {
        self.images.remove(id);
        Ok(())
    }
    fn visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
        self.visible.insert(id.to_owned(), visible);
        Ok(())
    }
    fn toggle(&mut self, track_id: &str, _knob_id: &str, checked: bool) -> Result<(), String> {
        self.toggles.insert(track_id.to_owned(), checked);
        Ok(())
    }
    fn position(&mut self, id: &str, left: f32, top: f32) -> Result<(), String> {
        self.positions.insert(id.to_owned(), (left, top));
        Ok(())
    }
    fn size(&mut self, id: &str, width: f32, height: f32) -> Result<(), String> {
        self.sizes.insert(id.to_owned(), (width, height));
        Ok(())
    }
    fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
        self.backgrounds.insert(id.to_owned(), color);
        Ok(())
    }
    fn border(&mut self, id: &str, color: UiColor) -> Result<(), String> {
        self.borders.insert(id.to_owned(), color);
        Ok(())
    }
    fn corner_radius(&mut self, id: &str, radius_px: f32) -> Result<(), String> {
        self.radii.insert(id.to_owned(), radius_px);
        Ok(())
    }
    fn foreground(&mut self, id: &str, color: UiColor) -> Result<(), String> {
        self.foregrounds.insert(id.to_owned(), color);
        Ok(())
    }
    fn foreground_variable(&mut self, id: &str, name: &str) -> Result<(), String> {
        self.foreground_variables
            .insert(id.to_owned(), name.to_owned());
        Ok(())
    }
    fn background_clear(&mut self, id: &str) -> Result<(), String> {
        self.backgrounds.remove(id);
        self.cleared.push(id.to_owned());
        Ok(())
    }
    fn border_clear(&mut self, id: &str) -> Result<(), String> {
        self.borders.remove(id);
        Ok(())
    }
    fn foreground_clear(&mut self, _id: &str) -> Result<(), String> {
        Ok(())
    }
    fn font_size(&mut self, _id: &str, _size_px: f32) -> Result<(), String> {
        Ok(())
    }
    fn font_weight(&mut self, _id: &str, _weight: u16) -> Result<(), String> {
        Ok(())
    }
    fn opacity(&mut self, id: &str, opacity: f32) -> Result<(), String> {
        self.opacities.insert(id.to_owned(), opacity);
        Ok(())
    }
    fn font_family(&mut self, _family: String) -> Result<(), String> {
        Ok(())
    }
    fn color_variable(&mut self, name: &str, color: UiColor) -> Result<(), String> {
        self.variables.insert(name.to_owned(), color);
        Ok(())
    }
    fn overflow(&mut self, _id: &str, _x: Overflow, _y: Overflow) -> Result<(), String> {
        Ok(())
    }
    fn layer(&mut self, _id: &str, _layer: UiLayer) -> Result<(), String> {
        Ok(())
    }
}

/// Lays out and paints a popup `document` over `backdrop` at `viewport` through the software chain
/// (`PaintCommand` -> glyph masks -> `PixelOp` -> RGBA8), as a Wayland presenter does. Returns
/// `width * height * 4` bytes; set `FLAMEWM_AUDIO_SHOT_DIR` in the caller to dump them.
pub fn paint_popup_software(
    document: UiDocument,
    viewport: (f32, f32),
    backdrop: [u8; 4],
) -> Vec<u8> {
    enum Pending {
        Mask(TextMask, [u8; 4]),
        Image,
    }
    let (width, height) = (viewport.0 as u32, viewport.1 as u32);
    let runtime = UiTemplate::new(document).into_runtime_document();
    let layout = LayoutEngine::compute(
        &runtime,
        viewport.0,
        viewport.1,
        InteractionState::default(),
    );
    // Fractional fills (25% of 350px is 87.5px) snap to device pixels as the presenters do.
    let commands = flamewm_render_core::lower::snap_commands(
        &flamewm_render_core::paint::build_paint_commands(
            &runtime,
            &layout,
            InteractionState::default(),
        ),
        1.0,
    );
    // One source per Text, TextShadow and Image command, in command order. Text without a
    // usable font gets an empty mask, so the label is simply not drawn; slider geometry is
    // unaffected. The popup carries no bitmap images, so Image sources are empty.
    let pending: Vec<Pending> = commands
        .iter()
        .filter_map(|command| match command {
            PaintCommand::Text {
                x,
                y,
                size,
                color,
                text,
                ..
            }
            | PaintCommand::TextShadow {
                x,
                y,
                size,
                color,
                text,
                ..
            } => {
                // The command's y is already the text baseline (see the popup template).
                let mask = text_mask(text, *size, *x, *y).unwrap_or(TextMask {
                    x: 0,
                    y: 0,
                    width: 0,
                    height: 0,
                    coverage: Vec::new(),
                });
                Some(Pending::Mask(mask, [color.r, color.g, color.b, color.a]))
            }
            PaintCommand::Image { .. } => Some(Pending::Image),
            _ => None,
        })
        .collect();
    let sources: Vec<FrameSource<'_>> = pending
        .iter()
        .map(|source| match source {
            Pending::Mask(mask, color) => FrameSource::Mask {
                x: mask.x,
                y: mask.y,
                width: mask.width,
                height: mask.height,
                alpha: &mask.coverage[..],
                color: *color,
            },
            Pending::Image => FrameSource::Image {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                rgba: &[],
            },
        })
        .collect();
    let ops = lower_commands(&commands, 1.0, &sources, (width, height), None)
        .expect("popup commands must lower");
    let mut frame: Vec<u8> = backdrop
        .iter()
        .copied()
        .cycle()
        .take(width as usize * height as usize * 4)
        .collect();
    SoftPixels::new()
        .paint(&mut frame, width, height, &ops)
        .expect("software paint");
    frame
}

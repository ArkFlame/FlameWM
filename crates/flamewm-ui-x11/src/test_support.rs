//! Shared recording `UiDocumentAccess` double for projection tests.
//!
//! Records the writes tests assert on and accepts every other write without effect.

use std::collections::HashMap;

use flamewm_render_core::{CompiledNode, NodeKind, Overflow, Style};
use flamewm_ui_core::style::UiLayer;

use crate::{RuntimeImage, UiColor, UiDocumentAccess};

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
        style: Style::default(),
        hover_style: None,
        active_style: None,
    }
}

#[derive(Default)]
pub struct RecordingUiDoc {
    pub texts: HashMap<String, String>,
    pub visible: HashMap<String, bool>,
    pub toggles: HashMap<String, bool>,
    pub backgrounds: HashMap<String, UiColor>,
    pub foregrounds: HashMap<String, UiColor>,
    pub variables: HashMap<String, UiColor>,
    pub images: HashMap<String, (u32, u32)>,
    pub radii: HashMap<String, f32>,
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
    fn position(&mut self, _id: &str, _left: f32, _top: f32) -> Result<(), String> {
        Ok(())
    }
    fn size(&mut self, _id: &str, _width: f32, _height: f32) -> Result<(), String> {
        Ok(())
    }
    fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
        self.backgrounds.insert(id.to_owned(), color);
        Ok(())
    }
    fn border(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
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
    fn background_clear(&mut self, id: &str) -> Result<(), String> {
        self.backgrounds.remove(id);
        self.cleared.push(id.to_owned());
        Ok(())
    }
    fn border_clear(&mut self, _id: &str) -> Result<(), String> {
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

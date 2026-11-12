//! Test-only `UiDocumentAccess` helpers. `Recording` forwards every write to an inner document and
//! keeps the values a projection assigned, so a test reads node state back through the same
//! projection code production runs. `NullDocument` is an inner for tests that need no backend.

use std::collections::HashMap;
use std::sync::Arc;

use flamewm_ui_core::style::UiLayer;
use flamewm_ui_x11::{Overflow, RuntimeImage, UiColor, UiDocumentAccess};

#[derive(Default)]
pub(crate) struct Recorded {
    pub(crate) texts: HashMap<String, String>,
    pub(crate) visible: HashMap<String, bool>,
    pub(crate) attributes: HashMap<(String, String), String>,
}

impl Recorded {
    pub(crate) fn attribute(&self, id: &str, name: &str) -> Option<&str> {
        self.attributes
            .get(&(id.to_owned(), name.to_owned()))
            .map(String::as_str)
    }
}

pub(crate) struct Recording<'a, D: UiDocumentAccess> {
    inner: &'a mut D,
    pub(crate) recorded: Recorded,
}

impl<'a, D: UiDocumentAccess> Recording<'a, D> {
    pub(crate) fn new(inner: &'a mut D) -> Self {
        Self {
            inner,
            recorded: Recorded::default(),
        }
    }
}

impl<D: UiDocumentAccess> UiDocumentAccess for Recording<'_, D> {
    fn text(&mut self, id: &str, text: String) -> Result<(), String> {
        self.recorded.texts.insert(id.to_owned(), text.clone());
        self.inner.text(id, text)
    }
    fn text_ref(&mut self, id: &str, text: &str) -> Result<(), String> {
        self.recorded.texts.insert(id.to_owned(), text.to_owned());
        self.inner.text_ref(id, text)
    }
    fn image_rgb8(&mut self, id: &str, image: RuntimeImage) -> Result<(), String> {
        self.inner.image_rgb8(id, image)
    }
    fn image_rgba8(&mut self, id: &str, image: RuntimeImage) -> Result<(), String> {
        self.inner.image_rgba8(id, image)
    }
    fn image_rgba8_shared(
        &mut self,
        id: &str,
        source: String,
        width: u32,
        height: u32,
        pixels: Arc<Vec<u8>>,
    ) -> Result<(), String> {
        self.inner
            .image_rgba8_shared(id, source, width, height, pixels)
    }
    fn image_reset(&mut self, id: &str) -> Result<(), String> {
        self.inner.image_reset(id)
    }
    fn visible(&mut self, id: &str, visible: bool) -> Result<(), String> {
        self.recorded.visible.insert(id.to_owned(), visible);
        self.inner.visible(id, visible)
    }
    fn toggle(&mut self, track_id: &str, knob_id: &str, checked: bool) -> Result<(), String> {
        self.inner.toggle(track_id, knob_id, checked)
    }
    fn position(&mut self, id: &str, left: f32, top: f32) -> Result<(), String> {
        self.inner.position(id, left, top)
    }
    fn size(&mut self, id: &str, width: f32, height: f32) -> Result<(), String> {
        self.inner.size(id, width, height)
    }
    fn layout_box(&self, id: &str, viewport: (f32, f32)) -> Option<(f32, f32, f32, f32)> {
        self.inner.layout_box(id, viewport)
    }
    fn background(&mut self, id: &str, color: UiColor) -> Result<(), String> {
        self.inner.background(id, color)
    }
    fn border(&mut self, id: &str, color: UiColor) -> Result<(), String> {
        self.inner.border(id, color)
    }
    fn corner_radius(&mut self, id: &str, radius_px: f32) -> Result<(), String> {
        self.inner.corner_radius(id, radius_px)
    }
    fn node_backdrop(&mut self, id: &str, blur_percent: Option<u8>) -> Result<(), String> {
        self.inner.node_backdrop(id, blur_percent)
    }
    fn foreground(&mut self, id: &str, color: UiColor) -> Result<(), String> {
        self.inner.foreground(id, color)
    }
    fn background_clear(&mut self, id: &str) -> Result<(), String> {
        self.inner.background_clear(id)
    }
    fn border_clear(&mut self, id: &str) -> Result<(), String> {
        self.inner.border_clear(id)
    }
    fn foreground_clear(&mut self, id: &str) -> Result<(), String> {
        self.inner.foreground_clear(id)
    }
    fn font_size(&mut self, id: &str, size_px: f32) -> Result<(), String> {
        self.inner.font_size(id, size_px)
    }
    fn font_weight(&mut self, id: &str, weight: u16) -> Result<(), String> {
        self.inner.font_weight(id, weight)
    }
    fn opacity(&mut self, id: &str, opacity: f32) -> Result<(), String> {
        self.inner.opacity(id, opacity)
    }
    fn ui_font_defaults(&mut self, size_offset: f32, bold: bool) -> Result<(), String> {
        self.inner.ui_font_defaults(size_offset, bold)
    }
    fn ui_text_shadow(&mut self, enabled: bool) -> Result<(), String> {
        self.inner.ui_text_shadow(enabled)
    }
    fn font_family(&mut self, family: String) -> Result<(), String> {
        self.inner.font_family(family)
    }
    fn color_variable(&mut self, name: &str, color: UiColor) -> Result<(), String> {
        self.inner.color_variable(name, color)
    }
    fn foreground_variable(&mut self, id: &str, name: &str) -> Result<(), String> {
        self.inner.foreground_variable(id, name)
    }
    fn overflow(&mut self, id: &str, x: Overflow, y: Overflow) -> Result<(), String> {
        self.inner.overflow(id, x, y)
    }
    fn layer(&mut self, id: &str, layer: UiLayer) -> Result<(), String> {
        self.inner.layer(id, layer)
    }
    fn attribute(&mut self, id: &str, name: &str, value: &str) -> Result<(), String> {
        self.recorded
            .attributes
            .insert((id.to_owned(), name.to_owned()), value.to_owned());
        self.inner.attribute(id, name, value)
    }
}

/// Accepts every write and keeps nothing. Use it as the inner of `Recording` when only the
/// projection's own assignments matter.
#[derive(Default)]
pub(crate) struct NullDocument;

impl UiDocumentAccess for NullDocument {
    fn text(&mut self, _id: &str, _text: String) -> Result<(), String> {
        Ok(())
    }
    fn image_rgb8(&mut self, _id: &str, _image: RuntimeImage) -> Result<(), String> {
        Ok(())
    }
    fn image_rgba8(&mut self, _id: &str, _image: RuntimeImage) -> Result<(), String> {
        Ok(())
    }
    fn visible(&mut self, _id: &str, _visible: bool) -> Result<(), String> {
        Ok(())
    }
    fn toggle(&mut self, _track_id: &str, _knob_id: &str, _checked: bool) -> Result<(), String> {
        Ok(())
    }
    fn position(&mut self, _id: &str, _left: f32, _top: f32) -> Result<(), String> {
        Ok(())
    }
    fn size(&mut self, _id: &str, _width: f32, _height: f32) -> Result<(), String> {
        Ok(())
    }
    fn background(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
        Ok(())
    }
    fn border(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
        Ok(())
    }
    fn foreground(&mut self, _id: &str, _color: UiColor) -> Result<(), String> {
        Ok(())
    }
    fn background_clear(&mut self, _id: &str) -> Result<(), String> {
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
    fn overflow(&mut self, _id: &str, _x: Overflow, _y: Overflow) -> Result<(), String> {
        Ok(())
    }
    fn layer(&mut self, _id: &str, _layer: UiLayer) -> Result<(), String> {
        Ok(())
    }
    fn attribute(&mut self, _id: &str, _name: &str, _value: &str) -> Result<(), String> {
        Ok(())
    }
}

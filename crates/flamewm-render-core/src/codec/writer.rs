use super::wire::Wire;
use crate::model::*;

pub(super) struct Writer {
    pub(super) bytes: Vec<u8>,
}

impl Writer {
    pub(super) fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }
    pub(super) fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    pub(super) fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    pub(super) fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    pub(super) fn f32(&mut self, value: f32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    pub(super) fn opt_u16(&mut self, value: Option<u16>) {
        self.u16(value.unwrap_or(u16::MAX));
    }
    pub(super) fn opt_u32(&mut self, value: Option<u32>) {
        self.u32(value.unwrap_or(u32::MAX));
    }
    pub(super) fn string(&mut self, value: &str) -> Result<(), String> {
        if value.len() > u32::MAX as usize {
            return Err("string is too large to encode".to_string());
        }
        self.u32(value.len() as u32);
        self.bytes.extend_from_slice(value.as_bytes());
        Ok(())
    }
    pub(super) fn tag<T: Wire>(&mut self, value: T) {
        self.u8(value.to_wire());
    }
    pub(super) fn opt_style(&mut self, style: Option<&Style>, version: u16) {
        self.u8(style.is_some() as u8);
        if let Some(style) = style {
            self.style(style, version);
        }
    }
    pub(super) fn color(&mut self, value: Color) {
        self.bytes
            .extend_from_slice(&[value.r, value.g, value.b, value.a]);
    }
    pub(super) fn color_value(&mut self, value: ColorValue) {
        match value {
            ColorValue::Literal(color) => {
                self.u8(0);
                self.color(color);
                self.u16(0);
            }
            ColorValue::Variable(index) => {
                self.u8(1);
                self.color(Color::TRANSPARENT);
                self.u16(index);
            }
        }
    }
    pub(super) fn length(&mut self, value: Length) {
        match value {
            Length::Auto => {
                self.u8(0);
                self.f32(0.0);
            }
            Length::Px(value) => {
                self.u8(1);
                self.f32(value);
            }
            Length::Percent(value) => {
                self.u8(2);
                self.f32(value);
            }
        }
    }
    pub(super) fn edges(&mut self, value: Edges) {
        self.f32(value.top);
        self.f32(value.right);
        self.f32(value.bottom);
        self.f32(value.left);
    }
    pub(super) fn style(&mut self, style: &Style, version: u16) {
        self.tag(style.display);
        self.tag(style.flex_direction);
        self.tag(style.justify_content);
        self.tag(style.align_items);
        self.tag(style.position);
        self.tag(style.cursor);
        for value in [
            style.width,
            style.height,
            style.min_width,
            style.min_height,
            style.max_width,
            style.max_height,
            style.top,
            style.right,
            style.bottom,
            style.left,
        ] {
            self.length(value);
        }
        self.edges(style.margin);
        self.edges(style.padding);
        self.f32(style.gap);
        self.f32(style.flex_grow);
        self.color_value(style.background);
        self.color_value(style.color);
        self.color_value(style.border_color);
        self.f32(style.border_width);
        self.f32(style.border_radius);
        self.f32(style.font_size);
        self.u16(style.font_weight);
        self.f32(style.opacity);
        self.tag(style.overflow_x);
        self.tag(style.overflow_y);
        self.tag(style.image_treatment);
        self.tag(style.text_wrap);
        self.u8(u8::from(style.break_anywhere));
        // v10-only field: pre-v10 layouts have no text-overflow tag.
        if version > FORMAT_VERSION_OVERFLOW_LEGACY {
            self.tag(style.text_overflow);
        }
        // v8-only field: pre-v8 documents have no hide-scrollbars byte.
        if version > FORMAT_VERSION_BLUR_LEGACY {
            self.u8(u8::from(style.hide_scrollbars));
        }
        match style.text_shadow {
            Some(shadow) => {
                self.u8(1);
                self.f32(shadow.offset_x);
                self.f32(shadow.offset_y);
                self.color_value(shadow.color);
                self.f32(shadow.blur_radius);
            }
            None => self.u8(0),
        }
    }
}

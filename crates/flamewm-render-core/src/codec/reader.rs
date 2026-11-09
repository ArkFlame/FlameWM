use super::wire::Wire;
use crate::model::*;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

pub(super) struct Reader<'a> {
    pub(super) bytes: &'a [u8],
    pub(super) cursor: usize,
}

impl<'a> Reader<'a> {
    pub(super) fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self
            .cursor
            .checked_add(count)
            .ok_or_else(|| "compiled document overflow".to_string())?;
        if end > self.bytes.len() {
            return Err("compiled document is truncated".to_string());
        }
        let slice = &self.bytes[self.cursor..end];
        self.cursor = end;
        Ok(slice)
    }
    pub(super) fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    pub(super) fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    pub(super) fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub(super) fn u64(&mut self) -> Result<u64, String> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
    pub(super) fn f32(&mut self) -> Result<f32, String> {
        let b = self.take(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub(super) fn opt_u16(&mut self) -> Result<Option<u16>, String> {
        let value = self.u16()?;
        Ok((value != u16::MAX).then_some(value))
    }
    pub(super) fn opt_u32(&mut self) -> Result<Option<u32>, String> {
        let value = self.u32()?;
        Ok((value != u32::MAX).then_some(value))
    }
    /// Borrows from the embedded bytes when `borrow` is set (the `decode_static` path).
    pub(super) fn string(
        &mut self,
        borrow: Option<&'static [u8]>,
    ) -> Result<Cow<'static, str>, String> {
        let len = self.u32()? as usize;
        let start = self.cursor;
        let bytes = self.take(len)?;
        let invalid = || "compiled document contains invalid UTF-8".to_string();
        Ok(match borrow {
            Some(static_bytes) => Cow::Borrowed(
                std::str::from_utf8(&static_bytes[start..start + len]).map_err(|_| invalid())?,
            ),
            None => Cow::Owned(
                std::str::from_utf8(bytes)
                    .map_err(|_| invalid())?
                    .to_string(),
            ),
        })
    }
    pub(super) fn tag<T: Wire>(&mut self) -> Result<T, String> {
        let value = self.u8()?;
        T::from_wire(value).ok_or_else(|| format!("invalid {} {value}", T::NAME))
    }
    pub(super) fn flag(&mut self, name: &str) -> Result<bool, String> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(format!("invalid {name} {v}")),
        }
    }
    pub(super) fn shared_style(
        &mut self,
        version: u16,
        styles: &mut HashMap<&'a [u8], Arc<Style>>,
    ) -> Result<Arc<Style>, String> {
        let start = self.cursor;
        let style = self.style_versioned(version)?;
        let bytes = self.bytes;
        let wire = &bytes[start..self.cursor];
        Ok(Arc::clone(
            styles.entry(wire).or_insert_with(|| Arc::new(style)),
        ))
    }
    pub(super) fn opt_shared_style(
        &mut self,
        version: u16,
        styles: &mut HashMap<&'a [u8], Arc<Style>>,
    ) -> Result<Option<Arc<Style>>, String> {
        if self.u8()? != 0 {
            self.shared_style(version, styles).map(Some)
        } else {
            Ok(None)
        }
    }
    pub(super) fn color(&mut self) -> Result<Color, String> {
        let b = self.take(4)?;
        Ok(Color {
            r: b[0],
            g: b[1],
            b: b[2],
            a: b[3],
        })
    }
    pub(super) fn color_value(&mut self) -> Result<ColorValue, String> {
        let kind = self.u8()?;
        let color = self.color()?;
        let index = self.u16()?;
        match kind {
            0 => Ok(ColorValue::Literal(color)),
            1 => Ok(ColorValue::Variable(index)),
            value => Err(format!("invalid color value kind {value}")),
        }
    }
    pub(super) fn length(&mut self) -> Result<Length, String> {
        let kind = self.u8()?;
        let value = self.f32()?;
        match kind {
            0 => Ok(Length::Auto),
            1 => Ok(Length::Px(value)),
            2 => Ok(Length::Percent(value)),
            other => Err(format!("invalid length kind {other}")),
        }
    }
    pub(super) fn edges(&mut self) -> Result<Edges, String> {
        Ok(Edges {
            top: self.f32()?,
            right: self.f32()?,
            bottom: self.f32()?,
            left: self.f32()?,
        })
    }
    pub(super) fn style_versioned(&mut self, version: u16) -> Result<Style, String> {
        let display = self.tag()?;
        let flex_direction = self.tag()?;
        let justify_content = self.tag()?;
        let align_items = self.tag()?;
        let position = self.tag()?;
        let cursor = self.tag()?;
        Ok(Style {
            display,
            flex_direction,
            justify_content,
            align_items,
            position,
            width: self.length()?,
            height: self.length()?,
            min_width: self.length()?,
            min_height: self.length()?,
            max_width: self.length()?,
            max_height: self.length()?,
            top: self.length()?,
            right: self.length()?,
            bottom: self.length()?,
            left: self.length()?,
            margin: self.edges()?,
            padding: self.edges()?,
            gap: self.f32()?,
            flex_grow: self.f32()?,
            background: self.color_value()?,
            color: self.color_value()?,
            border_color: self.color_value()?,
            border_width: self.f32()?,
            border_radius: self.f32()?,
            font_size: self.f32()?,
            font_weight: self.u16()?,
            opacity: self.f32()?,
            cursor,
            // Post-v3 addition: v2 legacy documents predate overflow bytes.
            overflow_x: if version == FORMAT_VERSION_RGB8_LEGACY {
                Overflow::Visible
            } else {
                self.tag()?
            },
            overflow_y: if version == FORMAT_VERSION_RGB8_LEGACY {
                Overflow::Visible
            } else {
                self.tag()?
            },
            image_treatment: if version >= FORMAT_VERSION_TEXT_WRAP_LEGACY
                || version == FORMAT_VERSION_TREATMENT_LEGACY
            {
                self.tag()?
            } else {
                ImageTreatment::Original
            },
            text_wrap: if version >= FORMAT_VERSION_TEXT_WRAP_LEGACY {
                self.tag()?
            } else {
                TextWrap::NoWrap
            },
            break_anywhere: if version >= FORMAT_VERSION_TEXT_WRAP_LEGACY {
                self.flag("break anywhere")?
            } else {
                false
            },
            text_overflow: if version > FORMAT_VERSION_OVERFLOW_LEGACY {
                self.tag()?
            } else {
                TextOverflow::Clip
            },
            hide_scrollbars: if version > FORMAT_VERSION_BLUR_LEGACY {
                self.flag("hide scrollbars")?
            } else {
                false
            },
            text_shadow: if version >= FORMAT_VERSION_SHADOW_LEGACY {
                match self.u8()? {
                    0 => None,
                    1 => Some(TextShadow {
                        offset_x: self.f32()?,
                        offset_y: self.f32()?,
                        color: self.color_value()?,
                        blur_radius: if version >= FORMAT_VERSION {
                            self.f32()?
                        } else {
                            0.0
                        },
                    }),
                    v => return Err(format!("invalid text shadow flag {v}")),
                }
            } else {
                None
            },
        })
    }
}

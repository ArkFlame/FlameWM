use crate::model::*;

// One table per wire enum: the tag bytes live here once, for both directions.
trait Wire: Sized {
    const NAME: &'static str;
    fn to_wire(&self) -> u8;
    fn from_wire(value: u8) -> Option<Self>;
}

macro_rules! wire_enum {
    ($name:literal $ty:ident { $($variant:ident = $tag:literal),+ $(,)? }) => {
        impl Wire for $ty {
            const NAME: &'static str = $name;
            fn to_wire(&self) -> u8 {
                match self {
                    $($ty::$variant => $tag,)+
                }
            }
            fn from_wire(value: u8) -> Option<Self> {
                match value {
                    $($tag => Some($ty::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

wire_enum!("node kind" NodeKind { Element = 0, Text = 1, Image = 2 });
wire_enum!("display" Display { None = 0, Block = 1, Flex = 2 });
wire_enum!("flex direction" FlexDirection { Row = 0, Column = 1 });
wire_enum!("justify content" JustifyContent { Start = 0, Center = 1, End = 2, SpaceBetween = 3 });
wire_enum!("align items" AlignItems { Start = 0, Center = 1, End = 2, Stretch = 3 });
wire_enum!("position" Position { Flow = 0, Absolute = 1 });
wire_enum!("cursor kind" CursorKind {
    Default = 0,
    Pointer = 1,
    Text = 2,
    Move = 3,
    ResizeHorizontal = 4,
    ResizeVertical = 5,
    ResizeNorthWestSouthEast = 6,
    ResizeNorthEastSouthWest = 7,
});
wire_enum!("overflow" Overflow { Visible = 0, Hidden = 1, Auto = 2, Scroll = 3 });
wire_enum!("image treatment" ImageTreatment { Original = 0, SymbolicForeground = 1 });
wire_enum!("text wrap" TextWrap { NoWrap = 0, Wrap = 1 });

pub fn encode(document: &CompiledDocument) -> Result<Vec<u8>, String> {
    document.validate()?;
    let mut writer = Writer { bytes: Vec::new() };
    writer.bytes.extend_from_slice(&FORMAT_MAGIC);
    writer.u16(FORMAT_VERSION);
    writer.u16(0);
    writer.u64(document.source_fingerprint);
    writer.u32(document.root);
    writer.u32(document.variables.len() as u32);
    writer.u32(document.assets.len() as u32);
    writer.u32(document.nodes.len() as u32);

    for variable in &document.variables {
        writer.string(&variable.name)?;
        writer.color(variable.default);
    }

    for asset in &document.assets {
        writer.string(&asset.source)?;
        writer.u32(asset.width);
        writer.u32(asset.height);
        writer.u32(asset.pixels.len() as u32);
        writer.bytes.extend_from_slice(&asset.pixels);
    }

    for node in &document.nodes {
        writer.tag(node.kind);
        writer.opt_u32(node.parent);
        writer.opt_u32(node.first_child);
        writer.opt_u32(node.next_sibling);
        writer.string(&node.id)?;
        writer.string(&node.action)?;
        writer.string(&node.text)?;
        writer.opt_u16(node.image);
        writer.style(&node.style);
        writer.opt_style(node.hover_style.as_ref());
        writer.opt_style(node.active_style.as_ref());
    }

    Ok(writer.bytes)
}

pub fn decode(bytes: &[u8]) -> Result<CompiledDocument, String> {
    let mut reader = Reader { bytes, cursor: 0 };
    let magic = reader.take(4)?;
    if magic != FORMAT_MAGIC {
        return Err("not a FlameWM Render compiled document".to_string());
    }
    let version = reader.u16()?;
    // Pixel-format decision: v3 is straight RGBA8 (4 bytes/px). v2 legacy
    // documents carry opaque RGB8 (3 bytes/px) and upconvert to alpha=255 so
    // existing PPM build artifacts keep decoding. Unknown versions are
    // rejected explicitly; no silent reinterpretation of pixel bytes.
    let bytes_per_pixel: u32 = match version {
        FORMAT_VERSION | FORMAT_VERSION_TREATMENT_LEGACY | FORMAT_VERSION_RGBA8_LEGACY => 4,
        FORMAT_VERSION_RGB8_LEGACY => 3,
        other => {
            return Err(format!(
                "unsupported FlameWM Render format version {other}; expected {FORMAT_VERSION} (legacy {FORMAT_VERSION_RGBA8_LEGACY} and {FORMAT_VERSION_RGB8_LEGACY} accepted)"
            ));
        }
    };
    let _flags = reader.u16()?;
    let source_fingerprint = reader.u64()?;
    let root = reader.u32()?;
    let variable_count = reader.u32()? as usize;
    let asset_count = reader.u32()? as usize;
    let node_count = reader.u32()? as usize;

    if variable_count > 65_535 {
        return Err("compiled document has too many variables".to_string());
    }
    if asset_count > 65_535 {
        return Err("compiled document has too many image assets".to_string());
    }
    if node_count > 1_000_000 {
        return Err("compiled document node count is unreasonable".to_string());
    }

    let mut variables = Vec::with_capacity(variable_count);
    for _ in 0..variable_count {
        variables.push(ColorVariable {
            name: reader.string()?,
            default: reader.color()?,
        });
    }

    let mut assets = Vec::with_capacity(asset_count);
    for _ in 0..asset_count {
        let source = reader.string()?;
        let width = reader.u32()?;
        let height = reader.u32()?;
        let byte_count = reader.u32()? as usize;
        let expected = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(bytes_per_pixel))
            .ok_or_else(|| "compiled image dimensions overflow".to_string())?
            as usize;
        if byte_count != expected {
            return Err(format!(
                "compiled image has {byte_count} bytes; expected {expected}"
            ));
        }
        let raw = reader.take(byte_count)?.to_vec();
        let pixels = if bytes_per_pixel == 3 {
            let mut rgba = Vec::with_capacity(raw.len() / 3 * 4);
            for rgb in raw.chunks_exact(3) {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            rgba
        } else {
            raw
        };
        assets.push(ImageAsset {
            source,
            width,
            height,
            pixels,
        });
    }

    let mut nodes = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        let kind = reader.tag()?;
        let parent = reader.opt_u32()?;
        let first_child = reader.opt_u32()?;
        let next_sibling = reader.opt_u32()?;
        let id = reader.string()?;
        let action = reader.string()?;
        let text = reader.string()?;
        let image = reader.opt_u16()?;
        let style = reader.style_versioned(version)?;
        let hover_style = reader.opt_style(version)?;
        let active_style = reader.opt_style(version)?;
        nodes.push(CompiledNode {
            kind,
            parent,
            first_child,
            next_sibling,
            id,
            action,
            text,
            image,
            style,
            hover_style,
            active_style,
        });
    }

    if reader.cursor != bytes.len() {
        return Err(format!(
            "compiled document has {} trailing bytes",
            bytes.len() - reader.cursor
        ));
    }

    let document = CompiledDocument {
        source_fingerprint,
        root,
        variables,
        assets,
        nodes,
    };
    document.validate()?;
    Ok(document)
}

struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }
    fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn f32(&mut self, value: f32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn opt_u16(&mut self, value: Option<u16>) {
        self.u16(value.unwrap_or(u16::MAX));
    }
    fn opt_u32(&mut self, value: Option<u32>) {
        self.u32(value.unwrap_or(u32::MAX));
    }
    fn string(&mut self, value: &str) -> Result<(), String> {
        if value.len() > u32::MAX as usize {
            return Err("string is too large to encode".to_string());
        }
        self.u32(value.len() as u32);
        self.bytes.extend_from_slice(value.as_bytes());
        Ok(())
    }
    fn tag<T: Wire>(&mut self, value: T) {
        self.u8(value.to_wire());
    }
    fn opt_style(&mut self, style: Option<&Style>) {
        self.u8(style.is_some() as u8);
        if let Some(style) = style {
            self.style(style);
        }
    }
    fn color(&mut self, value: Color) {
        self.bytes
            .extend_from_slice(&[value.r, value.g, value.b, value.a]);
    }
    fn color_value(&mut self, value: ColorValue) {
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
    fn length(&mut self, value: Length) {
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
    fn edges(&mut self, value: Edges) {
        self.f32(value.top);
        self.f32(value.right);
        self.f32(value.bottom);
        self.f32(value.left);
    }
    fn style(&mut self, style: &Style) {
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
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
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
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, String> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
    fn f32(&mut self) -> Result<f32, String> {
        let b = self.take(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn opt_u16(&mut self) -> Result<Option<u16>, String> {
        let value = self.u16()?;
        Ok((value != u16::MAX).then_some(value))
    }
    fn opt_u32(&mut self) -> Result<Option<u32>, String> {
        let value = self.u32()?;
        Ok((value != u32::MAX).then_some(value))
    }
    fn string(&mut self) -> Result<String, String> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| "compiled document contains invalid UTF-8".to_string())
    }
    fn tag<T: Wire>(&mut self) -> Result<T, String> {
        let value = self.u8()?;
        T::from_wire(value).ok_or_else(|| format!("invalid {} {value}", T::NAME))
    }
    fn opt_style(&mut self, version: u16) -> Result<Option<Style>, String> {
        if self.u8()? != 0 {
            Ok(Some(self.style_versioned(version)?))
        } else {
            Ok(None)
        }
    }
    fn color(&mut self) -> Result<Color, String> {
        let b = self.take(4)?;
        Ok(Color {
            r: b[0],
            g: b[1],
            b: b[2],
            a: b[3],
        })
    }
    fn color_value(&mut self) -> Result<ColorValue, String> {
        let kind = self.u8()?;
        let color = self.color()?;
        let index = self.u16()?;
        match kind {
            0 => Ok(ColorValue::Literal(color)),
            1 => Ok(ColorValue::Variable(index)),
            value => Err(format!("invalid color value kind {value}")),
        }
    }
    fn length(&mut self) -> Result<Length, String> {
        let kind = self.u8()?;
        let value = self.f32()?;
        match kind {
            0 => Ok(Length::Auto),
            1 => Ok(Length::Px(value)),
            2 => Ok(Length::Percent(value)),
            other => Err(format!("invalid length kind {other}")),
        }
    }
    fn edges(&mut self) -> Result<Edges, String> {
        Ok(Edges {
            top: self.f32()?,
            right: self.f32()?,
            bottom: self.f32()?,
            left: self.f32()?,
        })
    }
    fn style_versioned(&mut self, version: u16) -> Result<Style, String> {
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
            image_treatment: if version == FORMAT_VERSION
                || version == FORMAT_VERSION_TREATMENT_LEGACY
            {
                self.tag()?
            } else {
                ImageTreatment::Original
            },
            text_wrap: if version == FORMAT_VERSION {
                self.tag()?
            } else {
                TextWrap::NoWrap
            },
            break_anywhere: if version == FORMAT_VERSION {
                match self.u8()? {
                    0 => false,
                    1 => true,
                    v => return Err(format!("invalid break anywhere {v}")),
                }
            } else {
                false
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_round_trip() {
        let document = CompiledDocument {
            source_fingerprint: 42,
            root: 0,
            variables: vec![ColorVariable {
                name: "--accent".into(),
                default: Color::rgb(220, 38, 38),
            }],
            assets: vec![ImageAsset {
                source: "pixel.ppm".into(),
                width: 1,
                height: 1,
                pixels: vec![255, 0, 0, 255],
            }],
            nodes: vec![CompiledNode {
                kind: NodeKind::Element,
                parent: None,
                first_child: None,
                next_sibling: None,
                id: "root".into(),
                action: String::new(),
                text: String::new(),
                image: None,
                style: Style::default(),
                hover_style: None,
                active_style: None,
            }],
        };
        let bytes = encode(&document).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, document);
    }

    #[test]
    fn legacy_rgb8_document_upconverts_to_opaque_rgba8() {
        // Hand-build a v2 document: same layout as encode() but version=2
        // with 3-byte RGB pixels. The v3 decoder must accept it explicitly
        // and convert each pixel to alpha=255.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&FORMAT_MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION_RGB8_LEGACY.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&7u64.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        let source = b"legacy.ppm";
        bytes.extend_from_slice(&(source.len() as u32).to_le_bytes());
        bytes.extend_from_slice(source);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&[10, 20, 30]);
        // Minimal root element node to pass validate().
        bytes.push(0);
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 12]);
        bytes.extend_from_slice(&u16::MAX.to_le_bytes());
        // Default style encoding: display=Block(1), then 5 single-byte enums.
        bytes.extend_from_slice(&[1, 0, 0, 3, 0, 0]);
        // 10 auto lengths (kind byte + f32 each), then margin and padding edges.
        bytes.extend_from_slice(&[0u8; 50]);
        bytes.extend_from_slice(&[0u8; 32]);
        bytes.extend_from_slice(&0f32.to_le_bytes());
        bytes.extend_from_slice(&0f32.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        bytes.push(0);
        bytes.extend_from_slice(&[0, 0, 0, 255, 0, 0]);
        bytes.push(0);
        bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        bytes.extend_from_slice(&0f32.to_le_bytes());
        bytes.extend_from_slice(&0f32.to_le_bytes());
        bytes.extend_from_slice(&14f32.to_le_bytes());
        bytes.extend_from_slice(&400u16.to_le_bytes());
        bytes.extend_from_slice(&1f32.to_le_bytes());
        // v2 legacy style ends here: opacity f32, no overflow bytes.
        // hover/active presence flags (both absent).
        bytes.push(0);
        bytes.push(0);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.assets[0].pixels, vec![10, 20, 30, 255]);
        assert!(decoded.assets[0].is_fully_opaque());
    }

    #[test]
    fn unknown_version_is_rejected_explicitly() {
        let document = roundtrip_node(ImageTreatment::Original);
        let mut bytes = encode(&document).unwrap();
        bytes[4..6].copy_from_slice(&9u16.to_le_bytes());
        let error = decode(&bytes).expect_err("version 9 must fail");
        assert!(
            error.contains("unsupported FlameWM Render format version 9"),
            "unexpected: {error}"
        );
    }

    fn roundtrip_node(treatment: ImageTreatment) -> CompiledDocument {
        let mut style = Style::default();
        style.image_treatment = treatment;
        CompiledDocument {
            source_fingerprint: 1,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![CompiledNode {
                kind: NodeKind::Element,
                parent: None,
                first_child: None,
                next_sibling: None,
                id: String::new(),
                action: String::new(),
                text: String::new(),
                image: None,
                style,
                hover_style: None,
                active_style: None,
            }],
        }
    }

    #[test]
    fn v4_roundtrip_preserves_original_treatment() {
        let document = roundtrip_node(ImageTreatment::Original);
        let decoded = decode(&encode(&document).unwrap()).unwrap();
        assert_eq!(decoded, document);
        assert_eq!(
            decoded.nodes[0].style.image_treatment,
            ImageTreatment::Original
        );
    }

    #[test]
    fn v4_roundtrip_preserves_symbolic_treatment() {
        let document = roundtrip_node(ImageTreatment::SymbolicForeground);
        let decoded = decode(&encode(&document).unwrap()).unwrap();
        assert_eq!(decoded, document);
        assert_eq!(
            decoded.nodes[0].style.image_treatment,
            ImageTreatment::SymbolicForeground
        );
    }

    #[test]
    fn v4_roundtrip_preserves_wrap_fields() {
        let mut document = roundtrip_node(ImageTreatment::Original);
        document.nodes[0].style.text_wrap = TextWrap::Wrap;
        document.nodes[0].style.break_anywhere = true;
        let decoded = decode(&encode(&document).unwrap()).unwrap();
        assert_eq!(decoded, document);
    }

    #[test]
    fn v3_style_decodes_with_original_treatment_default() {
        let document = roundtrip_node(ImageTreatment::SymbolicForeground);
        let mut bytes = encode(&document).unwrap();
        bytes[4..6].copy_from_slice(&FORMAT_VERSION_RGBA8_LEGACY.to_le_bytes());
        // v5 appends 3 wrap bytes per style (treatment+wrap+break);
        // strip them so the payload matches v3 layout.
        assert_eq!(FORMAT_VERSION, 5);
        let tail = bytes.len() - 5;
        bytes.drain(tail..tail + 3);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(
            decoded.nodes[0].style.image_treatment,
            ImageTreatment::Original
        );
        assert_eq!(decoded.nodes[0].style.text_wrap, TextWrap::NoWrap);
        assert!(!decoded.nodes[0].style.break_anywhere);
        assert_eq!(decoded.nodes[0].style.overflow_x, Overflow::Visible);
        assert_eq!(decoded.nodes[0].style.overflow_y, Overflow::Visible);
    }

    #[test]
    fn v4_style_decodes_with_wrap_defaults() {
        let document = roundtrip_node(ImageTreatment::SymbolicForeground);
        let mut bytes = encode(&document).unwrap();
        assert_eq!(FORMAT_VERSION, 5);
        bytes[4..6].copy_from_slice(&FORMAT_VERSION_TREATMENT_LEGACY.to_le_bytes());
        // v5 appends wrap+break after treatment; strip those 2 bytes.
        let tail = bytes.len() - 4;
        bytes.drain(tail..tail + 2);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(
            decoded.nodes[0].style.image_treatment,
            ImageTreatment::SymbolicForeground
        );
        assert_eq!(decoded.nodes[0].style.text_wrap, TextWrap::NoWrap);
        assert!(!decoded.nodes[0].style.break_anywhere);
    }

    #[test]
    fn v2_style_decodes_with_original_treatment_and_overflow_defaults() {
        let document = roundtrip_node(ImageTreatment::Original);
        let mut bytes = encode(&document).unwrap();
        assert_eq!(FORMAT_VERSION, 5);
        // Convert to v2 layout: version=2 plus strip 3 wrap bytes and the
        // 2 overflow bytes from the single node style (v2 predates both).
        let style_tail = bytes.len() - 7;
        bytes.drain(style_tail..style_tail + 5);
        bytes[4..6].copy_from_slice(&FORMAT_VERSION_RGB8_LEGACY.to_le_bytes());
        let decoded = decode(&bytes).unwrap();
        assert_eq!(
            decoded.nodes[0].style.image_treatment,
            ImageTreatment::Original
        );
        assert_eq!(decoded.nodes[0].style.overflow_x, Overflow::Visible);
        assert_eq!(decoded.nodes[0].style.overflow_y, Overflow::Visible);
    }
}

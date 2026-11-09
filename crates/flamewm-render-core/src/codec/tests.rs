use super::*;
use std::sync::Arc;

/// Builds the newest pre-v8 wire layout (no hide-scrollbars byte), so the
/// legacy tests can strip the remaining v8 additions themselves.
fn encode_pre_v8(document: &CompiledDocument) -> Vec<u8> {
    encode_version(document, FORMAT_VERSION_BLUR_LEGACY).expect("encode pre-v8")
}

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
            pixels: vec![255, 0, 0, 255].into(),
        }],
        nodes: vec![CompiledNode {
            kind: NodeKind::Element,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: "root".into(),
            action: Cow::Borrowed(""),
            text: Cow::Borrowed(""),
            image: None,
            style: Style::default().into(),
            hover_style: None,
            active_style: None,
            focus_style: None,
        }],
    };
    let bytes = encode(&document).unwrap();
    let decoded = decode(&bytes).unwrap();
    assert_eq!(decoded, document);
}

#[test]
fn decode_static_borrows_pixels_and_matches_decode() {
    let document = CompiledDocument {
        source_fingerprint: 7,
        root: 0,
        variables: Vec::new(),
        assets: vec![ImageAsset {
            source: "pixel.png".into(),
            width: 1,
            height: 1,
            pixels: vec![1, 2, 3, 4].into(),
        }],
        nodes: vec![CompiledNode {
            kind: NodeKind::Image,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: Cow::Borrowed(""),
            action: Cow::Borrowed(""),
            text: Cow::Borrowed(""),
            image: Some(0),
            style: Style::default().into(),
            hover_style: None,
            active_style: None,
            focus_style: None,
        }],
    };
    let bytes: &'static [u8] = Box::leak(encode(&document).unwrap().into_boxed_slice());
    let borrowed = decode_static(bytes).unwrap();
    assert!(matches!(borrowed.assets[0].pixels, Cow::Borrowed(_)));
    assert!(matches!(borrowed.assets[0].source, Cow::Borrowed(_)));
    assert_eq!(borrowed, decode(bytes).unwrap());
    assert_eq!(borrowed, document);
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
    bytes[4..6].copy_from_slice(&11u16.to_le_bytes());
    let error = decode(&bytes).expect_err("version 11 must fail");
    assert!(
        error.contains("unsupported FlameWM Render format version 11"),
        "unexpected: {error}"
    );
}

fn roundtrip_node(treatment: ImageTreatment) -> CompiledDocument {
    let style = Style {
        image_treatment: treatment,
        ..Style::default()
    };
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
            id: Cow::Borrowed(""),
            action: Cow::Borrowed(""),
            text: Cow::Borrowed(""),
            image: None,
            style: style.into(),
            hover_style: None,
            active_style: None,
            focus_style: None,
        }],
    }
}

#[test]
fn decode_shares_identical_styles_between_nodes() {
    let mut document = roundtrip_node(ImageTreatment::Original);
    let mut other = document.nodes[0].clone();
    other.hover_style = Some(Arc::clone(&other.style));
    let mut distinct = document.nodes[0].clone();
    Arc::make_mut(&mut distinct.style).hide_scrollbars = true;
    document.nodes.push(other);
    document.nodes.push(distinct);
    let decoded = decode(&encode(&document).unwrap()).unwrap();
    assert_eq!(decoded, document);
    assert!(Arc::ptr_eq(
        &decoded.nodes[0].style,
        &decoded.nodes[1].style
    ));
    assert!(Arc::ptr_eq(
        &decoded.nodes[1].style,
        decoded.nodes[1].hover_style.as_ref().unwrap()
    ));
    assert!(!Arc::ptr_eq(
        &decoded.nodes[0].style,
        &decoded.nodes[2].style
    ));
}

#[test]
fn v4_roundtrip_preserves_image_treatment() {
    for treatment in [ImageTreatment::Original, ImageTreatment::SymbolicForeground] {
        let document = roundtrip_node(treatment);
        let decoded = decode(&encode(&document).unwrap()).unwrap();
        assert_eq!(decoded, document);
        assert_eq!(decoded.nodes[0].style.image_treatment, treatment);
    }
}

#[test]
fn v4_roundtrip_preserves_wrap_fields() {
    let mut document = roundtrip_node(ImageTreatment::Original);
    std::sync::Arc::make_mut(&mut document.nodes[0].style).text_wrap = TextWrap::Wrap;
    std::sync::Arc::make_mut(&mut document.nodes[0].style).break_anywhere = true;
    let decoded = decode(&encode(&document).unwrap()).unwrap();
    assert_eq!(decoded, document);
}

#[test]
fn v3_style_decodes_with_original_treatment_default() {
    let document = roundtrip_node(ImageTreatment::SymbolicForeground);
    let mut bytes = encode_pre_v8(&document);
    bytes[4..6].copy_from_slice(&FORMAT_VERSION_RGBA8_LEGACY.to_le_bytes());
    // v6 adds one shadow-presence byte in addition to the v5
    // treatment+wrap+break fields. Strip four bytes for v3.
    assert_eq!(FORMAT_VERSION, 10);
    let tail = bytes.len() - 6;
    bytes.drain(tail..tail + 4);
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
    let mut bytes = encode_pre_v8(&document);
    assert_eq!(FORMAT_VERSION, 10);
    bytes[4..6].copy_from_slice(&FORMAT_VERSION_TREATMENT_LEGACY.to_le_bytes());
    // Strip v5 wrap+break and v6 shadow flag after treatment.
    let tail = bytes.len() - 5;
    bytes.drain(tail..tail + 3);
    let decoded = decode(&bytes).unwrap();
    assert_eq!(
        decoded.nodes[0].style.image_treatment,
        ImageTreatment::SymbolicForeground
    );
    assert_eq!(decoded.nodes[0].style.text_wrap, TextWrap::NoWrap);
    assert!(!decoded.nodes[0].style.break_anywhere);
}

#[test]
fn v6_shadow_roundtrip_preserves_offset_and_color() {
    let mut document = roundtrip_node(ImageTreatment::Original);
    std::sync::Arc::make_mut(&mut document.nodes[0].style).text_shadow = Some(TextShadow {
        offset_x: -1.0,
        offset_y: 2.0,
        blur_radius: 2.0,
        color: ColorValue::Literal(Color {
            r: 0,
            g: 0,
            b: 0,
            a: 180,
        }),
    });
    assert_eq!(decode(&encode(&document).unwrap()).unwrap(), document);
}

#[test]
fn v6_shadow_decodes_with_zero_blur() {
    let mut doc = roundtrip_node(ImageTreatment::Original);
    std::sync::Arc::make_mut(&mut doc.nodes[0].style).text_shadow = Some(TextShadow {
        offset_x: 1.0,
        offset_y: -2.0,
        blur_radius: 3.0,
        color: ColorValue::Literal(Color {
            r: 1,
            g: 2,
            b: 3,
            a: 200,
        }),
    });
    let mut bytes = encode_pre_v8(&doc);
    bytes[4..6].copy_from_slice(&FORMAT_VERSION_SHADOW_LEGACY.to_le_bytes());
    // Last four style bytes are the newly appended blur radius for this one-node fixture.
    let at = bytes.len() - 6;
    bytes.drain(at..at + 4);
    let decoded = decode(&bytes).unwrap();
    assert_eq!(decoded.nodes[0].style.text_shadow.unwrap().blur_radius, 0.0);
    assert_eq!(decoded.nodes[0].style.text_shadow.unwrap().offset_y, -2.0);
}

#[test]
fn v5_style_decodes_without_text_shadow() {
    let document = roundtrip_node(ImageTreatment::Original);
    let mut bytes = encode_pre_v8(&document);
    bytes[4..6].copy_from_slice(&FORMAT_VERSION_TEXT_WRAP_LEGACY.to_le_bytes());
    // The last two bytes are the optional hover/active style flags.
    // The v6 shadow flag appears immediately before them.
    bytes.remove(bytes.len() - 3);
    let decoded = decode(&bytes).unwrap();
    assert_eq!(decoded.nodes[0].style.text_shadow, None);
}

#[test]
fn v2_style_decodes_with_original_treatment_and_overflow_defaults() {
    let document = roundtrip_node(ImageTreatment::Original);
    let mut bytes = encode_pre_v8(&document);
    assert_eq!(FORMAT_VERSION, 10);
    // Convert to v2: strip treatment, wrap, break, shadow, and two
    // overflow bytes from the single-node style.
    let style_tail = bytes.len() - 8;
    bytes.drain(style_tail..style_tail + 6);
    bytes[4..6].copy_from_slice(&FORMAT_VERSION_RGB8_LEGACY.to_le_bytes());
    let decoded = decode(&bytes).unwrap();
    assert_eq!(
        decoded.nodes[0].style.image_treatment,
        ImageTreatment::Original
    );
    assert_eq!(decoded.nodes[0].style.overflow_x, Overflow::Visible);
    assert_eq!(decoded.nodes[0].style.overflow_y, Overflow::Visible);
}

#[test]
fn v8_roundtrip_preserves_hide_scrollbars() {
    let mut document = roundtrip_node(ImageTreatment::Original);
    std::sync::Arc::make_mut(&mut document.nodes[0].style).hide_scrollbars = true;
    let decoded = decode(&encode(&document).unwrap()).unwrap();
    assert!(decoded.nodes[0].style.hide_scrollbars);
    assert_eq!(decoded, document);
}

#[test]
fn v7_style_decodes_without_hide_scrollbars() {
    let mut document = roundtrip_node(ImageTreatment::Original);
    std::sync::Arc::make_mut(&mut document.nodes[0].style).hide_scrollbars = true;
    // encode_version at v7 writes the v7 layout, which has no hide-scrollbars byte.
    let bytes = encode_pre_v8(&document);
    let decoded = decode(&bytes).unwrap();
    assert!(!decoded.nodes[0].style.hide_scrollbars);
}

#[test]
fn v9_roundtrip_preserves_focus_style() {
    let mut document = roundtrip_node(ImageTreatment::Original);
    document.nodes[0].focus_style = Some(std::sync::Arc::new(Style::default()));
    let decoded = decode(&encode(&document).unwrap()).unwrap();
    assert!(decoded.nodes[0].focus_style.is_some());
    assert_eq!(decoded, document);
}

#[test]
fn v8_document_decodes_without_focus_style() {
    let document = roundtrip_node(ImageTreatment::Original);
    let bytes = encode_version(&document, FORMAT_VERSION_FOCUS_LEGACY).unwrap();
    let decoded = decode(&bytes).unwrap();
    assert!(decoded.nodes[0].focus_style.is_none());
    assert_eq!(decoded, document);
}

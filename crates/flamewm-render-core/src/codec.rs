use crate::model::*;
use std::borrow::Cow;
use std::collections::HashMap;

mod reader;
#[cfg(test)]
mod tests;
mod wire;
mod writer;

use reader::Reader;
use writer::Writer;

pub fn encode(document: &CompiledDocument) -> Result<Vec<u8>, String> {
    encode_version(document, FORMAT_VERSION)
}

/// Encodes at an explicit `version`, so tests can rebuild pre-v8 wire layouts.
/// Only fields added in a newer revision are gated on `version`; the reader in
/// [`decode_with`] is the mirror image of these gates.
fn encode_version(document: &CompiledDocument, version: u16) -> Result<Vec<u8>, String> {
    document.validate()?;
    let mut writer = Writer { bytes: Vec::new() };
    writer.bytes.extend_from_slice(&FORMAT_MAGIC);
    writer.u16(version);
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
        writer.style(&node.style, version);
        writer.opt_style(node.hover_style.as_deref(), version);
        writer.opt_style(node.active_style.as_deref(), version);
        // v9-only field: pre-v9 layouts have no focus-style flag.
        if version > FORMAT_VERSION_FOCUS_LEGACY {
            writer.opt_style(node.focus_style.as_deref(), version);
        }
    }

    Ok(writer.bytes)
}

pub fn decode(bytes: &[u8]) -> Result<CompiledDocument, String> {
    decode_with(bytes, None)
}

/// Like [`decode`], but image pixels borrow from `bytes` instead of being copied to the heap.
pub fn decode_static(bytes: &'static [u8]) -> Result<CompiledDocument, String> {
    decode_with(bytes, Some(bytes))
}

fn decode_with(bytes: &[u8], borrow: Option<&'static [u8]>) -> Result<CompiledDocument, String> {
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
        FORMAT_VERSION
        | FORMAT_VERSION_OVERFLOW_LEGACY
        | FORMAT_VERSION_FOCUS_LEGACY
        | FORMAT_VERSION_BLUR_LEGACY
        | FORMAT_VERSION_SHADOW_LEGACY
        | FORMAT_VERSION_TEXT_WRAP_LEGACY
        | FORMAT_VERSION_TREATMENT_LEGACY
        | FORMAT_VERSION_RGBA8_LEGACY => 4,
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
            name: reader.string(borrow)?,
            default: reader.color()?,
        });
    }

    let mut assets = Vec::with_capacity(asset_count);
    for _ in 0..asset_count {
        let source = reader.string(borrow)?;
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
        let start = reader.cursor;
        let raw = reader.take(byte_count)?;
        let pixels = if bytes_per_pixel == 3 {
            let mut rgba = Vec::with_capacity(raw.len() / 3 * 4);
            for rgb in raw.chunks_exact(3) {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            Cow::Owned(rgba)
        } else if let Some(static_bytes) = borrow {
            Cow::Borrowed(&static_bytes[start..start + byte_count])
        } else {
            Cow::Owned(raw.to_vec())
        };
        assets.push(ImageAsset {
            source,
            width,
            height,
            pixels,
        });
    }

    let mut styles = HashMap::new();
    let mut nodes = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        let kind = reader.tag()?;
        let parent = reader.opt_u32()?;
        let first_child = reader.opt_u32()?;
        let next_sibling = reader.opt_u32()?;
        let id = reader.string(borrow)?;
        let action = reader.string(borrow)?;
        let text = reader.string(borrow)?;
        let image = reader.opt_u16()?;
        let style = reader.shared_style(version, &mut styles)?;
        let hover_style = reader.opt_shared_style(version, &mut styles)?;
        let active_style = reader.opt_shared_style(version, &mut styles)?;
        let focus_style = if version > FORMAT_VERSION_FOCUS_LEGACY {
            reader.opt_shared_style(version, &mut styles)?
        } else {
            None
        };
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
            focus_style,
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

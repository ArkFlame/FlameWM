//! Notification image hint validation, decoding and icon downscaling.

use dbus::arg::messageitem::MessageItem;

use super::*;

/// Image hint caps, checked from the `(iiibiiay)` header before any pixel bytes are read.
/// Rejects non-8-bit or non RGB/RGBA data, edges over `MAX_IMAGE_EDGE`, a rowstride or byte
/// count that does not match the declared size, and payloads over `MAX_IMAGE_BYTES`.
#[must_use]
pub fn image_data_within_caps(
    width: i32,
    height: i32,
    rowstride: i32,
    bits_per_sample: i32,
    channels: i32,
    byte_len: usize,
) -> bool {
    if !(1..=MAX_IMAGE_EDGE).contains(&width) || !(1..=MAX_IMAGE_EDGE).contains(&height) {
        return false;
    }
    if bits_per_sample != 8 || !matches!(channels, 3 | 4) {
        return false;
    }
    let (Ok(width), Ok(height), Ok(stride)) = (
        usize::try_from(width),
        usize::try_from(height),
        usize::try_from(rowstride),
    ) else {
        return false;
    };
    let min_stride = width * usize::try_from(channels).unwrap_or(4);
    stride >= min_stride && byte_len <= MAX_IMAGE_BYTES && byte_len == stride.saturating_mul(height)
}

/// Validates the `(iiibiiay)` header and payload type before copying any bytes, then
/// downscales. Invalid or oversized images are dropped without an error.
pub(super) fn image_data_hint(value: &MessageItem) -> Option<NotificationImage> {
    let MessageItem::Struct(fields) = value else {
        return None;
    };
    let [
        MessageItem::Int32(width),
        MessageItem::Int32(height),
        MessageItem::Int32(rowstride),
        MessageItem::Bool(_),
        MessageItem::Int32(bits),
        MessageItem::Int32(channels),
        MessageItem::Array(data),
    ] = fields.as_slice()
    else {
        return None;
    };
    if !image_data_within_caps(*width, *height, *rowstride, *bits, *channels, data.len()) {
        return None;
    }
    let bytes = data
        .iter()
        .map(|item| match item {
            MessageItem::Byte(byte) => Some(*byte),
            _ => None,
        })
        .collect::<Option<Vec<u8>>>()?;
    downscale_to_icon(
        u32::try_from(*width).ok()?,
        u32::try_from(*height).ok()?,
        usize::try_from(*channels).ok()?,
        usize::try_from(*rowstride).ok()?,
        &bytes,
    )
}

/// Nearest-neighbour downscale of a validated buffer so that neither edge exceeds
/// `NOTIFICATION_ICON_EDGE`. Output is always RGBA with alpha 255 when the source has none.
#[must_use]
pub fn downscale_to_icon(
    width: u32,
    height: u32,
    channels: usize,
    rowstride: usize,
    data: &[u8],
) -> Option<NotificationImage> {
    if width == 0 || height == 0 || !(3..=4).contains(&channels) {
        return None;
    }
    let step = width.max(height).div_ceil(NOTIFICATION_ICON_EDGE).max(1);
    let out_width = width.div_ceil(step);
    let out_height = height.div_ceil(step);
    let mut rgba = Vec::with_capacity((out_width * out_height * 4) as usize);
    for out_y in 0..out_height {
        let src_y = (out_y * step) as usize;
        for out_x in 0..out_width {
            let offset = src_y * rowstride + (out_x * step) as usize * channels;
            let pixel = data.get(offset..offset + channels)?;
            rgba.extend_from_slice(&[
                pixel[0],
                pixel[1],
                pixel[2],
                if channels == 4 { pixel[3] } else { 255 },
            ]);
        }
    }
    Some(NotificationImage {
        width: out_width,
        height: out_height,
        rgba,
    })
}

/// Decodes an absolute PNG `image-path` (plain path or `file://` URI) into a toast icon. The file
/// must be a regular file within `MAX_IMAGE_PATH_BYTES`, and its IHDR edges must fit
/// `MAX_IMAGE_EDGE`, before pixels are decoded. The result is downscaled once, as `image-data` is.
/// Anything else returns `None`, and the named-icon fallback applies.
pub(super) fn image_path_hint(reference: &str) -> Option<NotificationImage> {
    let reference = reference.trim();
    let path = match reference.strip_prefix("file://") {
        Some(rest) => percent_decode(rest)?,
        None => reference.to_owned(),
    };
    if !path.starts_with('/') || !path.to_ascii_lowercase().ends_with(".png") {
        return None;
    }
    let metadata = std::fs::metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_IMAGE_PATH_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .ok()?
        .take(MAX_IMAGE_PATH_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if u64::try_from(bytes.len()).ok()? > MAX_IMAGE_PATH_BYTES {
        return None;
    }
    // A PNG's IHDR carries big-endian width and height at offsets 16 and 20.
    let header = bytes.get(16..24)?;
    let width = u32::from_be_bytes(header[..4].try_into().ok()?);
    let height = u32::from_be_bytes(header[4..].try_into().ok()?);
    if width == 0 || height == 0 || i64::from(width.max(height)) > i64::from(MAX_IMAGE_EDGE) {
        return None;
    }
    let decoded = flamewm_image_core::png::decode(&bytes).ok()?;
    let stride = usize::try_from(decoded.width).ok()?.checked_mul(4)?;
    downscale_to_icon(decoded.width, decoded.height, 4, stride, &decoded.pixels)
}

/// Decodes `%XX` escapes of a `file://` URI path. Malformed escapes or non-UTF-8 results are
/// rejected.
pub(super) fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

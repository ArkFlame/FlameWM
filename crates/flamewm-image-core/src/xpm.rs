//! Bounded XPM version 3 decoder.

use crate::{RgbaImage, check_size_limits};

const MAX_HEADER_VALUES: u32 = 4096;
const MAX_QUOTED_LINE: usize = 65536;
const MAX_COLORS: usize = 4096;

/// Decodes XPM v3 (`/* XPM */` with quoted rows) to straight RGBA8.
///
/// `None`/`none` color entries decode to alpha 0. Only `#RRGGBB` colors
/// are accepted. Errors are deterministic strings.
pub fn decode(bytes: &[u8]) -> Result<RgbaImage, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "XPM is not valid UTF-8".to_string())?;
    if !text.contains("/* XPM */") {
        return Err("XPM missing version marker".to_string());
    }
    let mut quoted: Vec<&str> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.len() > MAX_QUOTED_LINE {
            return Err("XPM line exceeds maximum length".to_string());
        }
        if let Some(inner) = extract_quoted(line) {
            quoted.push(inner);
        }
    }
    if quoted.is_empty() {
        return Err("XPM has no quoted rows".to_string());
    }
    let header: Vec<u32> = quoted[0]
        .split_whitespace()
        .map(|part| {
            part.parse::<u32>()
                .map_err(|_| "XPM header values must be integers".to_string())
        })
        .collect::<Result<Vec<u32>, String>>()?;
    if header.len() != 4 {
        return Err("XPM header must have width height colors chars-per-pixel".to_string());
    }
    let (width, height, color_count, cpp) = (header[0], header[1], header[2], header[3]);
    for value in header {
        if value > MAX_HEADER_VALUES {
            return Err("XPM header value exceeds maximum".to_string());
        }
    }
    if width == 0 || height == 0 || color_count == 0 || cpp == 0 {
        return Err("XPM header values must be non-zero".to_string());
    }
    if cpp > 8 {
        return Err("XPM chars-per-pixel exceeds maximum".to_string());
    }
    if color_count as usize > MAX_COLORS {
        return Err("XPM color count exceeds maximum".to_string());
    }
    check_size_limits(width, height)?;
    let cpp_len = cpp as usize;
    let needed = 1 + color_count as usize + height as usize;
    if quoted.len() < needed {
        return Err("XPM is missing rows".to_string());
    }
    let mut palette: std::collections::HashMap<String, [u8; 4]> = std::collections::HashMap::new();
    for entry in &quoted[1..=(color_count as usize)] {
        if entry.len() < cpp_len {
            return Err("XPM color entry is too short".to_string());
        }
        let (key, rest) = entry.split_at(cpp_len);
        let color = parse_color_entry(rest)?;
        if palette.insert(key.to_string(), color).is_some() {
            return Err("XPM has a duplicate color key".to_string());
        }
    }
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for row in &quoted[1 + color_count as usize..needed] {
        if row.len() != width as usize * cpp_len {
            return Err("XPM row has an unexpected length".to_string());
        }
        for chunk in row.as_bytes().chunks_exact(cpp_len) {
            let key =
                std::str::from_utf8(chunk).map_err(|_| "XPM pixel key is invalid".to_string())?;
            match palette.get(key) {
                Some(color) => pixels.extend_from_slice(color),
                None => return Err("XPM pixel references an unknown color".to_string()),
            }
        }
    }
    RgbaImage::from_rgba8(width, height, pixels)
        .ok_or_else(|| "requested dimensions overflow".to_string())
}

fn extract_quoted(line: &str) -> Option<&str> {
    let start = line.find('"')?;
    let rest = &line[start + 1..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn parse_color_entry(rest: &str) -> Result<[u8; 4], String> {
    // Entries look like " c #RRGGBB" (or " s ... c #RRGGBB").
    let mut parts = rest.split_whitespace();
    let mut color: Option<&str> = None;
    while let Some(part) = parts.next() {
        if part == "c" {
            color = parts.next();
            break;
        }
    }
    let value = color.ok_or_else(|| "XPM color entry lacks a color field".to_string())?;
    if value.eq_ignore_ascii_case("none") {
        return Ok([0, 0, 0, 0]);
    }
    if value.len() == 7
        && value.as_bytes()[0] == b'#'
        && value[1..].chars().all(|c| c.is_ascii_hexdigit())
    {
        let channel = |range: std::ops::Range<usize>| {
            u8::from_str_radix(&value[range], 16)
                .map_err(|_| "XPM color value is invalid".to_string())
        };
        return Ok([channel(1..3)?, channel(3..5)?, channel(5..7)?, 255]);
    }
    Err("XPM only supports #RRGGBB and None colors".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &str = "/* XPM */\nstatic char *x[] = {\n\"2 2 2 1\",\n\"a c #FF0000\",\n\"b c None\",\n\"ab\",\n\"ba\"};\n";

    #[test]
    fn none_decodes_to_transparent() {
        let image = decode(SIMPLE.as_bytes()).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.pixels[0..4], [255, 0, 0, 255]);
        assert_eq!(image.pixels[4..8], [0, 0, 0, 0]);
    }

    #[test]
    fn malformed_inputs_are_rejected() {
        assert!(decode(b"not xpm").is_err());
        assert!(decode("/* XPM */\n\"1 1 1 1\",\n\"a c #FFF\",\n\"a\";".as_bytes()).is_err());
        assert!(decode("/* XPM */\n\"2 1 1 1\",\n\"a c #FF0000\",\n\"aaX\";".as_bytes()).is_err());
        assert!(decode("/* XPM */\n\"1 1 1 1\",\n\"a m #FF0000\",\n\"a\";".as_bytes()).is_err());
        assert!(decode("/* XPM */\n\"0 1 1 1\",\n\"a c #FF0000\",\n\"a\";".as_bytes()).is_err());
        assert!(decode("/* XPM */\n\"1 1 1 1\",\n\"a c #FF0000\",\n\"b\";".as_bytes()).is_err());
    }
}

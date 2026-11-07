use super::*;

pub(super) fn parse_length(input: &str) -> Result<Length, String> {
    let value = input.trim();
    if value == "auto" {
        return Ok(Length::Auto);
    }
    if value.starts_with("calc(") {
        let calc = eval_calc(value)?;
        return match calc.unit {
            CalcUnit::Px => Ok(Length::Px(calc.value)),
            CalcUnit::Percent => Ok(Length::Percent(calc.value / 100.0)),
            CalcUnit::Number => Err(format!("calc() length '{value}' needs px or %")),
        };
    }
    if let Some(number) = value.strip_suffix("px") {
        return Ok(Length::Px(parse_number(number)?));
    }
    if let Some(number) = value.strip_suffix('%') {
        return Ok(Length::Percent(parse_number(number)? / 100.0));
    }
    if value == "0" {
        return Ok(Length::Px(0.0));
    }
    Err(format!(
        "unsupported length '{value}'; use px, %, auto, or 0"
    ))
}

pub(super) fn parse_px(input: &str) -> Result<f32, String> {
    let value = input.trim();
    if value == "0" {
        return Ok(0.0);
    }
    if value.starts_with("calc(") {
        let calc = eval_calc(value)?;
        if calc.unit != CalcUnit::Px {
            return Err(format!("expected px length, found '{value}'"));
        }
        return Ok(calc.value);
    }
    let Some(number) = value.strip_suffix("px") else {
        return Err(format!("expected px length, found '{value}'"));
    };
    parse_number(number)
}

pub(super) fn parse_number(input: &str) -> Result<f32, String> {
    if input.trim().starts_with("calc(") {
        let calc = eval_calc(input.trim())?;
        if calc.unit != CalcUnit::Number {
            return Err(format!("expected a number, found '{}'", input.trim()));
        }
        return Ok(calc.value);
    }
    let value: f32 = input
        .trim()
        .parse()
        .map_err(|_| format!("invalid number '{}'", input.trim()))?;
    if !value.is_finite() {
        return Err("non-finite CSS numbers are not supported".to_string());
    }
    Ok(value)
}

/// Splits a value list on whitespace outside parentheses, so `calc(1px + 2px)` stays one value.
pub(super) fn split_values(input: &str) -> Vec<&str> {
    let mut values = Vec::new();
    let mut depth = 0i32;
    let mut start = None;
    for (index, ch) in input.char_indices() {
        match ch {
            c if c.is_ascii_whitespace() && depth == 0 => {
                if let Some(begin) = start.take() {
                    values.push(&input[begin..index]);
                }
            }
            _ => {
                if ch == '(' {
                    depth += 1;
                } else if ch == ')' {
                    depth -= 1;
                }
                start.get_or_insert(index);
            }
        }
    }
    if let Some(begin) = start {
        values.push(&input[begin..]);
    }
    values
}

pub(super) fn parse_flex_grow(input: &str) -> Result<f32, String> {
    if input.split_ascii_whitespace().count() != 1 {
        return Err(format!(
            "flex shorthand '{input}' is not supported; use flex-grow"
        ));
    }
    Ok(parse_number(input)?.max(0.0))
}

pub(super) fn parse_signed_px(input: &str) -> Result<f32, String> {
    let value = input
        .trim()
        .strip_suffix("px")
        .ok_or_else(|| format!("text-shadow length '{input}' requires px"))?;
    let parsed: f32 = value
        .trim()
        .parse()
        .map_err(|_| format!("invalid text-shadow offset '{input}'"))?;
    if !parsed.is_finite() || parsed.abs() > 16.0 {
        return Err(format!("text-shadow offset '{input}' is outside -16..16px"));
    }
    Ok(parsed)
}

pub(super) fn parse_font_weight(input: &str) -> Result<u16, String> {
    match input.trim() {
        "normal" => Ok(400),
        "bold" => Ok(700),
        value => {
            let parsed: u16 = value
                .parse()
                .map_err(|_| format!("invalid font-weight '{value}'"))?;
            if !(1..=1000).contains(&parsed) {
                return Err(format!("font-weight '{value}' is outside 1..=1000"));
            }
            Ok(parsed)
        }
    }
}

pub(super) fn parse_color_value(
    input: &str,
    variables: &HashMap<String, u16>,
) -> Result<ColorValue, String> {
    let value = input.trim();
    if let Some(inner) = value
        .strip_prefix("var(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let (name, fallback) = match inner.split_once(',') {
            Some((name, fallback)) => (name.trim(), Some(fallback.trim())),
            None => (inner.trim(), None),
        };
        match (variables.get(name).copied(), fallback) {
            (Some(index), _) => return Ok(ColorValue::Variable(index)),
            (None, Some(fallback)) => {
                return parse_color_literal(fallback)
                    .map(ColorValue::Literal)
                    .ok_or_else(|| format!("unsupported var() fallback '{fallback}'"));
            }
            (None, None) => return Err(format!("unknown CSS color variable '{name}'")),
        }
    }
    parse_color_literal(value)
        .map(ColorValue::Literal)
        .ok_or_else(|| format!("unsupported color '{value}'"))
}

pub fn parse_color_literal(input: &str) -> Option<Color> {
    let value = input.trim().to_ascii_lowercase();
    match value.as_str() {
        "transparent" => return Some(Color::TRANSPARENT),
        "black" => return Some(Color::BLACK),
        "white" => return Some(Color::WHITE),
        "red" => return Some(Color::rgb(255, 0, 0)),
        "gray" | "grey" => return Some(Color::rgb(128, 128, 128)),
        _ => {}
    }
    if let Some(hex) = value.strip_prefix('#') {
        // #rgb[a] digits each expand to one byte; #rrggbb[aa] reads two per channel.
        let (width, channels) = match hex.len() {
            3 => (1, 3),
            4 => (1, 4),
            6 => (2, 3),
            8 => (2, 4),
            _ => return None,
        };
        let channel = |index: usize| hex_channel(hex, index, width);
        let a = if channels == 4 { channel(3)? } else { 255 };
        return Some(Color {
            r: channel(0)?,
            g: channel(1)?,
            b: channel(2)?,
            a,
        });
    }
    if let Some(inner) = value
        .strip_prefix("rgb(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let values: Vec<u8> = inner
            .split(',')
            .map(|part| part.trim().parse::<u8>().ok())
            .collect::<Option<Vec<_>>>()?;
        if values.len() == 3 {
            return Some(Color::rgb(values[0], values[1], values[2]));
        }
    }
    if let Some(inner) = value
        .strip_prefix("rgba(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() == 4 {
            let r = parts[0].parse::<u8>().ok()?;
            let g = parts[1].parse::<u8>().ok()?;
            let b = parts[2].parse::<u8>().ok()?;
            let alpha = parts[3].parse::<f32>().ok()?;
            if alpha.is_finite() {
                return Some(Color {
                    r,
                    g,
                    b,
                    a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
                });
            }
        }
    }
    None
}

/// One hex channel at `index`: `width` 1 expands a digit (`a` -> `aa`), 2 reads a byte.
/// `get` rejects non-char-boundary slices instead of panicking on multi-byte input.
pub(super) fn hex_channel(hex: &str, index: usize, width: usize) -> Option<u8> {
    let digits = hex.get(index * width..(index + 1) * width)?;
    let value = u8::from_str_radix(digits, 16).ok()?;
    Some(if width == 1 {
        (value << 4) | value
    } else {
        value
    })
}

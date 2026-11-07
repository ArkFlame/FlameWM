use super::*;

/// `inset` shorthand: one to four lengths for top, right, bottom, left.
pub(super) fn parse_inset(input: &str) -> Result<[Length; 4], String> {
    let values = split_values(input)
        .into_iter()
        .map(parse_length)
        .collect::<Result<Vec<_>, _>>()?;
    match values.as_slice() {
        [all] => Ok([*all; 4]),
        [vertical, horizontal] => Ok([*vertical, *horizontal, *vertical, *horizontal]),
        [top, horizontal, bottom] => Ok([*top, *horizontal, *bottom, *horizontal]),
        [top, right, bottom, left] => Ok([*top, *right, *bottom, *left]),
        _ => Err(format!("invalid inset shorthand '{input}'")),
    }
}

pub(super) fn parse_edges(input: &str) -> Result<Edges, String> {
    let values: Vec<f32> = split_values(input)
        .into_iter()
        .map(parse_px)
        .collect::<Result<_, _>>()?;
    match values.as_slice() {
        [all] => Ok(Edges {
            top: *all,
            right: *all,
            bottom: *all,
            left: *all,
        }),
        [vertical, horizontal] => Ok(Edges {
            top: *vertical,
            right: *horizontal,
            bottom: *vertical,
            left: *horizontal,
        }),
        [top, horizontal, bottom] => Ok(Edges {
            top: *top,
            right: *horizontal,
            bottom: *bottom,
            left: *horizontal,
        }),
        [top, right, bottom, left] => Ok(Edges {
            top: *top,
            right: *right,
            bottom: *bottom,
            left: *left,
        }),
        _ => Err(format!("invalid edge shorthand '{input}'")),
    }
}
pub(super) fn apply_border(
    style: &mut Style,
    input: &str,
    variables: &HashMap<String, u16>,
) -> Result<(), String> {
    let input = input.trim();
    // Reset shorthands are common in app stylesheets. A width of zero already
    // disables the canonical uniform border painter, so no new paint path is needed.
    if matches!(input, "none" | "0" | "0px") {
        style.border_width = 0.0;
        // A shorthand resets the preceding border color as well as its width.
        style.border_color = ColorValue::Literal(Color::TRANSPARENT);
        return Ok(());
    }
    let mut parts = input.split_ascii_whitespace();
    let (Some(width), Some("solid"), Some(color), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(format!(
            "border shorthand '{input}' is unsupported; expected 'none', '0', or '<px> solid <color>'"
        ));
    };
    let width = parse_px(width)?;
    let color = parse_color_value(color, variables)?;
    style.border_width = width;
    style.border_color = color;
    Ok(())
}

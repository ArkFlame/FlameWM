use super::*;

pub(super) const TEXT_SHADOW_TOKEN: &str = "--text-shadow";
pub(super) const DEFAULT_TEXT_SHADOW: &str = "0px 1px 2px #00000059";

pub(super) fn root_text_shadow(
    rules: &[Rule],
    variables: &HashMap<String, u16>,
) -> Result<Option<TextShadow>, String> {
    let mut token = DEFAULT_TEXT_SHADOW;
    for rule in rules
        .iter()
        .filter(|rule| rule.selector.state == PseudoState::Root)
    {
        for declaration in rule
            .declarations
            .iter()
            .filter(|d| d.name == TEXT_SHADOW_TOKEN)
        {
            token = declaration.value.as_str();
        }
    }
    parse_text_shadow(token, variables)
}

pub(super) fn parse_text_shadow(
    value: &str,
    variables: &HashMap<String, u16>,
) -> Result<Option<TextShadow>, String> {
    if value == "none" {
        return Ok(None);
    }
    let parts: Vec<&str> = value.split_ascii_whitespace().collect();
    if !(3..=4).contains(&parts.len()) {
        return Err(format!(
            "text-shadow '{value}' expects '<x px> <y px> [<blur px>] <color>'"
        ));
    }
    let blur_radius = if parts.len() == 4 {
        let blur = parse_signed_px(parts[2])?;
        if !(0.0..=4.0).contains(&blur) {
            return Err(format!("text-shadow blur '{blur}' is outside 0..4px"));
        }
        blur
    } else {
        0.0
    };
    Ok(Some(TextShadow {
        offset_x: parse_signed_px(parts[0])?,
        offset_y: parse_signed_px(parts[1])?,
        blur_radius,
        color: parse_color_value(parts[parts.len() - 1], variables)?,
    }))
}

/// Built-in accent tokens, seeded before any sheet `:root` so a sheet can override them by name.
/// The accent tints are derived once here instead of repeated as literals in every sheet. Live
/// Settings overwrite `--accent` at runtime; the tints keep their default alpha steps.
pub(super) fn builtin_color_tokens() -> [(&'static str, Color); 10] {
    let accent = flamewm_api::settings::DEFAULT_ACCENT_RGB;
    let tint = |alpha: u8| Color {
        r: (accent >> 16) as u8,
        g: (accent >> 8) as u8,
        b: accent as u8,
        a: alpha,
    };
    [
        ("--accent", tint(0xff)),
        ("--accent-soft", tint(0x38)),
        ("--accent-strong", tint(0xdd)),
        ("--preview-outline", tint(0xff)),
        ("--snap-fill", tint(0x33)),
        ("--desktop-hover", tint(0x26)),
        ("--desktop-hover-border", tint(0x4d)),
        ("--desktop-selected", tint(0x47)),
        ("--desktop-selected-border", tint(0xa6)),
        ("--focus-ring", tint(0xff)),
    ]
}

/// Custom properties split by kind: colors are runtime-overridable indices,
/// lengths are resolved to their literal text at compile time.
pub(super) type CustomProperties = (
    Vec<ColorVariable>,
    HashMap<String, u16>,
    HashMap<String, String>,
);

pub(super) fn collect_variables(rules: &[Rule]) -> Result<CustomProperties, String> {
    let mut values: BTreeMap<String, Color> = builtin_color_tokens()
        .into_iter()
        .map(|(name, color)| (name.to_string(), color))
        .collect();
    let mut lengths = HashMap::new();
    for rule in rules {
        if rule.selector.state != PseudoState::Root {
            continue;
        }
        for declaration in &rule.declarations {
            if declaration.name == TEXT_SHADOW_TOKEN {
                continue;
            }
            if !declaration.name.starts_with("--") {
                return Err(format!("FlameWM Render 0.0.9 reserves :root for typed custom properties; unsupported declaration '{}: {}'", declaration.name, declaration.value));
            }
            if let Some(color) = parse_color_literal(&declaration.value) {
                values.insert(declaration.name.clone(), color);
            } else if parse_length(&declaration.value).is_ok() {
                lengths.insert(
                    declaration.name.clone(),
                    declaration.value.trim().to_string(),
                );
            } else {
                return Err(format!(
                    "custom property '{}' must be a literal color or length in FlameWM Render 0.0.9",
                    declaration.name
                ));
            }
        }
    }
    let variables: Vec<ColorVariable> = values
        .into_iter()
        .map(|(name, default)| ColorVariable {
            name: name.into(),
            default,
        })
        .collect();
    if variables.len() > u16::MAX as usize {
        return Err("too many CSS custom properties".to_string());
    }
    let variable_map = variables
        .iter()
        .enumerate()
        .map(|(index, variable)| (variable.name.to_string(), index as u16))
        .collect();
    Ok((variables, variable_map, lengths))
}

/// Replaces `var(--name[, fallback])` in a length-valued declaration with the
/// token's literal text, or the fallback when the token is not a length.
pub(super) fn resolve_length_vars(
    input: &str,
    lengths: &HashMap<String, String>,
) -> Result<String, String> {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("var(") {
        output.push_str(&rest[..start]);
        let after = &rest[start + "var(".len()..];
        let end = after
            .find(')')
            .ok_or_else(|| format!("unterminated var() in '{input}'"))?;
        let (name, fallback) = match after[..end].split_once(',') {
            Some((name, fallback)) => (name.trim(), Some(fallback.trim())),
            None => (after[..end].trim(), None),
        };
        match (lengths.get(name), fallback) {
            (Some(value), _) => output.push_str(value),
            (None, Some(fallback)) => output.push_str(fallback),
            (None, None) => return Err(format!("unknown CSS length variable '{name}'")),
        }
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

/// Properties whose values are lengths, so `var()` resolves through `lengths`.
pub(super) fn takes_length_var(name: &str) -> bool {
    matches!(
        name,
        "inset"
            | "width"
            | "height"
            | "min-width"
            | "min-height"
            | "max-width"
            | "max-height"
            | "top"
            | "right"
            | "bottom"
            | "left"
            | "margin"
            | "margin-top"
            | "margin-right"
            | "margin-bottom"
            | "margin-left"
            | "padding"
            | "padding-top"
            | "padding-right"
            | "padding-bottom"
            | "padding-left"
            | "gap"
            | "font-size"
            | "border-width"
            | "border-radius"
    )
}

use super::*;

pub(super) fn should_skip_tag(tag: &str) -> bool {
    matches!(tag, "head" | "style" | "link" | "meta" | "title" | "script")
}

pub(super) fn default_style_for_tag(tag: &str, inherited: &Style) -> Style {
    let mut style = Style::default();
    inherit_text_style(&mut style, inherited);
    match tag {
        "body" | "html" => {
            style.width = Length::Percent(1.0);
            style.height = Length::Percent(1.0);
        }
        "button" => {
            style.padding = Edges {
                top: 6.0,
                right: 10.0,
                bottom: 6.0,
                left: 10.0,
            };
            style.background = ColorValue::Literal(Color::rgb(238, 238, 238));
            style.border_color = ColorValue::Literal(Color::rgb(170, 170, 170));
            style.border_width = 1.0;
        }
        _ => {}
    }
    style
}

pub(super) fn inherit_text_style(target: &mut Style, inherited: &Style) {
    target.color = inherited.color;
    target.font_size = inherited.font_size;
    target.font_weight = inherited.font_weight;
    target.text_shadow = inherited.text_shadow;
    target.text_wrap = inherited.text_wrap;
    target.break_anywhere = inherited.break_anywhere;
    target.text_overflow = inherited.text_overflow;
    target.opacity = inherited.opacity;
}

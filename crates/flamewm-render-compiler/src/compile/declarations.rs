use super::*;

impl<'a> Compiler<'a> {
    pub(super) fn apply_declaration(
        &mut self,
        style: &mut Style,
        declaration: &Declaration,
    ) -> Result<(), String> {
        if declaration.name.starts_with("--") {
            return Ok(());
        }
        if declaration.value.contains("!important") {
            return self.unsupported(format!(
                "!important is not supported: {}: {}",
                declaration.name, declaration.value
            ));
        }
        let resolved = if takes_length_var(&declaration.name) && declaration.value.contains("var(")
        {
            resolve_length_vars(declaration.value.trim(), self.length_map)?
        } else {
            declaration.value.trim().to_string()
        };
        let value = resolved.as_str();
        match declaration.name.as_str() {
            "display" => {
                style.display = match value {
                    "none" => Display::None,
                    "block" => Display::Block,
                    "flex" => Display::Flex,
                    other => return self.unsupported(format!("display: {other}")),
                }
            }
            "flex-direction" => {
                style.flex_direction = match value {
                    "row" => FlexDirection::Row,
                    "column" => FlexDirection::Column,
                    other => return self.unsupported(format!("flex-direction: {other}")),
                }
            }
            "justify-content" => {
                style.justify_content = match value {
                    "flex-start" | "start" => JustifyContent::Start,
                    "center" => JustifyContent::Center,
                    "flex-end" | "end" => JustifyContent::End,
                    "space-between" => JustifyContent::SpaceBetween,
                    other => return self.unsupported(format!("justify-content: {other}")),
                }
            }
            "align-items" => {
                style.align_items = match value {
                    "flex-start" | "start" => AlignItems::Start,
                    "center" => AlignItems::Center,
                    "flex-end" | "end" => AlignItems::End,
                    "stretch" => AlignItems::Stretch,
                    other => return self.unsupported(format!("align-items: {other}")),
                }
            }
            "position" => {
                style.position = match value {
                    "static" | "relative" => Position::Flow,
                    "absolute" => Position::Absolute,
                    other => return self.unsupported(format!("position: {other}")),
                }
            }
            "width" => style.width = parse_length(value)?,
            "height" => style.height = parse_length(value)?,
            "min-width" => style.min_width = parse_length(value)?,
            "min-height" => style.min_height = parse_length(value)?,
            "max-width" => style.max_width = parse_length(value)?,
            "max-height" => style.max_height = parse_length(value)?,
            "inset" => {
                let [top, right, bottom, left] = parse_inset(value)?;
                style.top = top;
                style.right = right;
                style.bottom = bottom;
                style.left = left;
            }
            "top" => style.top = parse_length(value)?,
            "right" => style.right = parse_length(value)?,
            "bottom" => style.bottom = parse_length(value)?,
            "left" => style.left = parse_length(value)?,
            "margin" => style.margin = parse_edges(value)?,
            "margin-top" => style.margin.top = parse_px(value)?,
            "margin-right" => style.margin.right = parse_px(value)?,
            "margin-bottom" => style.margin.bottom = parse_px(value)?,
            "margin-left" => style.margin.left = parse_px(value)?,
            "padding" => style.padding = parse_edges(value)?,
            "padding-top" => style.padding.top = parse_px(value)?,
            "padding-right" => style.padding.right = parse_px(value)?,
            "padding-bottom" => style.padding.bottom = parse_px(value)?,
            "padding-left" => style.padding.left = parse_px(value)?,
            "gap" => style.gap = parse_px(value)?,
            "flex-grow" => style.flex_grow = parse_number(value)?.max(0.0),
            "flex" => style.flex_grow = parse_flex_grow(value)?,
            "background" | "background-color" => {
                style.background = parse_color_value(value, self.variable_map)?
            }
            "color" => style.color = parse_color_value(value, self.variable_map)?,
            "border-color" => style.border_color = parse_color_value(value, self.variable_map)?,
            "border-width" => style.border_width = parse_px(value)?,
            "border-radius" => style.border_radius = parse_px(value)?,
            "border" => apply_border(style, value, self.variable_map)?,
            "font-size" => style.font_size = parse_px(value)?.max(1.0),
            "font-weight" => style.font_weight = parse_font_weight(value)?,
            "text-shadow" => style.text_shadow = parse_text_shadow(value, self.variable_map)?,
            "opacity" => style.opacity = parse_number(value)?.clamp(0.0, 1.0),
            "cursor" => {
                style.cursor = match value {
                    "default" | "auto" => CursorKind::Default,
                    "pointer" => CursorKind::Pointer,
                    "text" => CursorKind::Text,
                    "move" | "grab" | "grabbing" => CursorKind::Move,
                    "ew-resize" | "e-resize" | "w-resize" | "col-resize" => {
                        CursorKind::ResizeHorizontal
                    }
                    "ns-resize" | "n-resize" | "s-resize" | "row-resize" => {
                        CursorKind::ResizeVertical
                    }
                    "nwse-resize" | "nw-resize" | "se-resize" => {
                        CursorKind::ResizeNorthWestSouthEast
                    }
                    "nesw-resize" | "ne-resize" | "sw-resize" => {
                        CursorKind::ResizeNorthEastSouthWest
                    }
                    other => return self.unsupported(format!("cursor: {other}")),
                }
            }
            "white-space" => match value {
                "normal" => {
                    style.text_wrap = TextWrap::Wrap;
                }
                "nowrap" => {
                    style.text_wrap = TextWrap::NoWrap;
                }
                other => return self.unsupported(format!("white-space: {other}")),
            },
            "text-overflow" => match value {
                "clip" => style.text_overflow = TextOverflow::Clip,
                "ellipsis" => style.text_overflow = TextOverflow::Ellipsis,
                other => return self.unsupported(format!("text-overflow: {other}")),
            },
            "overflow-wrap" | "word-wrap" => match value {
                "normal" => {
                    style.break_anywhere = false;
                }
                "anywhere" | "break-word" => {
                    style.break_anywhere = true;
                    style.text_wrap = TextWrap::Wrap;
                }
                other => return self.unsupported(format!("overflow-wrap: {other}")),
            },
            "box-sizing" if value == "border-box" => {}
            "overflow" | "overflow-x" | "overflow-y" => {
                let overflow = match value {
                    "visible" => Overflow::Visible,
                    "hidden" => Overflow::Hidden,
                    "auto" => Overflow::Auto,
                    "scroll" => Overflow::Scroll,
                    other => return self.unsupported(format!("overflow: {other}")),
                };
                match declaration.name.as_str() {
                    "overflow" => {
                        style.overflow_x = overflow;
                        style.overflow_y = overflow;
                    }
                    "overflow-x" => style.overflow_x = overflow,
                    _ => style.overflow_y = overflow,
                }
            }
            "scrollbar-width" if matches!(value, "none" | "auto") => {
                style.hide_scrollbars = value == "none";
            }
            "user-select" if matches!(value, "none" | "text" | "auto") => {}
            "-flamewm-image-treatment" => {
                style.image_treatment = match value {
                    "original" => ImageTreatment::Original,
                    "symbolic" => ImageTreatment::SymbolicForeground,
                    other => return self.unsupported(format!("-flamewm-image-treatment: {other}")),
                };
            }
            property => return self.unsupported(format!("unsupported CSS property '{property}'")),
        }
        Ok(())
    }

    pub(super) fn unsupported(&mut self, message: String) -> Result<(), String> {
        if self.options.strict {
            Err(message)
        } else {
            self.warnings.push(message);
            Ok(())
        }
    }
}

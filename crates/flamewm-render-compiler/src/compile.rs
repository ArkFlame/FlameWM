use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_render_core::{
    fnv1a64, AlignItems, Color, ColorValue, ColorVariable, CompiledDocument, CompiledNode,
    CursorKind, Display, Edges, FlexDirection, ImageAsset, ImageTreatment, JustifyContent, Length,
    NodeKind, Overflow, Position, Style, TextShadow, TextWrap,
};

use crate::css::{parse_declarations, parse_stylesheet, Declaration, PseudoState, Rule};
use crate::html::{parse_html, HtmlNodeKind, ParsedHtml};

#[derive(Clone, Copy, Debug)]
pub struct CompileOptions {
    pub strict: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self { strict: true }
    }
}

#[derive(Clone, Debug)]
pub struct CompileOutput {
    pub document: CompiledDocument,
    pub warnings: Vec<String>,
}

pub fn compile_str(
    html_source: &str,
    extra_css: &str,
    options: CompileOptions,
) -> Result<CompileOutput, String> {
    let html = parse_html(html_source)?;
    if !html.linked_css.is_empty() {
        return Err("compile_str cannot resolve <link rel=\"stylesheet\">; use compile_file or pass CSS explicitly".to_string());
    }
    let css = format!("{}\n{extra_css}", html.embedded_css);
    compile_parsed(html_source, &css, html, options, None)
}

pub fn compile_file(
    html_path: &Path,
    extra_css_paths: &[PathBuf],
    options: CompileOptions,
) -> Result<CompileOutput, String> {
    let html_source = fs::read_to_string(html_path)
        .map_err(|error| format!("failed to read {}: {error}", html_path.display()))?;
    let html = parse_html(&html_source)?;
    let base = html_path.parent().unwrap_or_else(|| Path::new("."));
    let mut css = html.embedded_css.clone();
    let mut sheets = Vec::with_capacity(html.linked_css.len() + extra_css_paths.len());
    for href in &html.linked_css {
        if href.contains("://") || href.starts_with("data:") {
            return Err(format!("runtime/network stylesheet URL '{href}' is forbidden; FlameWM Render stylesheets must be local build inputs"));
        }
        sheets.push(base.join(href));
    }
    sheets.extend_from_slice(extra_css_paths);
    for path in &sheets {
        css.push('\n');
        css.push_str(
            &fs::read_to_string(path).map_err(|error| {
                format!("failed to read stylesheet {}: {error}", path.display())
            })?,
        );
    }
    compile_parsed(&html_source, &css, html, options, Some(base))
}

fn compile_parsed(
    html_source: &str,
    css_source: &str,
    html: ParsedHtml,
    options: CompileOptions,
    asset_base: Option<&Path>,
) -> Result<CompileOutput, String> {
    let rules = parse_stylesheet(css_source)?;
    let (variables, variable_map) = collect_variables(&rules)?;
    let mut fingerprint_input = Vec::with_capacity(html_source.len() + css_source.len() + 1);
    fingerprint_input.extend_from_slice(html_source.as_bytes());
    fingerprint_input.push(0);
    fingerprint_input.extend_from_slice(css_source.as_bytes());

    let mut context = Compiler {
        html: &html,
        rules: &rules,
        variable_map: &variable_map,
        options,
        asset_base,
        warnings: Vec::new(),
        assets: Vec::new(),
        asset_index: HashMap::new(),
        nodes: Vec::new(),
    };

    let inherited = Style::default();
    let root = context.compile_subtree(html.root, None, &inherited)?;
    for asset in &context.assets {
        fingerprint_input.push(0xff);
        fingerprint_input.extend_from_slice(asset.source.as_bytes());
        fingerprint_input.extend_from_slice(&asset.width.to_le_bytes());
        fingerprint_input.extend_from_slice(&asset.height.to_le_bytes());
        fingerprint_input.extend_from_slice(&asset.pixels);
    }
    let document = CompiledDocument {
        source_fingerprint: fnv1a64(&fingerprint_input),
        root,
        variables,
        assets: context.assets,
        nodes: context.nodes,
    };
    document.validate()?;
    Ok(CompileOutput {
        document,
        warnings: context.warnings,
    })
}

fn collect_variables(rules: &[Rule]) -> Result<(Vec<ColorVariable>, HashMap<String, u16>), String> {
    let mut values: BTreeMap<String, Color> = BTreeMap::new();
    for rule in rules {
        if rule.selector.state != PseudoState::Root {
            continue;
        }
        for declaration in &rule.declarations {
            if !declaration.name.starts_with("--") {
                return Err(format!("FlameWM Render 0.0.9 reserves :root for typed custom color properties; unsupported declaration '{}: {}'", declaration.name, declaration.value));
            }
            let color = parse_color_literal(&declaration.value).ok_or_else(|| {
                format!(
                    "custom property '{}' must be a literal color in FlameWM Render 0.0.9",
                    declaration.name
                )
            })?;
            values.insert(declaration.name.clone(), color);
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
    Ok((variables, variable_map))
}

struct Compiler<'a> {
    html: &'a ParsedHtml,
    rules: &'a [Rule],
    variable_map: &'a HashMap<String, u16>,
    options: CompileOptions,
    asset_base: Option<&'a Path>,
    warnings: Vec<String>,
    assets: Vec<ImageAsset>,
    asset_index: HashMap<String, u16>,
    nodes: Vec<CompiledNode>,
}

impl<'a> Compiler<'a> {
    fn compile_subtree(
        &mut self,
        source_index: usize,
        parent: Option<u32>,
        inherited: &Style,
    ) -> Result<u32, String> {
        let source = &self.html.nodes[source_index];
        let node_index = self.nodes.len() as u32;

        if source.kind == HtmlNodeKind::Text {
            let mut style = Style::default();
            inherit_text_style(&mut style, inherited);
            self.nodes.push(CompiledNode {
                kind: NodeKind::Text,
                parent,
                first_child: None,
                next_sibling: None,
                id: std::borrow::Cow::Borrowed(""),
                action: std::borrow::Cow::Borrowed(""),
                text: source.text.clone().into(),
                image: None,
                style: style.into(),
                hover_style: None,
                active_style: None,
            });
            return Ok(node_index);
        }

        let base = self.compute_style(source_index, inherited, PseudoState::Base)?;
        let hover = self.compute_style(source_index, inherited, PseudoState::Hover)?;
        let active = self.compute_style(source_index, inherited, PseudoState::Active)?;
        let hover_style = (hover != base).then_some(hover);
        let active_style = (active != base).then_some(active);
        let id = source.attrs.get("id").cloned().unwrap_or_default();
        let action = source.attrs.get("data-action").cloned().unwrap_or_default();
        let (kind, image) = if source.tag == "img" {
            let src = source
                .attrs
                .get("src")
                .ok_or_else(|| "<img> requires src".to_string())?;
            (NodeKind::Image, Some(self.load_image(src)?))
        } else {
            (NodeKind::Element, None)
        };

        self.nodes.push(CompiledNode {
            kind,
            parent,
            first_child: None,
            next_sibling: None,
            id: id.into(),
            action: action.into(),
            text: std::borrow::Cow::Borrowed(""),
            image,
            style: base.clone().into(),
            hover_style: hover_style.map(Into::into),
            active_style: active_style.map(Into::into),
        });

        let mut first_child = None;
        let mut previous_child: Option<u32> = None;
        for child_source in &source.children {
            if kind == NodeKind::Image {
                break;
            }
            if should_skip_tag(&self.html.nodes[*child_source].tag) {
                continue;
            }
            let child = self.compile_subtree(*child_source, Some(node_index), &base)?;
            if first_child.is_none() {
                first_child = Some(child);
            }
            if let Some(previous) = previous_child {
                self.nodes[previous as usize].next_sibling = Some(child);
            }
            previous_child = Some(child);
        }
        self.nodes[node_index as usize].first_child = first_child;
        Ok(node_index)
    }

    fn load_image(&mut self, src: &str) -> Result<u16, String> {
        if src.contains("://") || src.starts_with("data:") {
            return Err(format!(
                "runtime/network image URL '{src}' is forbidden; images must be local build inputs"
            ));
        }
        if let Some(index) = self.asset_index.get(src).copied() {
            return Ok(index);
        }
        let base = self.asset_base.ok_or_else(|| {
            format!(
                "compile_str cannot resolve image '{src}'; use compile_file for local image assets"
            )
        })?;
        let path = base.join(src);
        let bytes = fs::read(&path)
            .map_err(|error| format!("failed to read image {}: {error}", path.display()))?;
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let image = match extension.as_str() {
            "ppm" => flamewm_image_core::ppm::decode(&bytes),
            "svg" => {
                let (width, height) =
                    flamewm_image_core::svg::natural_size(&bytes).map_err(|error| {
                        format!("failed to decode image {}: {error}", path.display())
                    })?;
                if flamewm_image_core::svg::uses_current_color_scheme(&bytes) {
                    flamewm_image_core::svg::render_with_color_scheme(
                        &bytes,
                        width,
                        height,
                        &flamewm_image_core::SvgColorScheme::flame_default(),
                    )
                } else {
                    flamewm_image_core::svg::render(&bytes, width, height)
                }
            }
            "png" => flamewm_image_core::png::decode(&bytes),
            _ => {
                return Err(format!(
                    "unsupported image format for {} (expected ppm, svg, or png)",
                    path.display()
                ));
            }
        }
        .map_err(|error| format!("failed to decode image {}: {error}", path.display()))?;
        // Opaque PPM build assets arrive with alpha=255 from image-core;
        // SVG/PNG preserve alpha. The RWRB v3 asset contract is 4 bytes/px.
        if self.assets.len() >= u16::MAX as usize {
            return Err("too many image assets".to_string());
        }
        let index = self.assets.len() as u16;
        self.assets.push(ImageAsset {
            source: src.to_string().into(),
            width: image.width,
            height: image.height,
            pixels: image.pixels.into(),
        });
        self.asset_index.insert(src.to_string(), index);
        Ok(index)
    }

    fn compute_style(
        &mut self,
        source_index: usize,
        inherited: &Style,
        state: PseudoState,
    ) -> Result<Style, String> {
        let source = &self.html.nodes[source_index];
        let mut style = default_style_for_tag(&source.tag, inherited);
        let mut matches: Vec<&Rule> = self
            .rules
            .iter()
            .filter(|rule| {
                rule.selector.state != PseudoState::Root
                    && (rule.selector.state == PseudoState::Base || rule.selector.state == state)
                    && rule.selector.matches(self.html, source_index)
            })
            .collect();
        matches.sort_by_key(|rule| (rule.selector.specificity, rule.order));
        for rule in matches {
            for declaration in &rule.declarations {
                self.apply_declaration(&mut style, declaration)?;
            }
        }
        if let Some(inline) = source.attrs.get("style") {
            for declaration in parse_declarations(inline)? {
                self.apply_declaration(&mut style, &declaration)?;
            }
        }
        Ok(style)
    }

    fn apply_declaration(
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
        let value = declaration.value.trim();
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
            "text-shadow" => {
                if value == "none" {
                    style.text_shadow = None;
                } else {
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
                    style.text_shadow = Some(TextShadow {
                        offset_x: parse_signed_px(parts[0])?,
                        offset_y: parse_signed_px(parts[1])?,
                        blur_radius,
                        color: parse_color_value(parts[parts.len() - 1], self.variable_map)?,
                    });
                }
            }
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

    fn unsupported(&mut self, message: String) -> Result<(), String> {
        if self.options.strict {
            Err(message)
        } else {
            self.warnings.push(message);
            Ok(())
        }
    }
}

fn should_skip_tag(tag: &str) -> bool {
    matches!(tag, "head" | "style" | "link" | "meta" | "title" | "script")
}

fn default_style_for_tag(tag: &str, inherited: &Style) -> Style {
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

fn inherit_text_style(target: &mut Style, inherited: &Style) {
    target.color = inherited.color;
    target.font_size = inherited.font_size;
    target.font_weight = inherited.font_weight;
    target.text_shadow = inherited.text_shadow;
    target.text_wrap = inherited.text_wrap;
    target.break_anywhere = inherited.break_anywhere;
    target.opacity = inherited.opacity;
}

fn parse_length(input: &str) -> Result<Length, String> {
    let value = input.trim();
    if value == "auto" {
        return Ok(Length::Auto);
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

fn parse_px(input: &str) -> Result<f32, String> {
    let value = input.trim();
    if value == "0" {
        return Ok(0.0);
    }
    let Some(number) = value.strip_suffix("px") else {
        return Err(format!("expected px length, found '{value}'"));
    };
    parse_number(number)
}

fn parse_number(input: &str) -> Result<f32, String> {
    let value: f32 = input
        .trim()
        .parse()
        .map_err(|_| format!("invalid number '{}'", input.trim()))?;
    if !value.is_finite() {
        return Err("non-finite CSS numbers are not supported".to_string());
    }
    Ok(value)
}

fn parse_edges(input: &str) -> Result<Edges, String> {
    let values: Vec<f32> = input
        .split_ascii_whitespace()
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

fn parse_flex_grow(input: &str) -> Result<f32, String> {
    if input.split_ascii_whitespace().count() != 1 {
        return Err(format!(
            "flex shorthand '{input}' is not supported; use flex-grow"
        ));
    }
    Ok(parse_number(input)?.max(0.0))
}

fn parse_signed_px(input: &str) -> Result<f32, String> {
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

fn parse_font_weight(input: &str) -> Result<u16, String> {
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

fn parse_color_value(input: &str, variables: &HashMap<String, u16>) -> Result<ColorValue, String> {
    let value = input.trim();
    if let Some(inner) = value
        .strip_prefix("var(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let name = inner.trim();
        let index = variables
            .get(name)
            .copied()
            .ok_or_else(|| format!("unknown CSS color variable '{name}'"))?;
        return Ok(ColorValue::Variable(index));
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
fn hex_channel(hex: &str, index: usize, width: usize) -> Option<u8> {
    let digits = hex.get(index * width..(index + 1) * width)?;
    let value = u8::from_str_radix(digits, 16).ok()?;
    Some(if width == 1 {
        (value << 4) | value
    } else {
        value
    })
}

fn apply_border(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_color_hex_and_variable_compile() {
        assert_eq!(
            parse_color_literal("#ef4444"),
            Some(Color::rgb(239, 68, 68))
        );
    }

    fn compile_body(html: &str) -> CompileOutput {
        compile_str(html, "", CompileOptions::default()).unwrap()
    }

    fn style_of(output: &CompileOutput, id: &str) -> Style {
        output
            .document
            .nodes
            .iter()
            .find(|node| node.id == id)
            .unwrap()
            .style
            .as_ref()
            .clone()
    }

    #[test]
    fn css_border_reset_shorthands_preserve_uniform_painter_semantics() {
        let html = r#"<body><style>
            .base { border: 2px solid #ef4444; }
            .none { border: 2px solid #ef4444; border: none; }
            .zero { border: 2px solid #ef4444; border: 0; }
            .pixel-zero { border: 2px solid #ef4444; border: 0px; }
            .restore { border: none; border: 1px solid #ffffff40; }
        </style>
        <div id="base" class="base">b</div>
        <div id="none" class="none">n</div>
        <div id="zero" class="zero">z</div>
        <div id="pixel-zero" class="pixel-zero">p</div>
        <div id="restore" class="restore">r</div></body>"#;
        let output = compile_body(html);
        assert_eq!(style_of(&output, "base").border_width, 2.0);
        for id in ["none", "zero", "pixel-zero"] {
            assert_eq!(style_of(&output, id).border_width, 0.0, "{id}");
            assert_eq!(
                style_of(&output, id).border_color,
                ColorValue::Literal(Color::TRANSPARENT)
            );
        }
        assert_eq!(style_of(&output, "restore").border_width, 1.0);
        assert_eq!(
            style_of(&output, "restore").border_color,
            ColorValue::Literal(Color {
                r: 255,
                g: 255,
                b: 255,
                a: 64
            })
        );
        let encoded = flamewm_render_core::encode(&output.document).unwrap();
        assert_eq!(
            flamewm_render_core::decode(&encoded).unwrap(),
            output.document
        );
    }

    #[test]
    fn css_border_reset_rejects_unsupported_values_without_partial_mutation() {
        for value in [
            "hidden",
            "dashed",
            "2px dashed red",
            "1px",
            "10",
            "auto",
            "2px solid",
        ] {
            assert!(
                compile_str(
                    &format!("<body><style>div {{ border: {value}; }}</style><div>ok</div></body>"),
                    "",
                    CompileOptions::default(),
                )
                .is_err(),
                "bad border value {value} must fail"
            );
        }
    }

    #[test]
    fn scrollbar_width_none_hides_scrollbars_per_node() {
        let output = compile_str(
            r#"<body><div class="list" id="list">x</div><div id="plain">x</div></body>"#,
            ".list { overflow-y: auto; scrollbar-width: none; }",
            CompileOptions::default(),
        )
        .unwrap();
        assert!(style_of(&output, "list").hide_scrollbars);
        assert_eq!(
            style_of(&output, "list").overflow_y,
            flamewm_render_core::Overflow::Auto
        );
        assert!(!style_of(&output, "plain").hide_scrollbars);
    }

    fn temp_fixture_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "flamewm-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|v| v.as_nanos())
                .unwrap_or(0)
        ))
    }

    fn interactive_fixture() -> CompileOutput {
        compile_body(
            r#"<body><style>:root { --accent: #ef4444; } body { display:flex; } button { background: var(--accent); width: 100px; } button:hover { background:#ffffff; }</style><button id="start" data-action="start.toggle">Start</button></body>"#,
        )
    }

    #[test]
    fn compiles_color_variable() {
        assert_eq!(interactive_fixture().document.variables.len(), 1);
    }

    #[test]
    fn compiles_native_action() {
        assert!(interactive_fixture()
            .document
            .nodes
            .iter()
            .any(|node| node.action == "start.toggle"));
    }

    #[test]
    fn compiles_pointer_cursor() {
        let output = compile_body(
            r#"<body><style>button { cursor: pointer; }</style><button id="open">Open</button></body>"#,
        );
        let node = output
            .document
            .nodes
            .iter()
            .find(|node| node.id == "open")
            .unwrap();
        assert_eq!(node.style.cursor, CursorKind::Pointer);
    }

    #[test]
    fn compiles_wrap_and_break_anywhere() {
        use flamewm_render_core::TextWrap;
        let output = compile_body(
            r#"<body><style>.a { white-space: normal; } .b { white-space: nowrap; } .c { overflow-wrap: anywhere; }</style><div class="a" id="a">x</div><div class="b" id="b">x</div><div class="c" id="c">x</div></body>"#,
        );
        let find = |id: &str| style_of(&output, id);
        assert_eq!(find("a").text_wrap, TextWrap::Wrap);
        assert!(!find("a").break_anywhere);
        assert_eq!(find("b").text_wrap, TextWrap::NoWrap);
        assert_eq!(find("c").text_wrap, TextWrap::Wrap);
        assert!(find("c").break_anywhere);
    }

    #[test]
    fn desktop_label_text_shadow_is_inherited_but_menus_remain_clean() {
        let css = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../ui/desktop/desktop.css"
        ));
        let result = compile_str(
            r#"<body><div class="desktop-label-line" id="label">Test label</div><div id="menu">Menu</div></body>"#,
            css,
            CompileOptions::default(),
        ).expect("desktop CSS must compile under strict mode");
        let label_id = result
            .document
            .nodes
            .iter()
            .position(|n| n.id == "label")
            .unwrap();
        let label = &result.document.nodes[label_id];
        let shadow = label
            .style
            .text_shadow
            .expect("desktop labels have a shadow");
        assert_eq!(
            (shadow.offset_x, shadow.offset_y, shadow.blur_radius),
            (0.0, 1.0, 2.0)
        );
        let child = &result.document.nodes[label.first_child.unwrap() as usize];
        assert_eq!(
            child.style.text_shadow,
            Some(shadow),
            "text inherits its label's shadow"
        );
        assert_eq!(style_of(&result, "menu").text_shadow, None);
        let binary =
            flamewm_render_core::encode(&result.document).expect("encode shadowed document");
        assert_eq!(
            flamewm_render_core::decode(&binary).expect("decode shadowed document"),
            result.document
        );
    }

    #[test]
    fn text_shadow_blur_is_bounded_and_legacy_syntax_is_pixel_equivalent() {
        let output = compile_body(
            r#"<body><style>
            .soft { text-shadow: 0px 1px 3px #000000b8; }
            .legacy { text-shadow: 0px 1px #000000b8; }
            .none { text-shadow: none; }
        </style><div id="soft" class="soft">two lines</div><div id="legacy" class="legacy">old</div><div id="none" class="none">none</div></body>"#,
        );
        assert_eq!(
            style_of(&output, "soft").text_shadow.unwrap().blur_radius,
            3.0
        );
        let old = style_of(&output, "legacy").text_shadow.unwrap();
        assert_eq!(
            (old.blur_radius, old.offset_x, old.offset_y),
            (0.0, 0.0, 1.0)
        );
        assert_eq!(style_of(&output, "none").text_shadow, None);
        let invalid = compile_str(
            r#"<body><style>#bad { text-shadow: 0px 1px 5px #000000; }</style><div id="bad">x</div></body>"#,
            "",
            CompileOptions::default(),
        );
        assert!(invalid.is_err(), "unbounded blur must be rejected");
    }

    #[test]
    fn desktop_sticky_text_wraps_while_other_ui_stays_nowrap() {
        use flamewm_render_core::TextWrap;
        let css = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../ui/desktop/desktop.css"
        ))
        .expect("read desktop UI CSS");
        assert!(
            css.contains(".sticky-text")
                && css.contains("white-space:normal")
                && css.contains("overflow-wrap:anywhere"),
            "desktop CSS must enable wrap only on .sticky-text"
        );
        let output = compile_str(
            r#"<body><div class="desktop-label-line" id="label">x</div><div class="desktop-menu-row" id="row">x</div><div class="sticky-text" id="sticky">x</div></body>"#,
            &css,
            CompileOptions::default(),
        )
        .unwrap();
        let find = |id: &str| style_of(&output, id);
        let sticky = find("sticky");
        assert_eq!(sticky.text_wrap, TextWrap::Wrap);
        assert!(sticky.break_anywhere);
        assert_eq!(sticky.overflow_x, flamewm_render_core::Overflow::Hidden);
        assert_eq!(find("label").text_wrap, TextWrap::NoWrap);
        assert!(!find("label").break_anywhere);
        assert_eq!(find("row").text_wrap, TextWrap::NoWrap);
        assert!(!find("row").break_anywhere);
    }

    #[test]
    fn desktop_convert_wheel_css_compiles_with_native_flex_label_centering() {
        // Regression: the strict RWR compiler rejects `text-align`, which
        // previously panicked the desktop build script before Xephyr started.
        // Compile the actual desktop and shared menu styles, not a mock rule.
        let desktop_css = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../ui/desktop/desktop.css"
        ));
        let shared_menu_css = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../ui/shared/menu.css"
        ));
        let css = format!("{desktop_css}\n{shared_menu_css}");
        let output = compile_str(
            r#"<html><body><section class="context-menu convert-wheel"><button id="convert-option" class="convert-wheel-option convert-wheel-pos-0">WEBP</button></section></body></html>"#,
            &css,
            CompileOptions::default(),
        )
        .expect("desktop conversion wheel CSS must compile in strict mode");
        let style = style_of(&output, "convert-option");
        assert_eq!(style.display, Display::Flex);
        assert_eq!(style.justify_content, JustifyContent::Center);
        assert_eq!(style.align_items, AlignItems::Center);
    }

    #[test]
    fn ordinary_ui_css_uses_default_arrow_cursor() {
        // C-CURSOR-ARROW: ordinary buttons/menu rows/desktop entries/titlebar
        // buttons -> default arrow; text/password entry -> text; explicit
        // resize edges only -> resize. Deferred follow-up lines (owned by
        // parallel jobs this wave): ui/desktop/desktop.css `.desktop-item
        // cursor:pointer`, ui/shell/flamewm.css `.desktop-icon cursor:move`.
        let files = [
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui/settings.css"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui/desktop/desktop.css"),
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../ui/shell/flamewm.css"),
        ];
        for path in files {
            let css = std::fs::read_to_string(path).expect("read active UI CSS");
            assert!(
                !css.contains("cursor:pointer"),
                "ordinary UI CSS must not use cursor:pointer: {path}"
            );
        }
        // Cascade-safe: legitimate text/move roles must still exist in the
        // active cascade (checked here so the guard fails closed if a
        // future edit strips them globally).
        let shell = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../ui/shell/flamewm.css"
        ))
        .expect("read shell UI CSS");
        assert!(
            shell.contains("cursor:text") && shell.contains("cursor:move"),
            "active cascade must keep legitimate text/move cursor roles"
        );
    }

    #[test]
    fn zero_width_seed_keeps_runtime_text_slot_mutable() {
        let output = compile_body(r#"<body><div id="slot">&#8203;</div></body>"#);
        let slot = output
            .document
            .nodes
            .iter()
            .position(|node| node.id == "slot")
            .expect("slot node");
        let child = output.document.nodes[slot]
            .first_child
            .expect("zero-width seed must compile to a text child");
        assert_eq!(output.document.nodes[child as usize].kind, NodeKind::Text);
    }

    #[test]
    fn parses_binary_ppm_asset_payload() {
        let mut bytes = b"P6\n1 1\n255\n".to_vec();
        bytes.extend_from_slice(&[0x12, 0x34, 0x56]);
        let parsed = flamewm_image_core::ppm::decode(&bytes).unwrap();
        assert_eq!(parsed.width, 1);
        assert_eq!(parsed.height, 1);
        assert_eq!(parsed.pixels, vec![0x12, 0x34, 0x56, 255]);
    }

    #[test]
    fn opaque_ppm_asset_compiles_to_fully_opaque_rgba8() {
        use std::io::Write;
        let dir = temp_fixture_dir("j05-ppm");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let ppm = dir.join("icon.ppm");
        let mut bytes = b"P6\n1 1\n255\n".to_vec();
        bytes.extend_from_slice(&[0x11, 0x22, 0x33]);
        std::fs::write(&ppm, &bytes).expect("temp ppm");
        let html = dir.join("doc.html");
        let mut file = std::fs::File::create(&html).expect("temp html");
        write!(
            file,
            "<html><body><img id=\"i\" src=\"icon.ppm\"></body></html>"
        )
        .expect("write html");
        let output = compile_file(&html, &[], CompileOptions::default()).expect("compile");
        assert_eq!(output.document.assets.len(), 1);
        assert_eq!(
            output.document.assets[0].pixels,
            vec![0x11, 0x22, 0x33, 255]
        );
        assert!(output.document.assets[0].is_fully_opaque());
    }

    fn compile_svg_html(name: &str, body: &str) -> CompileOutput {
        use std::io::Write;
        let breeze =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/web/breeze");
        let bytes = std::fs::read(breeze.join(name)).expect("read breeze svg");
        let dir = temp_fixture_dir(&format!("j03-svg-{name}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(name), &bytes).expect("copy svg");
        let html = dir.join("doc.html");
        let mut file = std::fs::File::create(&html).expect("temp html");
        write!(file, "<html><body>{body}</body></html>").expect("write html");
        compile_file(&html, &[], CompileOptions::default()).expect("compile")
    }

    fn compile_svg_fixture(name: &str) -> CompileOutput {
        compile_svg_html(name, &format!("<img id=\"i\" src=\"{name}\">"))
    }

    fn mean_rgb(pixels: &[u8]) -> (f32, f32, f32) {
        let mut sum = [0u64; 3];
        let mut count = 0u64;
        for pixel in pixels.chunks_exact(4) {
            if pixel[3] > 8 {
                sum[0] += u64::from(pixel[0]);
                sum[1] += u64::from(pixel[1]);
                sum[2] += u64::from(pixel[2]);
                count += 1;
            }
        }
        assert!(count > 0, "expected visible pixels");
        (
            sum[0] as f32 / count as f32,
            sum[1] as f32 / count as f32,
            sum[2] as f32 / count as f32,
        )
    }

    fn count_near(pixels: &[u8], target: [u8; 3], tol: i16) -> usize {
        pixels
            .chunks_exact(4)
            .filter(|pixel| {
                pixel[3] > 8
                    && (i16::from(pixel[0]) - i16::from(target[0])).abs() <= tol
                    && (i16::from(pixel[1]) - i16::from(target[1])).abs() <= tol
                    && (i16::from(pixel[2]) - i16::from(target[2])).abs() <= tol
            })
            .count()
    }

    #[test]
    fn semantic_svg_text_fallback_compiles_to_light_text() {
        let output = compile_svg_fixture("network-wireless.svg");
        assert_eq!(output.document.assets.len(), 1);
        let (r, g, b) = mean_rgb(&output.document.assets[0].pixels);
        assert!(
            (r - 0xf1 as f32).abs() < 20.0
                && (g - 0xf2 as f32).abs() < 20.0
                && (b - 0xf3 as f32).abs() < 20.0,
            "mean=({r},{g},{b}) expected near #f1f2f3"
        );
    }

    #[test]
    fn semantic_svg_accent_compiles_to_flame_red_preserving_alpha() {
        let output = compile_svg_fixture("folder.svg");
        assert_eq!(output.document.assets.len(), 1);
        let asset = &output.document.assets[0];
        assert!(asset.pixels.chunks_exact(4).any(|p| p[3] > 8 && p[3] < 255));
        assert!(count_near(&asset.pixels, [0xef, 0x40, 0x48], 32) > 200);
        // Dedup: same src twice in one document yields one asset, both nodes share it.
        let dup = compile_svg_html(
            "folder.svg",
            r#"<img id="a" src="folder.svg"><img id="b" src="folder.svg">"#,
        );
        assert_eq!(dup.document.assets.len(), 1);
        let image_of = |id: &str| {
            dup.document
                .nodes
                .iter()
                .find(|n| n.id == id)
                .unwrap()
                .image
                .unwrap()
        };
        assert_eq!(image_of("a"), image_of("b"));
    }

    #[test]
    fn full_color_svg_compiles_unchanged() {
        let output = compile_svg_fixture("applications-games.svg");
        assert_eq!(output.document.assets.len(), 1);
        let asset = &output.document.assets[0];
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/web/breeze/applications-games.svg"),
        )
        .expect("read games svg");
        let (w, h) = flamewm_image_core::svg::natural_size(&bytes).unwrap();
        let plain = flamewm_image_core::svg::render(&bytes, w, h).unwrap();
        assert_eq!(asset.width, w);
        assert_eq!(asset.height, h);
        assert_eq!(asset.pixels, plain.pixels);
        // Multi-color: distinct blue, green, red, yellow populations.
        assert!(count_near(&asset.pixels, [0x0c, 0x85, 0xdc], 64) > 0);
        assert!(count_near(&asset.pixels, [0x27, 0xab, 0x5f], 64) > 0);
        assert!(count_near(&asset.pixels, [0xda, 0x44, 0x53], 64) > 0);
        assert!(count_near(&asset.pixels, [0xfd, 0xae, 0x23], 64) > 0);
    }

    #[test]
    fn internet_svg_compiles_readable() {
        let output = compile_svg_fixture("internet-web-browser.svg");
        assert_eq!(output.document.assets.len(), 1);
        let asset = &output.document.assets[0];
        let visible: usize = asset.pixels.chunks_exact(4).filter(|p| p[3] > 8).count();
        assert!(visible > 100, "expected readable icon, visible={visible}");
        assert!(count_near(&asset.pixels, [0x19, 0x7c, 0xf1], 64) > 20);
    }
}

use super::*;

/// Deepest element nesting the compiler recurses into; deeper pages are rejected with an error.
const MAX_DOM_DEPTH: usize = 128;

pub(super) struct Compiler<'a> {
    pub(super) html: &'a ParsedHtml,
    pub(super) rules: &'a [Rule],
    pub(super) rule_index: &'a RuleIndex<'a>,
    pub(super) variable_map: &'a HashMap<String, u16>,
    pub(super) length_map: &'a HashMap<String, String>,
    pub(super) options: CompileOptions,
    pub(super) asset_base: Option<&'a Path>,
    pub(super) warnings: Vec<String>,
    pub(super) assets: Vec<ImageAsset>,
    pub(super) asset_index: HashMap<String, u16>,
    pub(super) nodes: Vec<CompiledNode>,
}

impl<'a> Compiler<'a> {
    pub(super) fn compile_subtree(
        &mut self,
        source_index: usize,
        parent: Option<u32>,
        inherited: &Style,
        depth: usize,
    ) -> Result<u32, String> {
        if depth > MAX_DOM_DEPTH {
            return Err(format!(
                "HTML elements nest deeper than {MAX_DOM_DEPTH} levels"
            ));
        }
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
                focus_style: None,
            });
            return Ok(node_index);
        }

        // Selector matching does not depend on the pseudo state, so match once per element and
        // parse the inline style once; each state then only filters and applies the matches.
        let matched = self.matching_rules(source_index);
        let inline = source
            .attrs
            .get("style")
            .map(|text| parse_declarations(text));
        let base = self.compute_style(
            source_index,
            inherited,
            &matched,
            inline.as_ref(),
            PseudoState::Base,
        )?;
        let hover = self.compute_style(
            source_index,
            inherited,
            &matched,
            inline.as_ref(),
            PseudoState::Hover,
        )?;
        let active = self.compute_style(
            source_index,
            inherited,
            &matched,
            inline.as_ref(),
            PseudoState::Active,
        )?;
        let focus = self.compute_style(
            source_index,
            inherited,
            &matched,
            inline.as_ref(),
            PseudoState::Focus,
        )?;
        let hover_style = (hover != base).then_some(hover);
        let active_style = (active != base).then_some(active);
        let focus_style = (focus != base).then_some(focus);
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
            focus_style: focus_style.map(Into::into),
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
            let child = self.compile_subtree(*child_source, Some(node_index), &base, depth + 1)?;
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

    pub(super) fn load_image(&mut self, src: &str) -> Result<u16, String> {
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

    /// Rules whose selector matches the element, in source order. Pseudo state is filtered later.
    fn matching_rules(&self, source_index: usize) -> Vec<&'a Rule> {
        let (html, rules) = (self.html, self.rules);
        self.rule_index
            .candidates(html, source_index)
            .into_iter()
            .map(|position| &rules[position])
            .filter(|rule| {
                rule.selector.state != PseudoState::Root
                    && rule.selector.matches(html, source_index)
            })
            .collect()
    }

    pub(super) fn compute_style(
        &mut self,
        source_index: usize,
        inherited: &Style,
        matched: &[&Rule],
        inline: Option<&Result<Vec<Declaration>, String>>,
        state: PseudoState,
    ) -> Result<Style, String> {
        let source = &self.html.nodes[source_index];
        let mut style = default_style_for_tag(&source.tag, inherited);
        let mut selected: Vec<&Rule> = matched
            .iter()
            .copied()
            .filter(|rule| rule.selector.state == PseudoState::Base || rule.selector.state == state)
            .collect();
        selected.sort_by_key(|rule| (rule.selector.specificity, rule.order));
        for rule in selected {
            for declaration in &rule.declarations {
                self.apply_declaration(&mut style, declaration)?;
            }
        }
        match inline {
            None => {}
            Some(Ok(declarations)) => {
                for declaration in declarations {
                    self.apply_declaration(&mut style, declaration)?;
                }
            }
            Some(Err(error)) => return Err(error.clone()),
        }
        Ok(style)
    }
}

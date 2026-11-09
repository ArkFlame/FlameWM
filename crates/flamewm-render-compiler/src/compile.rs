use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use flamewm_render_core::{
    fnv1a64, AlignItems, Color, ColorValue, ColorVariable, CompiledDocument, CompiledNode,
    CursorKind, Display, Edges, FlexDirection, ImageAsset, ImageTreatment, JustifyContent, Length,
    NodeKind, Overflow, Position, Style, TextOverflow, TextShadow, TextWrap,
};

use crate::cache;
use crate::calc::{eval_calc, CalcUnit};
use crate::css::{parse_declarations, parse_stylesheet, Declaration, PseudoState, Rule};
use crate::html::{parse_html, HtmlNodeKind, ParsedHtml};
use crate::imports::expand_imports;
use crate::rule_index::RuleIndex;

mod build;
mod declarations;
mod node;
mod shorthands;
pub(crate) mod values;
mod vars;

use self::build::Compiler;
use self::node::*;
use self::shorthands::*;
use self::values::*;
use self::vars::*;

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
    compile_file_in(
        html_path,
        extra_css_paths,
        options,
        cache::default_dir().as_deref(),
    )
}

/// Compiles a page from disk. With `cache_dir`, a previous compile of identical inputs is reused.
/// Outputs with warnings are never cached, so a hit always has an empty warning list.
fn compile_file_in(
    html_path: &Path,
    extra_css_paths: &[PathBuf],
    options: CompileOptions,
    cache_dir: Option<&Path>,
) -> Result<CompileOutput, String> {
    let html_source = fs::read_to_string(html_path)
        .map_err(|error| format!("failed to read {}: {error}", html_path.display()))?;
    let html = parse_html(&html_source)?;
    let base = html_path.parent().unwrap_or_else(|| Path::new("."));
    let mut css = expand_imports(html_path, &html.embedded_css)?;
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
        let source = fs::read_to_string(path)
            .map_err(|error| format!("failed to read stylesheet {}: {error}", path.display()))?;
        css.push_str(&expand_imports(path, &source)?);
    }
    let key = cache_dir.and_then(|_| {
        cache_key(
            &html_source,
            &css,
            base,
            &html,
            options,
            crate::utilities::UTILITIES_REVISION,
        )
    });
    if let (Some(dir), Some(key)) = (cache_dir, key) {
        if let Some(document) = cache::load(dir, key) {
            return Ok(CompileOutput {
                document,
                warnings: Vec::new(),
            });
        }
    }
    let output = compile_parsed(&html_source, &css, html, options, Some(base))?;
    if let (Some(dir), Some(key)) = (cache_dir, key) {
        if output.warnings.is_empty() {
            cache::store(dir, key, &output.document);
        }
    }
    Ok(output)
}

/// Hashes every input that changes the compiled output: compiler version, utility table revision,
/// options, page HTML, expanded CSS, and the bytes of each `src` image. `None` when an image cannot
/// be read, so the compile reports that error rather than caching around it.
fn cache_key(
    html_source: &str,
    css: &str,
    base: &Path,
    html: &ParsedHtml,
    options: CompileOptions,
    utilities_revision: u64,
) -> Option<u64> {
    let mut input = Vec::with_capacity(html_source.len() + css.len() + 64);
    input.extend_from_slice(env!("CARGO_PKG_VERSION").as_bytes());
    input.push(0);
    input.extend_from_slice(&utilities_revision.to_le_bytes());
    input.push(0);
    input.push(options.strict as u8);
    input.push(0);
    input.extend_from_slice(html_source.as_bytes());
    input.push(0);
    input.extend_from_slice(css.as_bytes());
    input.push(0);
    for node in &html.nodes {
        if let Some(src) = node.attrs.get("src") {
            let bytes = fs::read(base.join(src)).ok()?;
            input.extend_from_slice(src.as_bytes());
            input.push(0);
            input.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            input.extend_from_slice(&bytes);
        }
    }
    Some(fnv1a64(&input))
}

pub(crate) fn compile_parsed(
    html_source: &str,
    css_source: &str,
    html: ParsedHtml,
    options: CompileOptions,
    asset_base: Option<&Path>,
) -> Result<CompileOutput, String> {
    let mut rules = parse_stylesheet(css_source)?;
    rules.extend(crate::utilities::rules_for(&html, rules.len() as u32));
    let (variables, variable_map, length_map) = collect_variables(&rules)?;
    let mut fingerprint_input = Vec::with_capacity(html_source.len() + css_source.len() + 1);
    fingerprint_input.extend_from_slice(html_source.as_bytes());
    fingerprint_input.push(0);
    fingerprint_input.extend_from_slice(css_source.as_bytes());

    let rule_index = RuleIndex::new(&rules);
    let mut context = Compiler {
        html: &html,
        rules: &rules,
        rule_index: &rule_index,
        variable_map: &variable_map,
        length_map: &length_map,
        options,
        asset_base,
        warnings: Vec::new(),
        assets: Vec::new(),
        asset_index: HashMap::new(),
        nodes: Vec::new(),
    };

    let inherited = Style {
        text_shadow: root_text_shadow(&rules, &variable_map)?,
        ..Style::default()
    };
    let root = context.compile_subtree(html.root, None, &inherited, 0)?;
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

#[cfg(test)]
mod asset_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod ui_pages_tests;

#[cfg(test)]
mod css_feature_tests;

#[cfg(test)]
mod malformed_input_tests;

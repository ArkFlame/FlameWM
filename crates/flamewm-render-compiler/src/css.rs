use crate::html::{HtmlNodeKind, ParsedHtml};

mod selector;
pub(crate) use self::selector::class_rule;
use self::selector::parse_selector;
pub(crate) use self::selector::{Selector, SubjectKey};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PseudoState {
    Base,
    Hover,
    Active,
    /// `:focus-visible` (and `:focus`): keyboard focus only.
    Focus,
    Root,
}

#[derive(Clone, Debug)]
pub struct Declaration {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub selector: Selector,
    pub declarations: Vec<Declaration>,
    pub order: u32,
}

pub fn parse_stylesheet(input: &str) -> Result<Vec<Rule>, String> {
    let css = strip_comments(input)?;
    let mut rules = Vec::new();
    let mut order = 0u32;
    for item in split_items(&css)? {
        match item {
            Item::Declaration(text) if !text.trim().is_empty() => {
                return Err(format!(
                    "CSS declaration '{}' must be inside a rule",
                    text.trim()
                ));
            }
            Item::Declaration(_) => {}
            Item::Block { prelude, body } => {
                if prelude.starts_with('@') {
                    return Err(format!(
                        "unsupported CSS at-rule '{prelude}'; FlameWM Render 0.0.9 accepts no at-rules"
                    ));
                }
                let selectors = resolve_selectors(prelude, None)?;
                emit_rule_block(&selectors, body, &mut rules, &mut order, 0)?;
            }
        }
    }
    Ok(rules)
}

/// One top-level or nested item: a `;`-terminated declaration (or trailing text) or a `prelude { body }` block.
enum Item<'a> {
    Declaration(&'a str),
    Block { prelude: &'a str, body: &'a str },
}

fn split_items(source: &str) -> Result<Vec<Item<'_>>, String> {
    let mut items = Vec::new();
    let mut start = 0usize;
    while let Some((at, delimiter)) = find_unquoted_any(source, start, &[';', '{']) {
        if delimiter == ';' {
            items.push(Item::Declaration(&source[start..at]));
            start = at + 1;
        } else {
            let close = find_matching_brace(source, at)?;
            items.push(Item::Block {
                prelude: source[start..at].trim(),
                body: &source[at + 1..close],
            });
            start = close + 1;
        }
    }
    if !source[start..].trim().is_empty() {
        if source[start..].contains(':') {
            items.push(Item::Declaration(&source[start..]));
        } else {
            return Err("CSS rule is missing '{'".to_string());
        }
    }
    Ok(items)
}

/// Deepest rule nesting accepted in a stylesheet.
const MAX_NESTING: usize = 32;

/// Emits one rule per selector alternative for `body`, then recurses into its nested blocks.
fn emit_rule_block(
    selectors: &[String],
    body: &str,
    rules: &mut Vec<Rule>,
    order: &mut u32,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_NESTING {
        return Err(format!("CSS rules nest deeper than {MAX_NESTING} levels"));
    }
    let mut declarations = Vec::new();
    let mut nested = Vec::new();
    for item in split_items(body)? {
        match item {
            Item::Declaration(text) => declarations.extend(parse_declarations(text)?),
            Item::Block { prelude, body } => nested.push((prelude, body)),
        }
    }
    for selector_text in selectors {
        for (matched, specificity_text) in expand_pseudo(selector_text)? {
            let mut selector = parse_selector(&matched)?;
            if specificity_text != matched {
                let fallback = if specificity_text.is_empty() {
                    "*"
                } else {
                    &specificity_text
                };
                selector.specificity = parse_selector(fallback)?.specificity;
            }
            rules.push(Rule {
                selector,
                declarations: declarations.clone(),
                order: *order,
            });
            *order = order.wrapping_add(1);
        }
    }
    for (prelude, inner) in nested {
        let child = resolve_selectors(prelude, Some(selectors))?;
        emit_rule_block(&child, inner, rules, order, depth + 1)?;
    }
    Ok(())
}

/// Turns a selector list in a nested prelude into full selectors: `&` is replaced by each parent
/// selector, otherwise the nested selector is a descendant of each parent.
fn resolve_selectors(prelude: &str, parents: Option<&[String]>) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    for selector in split_top_level(prelude, ',')? {
        let selector = selector.trim();
        if selector.is_empty() {
            return Err("empty CSS selector".to_string());
        }
        match (selector.contains('&'), parents) {
            (true, None) => {
                return Err(format!("'&' in '{selector}' requires an enclosing rule"));
            }
            (true, Some(parents)) => {
                result.extend(parents.iter().map(|parent| selector.replace('&', parent)));
            }
            (false, None) => result.push(selector.to_string()),
            (false, Some(parents)) => {
                result.extend(parents.iter().map(|parent| format!("{parent} {selector}")));
            }
        }
    }
    Ok(result)
}

/// Upper bound on the selectors one `:is()`/`:where()` list may expand into.
const MAX_PSEUDO_ALTERNATIVES: usize = 64;

/// Expands `:is(...)` and `:where(...)` into plain selectors. Each alternative yields the text to
/// match and the text used for specificity: `:is` keeps its argument's specificity, `:where` adds none.
fn expand_pseudo(selector: &str) -> Result<Vec<(String, String)>, String> {
    let found = [(":is(", false), (":where(", true)]
        .into_iter()
        .filter_map(|(pattern, zero)| selector.find(pattern).map(|at| (at, pattern.len(), zero)))
        .min_by_key(|&(at, _, _)| at);
    let Some((at, pattern_len, is_where)) = found else {
        return Ok(vec![(selector.to_string(), selector.to_string())]);
    };
    let open = at + pattern_len - 1;
    let close = find_matching_paren(selector, open)?;
    let args = &selector[open + 1..close];
    if args.trim().is_empty() {
        return Err(format!("empty pseudo-class argument in '{selector}'"));
    }
    let prefix = &selector[..at];
    let mut arguments = Vec::new();
    for argument in split_top_level(args, ',')? {
        let argument = argument.trim();
        if argument.is_empty() {
            return Err(format!("empty selector in pseudo-class list '{selector}'"));
        }
        arguments.extend(expand_pseudo(argument)?);
    }
    let suffixes = expand_pseudo(&selector[close + 1..])?;
    let mut result = Vec::with_capacity(arguments.len() * suffixes.len());
    for (argument, argument_spec) in &arguments {
        for (suffix, suffix_spec) in &suffixes {
            if result.len() >= MAX_PSEUDO_ALTERNATIVES {
                return Err(format!(
                    "selector '{selector}' expands to more than {MAX_PSEUDO_ALTERNATIVES} alternatives"
                ));
            }
            // `:where` adds no specificity, but a `*` keeps a leading combinator or an empty
            // compound valid for the specificity re-parse.
            let spec_middle = match (is_where, prefix.is_empty() || prefix.ends_with(' ')) {
                (false, _) => argument_spec.as_str(),
                (true, true) => "*",
                (true, false) => "",
            };
            result.push((
                format!("{prefix}{argument}{suffix}"),
                format!("{prefix}{spec_middle}{suffix_spec}"),
            ));
        }
    }
    Ok(result)
}

fn find_matching_paren(input: &str, open: usize) -> Result<usize, String> {
    let mut depth = 0i32;
    for (offset, ch) in input[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(open + offset);
                }
            }
            _ => {}
        }
    }
    Err(format!("unterminated pseudo-class in '{input}'"))
}

/// Finds the first unquoted occurrence of any target character at or after `start`.
fn find_unquoted_any(input: &str, start: usize, targets: &[char]) -> Option<(usize, char)> {
    let mut quote: Option<char> = None;
    for (offset, ch) in input[start..].char_indices() {
        match quote {
            Some(active) if ch == active => quote = None,
            Some(_) => {}
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if targets.contains(&ch) => return Some((start + offset, ch)),
            None => {}
        }
    }
    None
}

pub fn parse_declarations(input: &str) -> Result<Vec<Declaration>, String> {
    let mut result = Vec::new();
    for chunk in split_top_level(input, ';')? {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        let colon = find_unquoted(chunk, 0, ':')
            .ok_or_else(|| format!("CSS declaration '{chunk}' is missing ':'"))?;
        let name = chunk[..colon].trim().to_ascii_lowercase();
        let value = chunk[colon + 1..].trim().to_string();
        if name.is_empty() || value.is_empty() {
            return Err(format!("invalid CSS declaration '{chunk}'"));
        }
        result.push(Declaration { name, value });
    }
    Ok(result)
}

pub(crate) fn strip_comments(input: &str) -> Result<String, String> {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while let Some(start_rel) = input[cursor..].find("/*") {
        let start = cursor + start_rel;
        output.push_str(&input[cursor..start]);
        let end = input[start + 2..]
            .find("*/")
            .map(|offset| start + 2 + offset + 2)
            .ok_or_else(|| "unterminated CSS comment".to_string())?;
        cursor = end;
    }
    output.push_str(&input[cursor..]);
    Ok(output)
}

fn split_top_level(input: &str, delimiter: char) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    let mut start = 0usize;
    let mut quote: Option<char> = None;
    let mut paren_depth = 0i32;
    for (index, ch) in input.char_indices() {
        match quote {
            Some(active) if ch == active => quote = None,
            Some(_) => continue,
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch == '(' => paren_depth += 1,
            None if ch == ')' => {
                paren_depth -= 1;
                if paren_depth < 0 {
                    return Err("CSS has unmatched ')'".to_string());
                }
            }
            None if ch == delimiter && paren_depth == 0 => {
                result.push(input[start..index].to_string());
                start = index + ch.len_utf8();
            }
            None => {}
        }
    }
    if quote.is_some() || paren_depth != 0 {
        return Err("CSS has an unterminated quote or parenthesis".to_string());
    }
    result.push(input[start..].to_string());
    Ok(result)
}

fn find_unquoted(input: &str, start: usize, target: char) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut paren_depth = 0i32;
    for (offset, ch) in input[start..].char_indices() {
        match quote {
            Some(active) if ch == active => quote = None,
            Some(_) => {}
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch == '(' => paren_depth += 1,
            None if ch == ')' => paren_depth -= 1,
            None if ch == target && paren_depth == 0 => return Some(start + offset),
            None => {}
        }
    }
    None
}

fn find_matching_brace(input: &str, open: usize) -> Result<usize, String> {
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    for (offset, ch) in input[open..].char_indices() {
        match quote {
            Some(active) if ch == active => quote = None,
            Some(_) => {}
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch == '{' => depth += 1,
            None if ch == '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(open + offset);
                }
            }
            None => {}
        }
    }
    Err("CSS rule has no matching '}'".to_string())
}

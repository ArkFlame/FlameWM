use crate::html::{HtmlNodeKind, ParsedHtml};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PseudoState {
    Base,
    Hover,
    Active,
    Root,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Combinator {
    Descendant,
    Child,
}

#[derive(Clone, Debug)]
struct CompoundSelector {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

#[derive(Clone, Debug)]
struct SelectorPart {
    compound: CompoundSelector,
    relation_to_left: Combinator,
}

#[derive(Clone, Debug)]
pub struct Selector {
    parts: Vec<SelectorPart>,
    pub specificity: (u16, u16, u16),
    pub state: PseudoState,
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
    let mut cursor = 0usize;
    let mut order = 0u32;

    while cursor < css.len() {
        cursor = skip_ascii_ws(&css, cursor);
        if cursor >= css.len() {
            break;
        }
        if css.as_bytes()[cursor] == b'@' {
            let remainder = &css[cursor..];
            let end = match (remainder.find('{'), remainder.find(';')) {
                (Some(a), Some(b)) => cursor + a.min(b),
                (Some(a), None) => cursor + a,
                (None, Some(b)) => cursor + b,
                (None, None) => css.len(),
            };
            return Err(format!(
                "unsupported CSS at-rule '{}'; FlameWM Render 0.0.9 accepts no at-rules",
                css[cursor..end].trim()
            ));
        }
        let open = find_unquoted(&css, cursor, '{')
            .ok_or_else(|| "CSS rule is missing '{'".to_string())?;
        let close = find_matching_brace(&css, open)?;
        let selector_source = css[cursor..open].trim();
        let body = &css[open + 1..close];
        if selector_source.is_empty() {
            return Err("empty CSS selector".to_string());
        }
        let declarations = parse_declarations(body)?;
        for selector_text in split_top_level(selector_source, ',')? {
            let selector = parse_selector(selector_text.trim())?;
            rules.push(Rule {
                selector,
                declarations: declarations.clone(),
                order,
            });
            order = order.wrapping_add(1);
        }
        cursor = close + 1;
    }
    Ok(rules)
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

impl Selector {
    pub fn matches(&self, html: &ParsedHtml, node_index: usize) -> bool {
        if self.state == PseudoState::Root {
            return node_index == html.root;
        }
        if html.nodes[node_index].kind != HtmlNodeKind::Element || self.parts.is_empty() {
            return false;
        }
        let mut current = node_index;
        let last = self.parts.len() - 1;
        if !matches_compound(html, current, &self.parts[last].compound) {
            return false;
        }
        for part_index in (1..=last).rev() {
            let relation = self.parts[part_index].relation_to_left;
            let wanted = &self.parts[part_index - 1].compound;
            match relation {
                Combinator::Child => {
                    let Some(parent) = html.nodes[current].parent else {
                        return false;
                    };
                    if !matches_compound(html, parent, wanted) {
                        return false;
                    }
                    current = parent;
                }
                Combinator::Descendant => {
                    let mut ancestor = html.nodes[current].parent;
                    let mut found = None;
                    while let Some(index) = ancestor {
                        if matches_compound(html, index, wanted) {
                            found = Some(index);
                            break;
                        }
                        ancestor = html.nodes[index].parent;
                    }
                    let Some(index) = found else {
                        return false;
                    };
                    current = index;
                }
            }
        }
        true
    }
}

fn parse_selector(input: &str) -> Result<Selector, String> {
    if input == ":root" {
        return Ok(Selector {
            parts: Vec::new(),
            specificity: (0, 1, 0),
            state: PseudoState::Root,
        });
    }
    let tokens = selector_tokens(input)?;
    if tokens.is_empty() {
        return Err("empty CSS selector".to_string());
    }
    let mut parts = Vec::new();
    let mut next_relation = Combinator::Descendant;
    let mut specificity = (0u16, 0u16, 0u16);
    let mut state = PseudoState::Base;
    let mut expect_compound = true;

    let token_count = tokens.len();
    for (token_index, token) in tokens.into_iter().enumerate() {
        if token == ">" {
            if expect_compound || parts.is_empty() {
                return Err(format!("invalid child combinator in selector '{input}'"));
            }
            next_relation = Combinator::Child;
            expect_compound = true;
            continue;
        }
        if !expect_compound && next_relation == Combinator::Child {
            return Err(format!("invalid selector '{input}'"));
        }
        let (compound, token_state, token_specificity) = parse_compound(&token)?;
        if token_state != PseudoState::Base {
            if token_index + 1 != token_count {
                return Err(format!(
                    "pseudo state in selector '{input}' must be on the rightmost compound"
                ));
            }
            if state != PseudoState::Base && state != token_state {
                return Err(format!(
                    "selector '{input}' mixes unsupported pseudo states"
                ));
            }
            state = token_state;
        }
        specificity.0 = specificity.0.saturating_add(token_specificity.0);
        specificity.1 = specificity.1.saturating_add(token_specificity.1);
        specificity.2 = specificity.2.saturating_add(token_specificity.2);
        parts.push(SelectorPart {
            compound,
            relation_to_left: if parts.is_empty() {
                Combinator::Descendant
            } else {
                next_relation
            },
        });
        next_relation = Combinator::Descendant;
        expect_compound = false;
    }
    if expect_compound {
        return Err(format!("selector '{input}' ends with a combinator"));
    }
    Ok(Selector {
        parts,
        specificity,
        state,
    })
}

fn selector_tokens(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in input.chars() {
        match ch {
            '>' => {
                if !current.trim().is_empty() {
                    tokens.push(current.trim().to_string());
                    current.clear();
                }
                tokens.push(">".to_string());
            }
            ch if ch.is_ascii_whitespace() => {
                if !current.trim().is_empty() {
                    tokens.push(current.trim().to_string());
                    current.clear();
                }
            }
            '[' | ']' | '+' | '~' => {
                return Err(format!("selector syntax '{ch}' is not supported in 0.0.9"))
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        tokens.push(current.trim().to_string());
    }
    Ok(tokens)
}

fn parse_compound(input: &str) -> Result<(CompoundSelector, PseudoState, (u16, u16, u16)), String> {
    let mut tag = None;
    let mut id = None;
    let mut classes = Vec::new();
    let mut state = PseudoState::Base;
    let mut ids = 0u16;
    let mut class_like = 0u16;
    let mut tags = 0u16;
    let bytes = input.as_bytes();
    let mut cursor = 0usize;

    if cursor < bytes.len()
        && bytes[cursor] != b'.'
        && bytes[cursor] != b'#'
        && bytes[cursor] != b':'
    {
        let start = cursor;
        while cursor < bytes.len() && !matches!(bytes[cursor], b'.' | b'#' | b':') {
            cursor += 1;
        }
        let value = &input[start..cursor];
        if value != "*" {
            if value.is_empty()
                || !value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
            {
                return Err(format!("unsupported tag selector '{value}'"));
            }
            tag = Some(value.to_ascii_lowercase());
            tags += 1;
        }
    }

    while cursor < bytes.len() {
        let prefix = bytes[cursor] as char;
        cursor += 1;
        let start = cursor;
        while cursor < bytes.len() && !matches!(bytes[cursor], b'.' | b'#' | b':') {
            cursor += 1;
        }
        let value = &input[start..cursor];
        if value.is_empty() {
            return Err(format!("empty selector component in '{input}'"));
        }
        match prefix {
            '.' => {
                classes.push(value.to_string());
                class_like += 1;
            }
            '#' => {
                if id.replace(value.to_string()).is_some() {
                    return Err(format!("selector '{input}' contains more than one id"));
                }
                ids += 1;
            }
            ':' => {
                class_like += 1;
                state = match value {
                    "hover" => PseudoState::Hover,
                    "active" => PseudoState::Active,
                    "root" => PseudoState::Root,
                    _ => return Err(format!("unsupported pseudo-class ':{value}'")),
                };
            }
            _ => unreachable!(),
        }
    }

    Ok((
        CompoundSelector { tag, id, classes },
        state,
        (ids, class_like, tags),
    ))
}

fn matches_compound(html: &ParsedHtml, index: usize, compound: &CompoundSelector) -> bool {
    let node = &html.nodes[index];
    if node.kind != HtmlNodeKind::Element {
        return false;
    }
    if let Some(tag) = compound.tag.as_ref() {
        if &node.tag != tag {
            return false;
        }
    }
    if let Some(id) = compound.id.as_ref() {
        if node.attrs.get("id") != Some(id) {
            return false;
        }
    }
    compound.classes.iter().all(|wanted| {
        node.attrs.get("class").is_some_and(|class_attr| {
            class_attr
                .split_ascii_whitespace()
                .any(|actual| actual == wanted)
        })
    })
}

fn strip_comments(input: &str) -> Result<String, String> {
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

fn skip_ascii_ws(input: &str, mut cursor: usize) -> usize {
    while cursor < input.len() && input.as_bytes()[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    cursor
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html::parse_html;

    #[test]
    fn descendant_and_child_selectors_match() {
        let html =
            parse_html("<body><div class='panel'><button id='start'>Start</button></div></body>")
                .unwrap();
        let rules = parse_stylesheet(".panel > #start:hover { background: #f00; }").unwrap();
        let button = html
            .nodes
            .iter()
            .position(|node| node.attrs.get("id").map(String::as_str) == Some("start"))
            .unwrap();
        assert!(rules[0].selector.matches(&html, button));
    }
}

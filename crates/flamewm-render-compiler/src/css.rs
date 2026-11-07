use crate::html::{HtmlNodeKind, ParsedHtml};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PseudoState {
    Base,
    Hover,
    Active,
    /// `:focus-visible` (and `:focus`): keyboard focus only.
    Focus,
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
                emit_rule_block(&selectors, body, &mut rules, &mut order)?;
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

/// Emits one rule per selector alternative for `body`, then recurses into its nested blocks.
fn emit_rule_block(
    selectors: &[String],
    body: &str,
    rules: &mut Vec<Rule>,
    order: &mut u32,
) -> Result<(), String> {
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
        emit_rule_block(&child, inner, rules, order)?;
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

type ParsedCompound = (CompoundSelector, PseudoState, (u16, u16, u16));

fn parse_compound(input: &str) -> Result<ParsedCompound, String> {
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
                    "focus" | "focus-visible" => PseudoState::Focus,
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

    fn selector_count(css: &str) -> usize {
        parse_stylesheet(css).unwrap().len()
    }

    #[test]
    fn nested_rules_flatten_with_descendant_and_ampersand_parents() {
        let html = parse_html(
            "<body><div class='panel'><button id='go' class='primary'>Go</button></div></body>",
        )
        .unwrap();
        let rules = parse_stylesheet(
            ".panel { width: 1px; button { height: 2px; } &.wide { height: 3px; } & > .x { color: red; } }",
        )
        .unwrap();
        // .panel, .panel button, .panel.wide, .panel > .x
        assert_eq!(rules.len(), 4);
        assert_eq!(rules[0].declarations.len(), 1);
        let button = html
            .nodes
            .iter()
            .position(|node| node.attrs.get("id").map(String::as_str) == Some("go"))
            .unwrap();
        assert!(rules[1].selector.matches(&html, button));
        assert_eq!(rules[1].declarations[0].name, "height");
        assert!(!rules[2].selector.matches(&html, button));
    }

    #[test]
    fn nested_hover_state_stays_on_the_rightmost_compound() {
        // `.row` (empty), `.row button`, `.row button:hover`
        let rules = parse_stylesheet(".row { button { &:hover { color: red; } } }").unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[2].selector.state, PseudoState::Hover);
        assert!(parse_stylesheet(".row { &:hover button { color: red; } }").is_err());
    }

    #[test]
    fn nesting_error_cases_are_rejected() {
        assert!(
            parse_stylesheet("& { color: red; }").is_err(),
            "& needs a parent"
        );
        assert!(
            parse_stylesheet("color: red;").is_err(),
            "top-level declaration"
        );
        assert!(parse_stylesheet(".a { @media x { color: red; } }").is_err());
        assert!(
            parse_stylesheet(".a { .b { color: red; }").is_err(),
            "unclosed block"
        );
        assert!(
            parse_stylesheet(".a, { color: red; }").is_err(),
            "empty selector"
        );
    }

    #[test]
    fn is_and_where_expand_into_alternatives() {
        assert_eq!(selector_count(".a :is(.b, .c) { width: 1px; }"), 2);
        assert_eq!(selector_count(":is(.a, .b):is(.c, .d) { width: 1px; }"), 4);
        assert_eq!(selector_count(":where(.a, .b) button { width: 1px; }"), 2);
        assert_eq!(
            selector_count("button:is(:hover, :focus) { width: 1px; }"),
            2
        );
    }

    #[test]
    fn is_takes_argument_specificity_and_where_contributes_none() {
        let is_rules = parse_stylesheet(":is(#a, .b) { width: 1px; }").unwrap();
        assert_eq!(is_rules[0].selector.specificity, (1, 0, 0));
        assert_eq!(is_rules[1].selector.specificity, (0, 1, 0));
        let where_rules = parse_stylesheet("div:where(#a, .b) { width: 1px; }").unwrap();
        assert_eq!(where_rules[0].selector.specificity, (0, 0, 1));
        assert_eq!(where_rules[1].selector.specificity, (0, 0, 1));
        let bare = parse_stylesheet(":where(.a) { width: 1px; }").unwrap();
        assert_eq!(bare[0].selector.specificity, (0, 0, 0));
    }

    #[test]
    fn is_where_match_like_their_expanded_selectors() {
        let html =
            parse_html("<body><div class='panel'><button id='start'>Start</button></div></body>")
                .unwrap();
        let button = html
            .nodes
            .iter()
            .position(|node| node.attrs.get("id").map(String::as_str) == Some("start"))
            .unwrap();
        let rules =
            parse_stylesheet(":where(.panel, .other) > :is(button, span) { width: 1px; }").unwrap();
        assert!(rules
            .iter()
            .any(|rule| rule.selector.matches(&html, button)));
    }

    #[test]
    fn is_where_error_cases_are_rejected() {
        for bad in [
            ":is() { width: 1px; }",
            ":is(.a, ) { width: 1px; }",
            ":where(.a { width: 1px; }",
            ":is(.a { width: 1px; }",
            ".a:is(.b) .c { width: 1px; } :is(:hover .x) { width: 1px; }",
        ] {
            assert!(parse_stylesheet(bad).is_err(), "{bad} must be rejected");
        }
        let many: Vec<String> = (0..9).map(|i| format!(".c{i}")).collect();
        let long = format!(
            ":is({}) :is({}) {{ width: 1px; }}",
            many.join(","),
            many.join(",")
        );
        assert!(
            parse_stylesheet(&long).is_err(),
            "81 alternatives exceed the bound"
        );
    }
}

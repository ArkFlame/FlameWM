//! Selector grammar and matching: compound parsing, pseudo-state, specificity and the subject index key.

use super::*;

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

/// The index key of a selector's subject (rightmost) compound. A selector can only match an element
/// that satisfies this key, so rules can be filed under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubjectKey<'a> {
    Id(&'a str),
    Class(&'a str),
    Tag(&'a str),
    Any,
}

impl Selector {
    pub(crate) fn subject_key(&self) -> SubjectKey<'_> {
        let Some(part) = self.parts.last() else {
            return SubjectKey::Any;
        };
        let compound = &part.compound;
        if let Some(id) = compound.id.as_deref() {
            SubjectKey::Id(id)
        } else if let Some(class) = compound.classes.first() {
            SubjectKey::Class(class)
        } else if let Some(tag) = compound.tag.as_deref() {
            SubjectKey::Tag(tag)
        } else {
            SubjectKey::Any
        }
    }

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

pub(super) fn parse_selector(input: &str) -> Result<Selector, String> {
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
            _ => return Err(format!("invalid selector '{input}'")),
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

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HtmlNodeKind {
    Element,
    Text,
}

#[derive(Clone, Debug)]
pub struct HtmlNode {
    pub kind: HtmlNodeKind,
    pub tag: String,
    pub attrs: BTreeMap<String, String>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct ParsedHtml {
    pub nodes: Vec<HtmlNode>,
    pub root: usize,
    pub embedded_css: String,
    pub linked_css: Vec<String>,
}

#[cfg(test)]
impl ParsedHtml {
    pub fn attr<'a>(&'a self, index: usize, name: &str) -> Option<&'a str> {
        self.nodes[index].attrs.get(name).map(String::as_str)
    }
}

pub fn parse_html(input: &str) -> Result<ParsedHtml, String> {
    let mut nodes = vec![HtmlNode {
        kind: HtmlNodeKind::Element,
        tag: "document".to_string(),
        attrs: BTreeMap::new(),
        parent: None,
        children: Vec::new(),
        text: String::new(),
    }];
    let mut stack = vec![0usize];
    let mut cursor = 0usize;
    let bytes = input.as_bytes();

    while cursor < bytes.len() {
        if bytes[cursor] != b'<' {
            let next = input[cursor..]
                .find('<')
                .map(|offset| cursor + offset)
                .unwrap_or(bytes.len());
            let raw = &input[cursor..next];
            let text = collapse_whitespace(&decode_entities(raw));
            if !text.is_empty() {
                let parent = *stack
                    .last()
                    .ok_or_else(|| "HTML parser stack underflow".to_string())?;
                push_node(
                    &mut nodes,
                    parent,
                    HtmlNode {
                        kind: HtmlNodeKind::Text,
                        tag: "#text".to_string(),
                        attrs: BTreeMap::new(),
                        parent: Some(parent),
                        children: Vec::new(),
                        text,
                    },
                );
            }
            cursor = next;
            continue;
        }

        if input[cursor..].starts_with("<!--") {
            let end = input[cursor + 4..]
                .find("-->")
                .map(|offset| cursor + 4 + offset + 3)
                .ok_or_else(|| "unterminated HTML comment".to_string())?;
            cursor = end;
            continue;
        }

        if input[cursor..].starts_with("<!") || input[cursor..].starts_with("<?") {
            let end = input[cursor..]
                .find('>')
                .map(|offset| cursor + offset + 1)
                .ok_or_else(|| "unterminated HTML declaration".to_string())?;
            cursor = end;
            continue;
        }

        if input[cursor..].starts_with("</") {
            let end = input[cursor..]
                .find('>')
                .map(|offset| cursor + offset)
                .ok_or_else(|| "unterminated closing tag".to_string())?;
            let name = input[cursor + 2..end].trim().to_ascii_lowercase();
            if stack.len() <= 1 {
                return Err(format!("unexpected closing tag </{name}>"));
            }
            let open = &nodes[*stack.last().unwrap()].tag;
            if *open != name {
                return Err(format!(
                    "mismatched closing tag: expected </{open}> but found </{name}>"
                ));
            }
            stack.pop();
            cursor = end + 1;
            continue;
        }

        let end = find_tag_end(input, cursor + 1)?;
        let raw = input[cursor + 1..end].trim();
        let self_closing = raw.ends_with('/');
        let inside = if self_closing {
            raw[..raw.len() - 1].trim_end()
        } else {
            raw
        };
        let (tag, attrs) = parse_tag(inside)?;
        let void = is_void_element(&tag);
        let parent = *stack
            .last()
            .ok_or_else(|| "HTML parser stack underflow".to_string())?;
        let index = push_node(
            &mut nodes,
            parent,
            HtmlNode {
                kind: HtmlNodeKind::Element,
                tag,
                attrs,
                parent: Some(parent),
                children: Vec::new(),
                text: String::new(),
            },
        );
        if !self_closing && !void {
            stack.push(index);
        }
        cursor = end + 1;
    }

    if stack.len() != 1 {
        let open = &nodes[*stack.last().unwrap()].tag;
        return Err(format!("unclosed HTML tag <{open}>"));
    }

    let root = find_first_tag(&nodes, "body")
        .or_else(|| find_first_tag(&nodes, "html"))
        .unwrap_or(0);
    let embedded_css = collect_style_text(&nodes);
    let linked_css = collect_stylesheet_links(&nodes);
    Ok(ParsedHtml {
        nodes,
        root,
        embedded_css,
        linked_css,
    })
}

fn push_node(nodes: &mut Vec<HtmlNode>, parent: usize, node: HtmlNode) -> usize {
    let index = nodes.len();
    nodes.push(node);
    nodes[parent].children.push(index);
    index
}

fn find_first_tag(nodes: &[HtmlNode], tag: &str) -> Option<usize> {
    nodes
        .iter()
        .position(|node| node.kind == HtmlNodeKind::Element && node.tag == tag)
}

fn collect_style_text(nodes: &[HtmlNode]) -> String {
    nodes
        .iter()
        .filter(|node| node.kind == HtmlNodeKind::Element && node.tag == "style")
        .flat_map(|node| &node.children)
        .filter(|child| nodes[**child].kind == HtmlNodeKind::Text)
        .map(|child| format!("{}\n", nodes[*child].text))
        .collect()
}

fn collect_stylesheet_links(nodes: &[HtmlNode]) -> Vec<String> {
    nodes
        .iter()
        .filter(|node| node.kind == HtmlNodeKind::Element && node.tag == "link")
        .filter(|node| {
            let rel = node.attrs.get("rel").map(String::as_str).unwrap_or("");
            rel.split_ascii_whitespace()
                .any(|part| part.eq_ignore_ascii_case("stylesheet"))
        })
        .filter_map(|node| node.attrs.get("href").filter(|href| !href.is_empty()))
        .cloned()
        .collect()
}

fn find_tag_end(input: &str, start: usize) -> Result<usize, String> {
    let mut quote: Option<char> = None;
    for (offset, ch) in input[start..].char_indices() {
        match quote {
            Some(active) if ch == active => quote = None,
            Some(_) => {}
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch == '>' => return Ok(start + offset),
            None => {}
        }
    }
    Err("unterminated opening tag".to_string())
}

fn parse_tag(input: &str) -> Result<(String, BTreeMap<String, String>), String> {
    let mut chars = input.char_indices().peekable();
    skip_ws(&mut chars);
    let name_start = chars.peek().map(|(i, _)| *i).unwrap_or(input.len());
    while let Some((_, ch)) = chars.peek() {
        if ch.is_ascii_whitespace() {
            break;
        }
        chars.next();
    }
    let name_end = chars.peek().map(|(i, _)| *i).unwrap_or(input.len());
    let tag = input[name_start..name_end].trim().to_ascii_lowercase();
    if tag.is_empty()
        || !tag
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(format!("invalid tag name '{tag}'"));
    }

    let mut attrs = BTreeMap::new();
    loop {
        skip_ws(&mut chars);
        let Some((name_start, _)) = chars.peek().copied() else {
            break;
        };
        while let Some((_, ch)) = chars.peek() {
            if ch.is_ascii_whitespace() || *ch == '=' {
                break;
            }
            chars.next();
        }
        let name_end = chars.peek().map(|(i, _)| *i).unwrap_or(input.len());
        let name = input[name_start..name_end].trim().to_ascii_lowercase();
        if name.is_empty() {
            break;
        }
        skip_ws(&mut chars);
        let mut value = String::new();
        if matches!(chars.peek(), Some((_, '='))) {
            chars.next();
            skip_ws(&mut chars);
            if let Some((_, quote @ ('\'' | '"'))) = chars.peek().copied() {
                chars.next();
                let start = chars.peek().map(|(i, _)| *i).unwrap_or(input.len());
                let mut end = input.len();
                while let Some((index, ch)) = chars.next() {
                    if ch == quote {
                        end = index;
                        break;
                    }
                }
                value = decode_entities(&input[start..end]);
            } else {
                let start = chars.peek().map(|(i, _)| *i).unwrap_or(input.len());
                while let Some((_, ch)) = chars.peek() {
                    if ch.is_ascii_whitespace() {
                        break;
                    }
                    chars.next();
                }
                let end = chars.peek().map(|(i, _)| *i).unwrap_or(input.len());
                value = decode_entities(&input[start..end]);
            }
        }
        attrs.insert(name, value);
    }
    Ok((tag, attrs))
}

fn skip_ws<I>(iter: &mut std::iter::Peekable<I>)
where
    I: Iterator<Item = (usize, char)>,
{
    while matches!(iter.peek(), Some((_, ch)) if ch.is_ascii_whitespace()) {
        iter.next();
    }
}

fn is_void_element(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn collapse_whitespace(input: &str) -> String {
    let mut result = String::new();
    let mut pending_space = false;
    for ch in input.chars() {
        if ch.is_whitespace() {
            pending_space = !result.is_empty();
        } else {
            if pending_space {
                result.push(' ');
                pending_space = false;
            }
            result.push(ch);
        }
    }
    result.trim().to_string()
}

fn decode_entities(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0usize;
    while let Some(relative) = input[cursor..].find('&') {
        let start = cursor + relative;
        output.push_str(&input[cursor..start]);
        let Some(end_rel) = input[start..].find(';') else {
            output.push_str(&input[start..]);
            return output;
        };
        let end = start + end_rel;
        let entity = &input[start + 1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                u32::from_str_radix(&entity[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if entity.starts_with('#') => {
                entity[1..].parse::<u32>().ok().and_then(char::from_u32)
            }
            _ => None,
        };
        if let Some(ch) = decoded {
            output.push(ch);
        } else {
            output.push_str(&input[start..=end]);
        }
        cursor = end + 1;
    }
    output.push_str(&input[cursor..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_markup_and_stylesheet() {
        let parsed = parse_html(r#"<!doctype html><html><head><style>.a { color: red; }</style></head><body><div id='x'>Hello <span>world</span></div></body></html>"#).unwrap();
        assert_eq!(parsed.nodes[parsed.root].tag, "body");
        assert!(parsed.embedded_css.contains("color: red"));
        let div = parsed
            .nodes
            .iter()
            .position(|node| node.attrs.get("id").map(String::as_str) == Some("x"))
            .expect("div#x");
        assert_eq!(parsed.attr(div, "id"), Some("x"));
    }
}

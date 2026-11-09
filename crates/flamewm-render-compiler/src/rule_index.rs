//! Candidate lookup for selector matching. Each rule is filed under the key its subject compound
//! requires (id, then first class, then tag), so an element only tests rules that could match it.

use std::collections::HashMap;

use crate::css::{Rule, SubjectKey};
use crate::html::ParsedHtml;

pub(crate) struct RuleIndex<'a> {
    by_id: HashMap<&'a str, Vec<usize>>,
    by_class: HashMap<&'a str, Vec<usize>>,
    by_tag: HashMap<&'a str, Vec<usize>>,
    unkeyed: Vec<usize>,
}

impl<'a> RuleIndex<'a> {
    pub(crate) fn new(rules: &'a [Rule]) -> Self {
        let mut index = Self {
            by_id: HashMap::new(),
            by_class: HashMap::new(),
            by_tag: HashMap::new(),
            unkeyed: Vec::new(),
        };
        for (position, rule) in rules.iter().enumerate() {
            let bucket = match rule.selector.subject_key() {
                SubjectKey::Id(id) => index.by_id.entry(id).or_default(),
                SubjectKey::Class(class) => index.by_class.entry(class).or_default(),
                SubjectKey::Tag(tag) => index.by_tag.entry(tag).or_default(),
                SubjectKey::Any => &mut index.unkeyed,
            };
            bucket.push(position);
        }
        index
    }

    /// Positions of every rule that might match `node`, ascending (source order), without duplicates.
    pub(crate) fn candidates(&self, html: &ParsedHtml, node: usize) -> Vec<usize> {
        let element = &html.nodes[node];
        let mut found = self.unkeyed.clone();
        let mut take = |bucket: Option<&Vec<usize>>| {
            if let Some(positions) = bucket {
                found.extend_from_slice(positions);
            }
        };
        take(
            element
                .attrs
                .get("id")
                .and_then(|id| self.by_id.get(id.as_str())),
        );
        take(self.by_tag.get(element.tag.as_str()));
        if let Some(classes) = element.attrs.get("class") {
            for class in classes.split_ascii_whitespace() {
                take(self.by_class.get(class));
            }
        }
        found.sort_unstable();
        found.dedup();
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::parse_stylesheet;
    use crate::html::parse_html;

    /// The index may only drop rules that cannot match, so every brute-force match must be a candidate.
    #[test]
    fn candidates_contain_every_matching_rule_in_source_order() {
        let css = "* { width: 1px; } div { width: 2px; } .a { width: 3px; } .a.b { width: 4px; } \
                   #x { width: 5px; } span#x { width: 6px; } p:hover { width: 7px; } \
                   .c .d { width: 8px; } #x.a { width: 9px; } :root { --gap: 1px; }";
        let rules = parse_stylesheet(css).unwrap();
        let html = parse_html(
            "<body><div id=\"x\" class=\"a  b\"><span class=\"c\"><p class=\"d\"></p></span></div>\
             <p class=\"b a\">t</p><div class=\"zzz\"></div></body>",
        )
        .unwrap();
        let index = RuleIndex::new(&rules);
        for node in 0..html.nodes.len() {
            let candidates = index.candidates(&html, node);
            assert!(
                candidates.windows(2).all(|pair| pair[0] < pair[1]),
                "sorted and unique"
            );
            for (position, rule) in rules.iter().enumerate() {
                if rule.selector.matches(&html, node) {
                    assert!(
                        candidates.contains(&position),
                        "node {node} misses rule {position}"
                    );
                }
            }
        }
    }
}

//! `com.canonical.dbusmenu` layout reduction. Pure: callers convert bus values into [`MenuNode`].

use std::collections::BTreeMap;

pub const MAX_MENU_DEPTH: usize = 8;
pub const MAX_MENU_ENTRIES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuValue {
    Str(String),
    Bool(bool),
    Int(i32),
}

/// One `(ia{sv}av)` layout node from `GetLayout`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MenuNode {
    pub id: i32,
    pub props: BTreeMap<String, MenuValue>,
    pub children: Vec<MenuNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    Separator,
    Item {
        id: i32,
        label: String,
        enabled: bool,
        /// `Some(checked)` for checkmark/radio items, `None` for plain items.
        toggle: Option<bool>,
        submenu: Vec<MenuEntry>,
    },
}

/// Reduces the children of the layout root into renderable entries. Hidden items are skipped,
/// and depth and total entry count are bounded so a hostile menu cannot grow without limit.
#[must_use]
pub fn parse_layout(root: &MenuNode) -> Vec<MenuEntry> {
    let mut budget = MAX_MENU_ENTRIES;
    parse_children(&root.children, 0, &mut budget)
}

fn parse_children(nodes: &[MenuNode], depth: usize, budget: &mut usize) -> Vec<MenuEntry> {
    let mut entries = Vec::new();
    for node in nodes {
        if *budget == 0 {
            break;
        }
        if !flag(node, "visible", true) {
            continue;
        }
        *budget -= 1;
        if str_prop(node, "type") == Some("separator") {
            entries.push(MenuEntry::Separator);
            continue;
        }
        let toggle = match str_prop(node, "toggle-type") {
            Some("checkmark" | "radio") => Some(int_prop(node, "toggle-state") == Some(1)),
            _ => None,
        };
        let submenu = if depth + 1 < MAX_MENU_DEPTH {
            parse_children(&node.children, depth + 1, budget)
        } else {
            Vec::new()
        };
        entries.push(MenuEntry::Item {
            id: node.id,
            label: clean_label(str_prop(node, "label").unwrap_or_default()),
            enabled: flag(node, "enabled", true),
            toggle,
            submenu,
        });
    }
    entries
}

fn flag(node: &MenuNode, key: &str, default: bool) -> bool {
    match node.props.get(key) {
        Some(MenuValue::Bool(value)) => *value,
        _ => default,
    }
}

fn str_prop<'a>(node: &'a MenuNode, key: &str) -> Option<&'a str> {
    match node.props.get(key) {
        Some(MenuValue::Str(value)) => Some(value.as_str()),
        _ => None,
    }
}

fn int_prop(node: &MenuNode, key: &str) -> Option<i32> {
    match node.props.get(key) {
        Some(MenuValue::Int(value)) => Some(*value),
        _ => None,
    }
}

/// dbusmenu mnemonics: a single `_` marks the next character and is dropped; `__` is a literal `_`.
#[must_use]
pub fn clean_label(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '_' {
            out.push(ch);
        } else if chars.peek() == Some(&'_') {
            chars.next();
            out.push('_');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: i32, props: &[(&str, MenuValue)], children: Vec<MenuNode>) -> MenuNode {
        MenuNode {
            id,
            props: props
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect(),
            children,
        }
    }

    fn label(text: &str) -> MenuValue {
        MenuValue::Str(text.to_owned())
    }

    #[test]
    fn mnemonics_are_stripped_and_double_underscore_kept() {
        assert_eq!(clean_label("_File"), "File");
        assert_eq!(clean_label("Save __As"), "Save _As");
        assert_eq!(clean_label("trailing_"), "trailing");
        assert_eq!(clean_label(""), "");
    }

    #[test]
    fn hidden_items_are_skipped_and_separators_kept() {
        let root = node(
            0,
            &[],
            vec![
                node(1, &[("label", label("_Open"))], vec![]),
                node(
                    2,
                    &[
                        ("visible", MenuValue::Bool(false)),
                        ("label", label("Hidden")),
                    ],
                    vec![],
                ),
                node(3, &[("type", label("separator"))], vec![]),
                node(
                    4,
                    &[
                        ("label", label("Quit")),
                        ("enabled", MenuValue::Bool(false)),
                    ],
                    vec![],
                ),
            ],
        );
        assert_eq!(
            parse_layout(&root),
            vec![
                MenuEntry::Item {
                    id: 1,
                    label: "Open".into(),
                    enabled: true,
                    toggle: None,
                    submenu: vec![]
                },
                MenuEntry::Separator,
                MenuEntry::Item {
                    id: 4,
                    label: "Quit".into(),
                    enabled: false,
                    toggle: None,
                    submenu: vec![]
                },
            ]
        );
    }

    #[test]
    fn toggles_and_submenus_are_nested() {
        let root = node(
            0,
            &[],
            vec![node(
                10,
                &[
                    ("label", label("View")),
                    ("children-display", label("submenu")),
                ],
                vec![node(
                    11,
                    &[
                        ("label", label("Wrap")),
                        ("toggle-type", label("checkmark")),
                        ("toggle-state", MenuValue::Int(1)),
                    ],
                    vec![],
                )],
            )],
        );
        assert_eq!(
            parse_layout(&root),
            vec![MenuEntry::Item {
                id: 10,
                label: "View".into(),
                enabled: true,
                toggle: None,
                submenu: vec![MenuEntry::Item {
                    id: 11,
                    label: "Wrap".into(),
                    enabled: true,
                    toggle: Some(true),
                    submenu: vec![],
                }],
            }]
        );
    }

    #[test]
    fn depth_and_entry_budget_are_bounded() {
        let mut deep = node(100, &[("label", label("leaf"))], vec![]);
        for id in (0..20).rev() {
            deep = node(id, &[("label", label("n"))], vec![deep]);
        }
        let root = node(0, &[], vec![deep]);
        let mut depth = 0;
        let mut cursor = parse_layout(&root);
        while let Some(MenuEntry::Item { submenu, .. }) = cursor.first().cloned() {
            depth += 1;
            cursor = submenu;
        }
        assert_eq!(depth, MAX_MENU_DEPTH);

        let wide = node(
            0,
            &[],
            (0..MAX_MENU_ENTRIES as i32 + 50)
                .map(|id| node(id, &[("label", label("x"))], vec![]))
                .collect(),
        );
        assert_eq!(parse_layout(&wide).len(), MAX_MENU_ENTRIES);
    }
}

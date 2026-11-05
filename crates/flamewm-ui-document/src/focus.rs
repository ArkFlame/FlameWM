//! Shared keyboard focus owner for compiled UI documents.
//!
//! One owner per document: it holds the focused node index, writes it into
//! [`InteractionState::focus`] so `:focus-visible` styles apply, and turns
//! Tab / Shift+Tab / Enter / Space into traversal or an activation request.
//! Focusable nodes are element nodes with a non-empty `data-action`: buttons,
//! inputs and list rows all carry one. Traversal is document order with wrap-around.

use crate::{CompiledDocument, CompiledNode, InteractionState, NodeKind};

/// Whether `node` takes keyboard focus.
#[must_use]
pub fn is_focusable(node: &CompiledNode) -> bool {
    node.kind == NodeKind::Element && !node.action.is_empty()
}

/// Next focusable node after `current` in document order (before it when `reverse`).
/// With no current focus, Tab picks the first node and Shift+Tab the last.
#[must_use]
pub fn next_focusable(document: &CompiledDocument, current: Option<u32>, reverse: bool) -> Option<u32> {
    let count = document.nodes.len();
    if count == 0 {
        return None;
    }
    let focusable = |index: usize| is_focusable(&document.nodes[index]);
    match current.map(|index| index as usize).filter(|index| *index < count) {
        None => {
            if reverse {
                (0..count).rev().find(|&index| focusable(index)).map(|index| index as u32)
            } else {
                (0..count).find(|&index| focusable(index)).map(|index| index as u32)
            }
        }
        Some(current) => {
            let steps = 1..count;
            let order: Box<dyn Iterator<Item = usize>> = if reverse {
                Box::new(steps.map(move |step| (current + count - step) % count))
            } else {
                Box::new(steps.map(move |step| (current + step) % count))
            };
            order.filter(|&index| focusable(index)).map(|index| index as u32).next()
        }
    }
}

/// Result of routing one key press through the focus owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusKey {
    /// The key is not a focus key; the caller keeps handling it.
    Ignored,
    /// Focus moved (or was cleared); repaint.
    Moved,
    /// Enter or Space on the focused node; the caller dispatches its `data-action`.
    Activate(u32),
}

/// Keyboard focus owner for one document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FocusOwner {
    focus: Option<u32>,
}

impl FocusOwner {
    #[must_use]
    pub fn focus(&self) -> Option<u32> {
        self.focus
    }

    /// Focus `index` when it is a focusable node, otherwise clear focus.
    pub fn set(&mut self, document: &CompiledDocument, index: Option<u32>) -> bool {
        let next = index.filter(|&index| {
            document
                .nodes
                .get(index as usize)
                .is_some_and(is_focusable)
        });
        let changed = next != self.focus;
        self.focus = next;
        changed
    }

    /// Copy the focus into the interaction state used by the layout and paint path.
    pub fn apply(&self, interaction: &mut InteractionState) {
        interaction.focus = self.focus;
    }

    /// Route one key press. `key` uses the X11 keysym names the UI event layer delivers.
    pub fn handle_key(&mut self, document: &CompiledDocument, key: &str, shift: bool) -> FocusKey {
        match key {
            "Tab" | "ISO_Left_Tab" => {
                let next = next_focusable(document, self.focus, shift);
                let changed = next != self.focus;
                self.focus = next;
                if changed { FocusKey::Moved } else { FocusKey::Ignored }
            }
            "Return" | "Enter" | "KP_Enter" | "space" | "Space" | " " => match self.focus {
                Some(index) => FocusKey::Activate(index),
                None => FocusKey::Ignored,
            },
            _ => FocusKey::Ignored,
        }
    }
}

/// The `data-action` of `index`, when it is focusable.
#[must_use]
pub fn action_for(document: &CompiledDocument, index: u32) -> Option<&str> {
    document
        .nodes
        .get(index as usize)
        .filter(|node| is_focusable(node))
        .map(|node| node.action.as_ref())
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::Arc;

    use crate::{CompiledDocument, CompiledNode, InteractionState, NodeKind, Style};

    use super::{action_for, next_focusable, FocusKey, FocusOwner};

    fn node(kind: NodeKind, action: &'static str) -> CompiledNode {
        CompiledNode {
            kind,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: Cow::Borrowed(""),
            action: Cow::Borrowed(action),
            text: Cow::Borrowed(""),
            image: None,
            style: Arc::new(Style::default()),
            hover_style: None,
            active_style: None,
            focus_style: None,
        }
    }

    /// 0 root (no action), 1 button, 2 text, 3 row, 4 button.
    fn document() -> CompiledDocument {
        CompiledDocument {
            source_fingerprint: 0,
            root: 0,
            variables: Vec::new(),
            assets: Vec::new(),
            nodes: vec![
                node(NodeKind::Element, ""),
                node(NodeKind::Element, "open"),
                node(NodeKind::Text, "ignored"),
                node(NodeKind::Element, "row"),
                node(NodeKind::Element, "close"),
            ],
        }
    }

    #[test]
    fn tab_walks_focusables_in_document_order_and_wraps() {
        let doc = document();
        assert_eq!(next_focusable(&doc, None, false), Some(1));
        assert_eq!(next_focusable(&doc, Some(1), false), Some(3));
        assert_eq!(next_focusable(&doc, Some(3), false), Some(4));
        assert_eq!(next_focusable(&doc, Some(4), false), Some(1));
    }

    #[test]
    fn shift_tab_walks_backwards_and_wraps() {
        let doc = document();
        assert_eq!(next_focusable(&doc, None, true), Some(4));
        assert_eq!(next_focusable(&doc, Some(4), true), Some(3));
        assert_eq!(next_focusable(&doc, Some(1), true), Some(4));
    }

    #[test]
    fn document_without_focusables_has_no_focus() {
        let doc = CompiledDocument { nodes: vec![node(NodeKind::Element, "")], ..document() };
        assert_eq!(next_focusable(&doc, None, false), None);
        assert_eq!(next_focusable(&CompiledDocument { nodes: vec![], ..document() }, None, false), None);
    }

    #[test]
    fn owner_moves_activates_and_writes_interaction_focus() {
        let doc = document();
        let mut owner = FocusOwner::default();
        assert_eq!(owner.handle_key(&doc, "Return", false), FocusKey::Ignored);
        assert_eq!(owner.handle_key(&doc, "Tab", false), FocusKey::Moved);
        assert_eq!(owner.focus(), Some(1));
        assert_eq!(owner.handle_key(&doc, "space", false), FocusKey::Activate(1));
        // X11 canonical name for the Space keysym (events.rs canonical_key_name).
        assert_eq!(owner.handle_key(&doc, "Space", false), FocusKey::Activate(1));
        assert_eq!(owner.handle_key(&doc, "Tab", true), FocusKey::Moved);
        assert_eq!(owner.focus(), Some(4));
        assert_eq!(action_for(&doc, 4), Some("close"));
        let mut interaction = InteractionState::default();
        owner.apply(&mut interaction);
        assert_eq!(interaction.focus, Some(4));
        assert_eq!(owner.handle_key(&doc, "Left", false), FocusKey::Ignored);
    }

    #[test]
    fn set_rejects_non_focusable_targets() {
        let doc = document();
        let mut owner = FocusOwner::default();
        assert!(!owner.set(&doc, Some(0)));
        assert!(!owner.set(&doc, Some(2)));
        assert!(!owner.set(&doc, Some(99)));
        assert!(owner.set(&doc, Some(3)));
        assert!(!owner.set(&doc, Some(3)));
        assert_eq!(owner.focus(), Some(3));
    }
}

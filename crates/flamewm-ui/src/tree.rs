use crate::id::WidgetId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiNode {
    pub id: WidgetId,
    pub kind: crate::components::WidgetKind,
    pub children: Vec<Self>,
}

impl UiNode {
    #[must_use]
    pub fn new(id: impl Into<WidgetId>, kind: crate::components::WidgetKind) -> Self {
        Self {
            id: id.into(),
            kind,
            children: Vec::new(),
        }
    }

    pub fn push(&mut self, child: Self) {
        self.children.push(child);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiTree {
    pub root: UiNode,
}

impl UiTree {
    #[must_use]
    pub fn new(root: UiNode) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn find(&self, id: &WidgetId) -> Option<&UiNode> {
        fn visit<'a>(node: &'a UiNode, id: &WidgetId) -> Option<&'a UiNode> {
            if &node.id == id {
                return Some(node);
            }
            node.children.iter().find_map(|child| visit(child, id))
        }
        visit(&self.root, id)
    }
}

pub use crate::components::WidgetKind;

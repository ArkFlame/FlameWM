//! Stable menu identity/model independent from any X11 renderer.

use std::collections::BTreeSet;

use crate::IconRole;

pub type MenuItemId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub id: MenuItemId,
    pub label: String,
    pub icon: IconRole,
    pub enabled: bool,
    pub checked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuModelEntry {
    Item(MenuItem),
    Separator { id: MenuItemId },
}

impl MenuModelEntry {
    #[must_use]
    pub const fn id(&self) -> MenuItemId {
        match self {
            Self::Item(item) => item.id,
            Self::Separator { id } => *id,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuModel {
    entries: Vec<MenuModelEntry>,
    ids: BTreeSet<MenuItemId>,
}

impl MenuModel {
    pub fn add_item(&mut self, item: MenuItem) -> bool {
        if item.id == 0 || !self.ids.insert(item.id) {
            return false;
        }
        self.entries.push(MenuModelEntry::Item(item));
        true
    }

    #[must_use]
    pub fn contains(&self, id: MenuItemId) -> bool {
        self.ids.contains(&id)
    }

    #[must_use]
    pub fn entries(&self) -> &[MenuModelEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_stable_menu_id_is_rejected() {
        let mut model = MenuModel::default();
        let item = MenuItem {
            id: 7,
            label: "Open".to_owned(),
            icon: IconRole::Start,
            enabled: true,
            checked: false,
        };
        assert!(model.add_item(item.clone()));
        assert!(!model.add_item(item));
    }
}

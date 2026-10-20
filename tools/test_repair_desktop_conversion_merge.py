"""Reproduce the E0432/E0599/E0061 partial merge and assert non-destructive repair."""
from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from repair_desktop_conversion_merge import (
    IncompatibleSource, apply, prepare, updated_core,
)
from verify_desktop_conversion_contract import contract_errors

LIB_LEGACY = "//! user untouched header\npub mod file_actions;\npub mod layout;\n"
ACTIONS_LEGACY = '''//! user untouched header
pub enum EntryMenuAction {
    EmptyTrash,
    Rename,
    CreateShortcut,
    Delete,
}
impl EntryMenuAction {
    pub const fn label(self) -> &'static str {
        match self {
            Self::EmptyTrash => "Empty Trash",
            Self::Rename => "Rename",
            Self::CreateShortcut => "Create Shortcut",
            Self::Delete => "Delete",
        }
    }
}
''' + '''#[must_use]
pub fn entry_context_menu(is_trash: bool) -> Vec<EntryMenuAction> {
    if is_trash {
        vec![EntryMenuAction::EmptyTrash]
    } else {
        vec![
            EntryMenuAction::Rename,
            EntryMenuAction::CreateShortcut,
            EntryMenuAction::Delete,
        ]
    }
}
''' + '''/// unrelated legacy operation
pub fn untouched() {}
'''
ACTIONS_LEGACY += '''#[cfg(test)]
mod tests {
    #[test]
    fn entry_menu_maps_trash_to_empty_trash_only() {
        assert_eq!(entry_context_menu(true), vec![EntryMenuAction::EmptyTrash]);
        assert_eq!(
            entry_context_menu(false),
            vec![
                EntryMenuAction::Rename,
                EntryMenuAction::CreateShortcut,
                EntryMenuAction::Delete
            ]
        );
    }
}
'''
CONSUMER = 'use flamewm_desktop_core::conversion::offered_formats;\nEntryMenuAction::Convert;\n'


class DesktopRepairTests(unittest.TestCase):
    def fixture(self, *, tamper=False):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        for filename, contents in {
            'Cargo.toml': '[workspace]\nmembers = ["crates/*"]\n',
            'crates/flamewm-desktop/src/main.rs': CONSUMER,
            'crates/flamewm-desktop-core/Cargo.toml': '[package]\nname = "flamewm-desktop-core"\n',
            'crates/flamewm-desktop-core/src/lib.rs': LIB_LEGACY,
            'crates/flamewm-desktop-core/src/file_actions.rs': (ACTIONS_LEGACY.replace(
                '    if is_trash {\n', '    if is_trash { /* concurrent edit */\n') if tamper else ACTIONS_LEGACY),
        }.items():
            target = root / filename
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(contents, encoding='utf-8')
        return root

    def test_reproduces_three_original_missing_api_errors(self):
        root = self.fixture()
        errors = contract_errors(root)
        self.assertTrue(any('does not export' in error for error in errors), errors)
        self.assertTrue(any('lacks Convert variant' in error for error in errors), errors)
        self.assertTrue(any('must accept' in error for error in errors), errors)
        self.assertTrue(any('conversion.rs' in error for error in errors), errors)

    def test_repair_preserves_other_code_and_backs_up_legacy_files(self):
        root = self.fixture()
        plan = prepare(root)
        self.assertEqual(len(plan), 3)
        self.assertNotEqual(plan['crates/flamewm-desktop-core/src/lib.rs'], LIB_LEGACY.encode())
        self.assertFalse((root / 'crates/flamewm-desktop-core/src/conversion.rs').exists())
        backup = apply(root, plan)
        self.assertEqual(contract_errors(root), [])
        self.assertIn('pub fn untouched()', (root / 'crates/flamewm-desktop-core/src/file_actions.rs').read_text())
        self.assertIn('entry_context_menu(true, true)', (root / 'crates/flamewm-desktop-core/src/file_actions.rs').read_text())
        self.assertEqual((backup / 'crates/flamewm-desktop-core/src/lib.rs').read_text(), LIB_LEGACY)
        self.assertEqual((backup / 'crates/flamewm-desktop-core/src/file_actions.rs').read_text(), ACTIONS_LEGACY)
        self.assertIsNone(apply(root, prepare(root)))
        self.assertEqual(prepare(root), {})

    def test_unknown_menu_layout_fails_closed_without_writes(self):
        root = self.fixture(tamper=True)
        before = (root / 'crates/flamewm-desktop-core/src/file_actions.rs').read_bytes()
        with self.assertRaisesRegex(IncompatibleSource, 'refusing to overwrite custom source'):
            prepare(root)
        self.assertEqual((root / 'crates/flamewm-desktop-core/src/file_actions.rs').read_bytes(), before)
        self.assertFalse((root / 'crates/flamewm-desktop-core/src/conversion.rs').exists())

    def test_stale_plan_refused(self):
        root = self.fixture()
        plan = prepare(root)
        lib = root / 'crates/flamewm-desktop-core/src/lib.rs'
        lib.write_text(lib.read_text() + '\n// concurrent source edit\n')
        with self.assertRaisesRegex(IncompatibleSource, 'source changed'):
            apply(root, plan)

    def test_updater_is_idempotent_for_existing_contract(self):
        updated_lib, updated_actions = updated_core(LIB_LEGACY, ACTIONS_LEGACY)
        self.assertEqual(updated_core(updated_lib, updated_actions), (updated_lib, updated_actions))
        self.assertIn('if convertible {', updated_actions)
        self.assertIn('rows.push(EntryMenuAction::Convert);', updated_actions)


if __name__ == '__main__':
    unittest.main()

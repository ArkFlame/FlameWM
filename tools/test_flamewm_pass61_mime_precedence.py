"""Pass 61: XDG MIME ordering and handler visibility source contracts.

These are source-level regressions. Behavioral Rust tests in mime.rs and the
shared application catalog must still run with cargo on the target host.
"""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
MIME = ROOT / 'crates/flamewm-explorer/src/mime.rs'
CATALOG = ROOT / 'crates/flamewm-applications/src/lib.rs'


class MimePrecedence(unittest.TestCase):
    def test_added_removed_associations_are_resolved(self):
        src = MIME.read_text()
        for symbol in ('parse_association_changes', 'associated_ids',
                       '"Added Associations"', '"Removed Associations"',
                       'accepted.push(id.clone())', 'removed.insert(id)',
                       'owners[index].is_some_and(|owner| owner < layer_index)'):
            self.assertIn(symbol, src)
        self.assertIn('higher_precedence_removal_and_addition_respect_desktop_file_priority', src)

    def test_xdg_precedence_is_explicit_and_worker_owned(self):
        src = MIME.read_text()
        for value in ('XDG_CONFIG_HOME', 'XDG_CONFIG_DIRS',
                      'XDG_DATA_HOME', 'XDG_DATA_DIRS', 'mimeapps.list'):
            self.assertIn(value, src)
        self.assertIn('association_layers()', src)
        self.assertIn('ApplicationCatalog::discover_file_handlers()', src)
        self.assertIn('xdg_query(&[OsStr::new("query")', src)
        main = (ROOT/'crates/flamewm-explorer/src/main.rs').read_text()
        self.assertIn('fn chooser_worker', main)
        self.assertNotIn('Command::new("xdg-mime")', main)

    def test_handler_visibility_is_distinct_from_start_menu(self):
        src = CATALOG.read_text()
        self.assertIn('pub fn discover_file_handlers()', src)
        self.assertIn('Self::from_dirs_with_visibility(&xdg_application_dirs(), true)', src)
        self.assertIn('Self::from_dirs_with_visibility(dirs, false)', src)
        self.assertIn('!handlers && is_true(values.get("NoDisplay"))', src)
        self.assertIn('!handlers && !desktop_environment_matches(&values)', src)
        self.assertIn('mime_handler_catalog_includes_nodisplay_and_ignores_one_bad_launcher', src)

    def test_one_broken_launcher_cannot_abort_discovery(self):
        src = CATALOG.read_text()
        self.assertIn('match parse_desktop_entry(&path, id, handlers)', src)
        self.assertIn('Err(error) => eprintln!', src)
        self.assertIn('item.file_type()', src)
        self.assertNotIn('if path.is_dir() {\n            collect_desktop_files(root', src)

    def test_explicit_default_stays_visible_after_always_use(self):
        src = MIME.read_text()
        self.assertIn('let is_default = entry.id().as_str() == default;', src)
        self.assertIn('let associated = is_default || positions.contains_key', src)
        self.assertIn('OsStr::new("default")', src)
        self.assertIn('fn set_default', src)


if __name__ == '__main__':
    unittest.main()

"""Offline regressions for the partial-merge E0432/E0599/E0061 failure."""
from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from verify_desktop_conversion_contract import contract_errors


class DesktopConversionContractTests(unittest.TestCase):
    def make_fixture(self, *, include_module=True, include_variant=True,
                     include_signature=True, include_provider=True, include_consumer=True,
                     include_menu_module=True, include_consumer_file=True):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        consumer = root / "crates/flamewm-desktop/src/main.rs"
        consumer.parent.mkdir(parents=True)
        consumer.write_text(
            "use flamewm_desktop_core::conversion::offered_formats;\n"
            + ("mod menu_actions;\n" if include_menu_module else ""),
            encoding="utf-8",
        )
        menu_actions = consumer.parent / "menu_actions.rs"
        if include_consumer_file:
            menu_actions.write_text(
                ('"entry.convert" => { offered_formats(path); convert_file(path, format); }\n'
                 if include_consumer else ""), encoding="utf-8")
        core = root / "crates/flamewm-desktop-core/src"
        core.mkdir(parents=True)
        (core / "lib.rs").write_text("pub mod file_actions;\n" +
              ("pub mod conversion;\n" if include_module else ""), encoding="utf-8")
        (core / "file_actions.rs").write_text(
            "pub enum EntryMenuAction { Rename, CreateShortcut, " +
            ("Convert, " if include_variant else "") + "Delete }\n" +
            "impl EntryMenuAction { fn label(self) -> &'static str { match self { " +
            ('Self::Convert => "Convert", ' if include_variant else "") +
            '_ => "Other" } } }\n' +
            ("pub fn entry_context_menu(is_trash: bool, convertible: bool) { if convertible { rows.push(EntryMenuAction::Convert); } }\n" if include_signature
             else "pub fn entry_context_menu(is_trash: bool) {}\n"),
            encoding="utf-8")
        if include_provider:
            (core / "conversion.rs").write_text(
                "pub const FORMAT_CHOICES: [&str; 0] = [];\n"
                "pub fn offered_formats() {}\n"
                "pub fn convert_image() {}\n"
                "pub fn convert_file() {}\n"
                "pub fn predicted_destination() {}\n", encoding="utf-8")
        return root

    def test_current_archive_contract(self):
        self.assertEqual(contract_errors(Path(__file__).resolve().parent.parent), [])

    def test_incomplete_merge_reports_the_three_compiler_causes(self):
        errors = contract_errors(self.make_fixture(include_module=False, include_variant=False,
                                                   include_signature=False, include_provider=False))
        self.assertTrue(any("does not export" in msg for msg in errors))
        self.assertTrue(any("lacks Convert variant" in msg for msg in errors))
        self.assertTrue(any("must accept" in msg for msg in errors))
        self.assertTrue(any("missing" in msg and "conversion.rs" in msg for msg in errors))

    def test_independent_partial_states_are_detected(self):
        for key in ("include_module", "include_variant", "include_signature", "include_provider"):
            with self.subTest(key=key):
                self.assertTrue(contract_errors(self.make_fixture(**{key: False})))

    def test_split_consumer_fixture_is_valid(self):
        self.assertEqual(contract_errors(self.make_fixture()), [])

    def test_missing_split_consumer_is_detected(self):
        errors = contract_errors(self.make_fixture(include_consumer=False))
        self.assertTrue(any("menu_actions.rs" in msg for msg in errors))

    def test_missing_split_consumer_module_is_detected(self):
        errors = contract_errors(self.make_fixture(include_menu_module=False))
        self.assertTrue(any("main.rs" in msg for msg in errors))

    def test_missing_split_consumer_file_is_detected(self):
        errors = contract_errors(self.make_fixture(include_consumer_file=False))
        self.assertTrue(any("menu_actions.rs" in msg for msg in errors))


if __name__ == "__main__":
    unittest.main()

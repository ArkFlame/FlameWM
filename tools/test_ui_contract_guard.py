"""Unit tests for tools/ui_contract_guard.py on small synthetic trees."""

import contextlib
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest

GUARD_PATH = Path(__file__).resolve().parent / "ui_contract_guard.py"
spec = importlib.util.spec_from_file_location("ui_contract_guard", GUARD_PATH)
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class UiContractGuardTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(tempfile.mkdtemp(prefix="ui-guard-"))
        self.write("ui/settings.css", ".panel { color: red; }\n")
        self.write(
            "ui/settings.html",
            '<div id="panel" class="panel"><button id="save" data-action="settings.save">x</button>'
            '<div id="row-0"></div><div id="row-1"></div></div>\n',
        )
        self.write("ui/desktop/index.html", "<div></div>\n")
        self.write("ui/desktop/desktop.css", "")
        self.write("ui/index.html", "<div></div>\n")
        self.write("ui/flamewm.css", "")
        self.write("ui/shell/index.html", "<div></div>\n")
        self.write("ui/shell/flamewm.css", "")
        self.write("ui/assistant/osd.html", "<div></div>\n")
        self.write("ui/assistant/osd.css", "")
        self.write("crates/flamewm-assistant/src/lib.rs", "")
        self.write("crates/flamewm-desktop/build.rs", "")
        self.write("crates/flamewm-desktop/src/lib.rs", "")

    def write(self, rel, text):
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def run_guard(self, rust, handle_save=True):
        handler = 'fn p(a: &str) { if a == "settings.save" {} }\n' if handle_save else ""
        self.write("crates/flamewm-settings/src/lib.rs", handler + rust)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = guard.main(self.root)
        return code, out.getvalue()

    def test_clean_tree_passes(self):
        code, out = self.run_guard(
            'fn f(d: &mut D) { document.text("save", x); document.visible(&format!("row-{i}"), true); }\n'
            'const ACCENT_IDS: &[&str] = &["save"];\n'
            'fn p(a: &str) { if a == "settings.save" {} }\n'
        )
        self.assertEqual(code, 0, out)

    def test_missing_literal_id_is_an_error(self):
        code, out = self.run_guard('fn f(d: &mut D) { document.text("gone", x); }\n')
        self.assertEqual(code, 1)
        self.assertIn("'gone' missing", out)

    def test_missing_template_is_an_error(self):
        code, out = self.run_guard('fn f(d: &mut D) { document.visible(&format!("nope-{i}"), true); }\n')
        self.assertEqual(code, 1)
        self.assertIn("'nope-{i}'", out)

    def test_missing_const_id_is_an_error(self):
        code, out = self.run_guard('const ACCENT_NODE_IDS: &[&str] = &["save", "absent"];\n')
        self.assertEqual(code, 1)
        self.assertIn("'absent'", out)

    def test_unhandled_action_is_an_error(self):
        code, out = self.run_guard('fn p(a: &str) { if a == "settings.other" {} }\n', handle_save=False)
        self.assertEqual(code, 1)
        self.assertIn("data-action 'settings.save'", out)

    def test_test_module_references_do_not_count(self):
        code, out = self.run_guard(
            '#[cfg(test)]\nmod tests {\n    fn t(d: &mut D) { document.text("only-in-tests", x); }\n}\n'
        )
        self.assertEqual(code, 0, out)

    def test_duplicate_id_is_an_error(self):
        self.write("ui/settings.html", '<div id="a"></div><div id="a"></div>\n')
        code, out = self.run_guard("")
        self.assertEqual(code, 1)
        self.assertIn("duplicate id 'a'", out)

    def test_template_placeholders_match_concrete_ids(self):
        pattern = guard.template_regex("display-output-{slot}-name")
        self.assertTrue(pattern.match("display-output-3-name"))
        self.assertFalse(pattern.match("display-output-3"))



class FocusVisibleCoverageTests(unittest.TestCase):
    """Regression for archiver: `<span class="btn" data-action>` had no focus ring."""

    def gaps(self, html, css):
        root = Path(tempfile.mkdtemp(prefix="ui-focus-"))
        (root / "ui").mkdir()
        (root / "ui/doc.html").write_text(html, encoding="utf-8")
        (root / "ui/doc.css").write_text(css, encoding="utf-8")
        (root / "ui/base").mkdir()
        (root / "ui/base/base.css").write_text("", encoding="utf-8")
        return guard.find_focus_gaps(root, "ui/doc.html", ["ui/doc.css"])

    def test_span_with_data_action_needs_focus_visible_rule(self):
        html = '<span class="btn" data-action="ar.prev">Prev</span>'
        self.assertEqual(self.gaps(html, ""), ["btn"])
        self.assertEqual(self.gaps(html, ".btn:focus-visible { border-color: red; }"), [])

    def test_button_elements_and_plain_spans_are_ignored(self):
        self.assertEqual(self.gaps('<button class="x" data-action="a">x</button>', ""), [])
        self.assertEqual(self.gaps('<span class="label">x</span>', ""), [])

    def test_one_ring_rule_on_any_class_covers_the_element(self):
        html = '<span class="btn primary" data-action="ar.all">Extract all</span>'
        self.assertEqual(self.gaps(html, ".btn:focus-visible { border-width: 1px; }"), [])

    def test_archiver_document_has_no_focus_gaps(self):
        root = Path(__file__).resolve().parents[1]
        self.assertEqual(
            guard.find_focus_gaps(root, "ui/archiver/archiver.html", ["ui/archiver/archiver.css"]), []
        )

if __name__ == "__main__":
    unittest.main()

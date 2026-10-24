"""Guard against desktop work_area_tests failing with Rust E0433."""

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DesktopTestScope(unittest.TestCase):
    def test_work_area_tests_import_btreemap_in_own_scope(self):
        module = (ROOT / "crates/flamewm-desktop/src/tests/work_area_tests.rs").read_text()
        self.assertRegex(module, r"(?m)^\s*use\s+std::collections::BTreeMap\s*;")
        self.assertIn("BTreeMap::new()", module)
        self.assertIn("BTreeMap::from(", module)


if __name__ == "__main__":
    unittest.main()

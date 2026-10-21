"""Guard against desktop work_area_tests failing with Rust E0433."""

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


class DesktopTestScope(unittest.TestCase):
    def test_work_area_tests_import_btreemap_in_own_scope(self):
        source = (ROOT / "crates/flamewm-desktop/src/main.rs").read_text()
        match = re.search(
            r"#\[cfg\(test\)\]\s*mod work_area_tests\s*\{(.*?)(?=\n#\[cfg\(test\)\]\s*mod |\Z)",
            source,
            re.S,
        )
        self.assertIsNotNone(match)
        module = match.group(1)
        self.assertRegex(module, r"(?m)^\s*use\s+std::collections::BTreeMap\s*;")
        self.assertIn("BTreeMap::new()", module)
        self.assertIn("BTreeMap::from(", module)


if __name__ == "__main__":
    unittest.main()

import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import architecture_guard as g

PROD = "pub fn a() {}\n"
TESTS = "#[cfg(test)]\nmod tests {\n" + "    fn t() {}\n" * 3000 + "}\n"


class SourceLocTest(unittest.TestCase):
    def loc(self, text):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "lib.rs"
            p.write_text(text, encoding="utf-8")
            return g.source_loc([p])

    def test_large_tests_do_not_count(self):
        self.assertEqual(self.loc(PROD + TESTS), 1)
        self.assertLessEqual(self.loc(PROD + TESTS), g.RESPONSIBILITY_LOC_LIMITS["core"])

    def test_large_production_still_exceeds(self):
        big = "pub fn a() {}\n" * 2500 + TESTS
        self.assertGreater(self.loc(big), g.RESPONSIBILITY_LOC_LIMITS["core"])


if __name__ == "__main__":
    unittest.main()

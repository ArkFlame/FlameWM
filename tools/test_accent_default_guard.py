import pathlib
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import accent_default_guard as guard  # noqa: E402


class AccentDefaultGuardTest(unittest.TestCase):
    def repo(self, files):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name)
        subprocess.run(["git", "-C", str(root), "init", "-q"], check=True)
        for name, text in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")
        subprocess.run(["git", "-C", str(root), "add", "-A"], check=True)
        return root

    def test_hardcoded_default_outside_owners_fails(self):
        root = self.repo({"crates/flamewm-finder/src/other.rs": "const A: u32 = 0x0053_9bff;\n",
                          "ui/shell/x.css": "a { color:#539BFF; }\n"})
        self.assertEqual(len(guard.violations(root)), 2)
        self.assertEqual(guard.main(str(root)), 1)

    def test_owner_files_and_unrelated_colours_pass(self):
        root = self.repo({"crates/flamewm-platform/src/settings.rs": 'accent: "#539BFF".to_owned(),\n',
                          "ui/shell/x.css": "a { color:#ef4048; }\n"})
        self.assertEqual(guard.violations(root), [])
        self.assertEqual(guard.main(str(root)), 0)

    def test_live_repository_has_no_stray_default_accent(self):
        self.assertEqual(guard.violations(pathlib.Path(guard.__file__).resolve().parent.parent), [])


if __name__ == "__main__":
    unittest.main()

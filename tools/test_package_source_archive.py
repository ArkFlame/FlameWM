import hashlib
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
BUILDER = ROOT / "scripts/package-source-archive"


class SourceArchiveTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name) / "flamewm"
        self.root.mkdir()
        self.manifest = self.root / "manifest.txt"
        self.manifest.write_text("crates/\nassets/\nui/\n.agents/\nroot-check\n@exec crates/run.sh\n@exec root-check\n", encoding="utf-8")
        (self.root / "crates").mkdir()
        (self.root / "assets").mkdir()
        (self.root / "assets/raster").mkdir()
        (self.root / "ui").mkdir()
        (self.root / "crates/tracked.rs").write_text("tracked", encoding="utf-8")
        (self.root / "crates/untracked.rs").write_text("untracked", encoding="utf-8")
        (self.root / "crates/run.sh").write_text("#!/bin/sh\n", encoding="utf-8")
        (self.root / "crates/run.sh").chmod(0o755)
        (self.root / "assets/icon.svg").write_text("<svg/>", encoding="utf-8")
        (self.root / "root-check").symlink_to("crates/run.sh")
        (self.root / ".agents/runtime").mkdir(parents=True)
        (self.root / ".agents/runtime/private-canary.txt").write_text("private", encoding="utf-8")
        (self.root / "crates/__pycache__").mkdir()
        (self.root / "crates/__pycache__/secret.pyc").write_bytes(b"cache")
        (self.root / "crates/.env.token").write_text("secret", encoding="utf-8")
        (self.root / "crates/.ENV").write_text("secret", encoding="utf-8")
        (self.root / "crates/private.pem").write_text("secret", encoding="utf-8")
        (self.root / "assets/icon-link").symlink_to("icon.svg")
        (self.root / "ui/assets").symlink_to("../assets/raster")

    def tearDown(self):
        self.temp.cleanup()

    def archive(self, output):
        subprocess.run([str(BUILDER), str(self.root), str(self.manifest), str(output)], check=True)

    def test_members_metadata_symlink_mode_and_reproducibility(self):
        first, second = self.root / "a.tar.gz", self.root / "b.tar.gz"
        self.archive(first)
        self.archive(second)
        self.assertEqual(hashlib.sha256(first.read_bytes()).digest(), hashlib.sha256(second.read_bytes()).digest())
        with tarfile.open(first, "r:gz") as archive:
            entries = {item.name.removeprefix("flamewm/"): item for item in archive.getmembers()}
        self.assertTrue({"crates/tracked.rs", "crates/untracked.rs", "crates/run.sh", "assets/icon.svg", "assets/icon-link", "root-check", "ui/assets"} <= entries.keys())
        self.assertFalse(any(word in name.lower() for name in entries for word in ("private-canary", "pycache", ".env", ".pem")))
        self.assertEqual(entries["crates/run.sh"].mode & 0o777, 0o755)
        self.assertTrue(entries["assets/icon-link"].issym())
        self.assertEqual(entries["assets/icon-link"].linkname, "icon.svg")
        self.assertTrue(entries["root-check"].issym())
        self.assertEqual(entries["root-check"].linkname, "crates/run.sh")
        self.assertTrue(entries["ui/assets"].issym())
        self.assertEqual(entries["ui/assets"].linkname, "../assets/raster")
        self.assertTrue(all(item.uid == item.gid == 0 and item.mtime == 0 for item in entries.values()))
        extracted = self.root / "extracted"
        with tarfile.open(first, "r:gz") as archive:
            archive.extractall(extracted)
        self.assertTrue((extracted / "flamewm/ui/assets").is_symlink())
        self.assertTrue((extracted / "flamewm/ui/assets").is_dir())

    def test_rejects_manifest_traversal_and_absolute_paths(self):
        for invalid in ("../outside\n", "/etc/passwd\n"):
            self.manifest.write_text(invalid, encoding="utf-8")
            result = subprocess.run([str(BUILDER), str(self.root), str(self.manifest), str(self.root / "bad.tar.gz")], capture_output=True)
            self.assertNotEqual(result.returncode, 0)

    def test_rejects_unregistered_executable(self):
        path = self.root / "crates/unregistered.sh"
        path.write_text("#!/bin/sh\n", encoding="utf-8")
        path.chmod(0o755)
        result = subprocess.run([str(BUILDER), str(self.root), str(self.manifest), str(self.root / "bad.tar.gz")], capture_output=True)
        self.assertNotEqual(result.returncode, 0)

    def test_rejects_escaping_symlink(self):
        (self.root / "assets/escape").symlink_to("../../outside")
        result = subprocess.run([str(BUILDER), str(self.root), str(self.manifest), str(self.root / "bad.tar.gz")], capture_output=True)
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()

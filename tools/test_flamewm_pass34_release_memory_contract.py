"""Pass 34 release gate: registered scripts and allocation-free steady launch-key reuse."""
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "packaging/source-manifest.txt"
BUILDER = ROOT / "scripts/package-source-archive"
REQUIRED = (
    "scripts/apply-pass30-cargo-layout",
    "scripts/apply-pass33-merge",
    "scripts/enable-post31-desktop-integration",
    "scripts/flamewm-session-wrapper",
    "tools/flamewm_desktop_services.py",
)

class Pass34ReleaseMemoryTests(unittest.TestCase):
    def test_all_present_executable_scripts_are_registered(self):
        entries = set(MANIFEST.read_text(encoding="utf-8").splitlines())
        for dirname in ("scripts", "tools"):
            for candidate in (ROOT / dirname).iterdir():
                if candidate.is_file() and candidate.stat().st_mode & 0o111:
                    self.assertIn("@exec " + str(candidate.relative_to(ROOT)), entries,
                        f"source archive will reject executable {candidate}")
        for rel in REQUIRED:
            self.assertIn("@exec " + rel, entries)
            self.assertTrue((ROOT / rel).stat().st_mode & 0o111)

    def test_original_missing_registration_fails_and_fix_packages(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "flamewm"
            (root / "scripts").mkdir(parents=True)
            script = root / "scripts/apply-pass30-cargo-layout"
            script.write_bytes((ROOT / "scripts/apply-pass30-cargo-layout").read_bytes())
            script.chmod(0o755)
            manifest = root / "manifest.txt"
            manifest.write_text("scripts/\n", encoding="utf-8")
            output = root / "source.tar.gz"
            command = ["python3", str(BUILDER), str(root), str(manifest), str(output)]
            broken = subprocess.run(command, capture_output=True, text=True)
            self.assertNotEqual(broken.returncode, 0)
            self.assertIn("executable is not registered: scripts/apply-pass30-cargo-layout", broken.stderr)
            self.assertFalse(output.exists())
            manifest.write_text("scripts/\n@exec scripts/apply-pass30-cargo-layout\n", encoding="utf-8")
            fixed = subprocess.run(command, capture_output=True, text=True)
            self.assertEqual(fixed.returncode, 0, fixed.stderr)
            with tarfile.open(output, "r:gz") as tar:
                self.assertEqual(tar.getmember("flamewm/scripts/apply-pass30-cargo-layout").mode & 0o111, 0o111)

    def test_launch_feedback_reuses_key_instead_of_cloning_sequence_per_poll(self):
        source = (ROOT / "crates/flamewm-shell/src/launch_feedback.rs").read_text()
        self.assertNotIn("self.sequences.visible().cloned()", source)
        self.assertIn("updated_launch_key(", source)
        self.assertIn("old_id.as_str() == id", source)
        self.assertIn("self.painted = self.requested.clone();", source)
        self.assertIn("stable_launch_key_does_not_allocate_another_owned_key", source)

    def test_wire_quoting_has_no_temporary_vector_for_each_character(self):
        source = (ROOT / "crates/flamewm-applications/src/launch_feedback.rs").read_text()
        quoting = source.split("fn quote_field(value: &str) -> String {", 1)[1].split("\npub fn new_id", 1)[0]
        self.assertNotIn("vec![", quoting)
        self.assertNotIn("format!", quoting)
        self.assertIn("String::with_capacity(value.len() + 2)", quoting)
        self.assertIn("quote_field_escapes_without_changing_startup_wire_format", source)

if __name__ == "__main__":
    unittest.main()

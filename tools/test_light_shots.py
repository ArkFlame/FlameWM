"""Contract for scripts/ui/light_shots.sh: reproducible Light/Dark nested session capture."""
from pathlib import Path
import stat
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/ui/light_shots.sh"


class LightShotsContract(unittest.TestCase):
    def test_script_is_executable_and_registered(self):
        self.assertTrue(SCRIPT.stat().st_mode & stat.S_IXUSR)
        manifest = (ROOT / "packaging/source-manifest.txt").read_text(encoding="utf-8")
        self.assertIn("@exec scripts/ui/light_shots.sh", manifest.splitlines())

    def test_script_drives_isolated_session_without_name_kills(self):
        text = SCRIPT.read_text(encoding="utf-8")
        self.assertIn("appearance=%s", text)
        self.assertIn("FLAMEWM_CONFIG_DIR=", text)
        self.assertIn("FLAMEWM_XEPHYR_BUILD=0", text)
        self.assertIn("setsid", text)
        self.assertNotIn("pkill", text)
        self.assertNotIn("killall", text)


if __name__ == "__main__":
    unittest.main()

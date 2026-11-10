"""Contract checks for scripts/look-capture (the deterministic look capture)."""

import os
import stat
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "look-capture"


class LookCaptureContract(unittest.TestCase):
    def test_script_is_executable_and_parses(self):
        mode = SCRIPT.stat().st_mode
        self.assertTrue(mode & stat.S_IXUSR)
        subprocess.run(["bash", "-n", str(SCRIPT)], check=True)

    def test_display_floor_is_440(self):
        text = SCRIPT.read_text(encoding="utf-8")
        self.assertIn("((NUM >= 440))", text)

    def test_archiver_gets_an_archive_argument(self):
        # Without an archive argument flamewm-archiver prints usage and exits: a blank capture.
        text = SCRIPT.read_text(encoding="utf-8")
        self.assertIn('"$BIN/flamewm-archiver" "$WORK/sample.tar.gz"', text)

    def test_settle_is_hash_based_not_fixed_sleep(self):
        text = SCRIPT.read_text(encoding="utf-8")
        self.assertIn('[[ "$cur" == "$prev" ]]', text)


if __name__ == "__main__":
    unittest.main()

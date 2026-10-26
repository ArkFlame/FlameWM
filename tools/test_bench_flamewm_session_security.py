"""Security regressions for the nested-session benchmark (no compositor required)."""
from __future__ import annotations

import importlib.util
import os
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("bench-flamewm-session.py")
SPEC = importlib.util.spec_from_file_location("bench_flamewm_session", SCRIPT)
bench = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(bench)


class BenchmarkSessionSecurityTests(unittest.TestCase):
    def test_existing_directory_is_never_deleted_even_when_empty(self):
        with tempfile.TemporaryDirectory() as tmp:
            for name in ("empty", "contains-data"):
                output = Path(tmp) / name
                output.mkdir()
                if name == "contains-data":
                    (output / "precious.txt").write_text("do not delete")
                with self.assertRaises(SystemExit):
                    bench.ensure_safe_output_dir(output)
                self.assertTrue(output.is_dir())
                if name == "contains-data":
                    self.assertEqual((output / "precious.txt").read_text(), "do not delete")

    def test_new_directory_is_private_and_create_is_exclusive(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "new-output"
            with patch.dict(os.environ, {}, clear=False):
                actual = bench.ensure_safe_output_dir(output)
            self.assertEqual(actual, output.resolve())
            self.assertTrue(actual.is_dir())
            self.assertEqual(actual.stat().st_mode & 0o777, 0o700)
            with self.assertRaises(SystemExit):
                bench.ensure_safe_output_dir(output)

    def test_disallows_arbitrary_directories_outside_trusted_roots(self):
        with tempfile.TemporaryDirectory() as tmp:
            with patch.object(bench.tempfile, "gettempdir", return_value=tmp), \
                 patch.object(bench.Path, "cwd", return_value=Path(tmp)):
                outside = Path(tmp).parent / "do-not-write-here-flamewm"
                with self.assertRaises(SystemExit):
                    bench.ensure_safe_output_dir(outside)
                self.assertFalse(outside.exists())

    def test_existing_symlink_target_is_not_replaced_or_deleted(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            precious = root / "precious"
            precious.mkdir()
            (precious / "keep").write_text("intact")
            link = root / "link"
            link.symlink_to(precious, target_is_directory=True)
            with self.assertRaises(SystemExit):
                bench.ensure_safe_output_dir(link)
            self.assertTrue(link.is_symlink())
            self.assertEqual((precious / "keep").read_text(), "intact")

    def test_environment_drops_secrets_and_isolates_user_paths(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            host = root / "original-home"
            host.mkdir()
            (host / ".Xauthority").write_text("X11 cookie")
            fake = {
                "HOME": str(host), "DISPLAY": ":7", "XDG_RUNTIME_DIR": "/run/user/1000",
                "DBUS_SESSION_BUS_ADDRESS": "unix:path=/tmp/benchmark-private-bus",
                "API_KEY": "secret", "GITHUB_TOKEN": "secret", "LD_PRELOAD": "/tmp/inject.so",
                "PYTHONPATH": "/tmp/host/python", "XDG_DATA_HOME": str(host / ".local/share"),
                "FLAMEWM_WAYLAND_AUTOSTART": "/tmp/untrusted",
            }
            with patch.dict(os.environ, fake, clear=True):
                env = bench.benchmark_environment(root / "private-home", root / "out", root / "bin")
            for secret in ("API_KEY", "GITHUB_TOKEN", "LD_PRELOAD", "PYTHONPATH"):
                self.assertNotIn(secret, env)
            self.assertEqual(env["HOME"], str(root / "private-home"))
            self.assertEqual(env["XDG_DATA_HOME"], str(root / "private-home/data"))
            self.assertEqual(env["DISPLAY"], ":7")
            self.assertEqual(env["XAUTHORITY"], str(host / ".Xauthority"))
            self.assertEqual(env["FLAMEWM_WAYLAND_AUTOSTART"],
                             f"{root}/bin/flamewm-desktop:{root}/bin/flamewm-shell")

    def test_failed_launch_cleans_only_its_private_home(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            home = root / "private-home"
            out = root / "benchmark-output"

            def make_home(**_kwargs):
                home.mkdir()
                (home / "private-state").write_text("temporary")
                return str(home)

            with patch.dict(os.environ, {"FLAMEWM_BENCH_PRIVATE_BUS": "1"}, clear=True), \
                 patch.object(sys, "argv", [str(SCRIPT), "--out", str(out),
                                             "--bin", str(root / "missing-binaries")]), \
                 patch.object(bench.tempfile, "mkdtemp", side_effect=make_home):
                with self.assertRaises(FileNotFoundError):
                    bench.main()
            self.assertFalse(home.exists())
            self.assertTrue(out.exists())
            self.assertFalse((out / "report.json").exists())

    def test_xtest_rejects_host_display_and_missing_child(self):
        with patch.object(bench, "children_of", return_value=[101, 102]), \
             patch.object(bench, "exe_basename", side_effect=lambda pid: "Xwayland" if pid == 102 else "flamewm-wayland"):
            bench.validate_xwayland(":42", 101, ":0.0")
            with self.assertRaisesRegex(RuntimeError, "host X11"):
                bench.validate_xwayland(":0", 101, ":0.0")
        with patch.object(bench, "children_of", return_value=[101]), \
             patch.object(bench, "exe_basename", return_value="flamewm-wayland"):
            with self.assertRaisesRegex(RuntimeError, "No Xwayland"):
                bench.validate_xwayland(":42", 101, ":0")


if __name__ == "__main__":
    unittest.main()

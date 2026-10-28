#!/usr/bin/env python3
"""Source-only regression tests for the review, nested diagnostic, and cpak packaging."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
MODULE = ROOT / 'tools/cpak_package.py'
spec = importlib.util.spec_from_file_location('cpak_package', MODULE)
cpak = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cpak)


class CpakContracts(unittest.TestCase):
    def test_digest_is_mandatory(self):
        for image in ['ghcr.io/arkflame/flamewm:latest', 'ghcr.io/arkflame/flamewm@sha256:bogus', '']:
            with self.assertRaises(ValueError):
                cpak.make_manifest(image, '0.1.3')

    def test_manifest_is_application_only(self):
        manifest = cpak.make_manifest('ghcr.io/arkflame/flamewm@sha256:' + 'a' * 64, '0.1.3')
        self.assertEqual(manifest['manifest_version'], '3.0')
        self.assertNotIn('sessions', manifest)
        self.assertEqual(set(manifest['binaries']), set(cpak.BINARIES))
        self.assertFalse(manifest['override']['process'])
        self.assertFalse(manifest['override']['network'])
        self.assertEqual(manifest['override']['filesystem'], [])
        self.assertTrue(manifest['override']['displayX11'])

    def test_prepared_payload_cannot_install_host_login_sessions(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            payload = root / 'staged'
            for bin in ('flamefetch', 'flamewm-terminal'):
                f = payload / 'usr/bin' / bin
                f.parent.mkdir(parents=True, exist_ok=True)
                f.write_text('binary fixture')
            desktop = payload / 'usr/share/applications/flamewm-terminal.desktop'
            desktop.parent.mkdir(parents=True)
            desktop.write_text('Exec=flamewm-terminal\nTryExec=flamewm-terminal\n')
            for rel in ('usr/share/xsessions', 'usr/share/wayland-sessions', 'etc/pam.d'):
                f = payload / rel / 'test'
                f.parent.mkdir(parents=True)
                f.write_text('dangerous host session entry')
            out = root / 'context'
            cpak.prepare(out, payload)
            self.assertEqual((out / 'payload/usr/share/applications/flamewm-terminal.desktop').read_text(),
                             'Exec=/usr/bin/flamewm-terminal\nTryExec=/usr/bin/flamewm-terminal\n')
            for rel in ('usr/share/xsessions', 'usr/share/wayland-sessions', 'etc/pam.d'):
                self.assertFalse((out / 'payload' / rel).exists())
            self.assertTrue((out / 'Containerfile').is_file())
            with self.assertRaises(ValueError):
                cpak.prepare(out, payload)

    def test_session_supervisor_retains_restart_plans(self):
        src = (ROOT / 'crates/flamewm-wm/src/bin/flamewm-session.rs').read_text()
        self.assertIn('let wm = role_plans[0].clone()', src)
        self.assertIn('let plan = role_plans[index].clone()', src)
        self.assertIn('SHUTDOWN_SIGNAL => Ok(())', src)

    def test_nested_smoke_rejects_tty_without_display(self):
        script = ROOT / 'scripts/flamewm-nested-smoke'
        env = dict(os.environ)
        env.pop('DISPLAY', None)
        env.pop('WAYLAND_DISPLAY', None)
        p = subprocess.run([str(script), '--dry-run'], capture_output=True, text=True, env=env)
        self.assertEqual(p.returncode, 2)
        self.assertIn('no existing Wayland/X11 graphical display', p.stderr)

    def test_nested_smoke_is_explicitly_winit(self):
        src = (ROOT / 'scripts/flamewm-nested-smoke').read_text()
        self.assertIn('FLAMEWM_WAYLAND_BACKEND=winit', src)
        self.assertIn("FLAMEWM_WAYLAND_AUTOSTART=''", src)
        self.assertIn('timeout --signal=TERM --kill-after=2s 10s', src)


if __name__ == '__main__':
    unittest.main()

#!/usr/bin/env python3
"""Prevent reintroducing Cargo E0601 when updating an existing FlameWM source tree."""
import os
import shutil
import subprocess
import tempfile
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PKG = ROOT / 'crates/flamewm-wm'

class Pass30CargoLayout(unittest.TestCase):
    def test_real_helper_is_outside_cargo_auto_bin_directory(self):
        src = PKG / 'src'
        runner = (src / 'bin/flamewm-session.rs').read_text()
        code = (src / 'session_app_compat.rs').read_text()
        self.assertIn('#[path = "../session_app_compat.rs"]', runner)
        self.assertIn('mod session_app_compat;', runner)
        self.assertIn('session_app_compat::configure(', runner)
        self.assertIn('session_app_compat::finalize_pam_kwallet_login(', runner)
        self.assertIn('pub fn configure(', code)
        self.assertIn('pub fn finalize_pam_kwallet_login(', code)
        stale = src / 'bin/desktop_compat.rs'
        if stale.exists():
            self.assertIn('fn main() {}', stale.read_text())
            self.assertNotIn('pub fn configure(', stale.read_text())

    def test_explicit_targets_are_stable(self):
        manifest = tomllib.loads((PKG / 'Cargo.toml').read_text())
        self.assertFalse(manifest['package']['autobins'])
        binaries = {target['name']: target['path'] for target in manifest['bin']}
        self.assertEqual(binaries, {
            'flamewm': 'src/main.rs',
            'flamewm-session': 'src/bin/flamewm-session.rs',
        })

    def test_migration_removes_stale_old_source_and_rejects_bad_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            dst = Path(tmp)
            package = dst / 'crates/flamewm-wm'
            (package / 'src/bin').mkdir(parents=True)
            (dst / 'scripts').mkdir()
            for src, rel in (
                (PKG / 'Cargo.toml', 'Cargo.toml'),
                (PKG / 'src/bin/flamewm-session.rs', 'src/bin/flamewm-session.rs'),
                (PKG / 'src/session_app_compat.rs', 'src/session_app_compat.rs'),
            ):
                target = package / rel
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(src, target)
            shutil.copyfile(ROOT / 'scripts/apply-pass30-cargo-layout',
                            dst / 'scripts/apply-pass30-cargo-layout')
            stale = package / 'src/bin/desktop_compat.rs'
            stale.write_text('// Old Pass 29 source, deliberately missing main.\npub fn configure() {}\n')
            def run():
                return subprocess.run(['bash', str(dst / 'scripts/apply-pass30-cargo-layout')],
                                      cwd=dst, text=True, capture_output=True, env=os.environ.copy())
            result = run()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(stale.exists())
            self.assertEqual(run().returncode, 0, 'migration must be idempotent')
            manifest = package / 'Cargo.toml'
            manifest.write_text(manifest.read_text().replace('autobins = false', 'autobins = true'))
            stale.write_text('pub fn configure() {}\n')
            result = run()
            self.assertNotEqual(result.returncode, 0, 'bad manifest must block mutation')
            self.assertTrue(stale.exists(), 'do not delete user file with bad manifest')

if __name__ == '__main__':
    unittest.main()

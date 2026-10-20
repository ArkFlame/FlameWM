#!/usr/bin/env python3
"""Guard the session helper against Cargo's unintended src/bin auto-target discovery."""
from pathlib import Path
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / 'crates/flamewm-wm/Cargo.toml'


def valid_targets(source: bytes) -> bool:
    parsed = tomllib.loads(source.decode())
    expected = {'flamewm': 'src/main.rs', 'flamewm-session': 'src/bin/flamewm-session.rs'}
    declared = {entry['name']: entry['path'] for entry in parsed.get('bin', [])}
    return parsed['package'].get('autobins') is False and declared == expected


class CargoSessionTargets(unittest.TestCase):
    def test_helper_is_module_not_independent_executable(self):
        manifest = MANIFEST.read_bytes()
        self.assertTrue(valid_targets(manifest), 'Cargo must build only the two explicitly named binaries')
        root = MANIFEST.parent
        self.assertTrue((root / 'src/session_app_compat.rs').is_file())
        legacy = root / 'src/bin/desktop_compat.rs'
        if legacy.exists():
            self.assertIn('fn main() {}', legacy.read_text(), 'overlay must not reintroduce E0601')
        self.assertIn('mod session_app_compat;', (root / 'src/bin/flamewm-session.rs').read_text())
        self.assertIn('session_app_compat::configure(', (root / 'src/bin/flamewm-session.rs').read_text())
        self.assertIn('#[path = "../session_app_compat.rs"]', (root / 'src/bin/flamewm-session.rs').read_text())
        self.assertIn('finalize_pam_kwallet_login', (root / 'src/bin/flamewm-session.rs').read_text())
        package = (ROOT / 'scripts/package').read_text()
        self.assertIn('for binary in flamewm flamewm-session ', package)

    def test_guard_rejects_original_cargo_regression(self):
        original = MANIFEST.read_bytes()
        self.assertTrue(valid_targets(original))
        self.assertFalse(valid_targets(original.replace(b'autobins = false', b'autobins = true')))
        self.assertFalse(valid_targets(original.replace(b'name = "flamewm-session"', b'name = "desktop_compat"')))


if __name__ == '__main__':
    unittest.main()

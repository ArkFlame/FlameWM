"""KDE Wallet / Secret Service interop contracts.

Switching Plasma -> FlameWM -> Plasma must never delete, recreate, re-encrypt or
shadow an existing wallet. These static contracts pin the repo-side guarantees;
live wallet behavior remains a user-owned runtime check (see docs/KWALLET.md).
"""
from __future__ import annotations

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCAN_ROOTS = ('packaging', 'scripts', 'tools', 'crates', 'docs', 'Cargo.toml')
SKIP_PARTS = {'history', 'sourceport', 'target', '.target', '.vendor', 'tests', '.git'}

DELETE_RE = re.compile(
    r'\b(rm|rmdir|rmtree|remove_file|remove_dir|remove_dir_all|unlink|os\.remove|'
    r'os\.unlink|shutil\.rmtree|shred|find\b.*-delete)\b'
)
# Rust/Python delete calls only; a bare `rm` inside a Rust string literal is not a deletion.
CODE_DELETE_RE = re.compile(
    r'(remove_file|remove_dir_all|remove_dir|fs::remove|std::fs::remove|os\.remove|'
    r'os\.unlink|shutil\.rmtree|\.unlink\()'
)
# Wallet and keyring user data, plus user-home roots that deletion must never reach.
PROTECTED_RE = re.compile(
    r'kwalletd|kwalletrc|\.local/share/kwallet|\.local/share/keyrings|gnome-keyring|'
    r'\$\{?HOME\}?/\.local|~/\.local|\$\{?HOME\}?/\.config|~/\.config|\.kde'
)
GNOME_KEYRING_RE = re.compile(r'gnome-keyring|pam_gnome_keyring|gnome_keyring')


def scanned_files():
    for name in SCAN_ROOTS:
        base = ROOT / name
        paths = [base] if base.is_file() else (sorted(base.rglob('*')) if base.is_dir() else [])
        for path in paths:
            if not path.is_file() or SKIP_PARTS & set(path.relative_to(ROOT).parts):
                continue
            if path.name.startswith('test_') or path.suffix in {'.png', '.svg', '.gz', '.zip', '.ttf', '.otf'}:
                continue
            try:
                yield path, path.read_text(encoding='utf-8')
            except (UnicodeDecodeError, OSError):
                continue


class NoUserWalletDeletion(unittest.TestCase):
    def _delete_offenders(self, suffixes=None, name_filter=None):
        offenders = []
        for path, text in scanned_files():
            if suffixes is not None and path.suffix not in suffixes:
                continue
            if name_filter is not None and not name_filter(path):
                continue
            delete_re = CODE_DELETE_RE if path.suffix in {'.rs', '.py'} else DELETE_RE
            for number, line in enumerate(text.splitlines(), 1):
                if line.lstrip().startswith(('#', '//')):
                    continue
                if delete_re.search(line) and PROTECTED_RE.search(line):
                    offenders.append(f'{path.relative_to(ROOT)}:{number}')
        return offenders

    def test_no_shell_delete_targets_wallet_or_home_paths(self):
        offenders = self._delete_offenders(suffixes={'', '.sh'}, name_filter=lambda p: 'scripts' in p.parts or 'packaging' in p.parts)
        self.assertEqual(offenders, [], 'shell deletion must not target wallet or user home data')

    def test_no_rust_or_python_delete_line_targets_wallet_or_home_paths(self):
        offenders = self._delete_offenders(suffixes={'.rs', '.py'})
        self.assertEqual(offenders, [], 'delete calls must not target wallet/keyring or home config paths')

    def test_packaging_has_no_maintainer_script_that_removes_user_data(self):
        maintainer = [p for p, _ in scanned_files() if re.search(r'(pre|post)(inst|rm)$', p.name)]
        self.assertEqual(maintainer, [], 'no deb/arch maintainer script may exist without a reviewed contract')
        offenders = self._delete_offenders(name_filter=lambda p: 'packaging' in p.parts or p.name == 'package')
        self.assertEqual(offenders, [], 'packaging must never remove user data')
        for path, text in scanned_files():
            if 'packaging' in path.parts or path.name == 'package':
                self.assertNotRegex(text, r'(?m)^[^#\n]*\bpurge\b[^\n]*\.local')

    def test_kwallet_session_code_never_removes_wallet_directory(self):
        compat = (ROOT / 'crates/flamewm-wm/src/session_app_compat.rs').read_text()
        non_test = compat.split('#[cfg(test)]')[0]
        for line in non_test.splitlines():
            if re.search(r'remove_(file|dir|dir_all)', line):
                self.assertNotIn('kwallet', line.lower())
                self.assertNotIn('.local', line)
        self.assertNotRegex(non_test, r'kwalletd|kwalletrc|\.local/share/kwallet')


class NoGnomeKeyringShadowing(unittest.TestCase):
    def test_no_repo_source_starts_or_ships_gnome_keyring(self):
        # docs/ may describe the rule; only shipped or executed code is checked.
        offenders = [str(p.relative_to(ROOT)) for p, text in scanned_files()
                     if 'docs' not in p.parts and GNOME_KEYRING_RE.search(text)]
        self.assertEqual(offenders, [], 'gnome-keyring would shadow org.freedesktop.secrets')

    def test_pam_lock_stack_has_no_gnome_keyring_module(self):
        text = (ROOT / 'packaging/pam.d/flamewm-lock').read_text()
        self.assertFalse(GNOME_KEYRING_RE.search(text))


class KWalletActivation(unittest.TestCase):
    def setUp(self):
        self.session = (ROOT / 'crates/flamewm-wm/src/bin/flamewm-session.rs').read_text()

    def test_session_publishes_activation_environment_before_children(self):
        self.assertIn('sync_activation_environment(&activation_environment)', self.session)
        self.assertLess(self.session.index('sync_activation_environment(&activation_environment)'),
                        self.session.index('for process in &mut plan.processes'))

    def test_session_runs_pam_kwallet_handoff_without_starting_a_daemon(self):
        self.assertIn('finalize_pam_kwallet_login(&activation_environment)', self.session)
        compat = (ROOT / 'crates/flamewm-wm/src/session_app_compat.rs').read_text()
        self.assertNotRegex(compat, r'Command::new\("kwalletd[56]?"\)')
        self.assertNotRegex(compat, r'Command::new\("gnome-keyring')

    def test_kwallet_generation_selection_covers_both_versions(self):
        core = (ROOT / 'crates/flamewm-session-core/src/lib.rs').read_text()
        self.assertIn('"/usr/bin/kwalletd6"', core)
        self.assertIn('"/usr/bin/kwalletd5"', core)
        self.assertIn('org.kde.kwalletd6.service', core)
        self.assertIn('org.kde.kwalletd5.service', core)

    def test_secret_portal_selects_kwallet_not_keyring(self):
        conf = (ROOT / 'packaging/xdg-desktop-portal/flamewm-portals.conf').read_text()
        self.assertRegex(conf, r'(?m)^org\.freedesktop\.impl\.portal\.Secret=kwallet$')
        self.assertNotIn('gnome', conf.lower())

    def test_kwallet_packages_are_declared_for_deb_and_acceptance(self):
        control = (ROOT / 'packaging/debian/control.in').read_text()
        acceptance = (ROOT / 'tools/flamewm_deb_acceptance.py').read_text()
        self.assertIn('kwallet6', control)
        self.assertIn('libpam-kwallet5', control)
        self.assertIn('"kwallet6"', acceptance)


if __name__ == '__main__':
    unittest.main()

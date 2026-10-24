"""Deterministic tests of the one-time, package-scoped Cargo cache repair."""
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

from ensure_desktop_conversion_cargo import BuildGateError, content_digest, desktop_ui_digest, run


class BuildGateTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        files = {
            'Cargo.toml': '[workspace]\nmembers = ["crates/*"]\n',
            'crates/flamewm-desktop/Cargo.toml': '[package]\nname = "flamewm-desktop"\n',
            'crates/flamewm-desktop/src/main.rs':
                'use flamewm_desktop_core::conversion::offered_formats;\nEntryMenuAction::Convert;\n',
            'crates/flamewm-desktop-core/Cargo.toml': '[package]\nname = "flamewm-desktop-core"\n',
            'crates/flamewm-desktop-core/src/lib.rs': 'pub mod file_actions;\npub mod conversion;\n',
            'crates/flamewm-desktop-core/src/file_actions.rs':
                'pub enum EntryMenuAction { Convert, }\n'
                'impl EntryMenuAction { fn label(self) -> &\'static str { match self { Self::Convert => "Convert" } } }\n'
                'pub fn entry_context_menu(is_trash: bool, convertible: bool) {\n'
                '    if convertible { rows.push(EntryMenuAction::Convert); }\n}\n',
            'crates/flamewm-desktop/build.rs': 'compile_file(ui_desktop);',
            'crates/flamewm-desktop/ui_contract.rs': 'desktop-convert-wheel',
            'ui/desktop/index.html': '<body><section id="desktop-convert-wheel"></section></body>',
            'ui/desktop/desktop.css': '.convert-wheel { display: flex; }',
            'ui/shared/menu.css': '.context-menu { display: flex; }',
            'crates/flamewm-render-compiler/src/lib.rs': 'pub mod compile;',
            'crates/flamewm-render-compiler/src/compile.rs': 'fn compile_file() {}',
            'crates/flamewm-render-compiler/src/html.rs': 'fn parse_html() {}',
            'crates/flamewm-render-compiler/src/css.rs': 'fn parse_css() {}',
            'crates/flamewm-desktop-core/src/conversion.rs':
                'pub const FORMAT_CHOICES: [&str; 0] = [];\npub fn offered_formats() {}\npub fn convert_image() {}\npub fn convert_file() {}\npub fn predicted_destination() {}\n',
        }
        for rel, data in files.items():
            p = self.root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text(data)
        self.commands = []
        self.meta = self.metadata()
        self.clean_code = 0

    def metadata(self):
        root = self.root
        return {
            'workspace_root': str(root),
            'target_directory': str(root / '.target'),
            'workspace_members': ['p-core', 'p-desktop'],
            'packages': [
                {'name': 'flamewm-desktop-core', 'id': 'p-core',
                 'manifest_path': str(root/'crates/flamewm-desktop-core/Cargo.toml'),
                 'targets': [{'kind': ['lib'], 'src_path': str(root/'crates/flamewm-desktop-core/src/lib.rs')}]},
                {'name': 'flamewm-desktop', 'id': 'p-desktop',
                 'manifest_path': str(root/'crates/flamewm-desktop/Cargo.toml'),
                 'dependencies': [{'name': 'flamewm-desktop-core', 'path': str(root/'crates/flamewm-desktop-core')}],
                 'targets': []},
            ],
        }

    def execute(self, command, **kwargs):
        self.commands.append(command)
        if 'metadata' in command:
            return SimpleNamespace(returncode=0, stdout=json.dumps(self.meta), stderr='')
        return SimpleNamespace(returncode=self.clean_code, stdout='', stderr='')

    def test_first_run_cleans_release_core_once_then_reuses_digest(self):
        self.assertTrue(run(self.root, executor=self.execute))
        self.assertEqual(self.commands[1], ['cargo','clean','-p','flamewm-desktop-core','--release'])
        self.assertFalse(run(self.root, executor=self.execute))
        self.assertEqual(len(self.commands), 3)
        stamp = self.root/'.target/flamewm/desktop-core-release-cache-invalidated.sha256'
        self.assertEqual(stamp.read_text().strip(), content_digest(self.root))

    def test_same_content_with_archived_1970_mtimes_is_not_cleaned_twice(self):
        provider = self.root/'crates/flamewm-desktop-core/src/conversion.rs'
        provider.touch()
        self.assertTrue(run(self.root, executor=self.execute))
        import os
        os.utime(provider, (0, 0))
        self.assertFalse(run(self.root, executor=self.execute))

    def test_changed_content_invalidates_again_and_force_is_supported(self):
        self.assertTrue(run(self.root, executor=self.execute))
        with (self.root/'crates/flamewm-desktop-core/src/conversion.rs').open('a') as f:
            f.write('// user source edit\n')
        self.assertTrue(run(self.root, executor=self.execute))
        self.assertTrue(run(self.root, force=True, executor=self.execute))
        self.assertEqual(sum('clean' in command for command in self.commands),3)

    def test_dev_and_release_have_independent_stamps(self):
        self.assertTrue(run(self.root, profile='dev', executor=self.execute))
        self.assertTrue(run(self.root, profile='release', executor=self.execute))
        self.assertFalse(run(self.root, profile='dev', executor=self.execute))
        self.assertFalse(run(self.root, profile='release', executor=self.execute))
        self.assertEqual(self.commands[1], ['cargo','clean','-p','flamewm-desktop-core'])

    def test_wrong_cargo_source_is_rejected_without_clean(self):
        self.meta['packages'][1]['dependencies'][0]['path'] = '/tmp/not-this-core'
        with self.assertRaisesRegex(BuildGateError, 'not the local path crate'):
            run(self.root, executor=self.execute)
        self.assertFalse(any('clean' in command for command in self.commands))

    def test_wrong_cargo_workspace_is_rejected_without_clean(self):
        self.meta['workspace_root'] = '/tmp/wrong-tree'
        with self.assertRaisesRegex(BuildGateError, 'workspace root mismatch'):
            run(self.root, executor=self.execute)
        self.assertFalse(any('clean' in command for command in self.commands))

    def test_failed_clean_does_not_write_success_stamp(self):
        self.clean_code = 1
        with self.assertRaisesRegex(BuildGateError, 'targeted Cargo clean failed'):
            run(self.root, executor=self.execute)
        self.assertFalse((self.root/'.target/flamewm/desktop-core-release-cache-invalidated.sha256').exists())

    def test_missing_module_is_rejected_before_cargo(self):
        (self.root/'crates/flamewm-desktop-core/src/lib.rs').write_text('pub mod file_actions;\n')
        with self.assertRaisesRegex(BuildGateError, 'source API mismatch'):
            run(self.root, executor=self.execute)
        self.assertEqual(self.commands, [])

    def test_ui_fingerprint_cleans_desktop_package_on_first_run(self):
        self.assertTrue(run(self.root, include_ui=True, executor=self.execute))
        self.assertEqual(self.commands[1], ['cargo', 'clean', '-p', 'flamewm-desktop-core', '--release'])
        self.assertEqual(self.commands[2], ['cargo', 'clean', '-p', 'flamewm-desktop', '--release'])
        stamp = self.root / '.target/flamewm/desktop-rwr-release-cache-invalidated.sha256'
        self.assertEqual(stamp.read_text().strip(), desktop_ui_digest(self.root))
        self.assertFalse(run(self.root, include_ui=True, executor=self.execute))
        self.assertEqual(len(self.commands), 4)  # two metadata probes, two scoped cleans

    def test_old_mtime_html_content_change_still_invalidates_compiled_rwr(self):
        html = self.root / 'ui/desktop/index.html'
        self.assertTrue(run(self.root, include_ui=True, executor=self.execute))
        import os
        html.write_text(html.read_text().replace('desktop-convert-wheel', 'desktop-removed-wheel'))
        os.utime(html, (0, 0))
        self.assertTrue(run(self.root, include_ui=True, executor=self.execute))
        self.assertEqual(self.commands[-1], ['cargo', 'clean', '-p', 'flamewm-desktop', '--release'])
        self.assertFalse(run(self.root, include_ui=True, executor=self.execute))

    def test_ui_clean_failure_never_stamps_unvalidated_artifact(self):
        # Prove initial core clean succeeded, then fail ONLY desktop clean.
        def fail_desktop(command, **kwargs):
            self.commands.append(command)
            if 'metadata' in command:
                return SimpleNamespace(returncode=0, stdout=json.dumps(self.meta), stderr='')
            return SimpleNamespace(returncode=1 if 'flamewm-desktop' in command else 0,
                                   stdout='', stderr='')
        with self.assertRaisesRegex(BuildGateError, 'targeted Cargo clean failed for flamewm-desktop'):
            run(self.root, include_ui=True, executor=fail_desktop)
        self.assertFalse((self.root / '.target/flamewm/desktop-rwr-release-cache-invalidated.sha256').exists())

    def test_missing_ui_input_blocks_clean_before_mutation(self):
        (self.root / 'ui/desktop/index.html').unlink()
        with self.assertRaisesRegex(BuildGateError, 'missing or unreadable build input'):
            run(self.root, include_ui=True, executor=self.execute)
        self.assertEqual(self.commands, [['cargo', 'metadata', '--format-version', '1', '--no-deps', '--offline']])

    def test_isolated_python_cli_imports_checker_module(self):
        """Xephyr uses python3 -I; a normal unittest import alone missed this."""
        script = Path(__file__).resolve().parent / 'ensure_desktop_conversion_cargo.py'
        completed = subprocess.run([sys.executable, '-I', str(script), '--help'],
                                   capture_output=True, text=True, timeout=10)
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn('--profile', completed.stdout)

    def test_routine_rerun_without_force_is_a_noop_with_no_forced_clean(self):
        self.assertTrue(run(self.root, include_ui=True, executor=self.execute))
        cleans = len(self.commands)
        self.assertFalse(run(self.root, include_ui=True, executor=self.execute))
        self.assertEqual(sum('clean' in command for command in self.commands[cleans:]), 0)

    def test_unlisted_core_module_with_old_mtime_and_new_bytes_invalidates(self):
        module = self.root / 'crates/flamewm-desktop-core/src/late_module.rs'
        module.write_text('pub fn before() {}\n')
        self.assertTrue(run(self.root, executor=self.execute))
        import os
        module.write_text('pub fn after() {}\n')
        os.utime(module, (0, 0))
        self.assertTrue(run(self.root, executor=self.execute))
        self.assertEqual(sum('clean' in command for command in self.commands), 2)

    def test_stamp_is_scoped_to_resolved_target_dir(self):
        self.assertTrue(run(self.root, executor=self.execute))
        stamp = self.root / '.target/flamewm/desktop-core-release-cache-invalidated.sha256'
        self.assertTrue(stamp.is_file())
        self.meta['target_directory'] = str(self.root / 'alternate-target')
        self.assertTrue(run(self.root, executor=self.execute))
        self.assertTrue((self.root / 'alternate-target/flamewm/desktop-core-release-cache-invalidated.sha256').is_file())
        self.assertEqual(sum('clean' in command for command in self.commands), 2)

    def test_metadata_without_target_directory_is_rejected_without_clean(self):
        del self.meta['target_directory']
        with self.assertRaisesRegex(BuildGateError, 'missing workspace/core/desktop/target'):
            run(self.root, executor=self.execute)
        self.assertFalse(any('clean' in command for command in self.commands))

    def test_metadata_failure_does_not_clean(self):
        def fail(command, **kwargs):
            self.commands.append(command)
            return SimpleNamespace(returncode=101, stdout='', stderr='metadata failed')
        with self.assertRaisesRegex(BuildGateError, 'Cargo metadata failed'):
            run(self.root, executor=fail)
        self.assertEqual(len(self.commands),1)


if __name__ == '__main__':
    unittest.main()

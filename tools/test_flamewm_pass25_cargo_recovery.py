"""Regression for the real E0432/E0599/E0061 conversion/core ABI mismatch."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class Pass25DesktopCoreRecoveryTests(unittest.TestCase):
    def test_actual_source_provides_symbols_reported_missing_by_cargo(self):
        lib = (ROOT / 'crates/flamewm-desktop-core/src/lib.rs').read_text()
        actions = (ROOT / 'crates/flamewm-desktop-core/src/file_actions.rs').read_text()
        conversion = (ROOT / 'crates/flamewm-desktop-core/src/conversion.rs').read_text()
        self.assertRegex(lib, r'(?m)^pub mod conversion;')
        self.assertRegex(actions, r'(?s)enum EntryMenuAction\s*\{[^}]*\bConvert,')
        self.assertRegex(actions, r'fn entry_context_menu\(is_trash: bool, convertible: bool\)')
        for symbol in ('offered_formats', 'convert_file', 'predicted_destination'):
            self.assertRegex(conversion, rf'pub fn {symbol}\(')

    def test_one_command_build_repairs_cached_release_before_compiling(self):
        script = (ROOT / 'scripts/build-deb').read_text()
        gate = script.index('python3 -I tools/ensure_desktop_conversion_cargo.py --profile release --include-ui --force')
        cargo = script.index('cargo build --release --workspace')
        self.assertLess(gate, cargo)
        self.assertLess(script.index('rm -f "$DEB" "$DIST/SHA256SUMS"'), gate)

    def test_dev_check_repairs_cached_dev_before_compiling(self):
        script = (ROOT / 'scripts/check-cargo').read_text()
        self.assertLess(script.index('--profile dev --include-ui --force'),
                        script.index('cargo check --workspace --all-targets'))

    def test_dist_source_manifest_includes_safe_dev_check(self):
        manifest = (ROOT / 'packaging/source-manifest.txt').read_text()
        self.assertIn('@exec scripts/check-cargo\n', manifest)


if __name__ == '__main__':
    unittest.main()

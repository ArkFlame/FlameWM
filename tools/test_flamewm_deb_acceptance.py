import importlib.util
import unittest
from pathlib import Path

_SPEC = importlib.util.spec_from_file_location(
    "flamewm_deb_acceptance", Path(__file__).resolve().parent / "flamewm_deb_acceptance.py")
deb = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(deb)

GOOD = "\n".join(
    f"-rwxr-xr-x root/root 100 2026-10-13 06:54 ./usr/bin/{n}" for n in deb.BINARIES)


class ContentsTests(unittest.TestCase):
    def test_good_modes_pass(self):
        self.assertEqual(deb.check_binary_modes(deb.parse_contents(GOOD)), [])

    def test_wrong_mode_fails(self):
        text = GOOD.replace("-rwxr-xr-x root/root 100 2026-10-13 06:54 ./usr/bin/flamewm-lock",
                            "-rw-r--r-- root/root 100 2026-10-13 06:54 ./usr/bin/flamewm-lock")
        problems = deb.check_binary_modes(deb.parse_contents(text))
        self.assertEqual(len(problems), 1)
        self.assertIn("usr/bin/flamewm-lock", problems[0])

    def test_missing_binary_fails(self):
        text = "\n".join(l for l in GOOD.splitlines() if "flamewm-shell" not in l)
        self.assertIn("usr/bin/flamewm-shell missing", deb.check_binary_modes(deb.parse_contents(text)))

    def test_nonroot_owner_fails(self):
        text = GOOD.replace("root/root", "linsaftw/linsaftw", 1)
        self.assertEqual(len(deb.check_binary_modes(deb.parse_contents(text))), 1)

    def test_symlink_target_not_treated_as_leak(self):
        entries = deb.parse_contents("lrwxrwxrwx root/root 0 2026-10-13 06:54 ./usr/bin/x -> flamewm")
        self.assertEqual(entries[0][2], "usr/bin/x")

    def test_leak_detection(self):
        self.assertEqual(deb.find_leaks(["usr/share/x", "opt/build-root/y", "opt/run/media/q"]),
                         ["opt/build-root/y", "opt/run/media/q"])

    def test_leak_ignores_clean_paths(self):
        self.assertEqual(deb.find_leaks(["usr/share/flamewm/VERSION"]), [])


class DependsTests(unittest.TestCase):
    CONTROL_IN = Path(__file__).resolve().parent.parent / "packaging/debian/control.in"

    def _control_depends(self):
        for line in self.CONTROL_IN.read_text().splitlines():
            if line.startswith("Depends:"):
                return deb.depend_names(line[len("Depends:"):])
        self.fail("no Depends line in control.in")

    def test_required_depends_include_dbus_and_pulse(self):
        self.assertIn("libdbus-1-3", deb.REQUIRED_DEPENDS)
        self.assertIn("libpulse0", deb.REQUIRED_DEPENDS)

    def test_control_in_depends_cover_required(self):
        declared = self._control_depends()
        self.assertEqual([d for d in deb.REQUIRED_DEPENDS if d not in declared], [])


class PamRuntimeTests(unittest.TestCase):
    def test_required_depends_include_pam_runtime(self):
        self.assertIn("libpam0g", deb.REQUIRED_DEPENDS)
        self.assertIn("libpam-runtime", deb.REQUIRED_DEPENDS)

    def test_conffiles_exact_pam_entry_passes(self):
        self.assertEqual(deb.check_conffiles("/etc/pam.d/flamewm-lock\n"), [])

    def test_conffiles_missing_leading_slash_fails(self):
        self.assertEqual(len(deb.check_conffiles("etc/pam.d/flamewm-lock\n")), 1)

    def test_conffiles_missing_trailing_newline_fails(self):
        self.assertEqual(len(deb.check_conffiles("/etc/pam.d/flamewm-lock")), 1)

    def test_conffiles_extra_entry_fails(self):
        self.assertEqual(len(deb.check_conffiles("/etc/pam.d/flamewm-lock\n/etc/x\n")), 1)

    def test_conffiles_empty_fails(self):
        self.assertEqual(len(deb.check_conffiles("")), 1)


def _asset_entries(skip=()):
    lines = []
    for theme in deb.ICON_THEMES:
        if theme not in skip:
            lines.append(f"-rw-r--r-- root/root 10 2026-10-13 06:54 ./usr/share/icons/{theme}/index.theme")
    lines.append("-rw-r--r-- root/root 10 2026-10-13 06:54 ./usr/share/doc/flamewm/LICENSES/THEMES.md")
    lines.append("-rw-r--r-- root/root 10 2026-10-13 06:54 ./usr/share/doc/flamewm/copyright")
    return deb.parse_contents("\n".join(lines))


def _by_id(checks):
    return {c["id"]: c["status"] for c in checks}


class SharedAssetTests(unittest.TestCase):
    def test_all_assets_present_pass(self):
        status = _by_id(deb.check_shipped_assets(_asset_entries()))
        self.assertEqual(set(status.values()), {"PASS"})
        for theme in deb.ICON_THEMES:
            self.assertEqual(status[f"icon_theme_{theme}"], "PASS")
        self.assertEqual(status["doc_licenses_present"], "PASS")
        self.assertEqual(status["doc_copyright_present"], "PASS")

    def test_missing_icon_theme_fails_only_that_theme(self):
        status = _by_id(deb.check_shipped_assets(_asset_entries(skip=("candy-icons",))))
        self.assertEqual(status["icon_theme_candy-icons"], "FAIL")
        self.assertEqual(status["icon_theme_McMojave-circle"], "PASS")

    def test_theme_directory_entry_alone_is_not_files(self):
        entries = deb.parse_contents("drwxr-xr-x root/root 0 2026-10-13 06:54 ./usr/share/icons/candy-icons/")
        status = _by_id(deb.check_shipped_assets(entries))
        self.assertEqual(status["icon_theme_candy-icons"], "FAIL")

    def test_empty_licenses_dir_fails(self):
        entries = deb.parse_contents("drwxr-xr-x root/root 0 2026-10-13 06:54 ./usr/share/doc/flamewm/LICENSES/")
        self.assertEqual(_by_id(deb.check_shipped_assets(entries))["doc_licenses_present"], "FAIL")

    def test_missing_copyright_fails(self):
        entries = deb.parse_contents("-rw-r--r-- root/root 10 2026-10-13 06:54 ./usr/share/doc/flamewm/LICENSE")
        self.assertEqual(_by_id(deb.check_shipped_assets(entries))["doc_copyright_present"], "FAIL")

    def test_check_ids_are_stable(self):
        ids = [c["id"] for c in deb.check_shipped_assets([])]
        self.assertEqual(ids, [f"icon_theme_{t}" for t in deb.ICON_THEMES]
                         + ["doc_licenses_present", "doc_copyright_present"])


class HelperTests(unittest.TestCase):
    def test_depends_names_strip_versions_and_alternatives(self):
        self.assertEqual(deb.depend_names("libx11-6 (>= 2:1.6), libxi6 | libxi7, libxft2"),
                         ["libx11-6", "libxi6", "libxft2"])

    def test_sha256sums_lookup_by_basename(self):
        table = deb.parse_sha256sums("abc123  .target/dist/flamewm_{v}_amd64.deb\n".format(v=deb.workspace_version()))
        self.assertEqual(table[f"flamewm_{deb.workspace_version()}_amd64.deb"], "abc123")

    def test_info_parser_reads_version(self):
        self.assertEqual(deb.parse_info(f" Package: flamewm\n Version: {deb.workspace_version()}\n")["Version"], deb.workspace_version())


if __name__ == "__main__":
    unittest.main()

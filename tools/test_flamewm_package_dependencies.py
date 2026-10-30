"""Packaging dependency contract: docs/DEPENDENCIES.md is the table of record.

The Debian control template, the Arch PKGBUILD and its generated .SRCINFO must agree with
the table. Run: python3 -I -m unittest discover -s tools -p 'test_flamewm_package_dependencies.py' -q
"""
import importlib.util
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONTROL = ROOT / "packaging/debian/control.in"
PKGBUILD = ROOT / "packaging/arch/PKGBUILD"
SRCINFO = ROOT / "packaging/arch/.SRCINFO"
DOC = ROOT / "docs/DEPENDENCIES.md"
CLASSES = ("Depends", "Recommends", "Suggests")

_SPEC = importlib.util.spec_from_file_location(
    "flamewm_deb_acceptance", ROOT / "tools/flamewm_deb_acceptance.py")
deb = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(deb)


def control_fields():
    """Return {field: [first alternative of each entry]} and assert each field appears once."""
    fields = {}
    for line in CONTROL.read_text().splitlines():
        for name in CLASSES:
            if line.startswith(name + ":"):
                assert name not in fields, f"duplicate {name} field"
                fields[name] = deb.depend_names(line[len(name) + 1:])
    return fields


def docs_rows():
    rows = []
    for line in DOC.read_text().splitlines():
        if not line.startswith("| ") or line.startswith("| Debian") or line.startswith("| ---"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        rows.append(cells)
    return rows


def pkgbuild_array(name):
    match = re.search(rf"^{name}=\(\n(.*?)^\)", PKGBUILD.read_text(), re.S | re.M)
    assert match, f"no {name} array in PKGBUILD"
    return [line.strip().strip("'") for line in match.group(1).splitlines()
            if line.strip() and not line.strip().startswith("#")]


class DependencyTableTests(unittest.TestCase):
    def test_table_rows_are_complete(self):
        rows = docs_rows()
        self.assertTrue(rows, "no dependency rows in docs/DEPENDENCIES.md")
        for cells in rows:
            self.assertEqual(len(cells), 5, cells)
            self.assertTrue(all(cells), cells)
            self.assertIn(cells[2], CLASSES, cells)

    def test_control_fields_match_table(self):
        fields = control_fields()
        for name in CLASSES:
            with self.subTest(field=name):
                from_table = sorted(r[0] for r in docs_rows() if r[2] == name)
                self.assertEqual(sorted(fields.get(name, [])), from_table)

    def test_no_package_in_two_classes(self):
        fields = control_fields()
        seen = [pkg for name in CLASSES for pkg in fields.get(name, [])]
        self.assertEqual(sorted(set(seen)), sorted(seen))

    def test_wayland_and_session_requirements_are_depends(self):
        depends = control_fields()["Depends"]
        for pkg in ("libxkbcommon0", "libgbm1", "libdrm2", "libinput10", "libudev1",
                    "libegl1", "libgles2", "xwayland", "libpam-runtime", "python3"):
            with self.subTest(pkg=pkg):
                self.assertIn(pkg, depends)

    def test_test_only_and_session_owned_tools_are_not_declared(self):
        declared = {pkg for names in control_fields().values() for pkg in names}
        for pkg in ("xvfb", "xserver-xorg", "xinit", "pipewire-bin", "tesseract-ocr",
                    "kwallet-pam", "libseat1", "ttf-ibm-plex"):
            with self.subTest(pkg=pkg):
                self.assertNotIn(pkg, declared)

    def test_recommends_stay_light(self):
        recommends = set(control_fields().get("Recommends", []))
        heavy = {"ffmpeg", "imagemagick", "dex", "x11-xserver-utils", "x11-utils", "ark",
                 "xdg-desktop-portal-kde", "polkit-kde-agent-1", "policykit-1-gnome"}
        self.assertFalse(heavy & recommends)


class ArchMappingTests(unittest.TestCase):
    def test_arch_depends_cover_debian_depends(self):
        depends = set(pkgbuild_array("depends"))
        for cells in docs_rows():
            if cells[2] == "Depends":
                with self.subTest(pkg=cells[0]):
                    self.assertIn(cells[1], depends)

    def test_arch_optdepends_cover_recommends_and_suggests(self):
        depends = set(pkgbuild_array("depends"))
        optional = {entry.split(":", 1)[0] for entry in pkgbuild_array("optdepends")}
        for cells in docs_rows():
            if cells[2] in ("Recommends", "Suggests") and cells[1] != "-":
                with self.subTest(pkg=cells[0]):
                    self.assertTrue(cells[1] in depends or cells[1] in optional, cells)

    def test_srcinfo_matches_pkgbuild(self):
        text = SRCINFO.read_text().splitlines()
        depends = [line.split("=", 1)[1].strip() for line in text if line.startswith("\tdepends = ")]
        optdepends = [line.split("=", 1)[1].strip() for line in text if line.startswith("\toptdepends = ")]
        self.assertEqual(depends, pkgbuild_array("depends"))
        self.assertEqual(optdepends, pkgbuild_array("optdepends"))

    def test_optdepends_have_reasons(self):
        for entry in pkgbuild_array("optdepends"):
            with self.subTest(entry=entry):
                name, _, reason = entry.partition(":")
                self.assertTrue(name.strip() and reason.strip())


class AcceptanceAgreementTests(unittest.TestCase):
    def test_deb_acceptance_required_depends_are_declared(self):
        declared = control_fields()["Depends"]
        self.assertEqual([d for d in deb.REQUIRED_DEPENDS if d not in declared], [])


if __name__ == "__main__":
    unittest.main()

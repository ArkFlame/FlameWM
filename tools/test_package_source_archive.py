import hashlib
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
BUILDER = ROOT / "scripts/package-source-archive"


class SourceArchiveTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name) / "flamewm"
        self.root.mkdir()
        self.manifest = self.root / "manifest.txt"
        self.manifest.write_text("crates/\nassets/\nui/\n.agents/\nroot-check\n@exec crates/run.sh\n@exec root-check\n", encoding="utf-8")
        (self.root / "crates").mkdir()
        (self.root / "assets").mkdir()
        (self.root / "assets/raster").mkdir()
        (self.root / "ui").mkdir()
        (self.root / "crates/tracked.rs").write_text("tracked", encoding="utf-8")
        (self.root / "crates/untracked.rs").write_text("untracked", encoding="utf-8")
        (self.root / "crates/run.sh").write_text("#!/bin/sh\n", encoding="utf-8")
        (self.root / "crates/run.sh").chmod(0o755)
        (self.root / "assets/icon.svg").write_text("<svg/>", encoding="utf-8")
        (self.root / "root-check").symlink_to("crates/run.sh")
        (self.root / ".agents/runtime").mkdir(parents=True)
        (self.root / ".agents/runtime/private-canary.txt").write_text("private", encoding="utf-8")
        (self.root / "crates/__pycache__").mkdir()
        (self.root / "crates/__pycache__/secret.pyc").write_bytes(b"cache")
        (self.root / "crates/.env.token").write_text("secret", encoding="utf-8")
        (self.root / "crates/.ENV").write_text("secret", encoding="utf-8")
        (self.root / "crates/private.pem").write_text("secret", encoding="utf-8")
        (self.root / "assets/icon-link").symlink_to("icon.svg")
        (self.root / "ui/assets").symlink_to("../assets/raster")

    def tearDown(self):
        self.temp.cleanup()

    def archive(self, output):
        subprocess.run([str(BUILDER), str(self.root), str(self.manifest), str(output)], check=True)

    def test_members_metadata_symlink_mode_and_reproducibility(self):
        first, second = self.root / "a.tar.gz", self.root / "b.tar.gz"
        self.archive(first)
        self.archive(second)
        self.assertEqual(hashlib.sha256(first.read_bytes()).digest(), hashlib.sha256(second.read_bytes()).digest())
        with tarfile.open(first, "r:gz") as archive:
            entries = {item.name.removeprefix("flamewm/"): item for item in archive.getmembers()}
        self.assertTrue({"crates/tracked.rs", "crates/untracked.rs", "crates/run.sh", "assets/icon.svg", "assets/icon-link", "root-check", "ui/assets"} <= entries.keys())
        self.assertFalse(any(word in name.lower() for name in entries for word in ("private-canary", "pycache", ".env", ".pem")))
        self.assertEqual(entries["crates/run.sh"].mode & 0o777, 0o755)
        self.assertTrue(entries["assets/icon-link"].issym())
        self.assertEqual(entries["assets/icon-link"].linkname, "icon.svg")
        self.assertTrue(entries["root-check"].issym())
        self.assertEqual(entries["root-check"].linkname, "crates/run.sh")
        self.assertTrue(entries["ui/assets"].issym())
        self.assertEqual(entries["ui/assets"].linkname, "../assets/raster")
        self.assertTrue(all(item.uid == item.gid == 0 and item.mtime == 0 for item in entries.values()))
        extracted = self.root / "extracted"
        with tarfile.open(first, "r:gz") as archive:
            archive.extractall(extracted)
        self.assertTrue((extracted / "flamewm/ui/assets").is_symlink())
        self.assertTrue((extracted / "flamewm/ui/assets").is_dir())

    def test_real_source_manifest_registers_executable_version_script(self):
        registry = (ROOT / "packaging/source-manifest.txt").read_text(encoding="utf-8")
        self.assertIn("@exec scripts/version\n", registry)
        self.assertTrue((ROOT / "scripts/version").stat().st_mode & 0o111)

    def test_executable_version_is_included_once_registered(self):
        (self.root / "scripts").mkdir()
        version = self.root / "scripts/version"
        version.write_text("#!/bin/sh\necho 0.1.0\n", encoding="utf-8")
        version.chmod(0o755)
        self.manifest.write_text(self.manifest.read_text() + "scripts/\n@exec scripts/version\n")
        out = self.root / "version.tgz"
        self.archive(out)
        with tarfile.open(out, "r:gz") as archive:
            self.assertEqual(archive.getmember("flamewm/scripts/version").mode & 0o111, 0o111)

    def test_rejects_manifest_traversal_and_absolute_paths(self):
        for invalid in ("../outside\n", "/etc/passwd\n"):
            self.manifest.write_text(invalid, encoding="utf-8")
            result = subprocess.run([str(BUILDER), str(self.root), str(self.manifest), str(self.root / "bad.tar.gz")], capture_output=True)
            self.assertNotEqual(result.returncode, 0)

    def test_rejects_unregistered_executable(self):
        path = self.root / "crates/unregistered.sh"
        path.write_text("#!/bin/sh\n", encoding="utf-8")
        path.chmod(0o755)
        result = subprocess.run([str(BUILDER), str(self.root), str(self.manifest), str(self.root / "bad.tar.gz")], capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / "bad.tar.gz").exists(),
                         "invalid manifest must not publish a partial source archive")

    def test_rejects_escaping_symlink(self):
        (self.root / "assets/escape").symlink_to("../../outside")
        result = subprocess.run([str(BUILDER), str(self.root), str(self.manifest), str(self.root / "bad.tar.gz")], capture_output=True)
        self.assertNotEqual(result.returncode, 0)

    def check(self):
        return subprocess.run([sys.executable, "-I", str(BUILDER), "--check", str(self.root), str(self.manifest)],
                              capture_output=True, text=True, timeout=30)

    def test_check_mode_accepts_valid_manifest_and_writes_nothing(self):
        before = sorted(p.relative_to(self.root).as_posix() for p in self.root.rglob("*"))
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("FLAMEWM-SOURCE-CHECK:", result.stdout)
        self.assertEqual(before, sorted(p.relative_to(self.root).as_posix() for p in self.root.rglob("*")))

    def test_check_mode_rejects_unregistered_executable_with_canonical_message(self):
        path = self.root / "crates/unregistered.sh"
        path.write_text("#!/bin/sh\n", encoding="utf-8")
        path.chmod(0o755)
        result = self.check()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("executable is not registered: crates/unregistered.sh", result.stderr)
        self.assertFalse((self.root / "bad.tar.gz").exists())


class TerminalPayloadPackageContractTests(unittest.TestCase):
    """Static contract: the .deb must ship the native Terminal binary and launcher.

    scripts/xephyr stages a nested flamewm-terminal entry from packaging/, which
    hides a packaging omission here. These checks read the release scripts and
    the install manifest only (no cargo, no dpkg-deb, no .deb build).
    """

    PACKAGE = ROOT / "scripts/package"
    BUILD_DEB = ROOT / "scripts/build-deb"
    ENTRY = ROOT / "packaging/flamewm-terminal.desktop"

    def package_binary_loops(self):
        text = self.PACKAGE.read_text(encoding="utf-8")
        loops = re.findall(r"^for binary in ([^;\n]+); do$", text, re.MULTILINE)
        self.assertEqual(len(loops), 2, "expected the existence loop and the install loop")
        return [loop.split() for loop in loops]

    def package_application_links(self):
        """Applications installed as hard links to the multi-call `flamewm-apps` executable."""
        text = self.PACKAGE.read_text(encoding="utf-8")
        links = re.findall(r"^for application in ([^;\n]+); do$", text, re.MULTILINE)
        self.assertEqual(len(links), 1, "expected one hard-link loop for the multi-call applications")
        self.assertIn('ln "$STAGE/usr/bin/flamewm-apps" "$STAGE/usr/bin/$application"', text)
        return links[0].split()

    def test_terminal_binary_is_in_both_package_binary_lists(self):
        for index, binaries in enumerate(self.package_binary_loops()):
            self.assertIn("flamewm-apps", binaries, f"binary list {index} omits flamewm-apps")
        self.assertIn("flamewm-terminal", self.package_application_links())

    def test_terminal_binary_is_installed_by_package(self):
        text = self.PACKAGE.read_text(encoding="utf-8")
        self.assertIn('install -m 0755 "$BUILD_DIR/$binary" "$STAGE/usr/bin/$binary"', text)

    def test_terminal_desktop_source_is_validated_and_installed(self):
        text = self.PACKAGE.read_text(encoding="utf-8")
        self.assertIn('"$ROOT/packaging/flamewm-terminal.desktop"', text)
        self.assertIn(
            'install -m 0644 "$ROOT/packaging/flamewm-terminal.desktop" '
            '"$STAGE/usr/share/applications/flamewm-terminal.desktop"',
            text,
        )

    def test_terminal_desktop_entry_launches_the_packaged_binary(self):
        fields = dict(
            line.split("=", 1) for line in self.ENTRY.read_text(encoding="utf-8").splitlines() if "=" in line
        )
        self.assertEqual(fields["Type"], "Application")
        self.assertEqual(fields["Exec"], "flamewm-terminal")
        self.assertEqual(fields["TryExec"], "flamewm-terminal")
        self.assertEqual(fields["StartupWMClass"], "flamewm-terminal")

    def test_install_manifest_ships_terminal_binary_and_entry(self):
        manifest = (ROOT / "packaging/install-manifest.txt").read_text(encoding="utf-8")
        self.assertIn(".target/release/flamewm-terminal         usr/bin/flamewm-terminal\n", manifest)
        self.assertIn(
            "packaging/flamewm-terminal.desktop        usr/share/applications/flamewm-terminal.desktop\n",
            manifest,
        )

    def test_manifest_binaries_and_launchers_match_package_lists(self):
        manifest = (ROOT / "packaging/install-manifest.txt").read_text(encoding="utf-8")
        entries = {}
        for line in manifest.splitlines():
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            source, _, installed = line.partition(" ")
            entries[source] = installed.strip()
        prefix = ".target/release/"
        shipped = {source[len(prefix):] for source in entries if source.startswith(prefix)}
        self.assertIn("flamewm-terminal", shipped)
        linked = set(self.package_application_links())
        for binaries in self.package_binary_loops():
            self.assertEqual(set(binaries) | linked, shipped)
        for binary in sorted(shipped):
            self.assertEqual(entries[f"{prefix}{binary}"], f"usr/bin/{binary}")
        package = self.PACKAGE.read_text(encoding="utf-8")
        launchers = sorted(
            source for source in entries if source.startswith("packaging/") and source.endswith(".desktop")
        )
        self.assertIn("packaging/flamewm-terminal.desktop", launchers)
        for source in launchers:
            self.assertIn(f'"$ROOT/{source}"', package, f"{source} is not installed by scripts/package")

    def test_build_deb_asserts_terminal_payload_in_the_built_deb(self):
        text = self.BUILD_DEB.read_text(encoding="utf-8")
        self.assertIn("dpkg-deb --contents \"$DEB\"", text)
        for required in ("./usr/bin/flamewm-terminal", "./usr/share/applications/flamewm-terminal.desktop"):
            self.assertIn(f"'{required}'", text, f"build-deb does not assert {required}")
        self.assertIn('fail "native Terminal payload missing from package: $required"', text)


if __name__ == "__main__":
    unittest.main()

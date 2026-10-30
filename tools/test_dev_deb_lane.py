"""Fixture tests for the development .deb lane (scripts/dev-deb, FLAMEWM_PACKAGE_PROFILE=fast).

Uses a fake cargo and stub executables: no Rust build runs. Verifies profile
selection, the ~dev/non-production labelling, separate output directory, and that
the fast lane never consumes release/ binaries or touches the production dist.
"""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]
VERSION = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
BINARIES = ("flamewm-apps", "flamewm-assistant", "flamewm-audio-helper", "flamewm-session", "flamewm-screenshot", "flamewm-render-x11",
            "flamewm-render-compile", "flamewm-render-inspect", "flamewm-lock", "flamewm-wayland")
COPIED = ("Cargo.toml", "LICENSE", "README.md", "packaging/flamewm.desktop",
          "packaging/flamewm-session.desktop", "packaging/flamewm-wayland.desktop", "packaging/flamewm-screenshot.desktop",
          "packaging/flamewm-explorer.desktop", "packaging/flamewm-terminal.desktop", "packaging/flamewm-task-manager.desktop",
          "packaging/flamewm-finder.desktop", "packaging/flamewm-assistant.desktop", "packaging/icons/flamewm-finder.svg", "packaging/pam.d/flamewm-lock", "packaging/menus/flamewm-applications.menu",
          "packaging/notifications/dunstrc", "packaging/install-manifest.txt",
          "packaging/debian/control.in", "packaging/debian/copyright",
          "packaging/xdg-desktop-portal/flamewm-portals.conf", "scripts/version", "scripts/package",
          "scripts/dist-checksums", "scripts/package-source-archive", "scripts/flamewm-session-wrapper", "scripts/flamewm-wayland-session", "scripts/dev-deb",
          "tools/flamewm_desktop_services.py")
FAKE_CARGO = """#!/bin/sh
printf 'CARGO_TARGET_DIR=%s ARGS=%s\\n' "$CARGO_TARGET_DIR" "$*" >> "$FLAMEWM_FAKE_EVENTS"
[ "$FLAMEWM_FAKE_CARGO_EXIT" = 0 ] || exit "$FLAMEWM_FAKE_CARGO_EXIT"
mkdir -p "$CARGO_TARGET_DIR/fast"
for bin in $FLAMEWM_FAKE_BINS; do
  printf '#!/bin/sh\\necho FlameWM fixture\\n' > "$CARGO_TARGET_DIR/fast/$bin"
  chmod 0755 "$CARGO_TARGET_DIR/fast/$bin"
done
exit 0
"""


@unittest.skipUnless(shutil.which("dpkg-deb") and shutil.which("dpkg") and shutil.which("tar") and
                     subprocess.run(["dpkg", "--print-architecture"], capture_output=True, text=True).stdout.strip() == "amd64",
                     "native amd64 dpkg-deb and GNU tar required")
class DevDebLaneTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="flamewm-dev-deb-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "flamewm-sample"
        for file in COPIED:
            dest = self.root / file
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / file, dest)
        for doc in ("INSTALL.md", "PACKAGING.md"):
            (self.root / "docs").mkdir(exist_ok=True)
            (self.root / "docs" / doc).write_text("Packaging fixture\n", encoding="utf-8")
        (self.root / "LICENSES").mkdir()
        (self.root / "LICENSES/sample.txt").write_text("License fixture\n", encoding="utf-8")
        cursor = self.root / "assets/cursors"
        (cursor / "cursors").mkdir(parents=True)
        (cursor / "cursors/left_ptr").write_bytes(b"fixture")
        (cursor / "index.theme").write_text("[Icon Theme]\nName=Breeze Dark\n", encoding="utf-8")
        (self.root / "assets/web/breeze").mkdir(parents=True)
        (self.root / "assets/web/breeze/application-menu.svg").write_text("<svg/>")
        for theme in ("assets/themes/icons/MacTahoe", "assets/themes/cursors/MacTahoe-dark-cursors"):
            (self.root / theme).mkdir(parents=True)
            (self.root / theme / "index.theme").write_text("[Icon Theme]\nName=MacTahoe\n", encoding="utf-8")
        (self.root / "packaging/source-manifest.txt").write_text(
            "scripts/\ntools/\nassets/\n@exec scripts/version\n@exec scripts/package\n@exec scripts/dist-checksums\n"
            "@exec scripts/package-source-archive\n@exec scripts/flamewm-session-wrapper\n@exec scripts/flamewm-wayland-session\n"
            "@exec scripts/dev-deb\n@exec tools/flamewm_desktop_services.py\n", encoding="utf-8")
        # Stale production binaries: the fast lane must never consume these.
        release = self.root / ".target/release"
        release.mkdir(parents=True)
        for binary in BINARIES:
            stub = release / binary
            stub.write_text("#!/bin/sh\necho RELEASE STALE\n", encoding="utf-8")
            stub.chmod(0o755)
        production = self.root / ".target/dist"
        production.mkdir(parents=True)
        self.production_marker = production / "PRODUCTION-SENTINEL"
        self.production_marker.write_text("keep\n", encoding="utf-8")
        self.fakebin = Path(self.temp.name) / "fakebin"
        self.fakebin.mkdir()
        cargo = self.fakebin / "cargo"
        cargo.write_text(FAKE_CARGO, encoding="utf-8")
        cargo.chmod(0o755)
        self.events = Path(self.temp.name) / "events.txt"
        self.targets = Path(self.temp.name) / "targets"
        (self.targets / "fastdeb").mkdir(parents=True)
        self.env = os.environ.copy()
        self.env.update({
            "PATH": f"{self.fakebin}{os.pathsep}{self.env.get('PATH', '')}",
            "FLAMEWM_FAKE_EVENTS": str(self.events),
            "FLAMEWM_FAKE_BINS": " ".join(BINARIES),
            "FLAMEWM_FAKE_CARGO_EXIT": "0",
            "CARGO_TARGET_DIR": str(self.targets / "fastdeb"),
            "FLAMEWM_TARGET_BASE": str(self.targets),
        })
        self.env.pop("FLAMEWM_PACKAGE_PROFILE", None)

    def run_script(self, name, **extra):
        env = dict(self.env, **extra)
        return subprocess.run([str(self.root / "scripts" / name)], cwd=self.root, env=env,
                              capture_output=True, text=True, timeout=120)

    def test_package_fast_profile_uses_fast_binaries_and_dev_dist(self):
        fast = self.targets / "fastdeb/fast"
        fast.mkdir(parents=True)
        for binary in BINARIES:
            (fast / binary).write_text("#!/bin/sh\necho FAST\n", encoding="utf-8")
            (fast / binary).chmod(0o755)
        result = self.run_script("package", FLAMEWM_PACKAGE_PROFILE="fast",
                                 CARGO_TARGET_DIR=str(self.targets / "fastdeb"))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        deb = self.root / f".target/dev-dist/flamewm_{VERSION}~dev_amd64.deb"
        self.assertTrue(deb.is_file())
        self.assertFalse((self.root / f".target/dist/flamewm_{VERSION}_amd64.deb").exists())
        self.assertEqual(self.production_marker.read_text(encoding="utf-8"), "keep\n")
        control = subprocess.run(["dpkg-deb", "--field", str(deb), "Description"],
                                 capture_output=True, text=True).stdout
        self.assertIn("[DEVELOPMENT FAST BUILD - NOT FOR PRODUCTION]", control)
        version = subprocess.run(["dpkg-deb", "--field", str(deb), "Version"],
                                 capture_output=True, text=True).stdout.strip()
        self.assertEqual(version, f"{VERSION}~dev")
        contents = subprocess.run(["dpkg-deb", "--contents", str(deb)], capture_output=True, text=True).stdout
        for binary in BINARIES:
            self.assertIn(f"./usr/bin/{binary}", contents)

    def test_fast_profile_refuses_stale_release_binaries(self):
        # Only release/ exists: fast packaging must fail closed instead of consuming it.
        result = self.run_script("package", FLAMEWM_PACKAGE_PROFILE="fast",
                                 CARGO_TARGET_DIR=str(self.targets / "fastdeb"))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing fast build directory", result.stderr)
        self.assertIn("fastdeb/fast", result.stderr)
        self.assertFalse((self.root / ".target/dev-dist").exists() and any((self.root / ".target/dev-dist").iterdir()))

    def test_unknown_profile_is_rejected(self):
        result = self.run_script("package", FLAMEWM_PACKAGE_PROFILE="bogus")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("expected release or fast", result.stderr)

    def test_dev_deb_builds_fast_profile_all_bins_and_packages_them(self):
        result = self.run_script("dev-deb")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        line = self.events.read_text(encoding="utf-8").strip()
        self.assertTrue(line.startswith(f"CARGO_TARGET_DIR={self.targets / 'fastdeb'} ARGS=build --profile fast --locked"))
        self.assertNotIn("release", line)
        self.assertEqual(line.count("--bin "), len(BINARIES))
        self.assertTrue((self.root / f".target/dev-dist/flamewm_{VERSION}~dev_amd64.deb").is_file())
        self.assertEqual(self.production_marker.read_text(encoding="utf-8"), "keep\n")

    def test_dev_deb_cargo_failure_publishes_nothing(self):
        result = self.run_script("dev-deb", FLAMEWM_FAKE_CARGO_EXIT="17")
        self.assertEqual(result.returncode, 17, result.stdout + result.stderr)
        self.assertFalse((self.root / ".target/dev-dist" / f"flamewm_{VERSION}~dev_amd64.deb").exists())

    def test_dev_deb_rejects_relative_target_dir(self):
        result = self.run_script("dev-deb", CARGO_TARGET_DIR="relative/dir")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("must be absolute", result.stderr)


class DevDebDrmFeatureTests(unittest.TestCase):
    def test_fast_build_enables_the_wayland_drm_feature(self):
        lines = [line for line in (ROOT / "scripts/dev-deb").read_text().splitlines()
                 if "cargo build --profile fast" in line]
        self.assertEqual(len(lines), 1, lines)
        self.assertIn("--locked", lines[0])
        self.assertIn("--features flamewm-wm-wayland/drm", lines[0])
        self.assertIn('"${bin_args[@]}"', lines[0])


if __name__ == "__main__":
    unittest.main()

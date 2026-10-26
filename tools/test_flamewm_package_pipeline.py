"""Native packaging regression: reject Architecture:any and stale incomplete releases.

Uses tiny executable stubs rather than compiling Rust; verifies the packaging
and dpkg metadata pipeline, never claims a native application runtime test.
"""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]
VERSION = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
BINARIES = ("flamewm-apps", "flamewm", "flamewm-session", "flamewm-desktop", "flamewm-shell", "flamewm-settings",
            "flamewm-screenshot", "flamewm-explorer", "flamewm-render-x11", "flamewm-render-compile",
            "flamewm-render-inspect", "flamewm-lock", "flamewm-terminal", "flamewm-wayland")
# Stub for the --capabilities contract: key=value lines, exit 0, no device access.
CAPABILITY_STUB = "#!/bin/sh\nprintf 'backend=native\\ndrm_compiled=true\\n'\nexit 0\n"
PKG_CONFIG_STUB = "#!/bin/sh\nexit \"${FLAMEWM_FAKE_PKGCONFIG_EXIT:-0}\"\n"


@unittest.skipUnless(shutil.which("dpkg-deb") and shutil.which("dpkg") and shutil.which("tar") and
                     subprocess.run(["dpkg", "--print-architecture"], capture_output=True, text=True).stdout.strip() == "amd64",
                     "native amd64 dpkg-deb and GNU tar required")
class DebianReleasePipelineTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="flamewm-package-pipeline-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "flamewm-sample"
        self.root.mkdir()
        for file in ("Cargo.toml", "LICENSE", "README.md", "packaging/flamewm.desktop",
                     "packaging/flamewm-session.desktop", "packaging/flamewm-screenshot.desktop",
                     "packaging/flamewm-explorer.desktop", "packaging/flamewm-terminal.desktop",
                     "packaging/pam.d/flamewm-lock",
                     "packaging/menus/flamewm-applications.menu", "packaging/notifications/dunstrc",
                     "packaging/install-manifest.txt", "packaging/debian/control.in",
                     "packaging/debian/copyright", "packaging/xdg-desktop-portal/flamewm-portals.conf", "scripts/version", "scripts/build-deb", "scripts/check-cargo", "scripts/package",
                     "scripts/package-source-archive", "scripts/flamewm-session-wrapper",
                     "scripts/flamewm-wayland-session", "packaging/flamewm-wayland.desktop",
                     "tools/flamewm_desktop_services.py"):
            src = ROOT / file
            dest = self.root / file
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dest)
        for doc in ("INSTALL.md", "PACKAGING.md"):
            dest = self.root / "docs" / doc
            dest.parent.mkdir(exist_ok=True)
            dest.write_text("Packaging fixture\n", encoding="utf-8")
        directory = self.root / "LICENSES"
        directory.mkdir()
        (directory / "sample.txt").write_text("License fixture\n", encoding="utf-8")
        cursor = self.root / "assets/cursors"
        (cursor / "cursors").mkdir(parents=True)
        (cursor / "cursors/left_ptr").write_bytes(b"fixture")
        (cursor / "index.theme").write_text("[Icon Theme]\nName=Breeze Dark\n", encoding="utf-8")
        (self.root / "assets/web/breeze").mkdir(parents=True)
        (self.root / "assets/web/breeze/application-menu.svg").write_text("<svg/>")
        release = self.root / ".target/release"
        release.mkdir(parents=True)
        for binary in BINARIES:
            stub = release / binary
            stub.write_text(CAPABILITY_STUB if binary == "flamewm-wayland"
                            else "#!/bin/sh\necho FlameWM fixture\n", encoding="utf-8")
            stub.chmod(0o755)
        self.manifest = self.root / "packaging/source-manifest.txt"
        self.manifest.write_text("scripts/\ntools/\nassets/\n@exec scripts/version\n@exec scripts/build-deb\n@exec scripts/check-cargo\n@exec scripts/package\n@exec scripts/package-source-archive\n@exec scripts/flamewm-session-wrapper\n@exec scripts/flamewm-wayland-session\n@exec tools/flamewm_desktop_services.py\n", encoding="utf-8")

        tools = self.root / 'tools'
        tools.mkdir(exist_ok=True)
        (tools / 'ensure_desktop_conversion_cargo.py').write_text("#!/usr/bin/env python3\nimport os, sys\nwith open(os.environ['FLAMEWM_FAKE_EVENTS'], 'a') as f:\n    f.write('GATE ' + ' '.join(sys.argv[1:]) + '\\n')\nif os.environ.get('FLAMEWM_FAKE_GATE_FAIL') == '1':\n    print('FLAMEWM-CONVERT-CARGO: BLOCKED mock gate failure', file=sys.stderr)\n    sys.exit(23)\n", encoding='utf-8')
        self.events = self.root / 'events.txt'

    def package(self):
        # Fixture binaries live in <root>/.target/release; never inherit a host CARGO_TARGET_DIR.
        env = {k: v for k, v in os.environ.items() if k != "CARGO_TARGET_DIR"}
        return subprocess.run([str(self.root / "scripts/package")], cwd=self.root, env=env,
                              capture_output=True, text=True, timeout=60)

    def build_deb(self, cargo_exit=0, gate_failure=False, cargo_target_dir=None, pkgconfig_missing=False):
        fakebin = self.root / "fakebin"
        fakebin.mkdir(exist_ok=True)
        cargo = fakebin / "cargo"
        cargo.write_text(f"#!/bin/sh\nprintf 'CARGO %s\\n' \"$*\" >> \"$FLAMEWM_FAKE_EVENTS\"\nexit {cargo_exit}\n", encoding="utf-8")
        cargo.chmod(0o755)
        # Preflight stubs: the native drm closure and Xwayland are host capabilities the
        # fixture must not require, so they are resolved from this fake PATH entry.
        pkgconfig = fakebin / "pkg-config"
        pkgconfig.write_text(PKG_CONFIG_STUB, encoding="utf-8")
        pkgconfig.chmod(0o755)
        xwayland = fakebin / "Xwayland"
        xwayland.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        xwayland.chmod(0o755)
        env = {k: v for k, v in os.environ.items() if k != "CARGO_TARGET_DIR"}
        env["PATH"] = str(fakebin) + os.pathsep + env.get("PATH", "")
        env['FLAMEWM_FAKE_EVENTS'] = str(self.events)
        env['FLAMEWM_FAKE_PKGCONFIG_EXIT'] = '1' if pkgconfig_missing else '0'
        if gate_failure:
            env['FLAMEWM_FAKE_GATE_FAIL'] = '1'
        if cargo_target_dir is not None:
            env['CARGO_TARGET_DIR'] = str(cargo_target_dir)
        return subprocess.run([str(self.root / "scripts/build-deb")], cwd=self.root,
                              capture_output=True, text=True, timeout=90, env=env)

    def test_build_deb_is_fresh_and_installable(self):
        result = self.build_deb()
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertIn("FLAMEWM-DEB-READY", result.stdout)
        events = self.events.read_text().splitlines()
        self.assertEqual(events[0], 'GATE --profile release --include-ui')
        self.assertTrue(events[1].startswith('CARGO build --release --workspace'), events)
        self.assertIn('--features flamewm-wm-wayland/drm', events[1])
        deb = self.root / f".target/dist/flamewm_{VERSION}_amd64.deb"
        self.assertEqual(subprocess.check_output(["dpkg-deb", "-f", str(deb), "Architecture"], text=True).strip(), "amd64")
        self.assertTrue((deb.parent / "SHA256SUMS").exists())
        contents = subprocess.check_output(["dpkg-deb", "--contents", str(deb)], text=True)
        self.assertIn("usr/share/applications/flamewm-screenshot.desktop", contents)
        self.assertIn("usr/bin/flamewm-screenshot", contents)
        # A17: the native Terminal binary and launcher must be in the built .deb.
        self.assertIn("./usr/bin/flamewm-terminal", contents)
        self.assertIn("./usr/share/applications/flamewm-terminal.desktop", contents)
        self.assertIn("usr/share/xdg-desktop-portal/flamewm-portals.conf", contents)
        self.assertIn("usr/libexec/flamewm/flamewm-session-wrapper", contents)
        self.assertIn("usr/libexec/flamewm/flamewm-desktop-services", contents)
        self.assertIn("etc/xdg/menus/flamewm-applications.menu", contents)
        self.assertIn("usr/share/flamewm/dunstrc", contents)

    def test_build_failure_removes_old_deb_and_checksum(self):
        dist = self.root / ".target/dist"
        dist.mkdir(parents=True)
        old = dist / f"flamewm_{VERSION}_amd64.deb"
        old.write_bytes(b"old Architecture:any payload")
        (dist / "SHA256SUMS").write_text("stale", encoding="utf-8")
        result = self.build_deb(cargo_exit=17)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("FLAMEWM-DEB-READY", result.stdout)
        self.assertFalse(old.exists())
        self.assertFalse((dist / "SHA256SUMS").exists())

    def test_explicit_cargo_target_dir_is_created_and_used(self):
        # The stub cargo writes no binaries, so packaging fails later by design;
        # this asserts only the target-dir handling that runs before Cargo.
        target = self.root / "cache/deep/targets"
        result = self.build_deb(cargo_target_dir=target)
        self.assertNotIn("CARGO_TARGET_DIR", result.stderr)
        self.assertTrue(target.is_dir())
        self.assertTrue(self.events.read_text().splitlines()[1].startswith("CARGO build --release --workspace"))

    def test_unusable_explicit_cargo_target_dir_fails_without_fallback(self):
        blocker = self.root / "not-a-dir"
        blocker.write_text("file, not a directory", encoding="utf-8")
        dist = self.root / ".target/dist"
        dist.mkdir(parents=True)
        old = dist / f"flamewm_{VERSION}_amd64.deb"
        old.write_bytes(b"old payload")
        result = self.build_deb(cargo_target_dir=blocker)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("CARGO_TARGET_DIR", result.stderr)
        self.assertFalse(old.exists())
        self.assertFalse(self.events.exists(), "cargo or the gate must not run")

    def test_missing_assets_preflight_removes_old_deb(self):
        dist = self.root / ".target/dist"
        dist.mkdir(parents=True)
        old = dist / f"flamewm_{VERSION}_amd64.deb"
        old.write_bytes(b"old invalid payload")
        (dist / "SHA256SUMS").write_text("stale", encoding="utf-8")
        shutil.rmtree(self.root / "assets/web/breeze")
        result = self.build_deb()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("assets missing", result.stderr)
        self.assertFalse(old.exists())
        self.assertFalse((dist / "SHA256SUMS").exists())

    def test_incomplete_core_gate_blocks_build_before_cargo_and_old_deb_is_gone(self):
        dist = self.root / '.target/dist'
        dist.mkdir(parents=True, exist_ok=True)
        old = dist / f'flamewm_{VERSION}_amd64.deb'
        old.write_bytes(b'stale')
        (dist / 'SHA256SUMS').write_text('old')
        result = self.build_deb(gate_failure=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('FLAMEWM-CONVERT-CARGO: BLOCKED', result.stderr)
        self.assertNotIn('FLAMEWM-DEB-READY', result.stdout)
        self.assertFalse(old.exists())
        self.assertFalse((dist / 'SHA256SUMS').exists())
        self.assertEqual(self.events.read_text().splitlines(),
                         ['GATE --profile release --include-ui'])

    def test_unregistered_executable_is_refused_before_cargo_or_gate(self):
        dist = self.root / '.target/dist'
        dist.mkdir(parents=True, exist_ok=True)
        old = dist / f'flamewm_{VERSION}_amd64.deb'
        old.write_bytes(b'stale')
        stray = self.root / 'scripts/unregistered-tool'
        stray.write_text('#!/bin/sh\nexit 0\n', encoding='utf-8')
        stray.chmod(0o755)
        result = self.build_deb()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('executable is not registered: scripts/unregistered-tool', result.stderr)
        self.assertNotIn('FLAMEWM-DEB-READY', result.stdout)
        self.assertFalse(self.events.exists(), 'Cargo and the desktop gate must never run')
        self.assertFalse(old.exists())
        self.assertFalse((dist / 'SHA256SUMS').exists())

    def test_dev_check_invalidate_before_cargo_check(self):
        self.build_deb()  # creates fake Cargo executable
        self.events.unlink()
        env = os.environ.copy()
        env['PATH'] = str(self.root / 'fakebin') + os.pathsep + env.get('PATH', '')
        env['FLAMEWM_FAKE_EVENTS'] = str(self.events)
        checked = subprocess.run([str(self.root / 'scripts/check-cargo')], cwd=self.root,
                                 env=env, capture_output=True, text=True, timeout=20)
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertEqual(self.events.read_text().splitlines(), [
            'GATE --profile dev --include-ui',
            'CARGO check --workspace --all-targets',
        ])

    def test_release_is_real_amd64_and_sha256_complete(self):
        result = self.package()
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        dist = self.root / ".target/dist"
        deb = dist / f"flamewm_{VERSION}_amd64.deb"
        self.assertTrue(deb.is_file())
        fields = subprocess.check_output(["dpkg-deb", "-f", str(deb), "Architecture"], text=True).strip()
        self.assertEqual(fields, "amd64")
        self.assertTrue((dist / "SHA256SUMS").exists())
        checked = subprocess.run(["sha256sum", "-c", "SHA256SUMS"], cwd=dist,
                                 capture_output=True, text=True)
        self.assertEqual(checked.returncode, 0, checked.stdout + checked.stderr)
        self.assertTrue((dist / f"flamewm-{VERSION}-source.tar.gz").exists())
        entry = subprocess.check_output(["dpkg-deb", "--contents", str(deb)], text=True)
        self.assertIn("./usr/share/xsessions/flamewm.desktop", entry)
        self.assertIn("./usr/bin/flamewm-terminal", entry)
        self.assertIn("./usr/share/applications/flamewm-terminal.desktop", entry)

    def test_wayland_binary_without_drm_capability_never_yields_a_login_entry(self):
        dist = self.root / ".target/dist"
        dist.mkdir(parents=True)
        (dist / f"flamewm_{VERSION}_amd64.deb").write_bytes(b"previous payload")
        stub = self.root / ".target/release/flamewm-wayland"
        stub.write_text("#!/bin/sh\nprintf 'backend=native\\ndrm_compiled=false\\n'\nexit 0\n", encoding="utf-8")
        stub.chmod(0o755)
        result = self.build_deb()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("drm_compiled=true", result.stderr)
        self.assertNotIn("FLAMEWM-DEB-READY", result.stdout)
        self.assertEqual(list(dist.glob("*.deb")), [], "a non-drm build must not produce an installable package")
        self.assertFalse((dist / "SHA256SUMS").exists())

    def test_wayland_binary_without_capabilities_flag_is_a_failure_not_a_probe(self):
        stub = self.root / ".target/release/flamewm-wayland"
        stub.write_text("#!/bin/sh\nprintf 'usage: flamewm-wayland\\n' >&2\nexit 2\n", encoding="utf-8")
        stub.chmod(0o755)
        result = self.build_deb()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--capabilities", result.stderr)
        self.assertNotIn("FLAMEWM-DEB-READY", result.stdout)
        self.assertEqual(list((self.root / ".target/dist").glob("*.deb")), [])

    def test_missing_native_preflight_stops_before_cargo_with_one_line(self):
        result = self.build_deb(pkgconfig_missing=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("libinput, gbm, libdrm, libudev", result.stderr)
        errors = [line for line in result.stderr.splitlines() if line.startswith("FLAMEWM-DEB-ERROR:")]
        self.assertEqual(len(errors), 1, result.stderr)
        self.assertFalse(self.events.exists(), "no gate or cargo run may happen without the native prerequisites")


    def test_unregistered_version_refuses_to_publish_deb(self):
        self.manifest.write_text(self.manifest.read_text().replace("@exec scripts/version\n", ""))
        result = self.package()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("executable is not registered: scripts/version", result.stderr)
        self.assertFalse((self.root / f".target/dist/flamewm_{VERSION}_amd64.deb").exists())
        self.assertFalse((self.root / ".target/dist/SHA256SUMS").exists())
        self.assertFalse((self.root / ".target/dist/flamewm-0.1.0-source.tar.gz").exists())


if __name__ == "__main__":
    unittest.main()

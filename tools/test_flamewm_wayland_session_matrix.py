"""Fixture tests for the Wayland session wrapper matrix (scripts/flamewm-wayland-session).

Runs `bash scripts/tests/wayland-session-matrix.sh` once and asserts every row of the session
wrapper matrix, so a lost or weakened check fails here instead of passing silently. The harness
already owns the stub compositor/bridge/dbus/Xwayland fixtures; this driver adds direct
(harness-independent) exit-code assertions for the cheap cases and pins the desktop `Exec`
contract. No seat, compositor, Xwayland, cargo or network is touched: every case runs in its own
temporary HOME/XDG_RUNTIME_DIR.
"""
import functools
import os
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
WRAPPER = ROOT / "scripts/flamewm-wayland-session"
MATRIX = ROOT / "scripts/tests/wayland-session-matrix.sh"
DESKTOP = ROOT / "packaging/flamewm-wayland.desktop"

CASE_RE = re.compile(r"^== case (?P<case>\S+)$")
CHECK_RE = re.compile(r"^(?P<status>ok|FAIL) +(?P<case>\S+): (?P<check>\S+)(?:: .*)?$")
SUMMARY_RE = re.compile(r"^SUMMARY cases=(?P<cases>\d+) passed=(?P<passed>\d+) failed=(?P<failed>\d+)$")

# The required matrix: case -> the checks that must be present AND green. Adding a harness case
# without listing it here (or listing a row the harness no longer emits) fails the driver.
REQUIRED = {
    # default backend is drm, with no inference from the environment at all
    "default-backend": {"exit", "backend-drm", "session-stage", "log-stage"},
    # an inherited DISPLAY/WAYLAND_DISPLAY from a greeter still selects drm and is dropped
    "inherited-display-drm": {"exit", "backend-drm", "no-winit", "inherited-display-dropped",
                              "startup-detail"},
    # explicit --backend=nested wins over the environment and an inherited DISPLAY
    "nested-flag-wins": {"exit", "backend-winit", "nested-gate-skipped", "nested-keeps-display"},
    # the flag is stripped before exec, other arguments survive
    "flag-stripped": {"exit", "flag-not-forwarded", "other-args-forwarded", "env-value"},
    # the desktop Exec form --backend=drm (and `--backend drm`) is accepted, stripped, gated
    "drm-flag-explicit": {"exit-equals-form", "backend-drm-equals", "gate-ran",
                          "inherited-display-dropped", "exit-split-form", "drm-invocations",
                          "flag-not-forwarded", "argv-preserved-split"},
    # missing drm_compiled=true blocks the drm session with exit 2 and the documented line
    "capability-gate-missing": {"exit", "gate-line", "compositor-not-started", "exit-stage"},
    # a build that cannot answer --capabilities fails the same gate
    "capability-unsupported": {"exit", "unsupported-line", "compositor-not-started"},
    # an advisory --probe failure never changes the session exit code
    "probe-advisory": {"exit", "probe-exit", "probe-fail-lines", "probe-stage"},
    # a missing XDG_RUNTIME_DIR warns with an actionable line and continues
    "runtime-missing": {"exit", "runtime-stage", "runtime-action", "runtime-detail"},
    "runtime-insecure": {"exit", "insecure-mode"},
    "missing-binary-127": {"exit", "exit-stage"},
    "not-executable-126": {"exit", "exit-stage"},
    # SIGTERM forwards TERM only, propagates the code, stops the bridge, leaves no zombie
    "sigterm-forward": {"exit", "got-term", "no-int", "no-hup", "forwarded", "bridge-stopped",
                        "exit-stage", "no-zombie-compositor", "no-zombie-bridge"},
    "sigterm-escalate": {"exit", "got-term", "escalated", "no-zombie-compositor"},
    # the log is 0600 and keeps exactly one .1
    "log-mode-rotation": {"first-exit", "log-0600", "second-exit", "log-0600-after-rotation",
                          "rotated-0600", "rotated-previous-run", "one-rotation-only"},
    "socket-xwayland-activation": {"exit", "socket-stage", "xwayland-stage", "ready-stage",
                                   "activation-wayland-display", "activation-xwayland-display",
                                   "activation-var-list", "dbus-address-not-logged",
                                   "xwayland-not-spawned-by-wrapper", "dbus-run-session-not-used"},
    "invalid-backend": {"exit", "invalid-backend-line", "nothing-executed"},
    # the packaging Exec line starts this wrapper and still yields a native (drm) login
    "desktop-exec-contract": {"desktop-exec-present", "desktop-exec-captured",
                              "desktop-exec-wrapper", "desktop-exec-exit",
                              "desktop-exec-backend-drm", "desktop-exec-gate-ran",
                              "desktop-exec-flag-not-forwarded"},
}

# Matrix row -> the wrapper exit code it stands for. Asserted both by the harness and, below,
# directly against the wrapper so the number is never taken on trust from another script.
EXIT_CODES = {"missing-binary-127": 127, "not-executable-126": 126, "capability-gate-missing": 2,
              "capability-unsupported": 2, "invalid-backend": 2, "probe-advisory": 0,
              "runtime-missing": 0, "nested-flag-wins": 0}


def parse_matrix(output):
    """-> (cases{case: {check: status}}, summary{str: int}, unexpected[line])"""
    cases = {}
    summary = {}
    unexpected = []
    current = None
    for raw in output.splitlines():
        line = raw.rstrip()
        if not line.strip():
            continue
        match = CASE_RE.match(line)
        if match:
            current = match.group("case")
            cases.setdefault(current, {})
            continue
        match = CHECK_RE.match(line)
        if match:
            cases.setdefault(match.group("case"), {})[match.group("check")] = match.group("status")
            continue
        match = SUMMARY_RE.match(line)
        if match:
            summary = {key: int(value) for key, value in match.groupdict().items()}
            continue
        if line.startswith("OK: ") or line.startswith("FAILED: "):
            continue
        unexpected.append(line)
    return cases, summary, unexpected


@functools.lru_cache(maxsize=1)
def matrix():
    env = dict(os.environ, TMPDIR=tempfile.gettempdir())
    result = subprocess.run(["bash", str(MATRIX)], cwd=ROOT, env=env, capture_output=True,
                            text=True, timeout=900)
    return result, parse_matrix(result.stdout + result.stderr)


def wrapper_env(base, **extra):
    """A minimal, fully controlled environment: no DISPLAY, no D-Bus, no backend leak."""
    runtime = Path(base) / "runtime"
    runtime.mkdir(mode=0o700, exist_ok=True)
    stub_bin = Path(base) / "stubbin"
    stub_bin.mkdir(exist_ok=True)
    update = stub_bin / "dbus-update-activation-environment"
    update.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    update.chmod(0o755)
    env = {
        "HOME": str(Path(base) / "home"),
        "PATH": f"{stub_bin}:/usr/bin:/bin:/usr/sbin:/sbin",
        "XDG_RUNTIME_DIR": str(runtime),
        "DBUS_SESSION_BUS_ADDRESS": f"unix:path={base}/fixture-bus",
        "FLAMEWM_WAYLAND_STARTUP_TIMEOUT_MS": "200",
        "FLAMEWM_WAYLAND_SHUTDOWN_TIMEOUT_MS": "200",
        "FLAMEWM_SERVICES_BINARY": f"{base}/no-such-bridge",
    }
    Path(env["HOME"]).mkdir(parents=True, exist_ok=True)
    env.update(extra)
    return env


def run_wrapper(base, *args, **extra):
    return subprocess.run([str(WRAPPER), *args], cwd=ROOT, env=wrapper_env(base, **extra),
                          capture_output=True, text=True, timeout=120)


STUB_CAPABILITIES_NO_DRM = """#!/bin/sh
case "${1:-}" in
  --capabilities) printf 'backend=native\\ndrm_compiled=false\\n'; exit 0 ;;
  --probe) printf '[ok] fixture\\n'; exit 0 ;;
esac
exit 0
"""

STUB_PROBE_FAILS = """#!/bin/sh
case "${1:-}" in
  --capabilities) printf 'backend=native\\ndrm_compiled=true\\n'; exit 0 ;;
  --probe) printf '[FAIL] fixture: no fixture seat\\n'; exit 3 ;;
esac
exit 0
"""


class WaylandSessionMatrixTests(unittest.TestCase):
    def test_harness_is_green(self):
        result, (cases, summary, unexpected) = matrix()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(unexpected, [], "harness printed lines the driver cannot classify")
        self.assertEqual(summary.get("failed"), 0, result.stdout + result.stderr)
        self.assertEqual(summary.get("cases"), len(cases))
        self.assertGreater(summary.get("passed", 0), 0)
        self.assertEqual(summary.get("passed"),
                         sum(len(checks) for checks in cases.values()),
                         "SUMMARY passed= disagrees with the per-check lines")

    def test_no_case_is_skipped_or_silently_dropped(self):
        result, (cases, _, _) = matrix()
        self.assertEqual(set(cases), set(REQUIRED),
                         f"harness cases drifted from the required matrix: stdout={result.stdout}")
        for name, checks in cases.items():
            self.assertTrue(checks, f"case {name} emitted no check")
            statuses = {status for status in checks.values()}
            self.assertEqual(statuses, {"ok"}, f"case {name} has non-ok checks: {checks}")

    def test_every_required_row_passes(self):
        _, (cases, _, _) = matrix()
        for name, required in REQUIRED.items():
            with self.subTest(case=name):
                self.assertIn(name, cases)
                missing = required - set(cases[name])
                self.assertEqual(missing, set(), f"case {name} lost checks {sorted(missing)}")
                for check in sorted(required):
                    self.assertEqual(cases[name][check], "ok", f"{name}/{check}")

    def test_exit_codes_are_pinned_by_a_case(self):
        _, (cases, _, _) = matrix()
        for name in EXIT_CODES:
            self.assertEqual(cases[name]["exit"], "ok", f"{name} does not pin an exit code")


class WaylandSessionWrapperExitCodeTests(unittest.TestCase):
    """Direct, harness-independent exit-code proofs (cheap cases only: no stub matrix needed)."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="flamewm-wayland-session-pytest-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)

    def test_missing_binary_exits_127(self):
        result = run_wrapper(self.base, FLAMEWM_WAYLAND_BINARY=f"{self.base}/absent-compositor")
        self.assertEqual(result.returncode, 127, result.stdout + result.stderr)
        log = (self.base / "home/.cache/flamewm/wayland-session.log").read_text(encoding="utf-8")
        self.assertIn("stage=exit status=fail code=127 detail=binary_missing", log)

    def test_non_executable_binary_exits_126(self):
        binary = self.base / "not-executable"
        binary.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        binary.chmod(0o644)
        result = run_wrapper(self.base, FLAMEWM_WAYLAND_BINARY=str(binary))
        self.assertEqual(result.returncode, 126, result.stdout + result.stderr)
        log = (self.base / "home/.cache/flamewm/wayland-session.log").read_text(encoding="utf-8")
        self.assertIn("stage=exit status=fail code=126 detail=binary_not_executable", log)

    def test_invalid_backend_flag_exits_2_before_anything_runs(self):
        result = run_wrapper(self.base, "--backend=surfaceless")
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("detail=invalid_backend,source=flag", result.stdout + result.stderr)

    def test_missing_drm_compiled_blocks_with_exit_2(self):
        binary = self.base / "no-drm"
        binary.write_text(STUB_CAPABILITIES_NO_DRM, encoding="utf-8")
        binary.chmod(0o755)
        result = run_wrapper(self.base, FLAMEWM_WAYLAND_BINARY=str(binary))
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        log = (self.base / "home/.cache/flamewm/wayland-session.log").read_text(encoding="utf-8")
        self.assertIn("stage=capabilities status=fail code=2 detail=drm_feature_missing", log)

    def test_probe_failure_does_not_change_the_exit_code(self):
        binary = self.base / "probe-fails"
        binary.write_text(STUB_PROBE_FAILS, encoding="utf-8")
        binary.chmod(0o755)
        result = run_wrapper(self.base, FLAMEWM_WAYLAND_BINARY=str(binary))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        log = (self.base / "home/.cache/flamewm/wayland-session.log").read_text(encoding="utf-8")
        self.assertIn("probe exit=3 (advisory; does not change the session exit code)", log)
        self.assertIn("stage=probe status=fail code=3 detail=advisory,continuing", log)

    def test_backend_flag_never_reaches_the_compositor(self):
        record = self.base / "record"
        binary = self.base / "recorder"
        binary.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$STUB_RECORD"\n'
                          'case "${1:-}" in\n'
                          '  --capabilities) printf "drm_compiled=true\\n"; exit 0 ;;\n'
                          '  --probe) exit 0 ;;\n'
                          'esac\nexit 0\n', encoding="utf-8")
        binary.chmod(0o755)
        result = run_wrapper(self.base, "--backend=drm", "--fixture-keep=1",
                             FLAMEWM_WAYLAND_BINARY=str(binary), STUB_RECORD=str(record))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        invocations = record.read_text(encoding="utf-8").splitlines()
        self.assertIn("--fixture-keep=1", invocations)
        self.assertNotIn("--backend", " ".join(invocations))


class WaylandSessionDesktopExecTests(unittest.TestCase):
    """The packaging lane's session entry must start this wrapper with a native (drm) backend."""

    def test_desktop_exec_starts_the_wrapper(self):
        fields = {}
        for line in DESKTOP.read_text(encoding="utf-8").splitlines():
            if "=" in line and not line.startswith("["):
                key, value = line.split("=", 1)
                fields[key] = value
        self.assertEqual(fields.get("Exec", "").split()[0],
                         "/usr/libexec/flamewm/flamewm-wayland-session")

    def test_desktop_exec_does_not_select_a_nested_backend(self):
        # The wrapper defaults to drm, so an Exec without the flag is still a native login; what the
        # packaging lane must never do is ship a nested/winit Exec. `--backend=drm` is the explicit
        # form the wrapper accepts (covered by the matrix and below).
        argv = shlex.split(next(line[len("Exec="):] for line in
                                DESKTOP.read_text(encoding="utf-8").splitlines()
                                if line.startswith("Exec=")))
        backend = None
        remaining = argv[1:]
        while remaining:
            token = remaining.pop(0)
            if token.startswith("--backend="):
                backend = token.split("=", 1)[1]
            elif token == "--backend":
                backend = remaining.pop(0) if remaining else ""
        self.assertNotIn(backend, ("winit", "nested"),
                         "the desktop Exec must not select a nested backend")
        self.assertIn(backend, (None, "drm", "native"), f"unexpected --backend value {backend!r}")

    def test_wrapper_accepts_the_documented_exec_form(self):
        temp = tempfile.TemporaryDirectory(prefix="flamewm-wayland-exec-")
        self.addCleanup(temp.cleanup)
        result = run_wrapper(temp.name, "--backend=drm",
                             FLAMEWM_WAYLAND_BINARY=f"{temp.name}/absent-compositor")
        # 127 proves the flag parsed and the wrapper reached the binary check: an unsupported
        # flag form would exit 2 before it.
        self.assertEqual(result.returncode, 127, result.stdout + result.stderr)
        self.assertNotIn("invalid_backend", result.stdout)


if __name__ == "__main__":
    unittest.main()

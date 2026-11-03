"""Synthetic /proc tests for the whole-session memory measurement.

The measured contract is: every FlameWM process including the Python session
services is reported, the X server owning the display socket is inside the
session total, an absent component or baseline is printed instead of being
silently dropped, and no number is produced without a /proc source.
"""
from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

SCRIPT = Path(__file__).with_name("bench_flamewm_session_memory.py")
SPEC = importlib.util.spec_from_file_location("bench_flamewm_session_memory", SCRIPT)
bench = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(bench)

PYTHON_UID = os.getuid()


def write_process(root: Path, pid: int, *, exe: str, comm: str | None = None,
                  ppid: int = 1, start: int = 10, argv=(),
                  display: str | None = ":5", pss: int = 1024, rss: int = 2048,
                  environ: bytes | None = None) -> Path:
    proc = root / str(pid)
    proc.mkdir()
    (proc / "comm").write_text((comm or Path(exe).name[:15]) + "\n")
    (proc / "stat").write_text(f"{pid} ({comm or exe}) S {ppid} " + "0 " * 17 + f"{start}\n")
    (proc / "status").write_text(f"Uid:\t{PYTHON_UID}\t{PYTHON_UID}\t{PYTHON_UID}\t{PYTHON_UID}\n")
    (proc / "smaps_rollup").write_text(f"Rss: {rss} kB\nPss: {pss} kB\n")
    (proc / "cmdline").write_bytes(b"\0".join(a.encode() for a in argv) + b"\0")
    if environ is None:
        environ = (f"DISPLAY={display}".encode() if display else b"")
    (proc / "environ").write_bytes(environ + b"\0")
    target = proc / "exe"
    target.symlink_to(exe)
    return proc


def fake_bin(tmp: Path, name: str) -> Path:
    path = tmp / "bin" / name
    path.parent.mkdir(exist_ok=True)
    path.touch()
    return path


class SessionMeasurementTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.base = Path(self.temp.name)
        self.proc = self.base / "proc"
        self.proc.mkdir()
        self.sockets = self.base / "sockets"
        self.sockets.mkdir()
        self.net_unix = self.base / "net_unix"
        self.net_unix.write_text("Num       RefCount Protocol Flags    Type St Inode Path\n")
        self._next_socket_inode = 5000

    def tearDown(self):
        self.temp.cleanup()

    def add_listener(self, display: str) -> int:
        """Register the kernel socket inode listening on /tmp/.X11-unix/X<n>.

        The inode comes from /proc/net/unix, not from the filesystem: a file
        `st_ino` never matches `socket:[inode]`, which is the trap this covers.
        """
        self._next_socket_inode += 1
        inode = self._next_socket_inode
        name = f"X{display.lstrip(':')}"
        with self.net_unix.open("a") as stream:
            stream.write(f"0000000000000000: 00000002 00000000 00010000 0001 01 "
                         f"{inode} {self.sockets}/{name}\n")
        return inode

    def add_server(self, pid: int = 900, display: str = ":5") -> None:
        """Xorg that owns the display socket, matched only through the socket."""
        socket_number = display.lstrip(":")
        (self.sockets / f"X{socket_number}").touch()
        inode = self.add_listener(display)
        proc = write_process(self.proc, pid, exe=str(fake_bin(self.base, "Xorg")),
                             comm="Xorg", display=None, pss=8192, rss=16384)
        (proc / "environ").write_bytes(b"HOME=/root\0")
        fd_dir = proc / "fd"
        fd_dir.mkdir()
        os.symlink(f"socket:[{inode}]", fd_dir / "3")

    def test_python_session_services_and_x_server_are_both_counted(self):
        write_process(self.proc, 100, exe=str(fake_bin(self.base, "flamewm")), pss=4096, rss=8192)
        write_process(self.proc, 101, exe=str(fake_bin(self.base, "flamewm-desktop")),
                      ppid=100, pss=2048, rss=4096)
        write_process(self.proc, 102, exe=str(fake_bin(self.base, "flamewm-shell")),
                      ppid=100, pss=1024, rss=2048)
        # Installed bridge: a python3 interpreter whose argv[0] is the script.
        write_process(self.proc, 103, exe=str(fake_bin(self.base, "python3.13")),
                      comm="python3.13", ppid=100, pss=512, rss=1024,
                      argv=("/usr/libexec/flamewm/flamewm-desktop-services", "--run"))
        self.add_server()
        rows, skipped = bench.collect(self.proc, ":5", socket_dir=self.sockets,
                                net_unix=self.net_unix)
        summary = bench.summarize(rows)
        roles = {row["role"] for row in rows}
        self.assertEqual(roles, {"wm", "desktop", "shell", "desktop-services", "xorg"})
        self.assertEqual(skipped, [])
        self.assertEqual(summary["flamewm"]["pss_kib"], 4096 + 2048 + 1024 + 512)
        self.assertEqual(summary["servers"]["pss_kib"], 8192)
        self.assertEqual(summary["session"]["pss_kib"],
                         summary["flamewm"]["pss_kib"] + 8192,
                         "the session total must include the X server")
        self.assertEqual(summary["missing_required_core"], [])
        report = bench.manifest(rows, skipped, summary)
        self.assertEqual(len(report["measured"]), 5)
        self.assertFalse([line for line in report["excluded"]
                          if "desktop-services" in line],
                         "a running bridge must not be reported excluded")

    def test_absent_bridge_is_named_and_never_silently_summed(self):
        write_process(self.proc, 100, exe=str(fake_bin(self.base, "flamewm")), pss=4096)
        write_process(self.proc, 101, exe=str(fake_bin(self.base, "flamewm-desktop")),
                      ppid=100, pss=2048)
        write_process(self.proc, 102, exe=str(fake_bin(self.base, "flamewm-shell")),
                      ppid=100, pss=1024)
        self.add_server()
        rows, skipped = bench.collect(self.proc, ":5", socket_dir=self.sockets,
                                net_unix=self.net_unix)
        summary = bench.summarize(rows)
        report = bench.manifest(rows, skipped, summary)
        self.assertEqual(summary["missing_required_core"], ["desktop-services"])
        excluded = [line for line in report["excluded"] if "desktop-services" in line]
        self.assertEqual(len(excluded), 1)
        self.assertIn("not-running", excluded[0])
        lines = bench.render(rows, summary, report, display=":5", label="t",
                             resolution="1920x1080", apps_requested=3, apps=None,
                             warnings=[])
        text = "\n".join(lines)
        self.assertIn("desktop_services=ABSENT", text)
        self.assertIn("the session total above omits the Python session services", text)
        self.assertIn("total_scope=PARTIAL", text)
        self.assertIn("missing=desktop-services", text)

    def test_unreadable_server_is_an_exclusion_not_a_zero(self):
        write_process(self.proc, 100, exe=str(fake_bin(self.base, "flamewm")), pss=4096)
        self.add_server(pid=901)
        server = self.proc / "901"
        (server / "smaps_rollup").unlink()
        rows, skipped = bench.collect(self.proc, ":5", socket_dir=self.sockets,
                                net_unix=self.net_unix)
        summary = bench.summarize(rows)
        self.assertEqual([row["role"] for row in rows], ["wm"])
        self.assertEqual([item["role"] for item in skipped], ["xorg"])
        self.assertTrue(summary["missing_server"])
        report = bench.manifest(rows, skipped, summary)
        self.assertTrue(any("x-server" in line for line in report["excluded"]))
        self.assertTrue(any("role=xorg pid=901" in line for line in report["excluded"]))
        # The unreadable server contributed nothing to the total.
        self.assertEqual(summary["session"]["pss_kib"], 4096)

    def test_foreign_processes_are_not_measured(self):
        write_process(self.proc, 100, exe=str(fake_bin(self.base, "flamewm")), pss=4096)
        other = write_process(self.proc, 200, exe=str(fake_bin(self.base, "flamewm-shell")),
                             pss=999, rss=999)
        (other / "status").write_text("Uid:\t4242\t4242\t4242\t4242\n")
        rows, skipped = bench.collect(self.proc, ":5", socket_dir=self.sockets,
                                net_unix=self.net_unix)
        self.assertEqual([row["pid"] for row in rows], [100])
        self.assertEqual(skipped, [])

    def test_ancestor_display_match_pulls_in_child_helper(self):
        write_process(self.proc, 100, exe=str(fake_bin(self.base, "flamewm")), pss=4096)
        write_process(self.proc, 140, exe=str(fake_bin(self.base, "flamewm-audio-helper")),
                      ppid=100, display=None, pss=1024)
        rows, _ = bench.collect(self.proc, ":5", socket_dir=self.sockets,
                                net_unix=self.net_unix)
        self.assertEqual({row["role"] for row in rows}, {"wm", "audio-helper"})

    def test_applications_are_detected_and_missing_ones_are_skipped_with_a_warning(self):
        installed = {"flamewm-terminal", "xterm"}

        def which(name):
            return f"/usr/bin/{name}" if name in installed else None

        chosen, missing = bench.application_selection(3, which)
        self.assertEqual(chosen, ["flamewm-terminal", "xterm"])
        self.assertNotIn("flamewm-terminal", missing)
        self.assertIn("flamewm-explorer", missing)
        spawned: list[list[str]] = []
        result = bench.launch_applications(":5", 3, which=which,
                                           spawn=lambda cmd, **_: spawned.append(cmd),
                                           settle=0)
        self.assertEqual(result["launched"], ["flamewm-terminal", "xterm"])
        self.assertEqual(spawned, [["/usr/bin/flamewm-terminal"], ["/usr/bin/xterm"]])
        self.assertTrue(result["missing"])

    def test_baseline_without_binaries_prints_skipped_and_never_a_number(self):
        lines = bench.baseline_lines("auto", "1920x1080", 3, which=lambda name: None,
                                     baseline_display=None, baseline_rows=None,
                                     baseline_apps=None)
        text = "\n".join(lines)
        self.assertIn("SKIPPED", text)
        self.assertIn("xfce4-session", text)
        self.assertIn("comparison=NOT-MEASURED", text)
        self.assertNotIn("PSS MiB", text)

    def test_baseline_binaries_present_but_no_live_display_states_next_command(self):
        def which(name):
            return f"/usr/bin/{name}" if name == "icewm-session" else None

        name, missing = bench.resolve_baseline("auto", which)
        self.assertEqual(name, "icewm")
        self.assertIn("icewm", missing)
        lines = bench.baseline_lines("auto", "1920x1080", 2, which=which,
                                     baseline_display=None, baseline_rows=None,
                                     baseline_apps=None)
        text = "\n".join(lines)
        self.assertIn("baseline=icewm", text)
        self.assertIn("comparison=NOT-MEASURED", text)
        self.assertIn("--baseline-display :167", text)

    def test_baseline_measured_on_a_display_yields_a_comparison_table(self):
        write_process(self.proc, 100, exe=str(fake_bin(self.base, "flamewm")), pss=4096)
        self.add_server()
        # A separate baseline display: its own Xephyr serving socket X7.
        (self.sockets / "X7").touch()
        base_inode = self.add_listener(":7")
        base_server = write_process(self.proc, 910, exe=str(fake_bin(self.base, "Xephyr")),
                                    comm="Xephyr", pss=2048)
        (base_server / "environ").write_bytes(b"HOME=/root\0")
        (base_server / "fd").mkdir()
        os.symlink(f"socket:[{base_inode}]", base_server / "fd" / "4")
        write_process(self.proc, 911, exe=str(fake_bin(self.base, "flamewm")),
                      pss=1000, argv=("flamewm",), display=":7")
        rows, _ = bench.collect(self.proc, ":5", socket_dir=self.sockets,
                                net_unix=self.net_unix)
        base_rows, _ = bench.collect(self.proc, ":7", socket_dir=self.sockets,
                                net_unix=self.net_unix)
        lines = bench.baseline_lines("icewm", "1920x1080", 2,
                                     which=lambda name: f"/usr/bin/{name}",
                                     baseline_display=":7", baseline_rows=base_rows,
                                     baseline_apps=None)
        lines.extend(bench.comparison_table(bench.summarize(rows), "icewm",
                                            bench.summarize(base_rows),
                                            resolution="1920x1080"))
        text = "\n".join(lines)
        # baseline session total = its own process + the X server it runs on.
        self.assertIn("session_pss_kib=3048", text)
        self.assertIn("| FlameWM | 1920x1080 |", text)
        self.assertIn("| icewm | 1920x1080 |", text)
        self.assertIn("not a ranking", text)

    def test_json_report_carries_scope_and_manifest(self):
        write_process(self.proc, 100, exe=str(fake_bin(self.base, "flamewm")), pss=4096)
        self.add_server()
        output = self.base / "report.json"
        with contextlib.redirect_stdout(io.StringIO()):
            code = bench.main(["--display", ":5", "--proc-root", str(self.proc),
                               "--socket-dir", str(self.sockets),
                               "--net-unix", str(self.net_unix), "--apps", "0",
                               "--baseline", "none", "--json", str(output)])
        document = json.loads(output.read_text())
        self.assertEqual(code, 0)
        self.assertEqual(document["scope"]["x_server_included"], True)
        self.assertEqual(document["scope"]["python_services_included"], False)
        self.assertEqual(document["resolution"]["source"], "declared")
        self.assertEqual(document["totals_kib"]["session"]["pss_kib"], 4096 + 8192)
        self.assertTrue(document["manifest"]["measured"])
        self.assertTrue(any("desktop-services" in line
                            for line in document["manifest"]["excluded"]))
        self.assertIsNone(document["baseline"]["totals_kib"])

    def test_no_matching_process_is_a_clean_error_not_an_empty_total(self):
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr), contextlib.redirect_stdout(io.StringIO()):
            code = bench.main(["--display", ":5", "--proc-root", str(self.proc),
                               "--socket-dir", str(self.sockets),
                               "--net-unix", str(self.net_unix), "--baseline", "none"])
        self.assertEqual(code, 3)
        self.assertIn("NO_FLAMEWM_PROCESSES", stderr.getvalue())

    def test_socket_identity_comes_from_proc_net_unix_not_the_file_inode(self):
        """A filesystem st_ino never equals the kernel socket inode."""
        (self.sockets / "X5").touch()
        file_inode = os.stat(self.sockets / "X5").st_ino
        kernel_inode = self.add_listener(":5")
        self.assertNotEqual(file_inode, kernel_inode)
        inodes = bench.display_socket_inodes(":5", socket_dir=self.sockets,
                                             net_unix=self.net_unix)
        self.assertEqual(inodes, {kernel_inode})
        self.assertNotIn(file_inode, inodes)
        # The abstract-namespace alias of the same socket carries its own entry.
        with self.net_unix.open("a") as stream:
            stream.write(f"0000000000000000: 00000003 00000000 00010000 0001 03 "
                         f"{kernel_inode + 1} @{self.sockets}/X5\n")
        self.assertEqual(bench.display_socket_inodes(":5", socket_dir=self.sockets,
                                                     net_unix=self.net_unix),
                         {kernel_inode, kernel_inode + 1})

    def test_unknown_display_or_missing_socket_table_yields_no_inodes(self):
        self.assertEqual(bench.display_socket_inodes("not-a-display"),
                         set())
        self.assertEqual(bench.display_socket_inodes(":5", socket_dir=self.sockets,
                                                     net_unix=self.base / "absent"),
                         set())
        self.assertFalse(bench.owns_display_socket(self.proc, 1, set()))

    def test_declared_resolution_parser_rejects_nonsense(self):
        self.assertEqual(bench.parse_resolution("1920x1080"), "1920x1080")
        self.assertEqual(bench.parse_resolution(" 1280X800 ".lower()), "1280x800")
        with self.assertRaises(Exception):
            bench.parse_resolution("1920")


class ScriptContractTests(unittest.TestCase):
    """The bash entry point must keep the legacy mode and offer session mode."""

    def test_legacy_mode_and_session_mode_are_both_documented(self):
        text = Path(__file__).with_name("bench_mem.sh").read_text()
        self.assertIn("--session", text)
        self.assertIn("--explain", text)
        # The legacy contract still prints the same table header and totals.
        self.assertIn("| process | args | RSS MiB | PSS MiB |", text)
        self.assertIn("**Total**", text)
        # Session-only processes are no longer absent from the legacy listing.
        self.assertIn("flamewm-desktop-services", text)
        self.assertIn("flamewm-xsettings", text)

    def test_notes_name_the_real_exclusions(self):
        text = Path(__file__).with_name("bench_mem.sh").read_text()
        notes = text.split("session-mode coverage:", 1)[1]
        for fact in ("Xorg, Xephyr or Xwayland", "flamewm-desktop-services",
                     "excluded: foreign applications", "not proven"):
            self.assertIn(fact, notes)

    def test_legacy_mode_measures_the_python_bridge_that_pgrep_cannot_find(self):
        """End to end against real /proc: the installed bridge is `python3`.

        `pgrep -x flamewm-desktop-services` matches nothing (the pattern is
        longer than the 15-character comm limit) and the bridge runs as a
        python interpreter, so only argv identifies it. This spawns such a
        process on a private display name and asserts the script reports it.
        """
        script = Path(__file__).with_name("bench_mem.sh")
        display = f":{20000 + os.getpid() % 1000}"
        with tempfile.TemporaryDirectory() as tmp:
            bridge = Path(tmp) / "flamewm-desktop-services"
            bridge.write_text("#!/usr/bin/env python3\nimport time\ntime.sleep(60)\n")
            bridge.chmod(0o755)
            process = subprocess.Popen([str(bridge)],
                                       env={**os.environ, "DISPLAY": display},
                                       stdout=subprocess.DEVNULL,
                                       stderr=subprocess.DEVNULL)
            try:
                for _ in range(60):
                    if process.poll() is not None:
                        self.skipTest("bridge process exited before it could run")
                    try:
                        comm = Path(f"/proc/{process.pid}/comm").read_text().strip()
                    except OSError:
                        comm = ""
                    if comm.startswith("python"):
                        break
                    time.sleep(0.02)
                result = subprocess.run([str(script), "regression"],
                                        env={**os.environ, "BENCH_DISPLAY": display},
                                        capture_output=True, text=True, timeout=120)
            finally:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"desktop_services=present pid={process.pid}", result.stdout)
        self.assertNotIn("ABSENT", result.stdout)
        self.assertIn("**Total**", result.stdout)

    def test_legacy_mode_states_an_absent_bridge_instead_of_omitting_it(self):
        script = Path(__file__).with_name("bench_mem.sh")
        display = f":{21000 + os.getpid() % 1000}"
        result = subprocess.run([str(script), "regression"],
                                env={**os.environ, "BENCH_DISPLAY": display},
                                capture_output=True, text=True, timeout=120)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("desktop_services=ABSENT", result.stdout)
        self.assertIn("omits the Python session services", result.stdout)
        self.assertIn("excluded=", result.stdout)

    def test_explain_and_help_are_available_offline(self):
        script = Path(__file__).with_name("bench_mem.sh")
        for flag in ("--explain", "--help"):
            result = subprocess.run([str(script), flag], capture_output=True,
                                    text=True, timeout=60)
            self.assertEqual(result.returncode, 0, result.stderr)
        explained = subprocess.run([str(script), "--explain"], capture_output=True,
                                   text=True, timeout=60).stdout
        self.assertIn("included: the X server owning the display socket", explained)
        self.assertIn("excluded: swap", explained)


if __name__ == "__main__":
    unittest.main()

"""Synthetic /proc tests for the repeatable FlameWM memory collector."""
from __future__ import annotations

import contextlib
import csv
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("bench-flamewm-memory.py")
SPEC = importlib.util.spec_from_file_location("bench_flamewm_memory", SCRIPT)
bench = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(bench)


def write_process(root: Path, pid: int, *, exe: Path, comm: str, ppid: int = 1,
                  start: int = 10, cmdline: tuple[str, ...] = ()) -> None:
    proc = root / str(pid)
    proc.mkdir()
    fields = ["S", str(ppid), *(["0"] * 17), str(start)]
    (proc / "stat").write_text(f"{pid} ({comm}) {' '.join(fields)}\n")
    (proc / "comm").write_text(f"{comm}\n")
    (proc / "status").write_text(
        f"Uid:\t{os.getuid()}\t{os.getuid()}\t{os.getuid()}\t{os.getuid()}\n"
        "Threads:\t3\nvoluntary_ctxt_switches:\t8\nnonvoluntary_ctxt_switches:\t2\n"
    )
    (proc / "smaps_rollup").write_text(
        "Rss: 100 kB\nPss: 60 kB\nPrivate_Dirty: 20 kB\nPrivate_Clean: 10 kB\n"
        "Anonymous: 30 kB\nSwap: 2 kB\n"
    )
    (proc / "cmdline").write_bytes(b"\0".join(arg.encode() for arg in cmdline) + b"\0")
    (proc / "exe").symlink_to(exe)
    (proc / "sched").write_text("nr_switches : 14\nnr_wakeups : 9\n")


class MemoryBenchTests(unittest.TestCase):
    def test_rollup_reads_required_pss_rss_private_anonymous_and_swap_fields(self):
        values = bench.parse_smaps_rollup(
            "Rss: 10 kB\nPss: 8 kB\nPrivate_Dirty: 3 kB\nPrivate_Clean: 2 kB\n"
            "Anonymous: 4 kB\nSwap: 1 kB\n"
        )
        self.assertEqual(values, {"Rss": 10, "Pss": 8, "Private_Dirty": 3,
                                  "Private_Clean": 2, "Anonymous": 4, "Swap": 1, "USS": 5})
        with self.assertRaises(ValueError):
            bench.parse_smaps_rollup("Pss: 2 kB\n")

    def test_process_stat_handles_parentheses_and_tracks_restart_generation(self):
        self.assertEqual(bench.parse_proc_stat("9 (flamewm (nested)) S 4 " + "0 " * 17 + "71"), (4, 71))

    def test_exact_executable_identity_and_owned_children_are_unique(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "proc"
            root.mkdir()
            bin_dir = Path(tmp) / "bin"
            bin_dir.mkdir()
            wm = bin_dir / "flamewm"
            shell = bin_dir / "flamewm-shell"
            sleeper = bin_dir / "sleep"
            python = bin_dir / "python3"
            for path in (wm, shell, sleeper, python):
                path.touch()
            write_process(root, 101, exe=wm, comm="flamewm", start=55)
            write_process(root, 102, exe=shell, comm="flamewm-shell", ppid=101, start=66)
            write_process(root, 103, exe=sleeper, comm="sleep", ppid=101, start=77)
            write_process(root, 104, exe=python, comm="python3", ppid=999, start=88,
                          cmdline=("python3", "-c", "flamewm_desktop_services.py"))
            owned = bench.scan_processes(root)
            self.assertEqual(set(owned), {101, 102, 103})
            rows, totals = bench.sample_snapshot(root)
            self.assertEqual(len(rows), 3)
            self.assertEqual(totals["Pss"], 180)
            self.assertEqual(totals["process_count"], 3)
            self.assertEqual(totals["child_count"], 1)
            self.assertEqual(owned[102]["role"], "shell")
            self.assertEqual(owned[103]["role"], "child:sleep")

    def test_profile_gauges_keep_real_process_and_source_identity(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "flamewm.profile.log"
            path.write_text("MEMORY_TOP:\n render.x11.pixmap_bytes current=1B peak=2B\n"
                            "INTERVAL_SECS=60 TOP=8\nMEMORY_TOP:\n"
                            " render.x11.pixmap_bytes current=120B peak=240B\n"
                            " render.x11.backbuffer_bytes current=30B peak=60B\n"
                            "INTERVAL_SECS=60 TOP=8\n")
            result = bench.parse_profile_gauges(Path(tmp))
        self.assertEqual(result["status"], "latest_available")
        self.assertEqual({item["label"] for item in result["items"]},
                         {"render.x11.pixmap_bytes", "render.x11.backbuffer_bytes"})
        self.assertEqual(result["items"][0]["process"], "flamewm")
        self.assertEqual(result["items"][0]["current_bytes"], 120)

    def test_short_lived_process_and_pid_restart_do_not_stick_or_double_count(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "proc"
            root.mkdir()
            binary = Path(tmp) / "flamewm"
            binary.touch()
            process = root / "301"
            write_process(root, 301, exe=binary, comm="flamewm", start=10)
            first, total = bench.sample_snapshot(root)
            self.assertEqual(total["process_count"], 1)
            stat = process / "stat"
            fields = ["S", "1", *(["0"] * 17), "11"]
            stat.write_text(f"301 (flamewm) {' '.join(fields)}\n")
            restarted, total = bench.sample_snapshot(root)
            self.assertEqual(restarted[0]["start_ticks"], 11)
            self.assertEqual(total["process_count"], 1)
            shutil.rmtree(process)
            gone, total = bench.sample_snapshot(root)
        self.assertEqual(first[0]["start_ticks"], 10)
        self.assertEqual(gone, [])
        self.assertEqual(total["process_count"], 0)

    def test_dry_run_prints_matrix_and_does_not_touch_proc_or_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "must-not-exist"
            stdout = io.StringIO()
            with contextlib.redirect_stdout(stdout):
                code = bench.main(["--dry-run", "--proc-root", "/not/proc",
                                   "--output", str(output), "--workload", "icons-250",
                                   "--phase", "startup=2", "--phase", "steady=4"])
            plan = json.loads(stdout.getvalue())
        self.assertEqual(code, 0)
        self.assertFalse(output.exists())
        self.assertEqual(len(plan["workload_matrix"]), 11)
        self.assertEqual([p["name"] for p in plan["phases"]], ["startup", "steady"])

    def test_timeline_writes_unique_aggregate_and_observed_peak(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "proc"
            root.mkdir()
            binary = Path(tmp) / "flamewm"
            binary.write_text("test binary")
            write_process(root, 201, exe=binary, comm="flamewm", start=1)
            out = Path(tmp) / "out"
            with contextlib.redirect_stdout(io.StringIO()):
                code = bench.main(["--proc-root", str(root), "--output", str(out),
                                   "--interval-ms", "200", "--duration", "0.42"])
            lines = (out / "samples.csv").read_text().splitlines()
            summary = json.loads((out / "summary.json").read_text())
        self.assertEqual(code, 0)
        rows = list(csv.DictReader(lines))
        self.assertGreaterEqual(sum(row["sample_kind"] == "aggregate" for row in rows), 2)
        self.assertGreaterEqual(summary["sample_count"], 2)
        self.assertEqual(summary["observed_peak_aggregate_pss_kib"]["pss_kib"], 60)
        self.assertEqual(summary["latest_unique_process_aggregate"]["Pss"], 60)
        self.assertEqual(summary["x11_server_resources"]["status"], "unavailable")


if __name__ == "__main__":
    unittest.main()

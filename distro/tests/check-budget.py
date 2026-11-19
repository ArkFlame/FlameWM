#!/usr/bin/env python3
"""check-budget.py [metrics.json] [budget.toml] - gate boot-smoke metrics against budget.toml.

stdlib only (tomllib needs Python 3.11+). Run as: python3 -I check-budget.py
Exit: 0 all budgets met, 1 a budget exceeded or metric missing/failed, 2 unreadable input.
"""
import json
import os
import sys
import tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))


def load(path, binary=False):
    try:
        with open(path, "rb" if binary else "r", encoding=None if binary else "utf-8") as f:
            return tomllib.load(f) if binary else json.load(f)
    except (OSError, ValueError) as exc:
        print(f"ERROR cannot read {path}: {exc}", file=sys.stderr)
        sys.exit(2)


def dig(doc, dotted):
    cur = doc
    for part in dotted.split("."):
        if not isinstance(cur, dict) or part not in cur:
            return None
        cur = cur[part]
    return cur


def main(argv):
    metrics_path = argv[1] if len(argv) > 1 else os.path.join(REPO, "distro", "out", "metrics.json")
    budget_path = argv[2] if len(argv) > 2 else os.path.join(HERE, "budget.toml")
    metrics = load(metrics_path)
    budget = load(budget_path, binary=True)

    rows = []  # (name, actual, limit, ok)
    if metrics.get("status") != "PASS":
        rows.append(("boot-smoke status", metrics.get("status"), "PASS", False))

    def max_rule(name, metric_path, limit):
        actual = dig(metrics, metric_path)
        ok = isinstance(actual, (int, float)) and actual <= limit
        rows.append((name, actual, f"<= {limit}", ok))

    b = budget.get("iso", {})
    if "max_size_mb" in b:
        max_rule("iso size MB", "iso.size_mb", b["max_size_mb"])
    b = budget.get("boot", {})
    if "max_wall_seconds" in b:
        max_rule("boot wall seconds", "boot.wall_seconds", b["max_wall_seconds"])
    if "allowed_systemd_states" in b:
        state = dig(metrics, "systemd.state")
        ok = state in b["allowed_systemd_states"]
        rows.append(("systemd state", state, "in " + ",".join(b["allowed_systemd_states"]), ok))
    b = budget.get("ram", {})
    if "max_idle_used_mb" in b:
        max_rule("idle RAM used MB", "memory.used_mb", b["max_idle_used_mb"])
    b = budget.get("services", {})
    if "max_running" in b:
        max_rule("running services", "services.running", b["max_running"])

    width = max(len(r[0]) for r in rows) if rows else 10
    failed = 0
    for name, actual, limit, ok in rows:
        failed += 0 if ok else 1
        print(f"{'PASS' if ok else 'FAIL'}  {name:<{width}}  actual={actual}  budget {limit}")
    print(f"RESULT {'PASS' if failed == 0 else 'FAIL'} ({failed} over budget)")
    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))

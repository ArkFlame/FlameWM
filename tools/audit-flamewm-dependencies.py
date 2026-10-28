#!/usr/bin/env python3
"""Inspect direct declarations and per-binary resolved Cargo graphs (not line counts)."""
from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TARGETS = (
    ("x11-wm", "flamewm-wm-x11", None),
    ("desktop", "flamewm-desktop", None),
    ("settings", "flamewm-settings", None),
    ("wayland-nested", "flamewm-wm-wayland", None),
    ("wayland-drm", "flamewm-wm-wayland", "drm"),
)


def declaration_counts() -> dict:
    crates = sorted((ROOT / "crates").glob("*/Cargo.toml"))
    manifests = [tomllib.loads(path.read_text()) for path in crates]
    package_names = {item["package"]["name"] for item in manifests}
    direct = sum(len(item.get("dependencies", {})) for item in manifests)
    internal = sum(
        dep in package_names for item in manifests for dep in item.get("dependencies", {})
    )
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    return {
        "first_party_workspace_crates": len(package_names),
        "direct_dependency_declarations": direct,
        "first_party_direct_declarations": internal,
        "external_direct_declarations": direct - internal,
        "locked_package_versions_all_platforms_and_features": len(lock["package"]),
        "locked_unique_names_all_platforms_and_features": len({p["name"] for p in lock["package"]}),
    }


def resolved_graph(package: str, feature: str | None) -> dict:
    cmd = ["cargo", "tree", "--locked", "--offline", "-p", package,
           "--edges", "normal,build", "--prefix", "none", "--format", "{p}"]
    if feature:
        cmd.extend(("--features", f"{package}/{feature}"))
    result = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, check=False)
    if result.returncode:
        return {"error": result.stderr.strip() or result.stdout.strip(), "command": cmd}
    # Cargo can display the same version repeatedly or with a trailing (*) marker.
    # Count unique name/version/package-source labels, not printed graph lines.
    versions = {line.removesuffix(" (*)").strip() for line in result.stdout.splitlines() if line.strip()}
    return {"unique_package_versions": len(versions), "command": cmd}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", type=Path, help="write structured results")
    parser.add_argument("--resolve", action="store_true", help="include five native cargo-tree graphs")
    args = parser.parse_args()
    report = {"declarations": declaration_counts()}
    if args.resolve:
        if shutil.which("cargo") is None:
            report["resolved_graph_error"] = "cargo is not available; no compiled-graph claims can be made"
        else:
            report["binary_graphs"] = {
                key: resolved_graph(package, feature) for key, package, feature in TARGETS
            }
    print(json.dumps(report, indent=2))
    if args.json:
        args.json.write_text(json.dumps(report, indent=2) + "\n")
    return int(args.resolve and ("resolved_graph_error" in report or any(
        "error" in item for item in report.get("binary_graphs", {}).values()
    )))


if __name__ == "__main__":
    sys.exit(main())

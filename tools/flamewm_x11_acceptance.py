#!/usr/bin/env python3
"""Run selected existing Xephyr canary scenarios in an isolated environment."""

from __future__ import annotations

import argparse
import os
import re
import signal
import secrets
import subprocess
import sys
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
HARNESS = ROOT / "scripts" / "xephyr"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dry-run", action="store_true", help="validate inputs and print the isolated launch plan")
    parser.add_argument("--scenario", choices=("sr05", "sr07", "sr08", "sr15", "all"), default="all",
                        help="bounded existing canary scenario (default: all four supported scenarios)")
    parser.add_argument("--timeout", type=float, default=90.0, help="maximum acceptance run duration in seconds")
    parser.add_argument("--evidence-dir", type=Path,
                        help="unique task-scoped root for all generated acceptance output")
    parser.add_argument("--click", metavar="X,Y", help="send an XTEST pointer click on the nested display")
    parser.add_argument("--key", metavar="KEYSYM", help="send an XTEST key press/release on the nested display")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.timeout <= 0:
        print("FAIL: --timeout must be greater than zero", file=sys.stderr)
        return 1
    if args.click and not re.fullmatch(r"\d+,\d+", args.click):
        print("FAIL: --click must be X,Y non-negative integers", file=sys.stderr)
        return 1
    if args.key and not re.fullmatch(r"[A-Za-z0-9_.+-]+", args.key):
        print("FAIL: --key must be a keysym name", file=sys.stderr)
        return 1
    if not HARNESS.is_file():
        print(f"FAIL: missing existing Xephyr harness: {HARNESS}", file=sys.stderr)
        return 1
    if not os.environ.get("DISPLAY"):
        print("ENVIRONMENT_BLOCKED: host DISPLAY is unset; Xephyr requires an existing X server", file=sys.stderr)
        return 3
    if not args.dry_run and not (os.environ.get("FLAMEWM_XEPHYR_BUILD") == "0" or shutil_which("cargo")):
        print("ENVIRONMENT_BLOCKED: cargo is required unless FLAMEWM_XEPHYR_BUILD=0", file=sys.stderr)
        return 3
    if args.dry_run:
        print(f"DRY_RUN: harness={HARNESS} scenario={args.scenario} click={args.click or 'none'} key={args.key or 'none'} source={ROOT} binary=selected-by-harness; private HOME/XDG and D-Bus; evidence={args.evidence_dir or 'unique temporary task root'}")
        return 0

    with tempfile.TemporaryDirectory(prefix="flamewm-x11-acceptance-") as private_home:
        private_home_path = Path(private_home)
        xdg = {key: private_home_path / value for key, value in (
            ("XDG_CONFIG_HOME", ".config"), ("XDG_DATA_HOME", ".local/share"), ("XDG_CACHE_HOME", ".cache"))}
        for directory in xdg.values():
            directory.mkdir(parents=True, exist_ok=True)
        env = os.environ.copy()
        env.update({
            "HOME": private_home,
            **{key: str(value) for key, value in xdg.items()},
            "FLAMEWM_XEPHYR_PRIVATE_DBUS": "1",
            "FLAMEWM_XEPHYR_CANARY": "1",
            "FLAMEWM_XEPHYR_CANARY_SCENARIOS": "sr05,sr07,sr08,sr15" if args.scenario == "all" else args.scenario,
            "FLAMEWM_XEPHYR_BUILD": env.get("FLAMEWM_XEPHYR_BUILD", "1"),
        })
        evidence_root = (args.evidence_dir or (ROOT / ".agents" / "runtime" / f"acceptance-{secrets.token_hex(12)}"))
        evidence_root = evidence_root.expanduser().resolve()
        output_root = evidence_root / "output"
        env.update({
            "FLAMEWM_XEPHYR_OUTPUT_DIR": str(output_root),
            "FLAMEWM_XEPHYR_EVIDENCE_ROOT": str(evidence_root / "performance"),
            "FLAMEWM_XEPHYR_DEBUG_ROOT": str(evidence_root / "debug"),
        })
        if args.click or args.key:
            nested = next((f":{number}" for number in range(97, 120)
                           if not Path(f"/tmp/.X{number}-lock").exists()
                           and not Path(f"/tmp/.X11-unix/X{number}").exists()), None)
            if nested is None:
                print("ENVIRONMENT_BLOCKED: no unused display in :97..:119", file=sys.stderr)
                return 3
            env["FLAMEWM_XEPHYR_DISPLAY"] = nested
            owner_token = secrets.token_hex(32)
            owner_marker = private_home_path / "xephyr-owner"
            env["FLAMEWM_XEPHYR_XTEST_OWNER_TOKEN"] = owner_token
            env["FLAMEWM_XEPHYR_XTEST_OWNER_MARKER"] = str(owner_marker)
        selected_binary = env.get("FLAMEWM_XEPHYR_CANARY_BIN", str(ROOT / ".target/release/flamewm-x11-canary"))
        print(f"EVIDENCE: harness outputs under {evidence_root}; scenario={args.scenario}; canary_binary={selected_binary}; source={ROOT}")
        # Xephyr chooses a currently-unused nested display in its bounded range.
        child = subprocess.Popen([str(HARNESS)], cwd=ROOT, env=env, start_new_session=True)
        try:
            if args.click or args.key:
                import time
                deadline = time.monotonic() + min(args.timeout, 15.0)
                socket = Path("/tmp/.X11-unix") / f"X{nested[1:]}"
                while time.monotonic() < deadline and child.poll() is None and (not socket.exists() or not owner_marker.is_file()):
                    time.sleep(0.05)
                if child.poll() is not None or not socket.exists() or not owner_marker.is_file():
                    print("FAIL: nested display did not become ready while harness remained alive", file=sys.stderr)
                    terminate_owned_group(child)
                    return 1
                command = [sys.executable, str(ROOT / "tools/flamewm_xtest.py"), "--display", nested,
                           "--owner-token", owner_token, "--owner-marker", str(owner_marker)]
                if args.click:
                    command.extend(("--click", args.click))
                if args.key:
                    command.extend(("--key", args.key))
                input_result = subprocess.run(command, env=env, timeout=max(0.1, deadline - time.monotonic()), check=False)
                if input_result.returncode != 0:
                    terminate_owned_group(child)
                    return input_result.returncode
                print("INPUT_SENT_UNVERIFIED: no post-input state assertion was executed")
            status = child.wait(timeout=args.timeout)
        except subprocess.TimeoutExpired:
            print(f"FAIL: acceptance exceeded {args.timeout:g}s; terminating owned process group", file=sys.stderr)
            terminate_owned_group(child)
            return 1
        if status == 0:
            print(f"UNVERIFIED_SCENARIOS: harness exited successfully; selected={args.scenario}. This does not prove interactive Start/file fixture behavior.")
            return 0
        if status == 3:
            print("ENVIRONMENT_BLOCKED: Xephyr harness reported a missing environment prerequisite", file=sys.stderr)
            return 3
        print(f"FAIL: Xephyr harness exited with status {status}", file=sys.stderr)
        return 1


def shutil_which(command: str) -> str | None:
    for directory in os.environ.get("PATH", "").split(os.pathsep):
        candidate = Path(directory) / command
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return str(candidate)
    return None


def terminate_owned_group(child: subprocess.Popen[bytes]) -> None:
    """Signal only the process group created for this harness invocation."""
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        child.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait()


if __name__ == "__main__":
    raise SystemExit(main())

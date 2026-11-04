#!/usr/bin/env python3
"""Fail-closed glibc runtime baseline gate for packaged ELF payloads.

The Debian lane targets the oldest supported runtime: Debian 12 "bookworm"
(antiX 23 is bookworm-based). A package built on a newer host silently links
against that host's glibc, so dpkg then refuses to install it on the target.

This gate reads the symbol versions each ELF actually requires and compares
them with the single baseline declared in ``packaging/target-baseline.toml``.
Every unreadable input, missing binutils tool or empty payload exits 1, so a
green result always means "verified", never "not checked".

Usage:
    python3 -I tools/flamewm_glibc_baseline.py --deb .target/dist/flamewm_*.deb
    python3 -I tools/flamewm_glibc_baseline.py --dir .target/release
    python3 -I tools/flamewm_glibc_baseline.py .target/release/flamewm
"""
from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_BASELINE_FILE = ROOT / "packaging/target-baseline.toml"
ELF_MAGIC = b"\x7fELF"
# GLIBC_PRIVATE carries no version number and therefore never matches.
GLIBC_VERSION = re.compile(r"GLIBC_(\d+(?:\.\d+)+)")


class GateError(Exception):
    """A fail-closed condition: the payload could not be verified."""


def version_tuple(text: str) -> tuple[int, ...]:
    """'2.2.5' -> (2, 2, 5) so GLIBC_2.2.5 ranks below GLIBC_2.36."""
    try:
        return tuple(int(part) for part in text.split("."))
    except ValueError as error:
        raise GateError(f"invalid glibc version {text!r}") from error


def read_baseline(path: Path) -> str:
    if not path.is_file():
        raise GateError(f"baseline file missing: {path}")
    try:
        baseline = tomllib.loads(path.read_text(encoding="utf-8"))["debian"]["glibc"]
    except (OSError, ValueError, KeyError) as error:
        raise GateError(f"cannot read [debian].glibc from {path}: {error}") from error
    version_tuple(str(baseline))
    return str(baseline)


def required_versions(path: Path) -> dict[str, tuple[int, ...]]:
    """GLIBC versions the ELF needs, keyed by their original spelling.

    A definition-only occurrence is treated as required too: over-reporting is
    fail-closed, under-reporting would ship a package that cannot install.
    """
    report = None
    for tool, args in (("readelf", "--version-info"), ("objdump", "-T")):
        executable = shutil.which(tool)
        if executable is None:
            continue
        result = subprocess.run([executable, args, str(path)], capture_output=True, text=True, check=False)
        if result.returncode == 0 and result.stdout:
            report = result.stdout
            break
    if report is None:
        raise GateError(f"cannot read ELF version info with readelf or objdump: {path}")
    return {match.group(1): version_tuple(match.group(1)) for match in GLIBC_VERSION.finditer(report)}


def deb_elfs(deb: Path, workdir: Path) -> list[tuple[str, Path]]:
    """Stream the .deb payload and materialize only its ELF members."""
    if not deb.is_file():
        raise GateError(f"package not found: {deb}")
    proc = subprocess.Popen(
        ["dpkg-deb", "--fsys-tarfile", str(deb)],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    elfs: list[tuple[str, Path]] = []
    error: Exception | None = None
    try:
        with tarfile.open(fileobj=proc.stdout, mode="r|") as archive:
            for member in archive:
                if not member.isfile():
                    continue
                handle = archive.extractfile(member)
                if handle is None:
                    continue
                head = handle.read(len(ELF_MAGIC))
                if head != ELF_MAGIC:
                    continue
                extracted = workdir / f"{len(elfs):04d}"
                extracted.write_bytes(head + handle.read())
                elfs.append((member.name, extracted))
    except (tarfile.TarError, OSError) as exc:
        error = exc
        proc.kill()
    _, stderr = proc.communicate()
    if error is not None or proc.returncode != 0:
        detail = (error or stderr.decode("utf-8", "replace")).strip()
        raise GateError(f"cannot read package payload {deb}: {detail}")
    return elfs


def dir_elfs(directory: Path) -> list[tuple[str, Path]]:
    if not directory.is_dir():
        raise GateError(f"directory not found: {directory}")
    elfs = []
    for path in sorted(directory.rglob("*")):
        if not path.is_file() or path.is_symlink():
            continue
        with path.open("rb") as handle:
            if handle.read(len(ELF_MAGIC)) == ELF_MAGIC:
                elfs.append((path.relative_to(directory).as_posix(), path))
    return elfs


def file_elfs(files: list[Path]) -> list[tuple[str, Path]]:
    elfs = []
    for path in files:
        try:
            with path.open("rb") as handle:
                is_elf = handle.read(len(ELF_MAGIC)) == ELF_MAGIC
        except OSError as error:
            raise GateError(f"cannot read {path}: {error}") from error
        if not is_elf:
            raise GateError(f"not an ELF file: {path}")
        elfs.append((str(path), path))
    return elfs


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Reject ELF payloads that require a glibc newer than the packaging baseline.",
    )
    source = parser.add_mutually_exclusive_group()
    source.add_argument("--deb", type=Path, help="Debian package to inspect (payload is streamed, not installed)")
    source.add_argument("--dir", dest="directory", type=Path, help="directory tree scanned for ELF files")
    parser.add_argument("files", nargs="*", type=Path, help="ELF files to inspect")
    parser.add_argument(
        "--baseline-file",
        type=Path,
        default=DEFAULT_BASELINE_FILE,
        help="TOML holding the baseline (default: packaging/target-baseline.toml next to this script's repo)",
    )
    parser.add_argument("--baseline", help="explicit baseline version; overrides --baseline-file")
    parser.add_argument("--json", action="store_true", help="print a machine-readable result")
    args = parser.parse_args(argv)

    try:
        baseline_text = args.baseline or read_baseline(args.baseline_file)
        baseline = version_tuple(baseline_text)
        if args.deb is None and args.directory is None and not args.files:
            raise GateError("nothing to check: pass --deb PATH, --dir PATH, or ELF files")
        if (args.deb is not None or args.directory is not None) and args.files:
            raise GateError("choose one source: --deb, --dir, or explicit ELF files")
        if shutil.which("readelf") is None and shutil.which("objdump") is None:
            raise GateError("neither readelf nor objdump is available (install binutils)")
        with tempfile.TemporaryDirectory(prefix="flamewm-glibc-") as workdir:
            if args.deb is not None:
                elfs = deb_elfs(args.deb, Path(workdir))
            elif args.directory is not None:
                elfs = dir_elfs(args.directory)
            else:
                elfs = file_elfs(args.files)
            if not elfs:
                raise GateError(f"no ELF files found in {args.deb or args.directory or args.files}")
            scanned = []
            for name, path in elfs:
                versions = required_versions(path)
                if not versions:
                    continue
                peak = max(versions.values())
                top = max(versions, key=versions.get)
                scanned.append((name, top, peak))
    except (GateError, OSError) as error:
        print(f"FLAMEWM-GLIBC-ERROR: {error}", file=sys.stderr)
        return 1

    offenders = [entry for entry in scanned if entry[2] > baseline]
    highest = max((entry[2] for entry in scanned), default=None)
    highest_text = next((entry[1] for entry in scanned if entry[2] == highest), None) or "none"

    if args.json:
        print(
            json.dumps(
                {
                    "ok": not offenders,
                    "baseline": baseline_text,
                    "max": highest_text,
                    "elfs": len(elfs),
                    "offenders": [{"file": name, "requires": version} for name, version, _ in offenders],
                },
                sort_keys=True,
            )
        )
    elif not offenders:
        print(f"OK glibc-baseline: baseline={baseline_text} max={highest_text} elfs={len(elfs)}")
    else:
        for name, version, _ in offenders:
            print(
                f"FLAMEWM-GLIBC-ERROR: {name} requires GLIBC_{version} (baseline {baseline_text})",
                file=sys.stderr,
            )

    if offenders:
        print(
            f"FLAMEWM-GLIBC-ERROR: {len(offenders)} of {len(elfs)} ELF files exceed the "
            f"{baseline_text} baseline (max GLIBC_{highest_text}); build the package in the "
            "baseline container with ./scripts/build-deb",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

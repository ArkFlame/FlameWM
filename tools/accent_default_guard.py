#!/usr/bin/env python3
"""Fail when the default accent literal (#539BFF) is hardcoded outside its owners.

The default lives once in `flamewm-platform` settings. The files below are the only
other copies allowed: per-app fallbacks used when the snapshot lacks `accent`, and
CSS `:root` token defaults that the live Settings projection overwrites. Any other
occurrence is a hardcoded accent that would ignore the Settings key.
"""

import pathlib
import re
import subprocess
import sys

DEFAULT_ACCENT = re.compile(r"539bff|0x0053_9bff|0x539bff", re.IGNORECASE)
SCANNED_SUFFIXES = (".rs", ".css", ".html", ".toml")
SCANNED_ROOTS = ("crates", "ui")
ALLOWED = frozenset(
    {
        "crates/flamewm-platform/src/settings.rs",
        "crates/flamewm-settings/src/lib.rs",
        "crates/flamewm-finder/src/lib.rs",
        "crates/flamewm-explorer/src/theme.rs",
        "crates/flamewm-terminal/src/theme.rs",
        "crates/flamewm-task-manager/src/lib.rs",
        "crates/flamewm-lock/src/settings.rs",
        "ui/flamewm.css",
        "ui/settings.css",
        "ui/finder/finder.css",
    }
)


def scanned_files(root: pathlib.Path):
    listing = subprocess.run(
        ["git", "-C", str(root), "ls-files", *SCANNED_ROOTS],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split()
    return [
        name
        for name in listing
        if name.endswith(SCANNED_SUFFIXES) and "/tests/" not in name
    ]


def violations(root: pathlib.Path):
    found = []
    for name in scanned_files(root):
        if name in ALLOWED:
            continue
        text = (root / name).read_text(encoding="utf-8", errors="ignore")
        for number, line in enumerate(text.splitlines(), 1):
            if DEFAULT_ACCENT.search(line):
                found.append(f"{name}:{number}: hardcoded default accent")
    return found


def main(root=None) -> int:
    base = pathlib.Path(root) if root else pathlib.Path(__file__).resolve().parent.parent
    found = violations(base)
    for line in found:
        print(line, file=sys.stderr)
    if found:
        print(f"accent_default_guard: FAILED ({len(found)} hardcoded default accents)", file=sys.stderr)
        return 1
    print("accent_default_guard: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())

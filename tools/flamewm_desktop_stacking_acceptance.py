#!/usr/bin/env python3
"""Native stacking check: the FlameWM Desktop window is the bottom-most root child.

`xwininfo -root -tree` lists root children top-to-bottom (first line = topmost),
the reverse of XQueryTree order. The check passes only when the Desktop window is
the LAST root child in that listing, i.e. below every frame, the panel and the
other FlameWM surfaces.

Usage: flamewm_desktop_stacking_acceptance.py --display :N
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys

DESKTOP_NAME = "FlameWM Desktop"
# Root children are indented by exactly five spaces in xwininfo output.
ROOT_CHILD_RE = re.compile(r"^ {5}(0x[0-9a-f]+)\s*(.*)$")


def root_children_top_first(tree_text: str) -> list[tuple[str, str]]:
    """Return (window id, rest-of-line) for each root child, topmost first."""
    children = []
    for line in tree_text.splitlines():
        match = ROOT_CHILD_RE.match(line)
        if match:
            children.append((match.group(1), match.group(2)))
    return children


def check_desktop_bottom(tree_text: str) -> tuple[bool, str]:
    children = root_children_top_first(tree_text)
    desktop = [i for i, (_, rest) in enumerate(children) if f'"{DESKTOP_NAME}"' in rest]
    if len(desktop) != 1:
        return False, f"expected exactly one '{DESKTOP_NAME}' root child, found {len(desktop)}"
    index = desktop[0]
    if index != len(children) - 1:
        above = [rest.split('"')[1] if '"' in rest else rest for _, rest in children[index + 1 :]]
        return False, f"'{DESKTOP_NAME}' is not bottom-most; windows beneath it: {above}"
    return True, f"'{DESKTOP_NAME}' is the bottom-most of {len(children)} root children"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--display", required=True, help="X display to inspect, e.g. :135")
    args = parser.parse_args(argv)
    env = dict(os.environ, DISPLAY=args.display)
    result = subprocess.run(
        ["xwininfo", "-root", "-tree"], env=env, capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        print(f"FAIL: xwininfo exited {result.returncode}: {result.stderr.strip()}")
        return 1
    ok, message = check_desktop_bottom(result.stdout)
    print(("PASS: " if ok else "FAIL: ") + message)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())

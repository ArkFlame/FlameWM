#!/usr/bin/env python3
# Pixel verdict for scripts/break/alttab-thumb.sh. Parses the dbus-monitor WindowSwitcherChanged log.
# Each signal: boolean (open) then entries of appId, thumb{generation,height,rgba,width}, title.
# Pixels are classified channel-order agnostically: a pixel is one-hot when exactly one of its first
# three bytes is saturated (red, green or blue) and two-hot when two are (yellow).
# Usage: python3 -I alttab_thumb_verdict.py <signals.log>   Exit 0 = PASS, 1 = FAIL.
import re
import sys

HEX = re.compile(r"\b[0-9a-f]{2}\b")


def parse_opens(path):
    """Returns a list of opens; each open maps title -> list of rgba byte strings seen while open."""
    opens = []
    cur = None
    sig_open = None
    pending = None
    in_rgba = False
    rgba = []
    for line in open(path, encoding="utf-8", errors="replace"):
        line = line.rstrip("\n")
        if line.startswith("signal time="):
            sig_open = None
            pending = None
            in_rgba = False
            continue
        if sig_open is None:
            m = re.match(r"^\s+boolean (true|false)$", line)
            if m:
                sig_open = m.group(1) == "true"
                if sig_open and cur is None:
                    cur = {}
                    opens.append(cur)
                elif not sig_open:
                    cur = None
            continue
        if 'string "rgba"' in line:
            in_rgba, rgba = True, []
            continue
        if in_rgba:
            if "]" in line:
                in_rgba = False
                rgba += HEX.findall(line.split("]")[0])
                pending = bytes(int(n, 16) for n in rgba)
            else:
                rgba += HEX.findall(line)
            continue
        if 'string "title"' in line:
            continue
        m = re.search(r'variant\s+string "(.*)"$', line)
        if m and pending is not None and cur is not None:
            cur.setdefault(m.group(1), []).append(pending)
            pending = None
    return opens


def hot_positions(buf):
    """Counts one-hot pixels and records which colour channel is hot for each."""
    one, two, channels = 0, 0, set()
    for i in range(0, len(buf) - 3, 4):
        px = buf[i:i + 3]
        hot = [j for j, b in enumerate(px) if b >= 0xE0]
        if len(hot) == 1:
            one += 1
            channels.add(hot[0])
        elif len(hot) == 2:
            two += 1
    return one, two, channels


def main():
    if len(sys.argv) != 2:
        print("usage: alttab_thumb_verdict.py <signals.log>", file=sys.stderr)
        return 2
    opens = parse_opens(sys.argv[1])
    if len(opens) < 2:
        print(f"PIXELS: FAIL opens_with_entries={len(opens)} (need 2)")
        return 1
    failures = []
    # Open 1: repaint-A alternates red and blue, so over its frames the hot channel set must
    # cover at least two different channels with pure one-hot pixels.
    a_frames = opens[0].get("repaint-A", [])
    a_channels = set()
    for buf in a_frames:
        a_channels |= hot_positions(buf)[2]
    if len(a_frames) < 2 or len(a_channels) < 2:
        failures.append(f"repaint-A showed <2 colours in open 1 (frames={len(a_frames)})")
    print(f"repaint-A open1 frames={len(a_frames)} hot_channels={sorted(a_channels)}")
    # Open 2: minimised-B must show its last visible frame (green, one-hot) and never the
    # yellow frame it repainted while minimised (two-hot).
    b_frames = opens[1].get("minimised-B", [])
    if not b_frames:
        failures.append("minimised-B has no thumb in open 2")
    else:
        buf = b_frames[-1]
        one, two, channels = hot_positions(buf)
        print(f"minimised-B open2 frames={len(b_frames)} one_hot={one} two_hot={two} hot_channels={sorted(channels)}")
        if one == 0 or two > 0:
            failures.append(f"minimised-B open2 not the green frame (one_hot={one} two_hot={two})")
    print("PIXELS: " + ("FAIL " + "; ".join(failures) if failures else "PASS"))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

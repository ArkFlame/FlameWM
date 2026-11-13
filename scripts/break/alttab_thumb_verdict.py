#!/usr/bin/env python3
# Verdict for scripts/break/alttab-thumb.sh. Parses the dbus-monitor WindowSwitcherChanged log.
# Each entry is a dict whose keys dbus-monitor prints sorted: minimised (only when true),
# otherWorkspace (only when true), appId, thumb{generation,height,rgba,width}, title.
# Pixels are classified channel-order agnostically: a pixel is one-hot when exactly one of its first
# three bytes is saturated (red, green or blue) and two-hot when two are (yellow).
# Usage: python3 -I alttab_thumb_verdict.py <signals.log>   Exit 0 = PASS, 1 = FAIL.
import re
import sys

HEX = re.compile(r"\b[0-9a-f]{2}\b")


def parse_opens(path):
    """Each open maps title -> {"pixels": [rgba bytes], "hidden": [bool per signal]}."""
    opens = []
    cur = None
    sig_open = None
    pending = None
    pending_hidden = False
    in_rgba = False
    rgba = []
    key = None
    for line in open(path, encoding="utf-8", errors="replace"):
        line = line.rstrip("\n")
        if line.startswith("signal time="):
            sig_open, pending, pending_hidden, in_rgba = None, None, False, False
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
        m = re.search(r'string "(minimised|otherWorkspace|appId|thumb|title)"', line)
        if m:
            key = m.group(1)
            continue
        if key == "minimised" or key == "otherWorkspace":
            v = re.search(r"variant\s+boolean (true|false)$", line)
            if v:
                pending_hidden = pending_hidden or v.group(1) == "true"
                key = None
            continue
        if key == "title" and cur is not None:
            v = re.search(r'variant\s+string "(.*)"$', line)
            if v:
                entry = cur.setdefault(v.group(1), {"pixels": [], "hidden": []})
                if pending is not None:
                    entry["pixels"].append(pending)
                entry["hidden"].append(pending_hidden)
                pending, pending_hidden, key = None, False, None
    return opens


def hot_positions(buf):
    """Counts one-hot and two-hot pixels and records the hot channel of each one-hot pixel."""
    one, two, channels = 0, 0, set()
    for i in range(0, len(buf) - 3, 4):
        hot = [j for j, b in enumerate(buf[i:i + 3]) if b >= 0xE0]
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
    # Open 1: repaint-A alternates red and blue while visible, so its frames cover two channels.
    a1 = opens[0].get("repaint-A", {"pixels": [], "hidden": []})
    a_channels = set()
    for buf in a1["pixels"]:
        a_channels |= hot_positions(buf)[2]
    if len(a1["pixels"]) < 2 or len(a_channels) < 2:
        failures.append(f"repaint-A showed <2 colours in open 1 (frames={len(a1['pixels'])})")
    if any(a1["hidden"]):
        failures.append("repaint-A flagged hidden in open 1")
    print(f"repaint-A open1 frames={len(a1['pixels'])} hot_channels={sorted(a_channels)}")
    # Open 2: minimised-B is flagged minimised and shows its last visible frame (green, one-hot),
    # never the yellow frame it repainted while minimised (two-hot). The visible A is not flagged.
    b2 = opens[1].get("minimised-B", {"pixels": [], "hidden": []})
    a2 = opens[1].get("repaint-A", {"pixels": [], "hidden": []})
    if not b2["pixels"]:
        failures.append("minimised-B has no thumb in open 2")
    else:
        one, two, channels = hot_positions(b2["pixels"][-1])
        print(f"minimised-B open2 frames={len(b2['pixels'])} one_hot={one} two_hot={two} hot_channels={sorted(channels)}")
        if one == 0 or two > 0:
            failures.append(f"minimised-B open2 not the green frame (one_hot={one} two_hot={two})")
    if not b2["hidden"] or not all(b2["hidden"]):
        failures.append("minimised-B not flagged minimised in open 2")
    if not a2["hidden"] or any(a2["hidden"]):
        failures.append("visible repaint-A flagged hidden in open 2")
    print(f"flags open2 minimised-B_hidden={b2['hidden'][:1]} repaint-A_hidden={a2['hidden'][:1]}")
    print("PIXELS: " + ("FAIL " + "; ".join(failures) if failures else "PASS"))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())

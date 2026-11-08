# Competitor tracking

Index of competing Linux desktops studied against FlameWM. Each entry records what the competitor ships, what it does better than FlameWM, what FlameWM does better, and where the evidence comes from.

Ground rules for every entry:

- Source and documentation paths are cited. Competitor source is read from `.vendor/**` as read-only reference.
- Performance numbers are reported only with their measurement scope. Numbers from different machines, display servers or process sets are not compared as if they were equal.
- Unmeasured items are marked `unmeasured`. They are not estimated.
- Design scores come from screenshots only and are marked with the image they rest on.

| Competitor | Session type | Licence | Snapshot date | Entry |
| --- | --- | --- | --- | --- |
| Vela (`vela-desktop/vela`, local copy `.vendor/vela-main`) | Wayland (own compositor, Vulkan, Qt Quick shell) | GPL-3.0-or-later | 2026-11-08 | [FEATURES](vela/FEATURES.md), [DESIGN_COMPARISON](vela/DESIGN_COMPARISON.md) |
| Xfce xfwm4 + xfdesktop, MATE libmate-desktop (`.vendor/xfwm4-master`, `xfdesktop-master`, `mate-desktop-master`) | X11 window manager, desktop, shared helper library | GPL-2.0-or-later (xfwm4, xfdesktop); GPL-2.0/LGPL (MATE) | 2026-11-08 | [xfce LEARNINGS](xfce/LEARNINGS.md), [mate LEARNINGS](mate/LEARNINGS.md) |

Add a new competitor by creating `docs/competitors/<name>/` with the same two files and one row in the table above.

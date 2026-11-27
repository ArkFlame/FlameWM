# Unit p47-render-damage-only
STATUS: OPEN

Crates flamewm-wm-wayland and flamewm-render-x11: find full-surface repaints where only a small region changed (clock tick, hover); restrict to damage rects, pixels identical. Report with file:line, max 3 edits.

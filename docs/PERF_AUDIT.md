# Performance audit log

## Open

- **Stale pixels on right edge after maximize** (2026-11-09, low priority). Seen only in the nested
  `flamewm-wayland` run at the 1350x641 default. At 1280x800 (`FLAMEWM_WAYLAND_WIDTH=1280
  FLAMEWM_WAYLAND_HEIGHT=800`) with autoHide on, `/tmp/claude-1000/dockshots/size-on.png` shows no
  artifact. Not reproduced at 1280 with autoHide off. Pre-D1 (4214fe2f) comparison not run.
  Next: reproduce at 1280 with autoHide off, then at 4214fe2f before deciding regression vs old.

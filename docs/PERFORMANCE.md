# Performance Contract

## Runtime rules

1. No browser/WebView/JavaScript/WGPU runtime.
2. HTML/CSS parsing/cascade occurs at build time only.
3. No continuous frame loop.
4. Layout/repaint follows state/damage/input events only.
5. Pointer motion is coalesced only across contiguous same-window MotionNotify events; event ordering is never crossed.
6. Cached X11 pixmaps/masks/fonts/colors are reused.
7. GraphicsExpose events are disabled for retained image copy operations.
8. Network/audio/media use subscriptions, not repeated CLI polling.
9. The WM remains the single managed-window/workspace authority.
10. Measure PSS, idle CPU/wakeups, startup and interaction latency before accepting additional architecture.

## Release profile

```text
opt-level = 3
lto = thin
codegen-units = 1
panic = abort
strip = symbols
```

The compiler crate uses package-specific `opt-level=2`; parser speed is not steady-state interaction cost.

## Acceptance targets

- combined Flame-owned graphical process PSS <= 40 MiB target;
- near-zero idle CPU outside real events/timers;
- no visible drag backlog in Xephyr;
- no stale motion rendered after release;
- Start/taskbar/window manipulation must not block on disk/network/system commands.

## Measured status (0.0.9)

Measurements from a development machine. They are not release certification.

| Scenario | Result | Against target |
| --- | --- | --- |
| Harness, no theme, combined WM + shell, idle | about 35 MiB RSS; about 21 MiB PSS | PSS is below the 40 MiB PSS target. |
| Real session with an icon theme | about 80 MiB RSS | Not evaluated against the PSS target; PSS not reported. |

- The memory tooling in `tools/flamewm_mem_phase_compare.py` uses a 34 MiB threshold (35651584 B) on combined WM + shell PSS and RSS. That threshold is **not reached for RSS** in the figures above.
- The X server process is separate and is excluded from these figures.
- Frame-time p95 has **not been measured**.
- Idle CPU/wakeups, startup latency, and interaction latency are not recorded in this document.
- Not run: the installed `.deb`, two-output RandR, and a real NetworkManager session.

## Profiler (`flamewm-profiler`, frozen seams R01-R06)

- Static `&'static str` labels only; no per-span allocation or thread.
- Env: `FLAMEWM_PROFILE=1`, `FLAMEWM_PROFILE_INTERVAL` (default 60, min 10),
  `FLAMEWM_PROFILE_TOP` (default 8, max 8).
- Report: single `FLAMEWM_PROFILE_SUMMARY` block via `report_window()` with
  `SLOWEST_SINGLE` / `HIGHEST_TOTAL` / `HIGHEST_CALL_COUNT` / `MEMORY_TOP`.
- No product instrumentation yet; product crates opt in later.

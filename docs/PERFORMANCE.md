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
lto = fat
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

## Benchmark

### Idle Desktop

| Process | PSS | Code | Libraries | Heap | Fonts | CPU |
| --- | --- | --- | --- | --- | --- | --- |
| `flamewm-session` | 1.5 MiB | 0.8 MiB | 0.5 MiB | 0.3 MiB | 0.0 MiB | 0.0% |
| `flamewm` | 5.5 MiB | 2.8 MiB | 1.9 MiB | 0.7 MiB | 0.0 MiB | 0.0% |
| `flamewm-desktop` | 5.0 MiB | 2.8 MiB | 1.3 MiB | 0.9 MiB | 0.0 MiB | 0.0% |
| `flamewm-shell` | 11.0 MiB | 2.8 MiB | 4.7 MiB | 2.3 MiB | 1.0 MiB | 0.0% |
| `flamewm-shell --quick-control-host` | 2.8 MiB | 1.2 MiB | 1.3 MiB | 0.3 MiB | 0.0 MiB | 0.0% |
| **Total** | **25.8 MiB** | **10.4 MiB** | **9.6 MiB** | **4.4 MiB** | **1.0 MiB** | **0.0%** |

### Idle Suite

| Process | PSS | CPU |
| --- | --- | --- |
| `flamewm-session` | 1.5 MiB | 0.0% |
| `flamewm` | 8.8 MiB | 0.0% |
| `flamewm-desktop` | 3.8 MiB | 0.0% |
| `flamewm-shell` | 8.4 MiB | 0.0% |
| `flamewm-shell --quick-control-host` | 2.1 MiB | 0.0% |
| `flamewm-terminal` | 19.8 MiB | 0.0% |
| `flamewm-explorer` | 11.5 MiB | 0.0% |
| `flamewm-task-manager` | 12.6 MiB | 0.6% |
| **Total** | **68.5 MiB** | **0.6%** |

### Interaction

| CPU, % of one core | WM | Desktop | Shell | Terminal | Xorg | Total |
| --- | --- | --- | --- | --- | --- | --- |
| Window drag | 1.6 | 0.5 | 4.4 | 0.5 | 8.2 | 15.3 |
| Start popup open and close | 0.0 | 0.0 | 8.0 | 0.0 | 26.0 | 37.9 |

| Measure | Result |
| --- | --- |
| Session start to panels mapped | 719 ms |

## Profiler (`flamewm-profiler`, frozen seams R01-R06)

- Static `&'static str` labels only; no per-span allocation or thread.
- Env: `FLAMEWM_PROFILE=1`, `FLAMEWM_PROFILE_INTERVAL` (default 60, min 10),
  `FLAMEWM_PROFILE_TOP` (default 8, max 8).
- Report: single `FLAMEWM_PROFILE_SUMMARY` block via `report_window()` with
  `SLOWEST_SINGLE` / `HIGHEST_TOTAL` / `HIGHEST_CALL_COUNT` / `MEMORY_TOP`.
- No product instrumentation yet; product crates opt in later.

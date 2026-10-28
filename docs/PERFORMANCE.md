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

## Measured status

Measured on 2026-10-28 with `tools/flamewm_visual_tests.py bench`: Xorg with the dummy
driver at 1280x800, the release build, an Intel Core i5-6500 and an otherwise idle
machine. They are development figures, not release certification. Rerun them with:

```sh
python3 -I tools/flamewm_visual_tests.py --bin-dir .target/release bench
python3 -I tools/flamewm_visual_tests.py --bin-dir .target/release --contents-while-dragging bench
```

The bench starts a session with one `flamewm-terminal` window, then samples
`/proc/<pid>/stat` over 5 s windows, five times per case. CPU is percent of one core
averaged over the whole window. A `window-move` sample holds one drag of the terminal
across 400x250 px and back, which takes about 1.6 s of the 5 s.

| Measure | Result |
| --- | --- |
| Session start to panels mapped | about 750 ms |
| Idle PSS: WM / desktop / shell / session | 12.2 / 16.4 / 11.5 / 1.5 MiB |
| Idle PSS: `flamewm-terminal` | 18.1 MiB |
| Idle CPU, every FlameWM process and Xorg | 0.0-0.2% |
| Start popup visible after the click | 310-335 ms |
| Drag feedback after the first motion | 5-7 ms |

| Case (CPU % of one core, median of 5) | WM | Desktop | Shell | Terminal | Xorg | Total |
| --- | --- | --- | --- | --- | --- | --- |
| Window drag, outline (default) | 1.6 | 0.0 | 3.2 | 0.5 | 5.3 | 11.0 |
| Window drag, contents while dragging | 4.2 | 0.5 | 1.0 | 21.0 | 20.7 | 47.6 |
| Start popup open and close | 0.0 | 0.0 | 4.0 | 0.0 | 14.0 | 20.0 |

- WM, desktop and shell together use 40.1 MiB PSS at idle, just above the 40 MiB target.
- With contents shown while dragging, the translucent terminal updates its blurred
  backdrop on every move step, computing only the strips the move exposes. Before that
  change and the separable crop it used 53.9% of a core in the same case.
- Frame-time p95 has **not been measured**.
- Not measured here: the installed `.deb`, a real GPU driver, a real NetworkManager session.
- Two outputs are covered by the visual checks (`--layout dual`), not by these numbers.

## Profiler (`flamewm-profiler`, frozen seams R01-R06)

- Static `&'static str` labels only; no per-span allocation or thread.
- Env: `FLAMEWM_PROFILE=1`, `FLAMEWM_PROFILE_INTERVAL` (default 60, min 10),
  `FLAMEWM_PROFILE_TOP` (default 8, max 8).
- Report: single `FLAMEWM_PROFILE_SUMMARY` block via `report_window()` with
  `SLOWEST_SINGLE` / `HIGHEST_TOTAL` / `HIGHEST_CALL_COUNT` / `MEMORY_TOP`.
- No product instrumentation yet; product crates opt in later.

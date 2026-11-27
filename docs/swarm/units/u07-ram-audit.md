# u07 — RAM and binary size audit (embedded assets, pools, caches)

**Scope:** read-only audit plus the ranked contributor list in `docs/PERF_HOTLIST.md`. No
`git`, no `cargo`, no builds, tests, Python or sleep loops; shared checkout, orchestrator
commits. Visuals are unchanged.

## Report

The ranked hotlist puts the Wayland dock/backdrop capture and blur first (70% of compositor
frame time in the u159 sample, still 87–90% of remaining frames after the u156/u164 caps),
followed by the terminal's three pre-baked UI pool tiers (~10 of 13.3–14 MB of binary on
disk; the decoded active tier costs 4.2 MB at tier 1 and 12.2 MB at tier 2, while unused
tiers stay file-backed), then the wallpaper pipeline (an ~8–9 MiB retained 1080p raster and
~90% of an uncached lock repaint before the warm-cache and 256-entry dim-table cuts), with
the shell icon LRU (2048 entries / 32 MiB), the task-manager 1 Hz `/proc` sampler (~2.5% of
one core while visible), X11 retained image caches (16 MiB per app, 64 MiB for eight
minimized windows, 4 MiB titlebars), the 31.64 MiB 4K screenshot readback (now converted
in place), explorer's 256 KiB thumbnail LRU and the 4 MiB-capped terminal output queue
behind them; idle wakeups are not a ranked hotspot because explorer, terminal and shell
park at zero ticks and the residual 15 ms icon wait, boot-time readiness poll and tab-close
hangup poll are bounded or event driven. No unbounded cache or missing LRU remains among
the audited owners: the previously unbounded terminal output queue and desktop watcher
name set are capped, the shell catalog shadow was compacted, and scrollback, icon rasters,
thumbnails, search hits, trash entries, undo/remote recents and compositor glyph caches all
have explicit caps; the only open RAM lever is lazy run-node allocation inside a terminal
tier, which needs a live measurement of slot occupancy before any change. Embedded assets
are small and lazy (640 KB lock wallpaper, 234 KB desktop wallpaper, staged menu SVGs, no
embedded font binaries), and the release profile already uses fat LTO, one codegen unit,
`panic = "abort"`, stripped symbols and `opt-level = "s"` for idle-facing binaries, so no
Cargo.toml profile or feature change was made — no safe tweak exists and dependency trim
is a disk-only question with no runtime share.

The retry extends the same hotlist with rows 13–20 in a second table: Xwayland idle RSS/PSS
(61.6 MB PSS / 114.6 MB RSS, 14.8 MB of it output-sized shared pixmaps), Mesa llvmpipe mappings
(about 33% of the flamewm-wayland idle PSS sample), the unattributed allocator heap (about 40 MB,
with only 0.46 MB of live Rust bytes in wm), unmeasured per-process font tables, thread and channel
wake mechanics, the previously unbounded desktop watcher name set (capped at 1024), explorer burst
refresh debounce and reactor timer realignment. They are appended rather than folded into rows
1–12 because the cited samples come from different revisions and harnesses; no new build, runtime
sample or visual change was made.

## Evidence paths

- Ranked table: `docs/PERF_HOTLIST.md`
- Scoped CPU compositions and RAM/idle samples: `docs/perf-loop/LOG.md:39,123-126,162-164,217-227,234-242,251-256,285-307,373-396`
- Per-app RSS/PSS and provenance: `docs/PERFORMANCE.md:130-205`
- Cache bounds survey: `docs/perf-loop/LOG-u75-ram-structs.md:9-19,57-73`
- X11 retained caches and readback: `docs/history/FLAMEWM_PASS48_NONWALLPAPER_CAPTURE_AND_RENDERER_MEMORY_2026-10-20.md:5-15`
- Wallpaper peak allocations: `docs/history/FLAMEWM_PASS46_WALLPAPER_PEAK_MEMORY_AUDIT_2026-10-20.md:10-12`
- Pools and embedded assets: `crates/flamewm-terminal/build.rs:46-65`, `crates/flamewm-terminal/src/pool.rs:14-25`, `crates/flamewm-terminal/src/main.rs:31-33,179`, `crates/flamewm-lock/src/document.rs:21`, `crates/flamewm-desktop/build.rs:28,90`
- Release profile: `Cargo.toml:66-71,76-121`

- Added extension rows 13–20: `docs/backlog/wayland-rss.md:13-14,18,22-26,32-36,39,43-56`;
  `docs/PERFORMANCE_PSS_DUPLICATION_SCOUT_2026-11-05.md:4-16`;
  `docs/perf-loop/LOG-u77-thread-audit.md:10-23,27-52,53-66`;
  `docs/perf-loop/LOG-u75-ram-structs.md:43-52`;
  `docs/perf-loop/LOG-u110-debounce-audit.md:45-58`;
  `docs/perf-loop/LOG-u11-reactor-timers.md:43`.

**Changed paths:** `docs/PERF_HOTLIST.md`, `docs/swarm/units/u07-ram-audit.md`.
**Verification:** none executed by design (no cargo/git/tests in this unit); all figures
are cited from existing measurements and code budgets, not fresh runtime evidence.
**Risk:** shares are from different revisions and harnesses and are not comparable as a
single distribution; row 2 and row 1 need live samples before any cut.

# u107-startup-lazy

Area: process startup for explorer, task manager, terminal and shell. The composition
question is first-frame latency: which synchronous work sits between process entry and
the first painted frame. Fetching the session settings snapshot is the one non-critical
item left on that path in three of the four processes; everything else (template decode,
surface creation, panel projection) is needed to paint. Icons and fonts were already
lazy (explorer/terminal glyphs and thumbnails are per-row, shell spans `IconLoader` is
optional until the first named icon), so only the settings round trip was moved.

Changes:
- `crates/flamewm-explorer/src/main.rs`: `initial_settings` now runs on its own
  `flamewm-explorer-settings` worker and posts the snapshot through the existing wake
  socket instead of being `join`ed before the window is built. The first frame uses
  `ExplorerTheme::default()` with no backdrop; the wake callback applies the theme and
  calls `request_window_backdrop`, and the live `SettingsSnapshotChanged` path uses the
  same code so a wallpaper/blur change after startup also re-requests the backdrop.
- `crates/flamewm-task-manager/src/lib.rs`: the app starts on
  `SettingsSnapshot::new(0)` (the previous fallback when the bus was unavailable) and
  fetches the live snapshot on a worker through the sample wake socket. The wake
  callback swaps `app.theme`, marks `theme_dirty` and requests the matching window
  backdrop, then paints in the same turn.
- `crates/flamewm-shell/src/main.rs`: the startup bootstrap no longer includes
  `GetSettings`. `load_panels_bootstrap` returns only the snapshot the first panel frame
  needs; `load_startup_settings` fetches settings on a worker and a
  `UnixStream` self-pipe registered with the reactor installs it through the ordinary
  `apply_settings_snapshot` path (same entry point as a live signal). The late-attach
  and degraded-start paths keep `load_bootstrap_and_settings`.
- `crates/flamewm-terminal/src/main.rs`: unchanged. It already paints from the cached
  snapshot and fetches `GetSettings` on `spawn_settings_fetch`, which is the pattern the
  other three now follow.

Expected composition shift: first-frame latency in explorer and task manager no longer
includes a control-bus settings round trip (one connect plus a `GetSettings` call, with
the timeout as the worst case); shell's first panel frame drops the extra
`GetSettings` on the startup connection. Steady-state wakeups are unchanged: the
workers post one byte each and exit, and the snapshots are applied in the existing
wake turn. Visuals/features are preserved: defaults already matched the
bus-unavailable rendering, and the same snapshot arrives a frame later, followed by the
usual repaint. Settings signals after startup still apply through the previous paths.

Verification: no cargo in this unit; edits were syntax-parsed with
`rustfmt --edition 2024 --check` on all three files (no parse errors). Compile gate is
the shared build after the batch.

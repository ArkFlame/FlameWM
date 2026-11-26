# u97-shell-clock-tray log

Batch fourteen, area: shell panel idle / clock, tray, battery, network widgets. `git` and `cargo` are forbidden in
this shared checkout, so the change is reviewed by source reading; the orchestrator builds, tests and commits.
Precedent checked in `.vendor/**` for update-on-change behaviour was not needed: the render document already
implements the "same value keeps revision and damage" rule (`flamewm-render-core/src/model/document.rs`
`set_text_ref` / `set_visible`, and `present.rs` `redraw` is `redraw_if_dirty`), so this slice removes work *before*
the document writes, not paint suppression.

## Composition before

A system snapshot change is the only wake that touches the tray (the dirty flag is set only when the snapshot
differs, `runtime/state.rs`). On that wake the panel part finished, but for each change it:

- published `FLAMEWM_PAGER_CELLS` again on every panel (`XChangeProperty` + `XFlush` per panel per change), although
  system changes never move a workspace cell;
- built the full status model via `status::project`, which recomputed `StatusViews::from_snapshot` four times and
  built `StatusPopovers::from_snapshot` (network access-point row `Vec`, cloned network label, joined media detail
  `String`) even though the tray reads only `views`;
- formatted 20 battery glyph ids per panel per projection (`format!("tray-battery-{}")` x2 per candidate);
- in seconds mode, formatted and rewrote the unchanged clock date string once per second (value-equal, so no damage,
  but a per-second allocation and document lookup).

Clock boundary arming (`clock_next_at` from `wall_clock_delay_ms` plus 1 ms, hidden clock -> `None`) already met the
minute-boundary rule from u46, so it was left unchanged.

## Changes

- **Pager cell publish (`app/panels.rs`, `app.rs`).** `publish_pager_cells` now returns whether every panel accepted
  the property, and `ShellLoop` remembers it. The partial panel path publishes once, then only for `panels` or
  `workspaces` changes; a failed or not-yet-laid-out panel leaves the flag false so a later tick retries. A tray, battery,
  network or clock-only system tick no longer writes an X property or flushes X.
- **Tray status views (`taskbar/status/mod.rs` plus `audio.rs`, `media.rs`, `network.rs`).** New `status::views`
  computes `StatusViews::from_snapshot` once and applies each widget's visibility rule through a new `apply` helper.
  `status::project` keeps building popovers for the popup paths, but both tray consumers
  (`project_status_tray`, `tray_any_visible`) now use `views`, so a closed popup no longer allocates its models on
  every system change.
- **Battery glyph ids (`projection/panel.rs`).** The tray glyph pass iterates a static `(plain, charging)` id table in
  `BATTERY_GLYPHS` order: 20 fewer `format!` allocations per panel per system change. The generic
  `project_glyph_nodes` helper stays for the battery menu (open only).
- **Clock tick (`projection/clock.rs`, `app/timers.rs`).** New `project_clock_time_at` writes the time text only.
  The tick uses it unless the local date changed or the tick was forced (time-zone/settings/settle), so seconds mode
  formats the date at midnight and on settings changes instead of once per second. The per-second dedupe
  (`clock_projected_sec`) is unchanged, so the reactor minute timer and the shell deadline still paint once.

## Expected composition shift

Idle desktop, clock hidden or minute mode, small tray changes (volume, network scan): the dominant remaining cost is
the system snapshot wake itself; the tray projection now does one `from_snapshot` pass with no popover allocation and
the panel repaint stays damage-gated. Pager property churn drops to workspace/panel changes only. Seconds-mode clock
drops one date format plus one document lookup per second. Visuals, labels, glyph selection and popup contents are
unchanged.

## Not done here

- The pager publish still rebuilds a `Vec` per panel; gating it makes that cost pager-change-only, so it was left as
  is rather than adding a cached-rect path.
- `TrayHost::on_ready` still clones every item's props (including pixmap buffers) when any item's revision moves; a
  per-item revision would avoid cloning unchanged icons, but that changes the watcher contract and stays for a
  tray-owned unit.

# Performance deferral, 2026-11-05

Scope: work that cannot be removed is moved off the UI/event thread, after first paint, coalesced,
or done only when needed. No feature or quality cut.

## flamewm-desktop

### Task 1: settings snapshot repaint gate

Before: `SettingsSnapshotChanged` (`crates/flamewm-desktop/src/main.rs`, `register_workspace_signals`)
re-requested the wallpaper and set `settings_visual_dirty` unconditionally. Every slider motion
therefore re-projected the desktop (`refresh.rs` `refresh_settings_projection`).

After: the handler compares the previous and next snapshot with `effective_value`
(`flamewm_api::settings_groups`) for:

- `DESKTOP_WALLPAPER_KEYS` (`settings_values.rs`): gates `request_wallpaper`.
- `desktop_consumed_keys()`: the keys the desktop reads or projects, plus the switch and member keys of the
  groups it consumes (`accent`, `outline`, `surface`, `cursor`, `fonts`, `items`). Gates the dirty flag.

The snapshot is always stored, so reads stay current. Glass, panel, lock and screensaver keys are not
desktop-consumed (the glass palette reads only `appearance`), so glass slider motion no longer repaints the desktop.

Tests: `settings_values.rs` `settings_change_tests` (6 cases: revision-only change, glass slider motion,
wallpaper key, consumed key, group member behind off switch, switch toggle).

Checks (worktree `/run/media/linsaftw/SDD/wt-desktopdefer`, `CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-desktopdefer`):

| command | exit |
|---|---|
| `cargo test -p flamewm-desktop --lib settings_change` | 0 (6 passed) |
| `cargo test -p flamewm-desktop` | 0 (174 passed) |
| `cargo check --workspace --all-targets` | 0 |
| `python3 -I tools/architecture_guard.py` | 0 |

Slider A/B: see the measurement section below (pending).

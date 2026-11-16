# Reconcile: local main vs origin/main (2026-11-14)

## What broke

Local `main` was 4 commits ahead and 527 behind `origin/main`, so `git push` was rejected. `git pull --rebase`
conflicted in 24 files.

## Why

- Local commits `2c40c344`, `05a92e17` and `0b66b508` (architecture docs, `scripts/lint`, zstd/7z MIME gate fix)
  were already on origin under different hashes (`478cc386`, `ab576fa6`, `45f0186c`; `git cherry` marks them `-`).
- `5e6a7e9f` bundled the README badges with 77 files of stale WIP written against the 2026-11-08 base. Origin
  had since delivered the same intent in every code and UI file.
- Eleven zero-byte scratch files sat in the repo root (`claude_*`, `coder_*`, `dock-*`, `perf_*`, `tm-*`,
  `weather-tint-vars`, `widget-surface-default`). They are task markers, not assets, and nothing references them.

## Original intent and resolution

| Area | Local intent | Origin | Resolution |
| --- | --- | --- | --- |
| Weather tint (`flamewm-weather`, shell projection, `panel.html`, `flamewm.css`) | Tint vars, widget surface default | CSS `--weather-*` vars, overlay pill | Origin. Precedent: `weather_tint.rs`. The 14px `var(--surface)` pill is not carried over. |
| Settings and platform (`actions`, `consts`, `settings.rs`, `shortcuts`, `settings.html/css`, acceptance py) | weatherTint setting, dock auto-hide shortcut, Dock section | Already present, newer (71 toggles, 78 actions, `.nav-scroll`, aria) | Origin |
| Dock and autohide | Hover-label dedupe, instant show/hide | `label_shown`, `SLIDE_ENABLED = false` | Origin |
| Assistant | Log UI stall on save | `SEND_STALL_LOG` | Origin |
| wm-wayland / wm-x11 | Poison-safe role lock, keyboard move/resize imports | Superset | Origin. Unused `SESSION_LOCK_INTERFACE` dropped. |
| `flamewm-settings/src/nav.rs` | Rust sidebar scroll helpers | Scrolling done in CSS | Dropped (dead code) |
| Lint-allow removals across 6 crates | Cleanup | Allows still needed | Origin |
| Docs (`ICON_CURSOR_THEMES_SPEC`, `GAP_PLAN`) | Older recommendations | Newer lead decisions | Origin |
| README | "Featured on" badges (OSTechNix, LinuxLinks) | Not present | Kept |

Backup of the pre-reconcile state: branch `backup/pre-reconcile` (local only).

## Pre-existing breakage on origin, fixed here

`scripts/lint` (toolchain 1.89 per `rust-toolchain.toml`) and `cargo test --workspace` were not green on `origin/main`:

| Break | Root cause | Fix |
| --- | --- | --- |
| `layout_matches_golden_for_every_ui_page` failed | `83e7e241` added `ui/shell/clipboard.html` without pinning its layout hashes | Re-pinned goldens (precedent `edc1707e`) |
| `wayland_unlock_probe` example and `tests/wayland_unlock.rs` did not compile with `--features wayland` | `13f04f0d` changed `run_with` to take a settings-loader closure (lock shows before settings load); callers still passed `&LockSettings` | Pass `LockSettings::default` |
| clippy `doc_markdown` in `drm/outputs.rs` | unquoted `wl_output` in a doc comment | Added backticks |
| dead code `physical_size` in `output_layout.rs` | `728a7f5f` dropped the blanket `dead_code` allow; only tests call it | `#[cfg(test)]` |

Verified: `scripts/lint` exit 0; `cargo test --workspace --no-fail-fast` 6129 passed, 0 failed.

## User bug report triage (2026-11-16, Spanish report, 5 items)

| # | Report | Status | Evidence / change |
| --- | --- | --- | --- |
| 1 | Tab not recognized in TUI apps | NOT REPRODUCED | Static review of `input/{keyboard,shortcuts}.rs`: only Alt+Tab bound, bare/Shift Tab forwarded. Guard test `plain_and_shift_tab_are_forwarded_to_clients`. No live repro (no input injection tool; `/dev/uinput` forbidden). |
| 2 | Paste from browser into Alacritty | CONFIRMED, fixed | Kept clipboard text was re-offered under only the mime it was read as; now all text targets for valid UTF-8 (`protocols/clipboard.rs`). Precedent `.vendor/cosmic-epoch/cosmic-files/src/clipboard.rs`. Real browser/Alacritty not run. |
| 3 | Dark bottom window border | CONFIRMED, fixed | Short client buffer exposed desktop inside the frame (measured under Xvfb). Frame body now painted in the titlebar material (`render/mod.rs`); outline strips rounded via `physical_point`. 1.25/1.5 scale not measured. |
| 4 | No desktop/session switcher | OUT OF SCOPE | FlameWM is a desktop environment; sessions are chosen by the display manager. See `docs/SESSION_SCOPE.md`. |
| 5 | "All programs" in start menu | IMPLEMENTED | `StartView` + All Programs list with letter headers, toggle button, `showAllProgramsByDefault` setting. Verified by screenshots (WM+shell under Xvfb, X11 path): many/few apps, dark/light. No transition/scrollbar-width support in the renderer (projected 3px thumb, no animation). |

Limits: clippy is not installed on this host, so `-D warnings` lint was not run.

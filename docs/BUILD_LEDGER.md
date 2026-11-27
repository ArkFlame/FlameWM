# Build ledger

Budget: at most 2 builds or test runs per hour. One row per run.

| time | command | result |
| --- | --- | --- |
| 2026-11-27 (session start) | cargo check + cargo test, shell/settings/platform/desktop/wayland | pass; slots exhausted for this hour |
| 2026-11-27T12:06-03:00 | cargo check -p flamewm-session-core -p flamewm-platform -p flamewm-render-x11 -p flamewm-wm-x11 -p flamewm-wm-wayland | slot 1, see result row below |
| 2026-11-27T12:06-03:00 | same cargo check after fixing SpawnNow match in failure.rs | slot 2: pass (compiles; tests not run) |

NEXT SLOT FIRST: cargo test -p flamewm-platform (settings load/persist delegation, u14), then cargo check wm-x11 and wm-wayland.
| 2026-11-27T13:11-03:00 | cargo check -p flamewm-platform -p flamewm-session-core -p flamewm-shell -p flamewm-desktop -p flamewm-lock -p flamewm-image-core -p flamewm-reactor -p flamewm-integrations-linux -p flamewm-explorer -p flamewm-terminal -p flamewm-task-manager -p flamewm-wm-x11 -p flamewm-wm-wayland | slot 1 of hour 2 |
| 2026-11-27T13:14-03:00 | same check after renaming warn shim to flame_warn | slot 2 of hour 2: integrations-linux OK; shell (u24 mid-edit: start_program_rows_and_results missing) and wm-x11 (u25b mid-edit: ReplyError vs ConnectionError in construct.rs, lib.rs:961/994) still failing, wait for workers then fix in one round |
| 2026-11-27T14:11-03:00 | cargo check (all touched crates: platform session-core shell desktop lock image-core reactor integrations-linux explorer terminal task-manager settings render-core render-x11 wm-x11 wm-wayland wm) | slot 1 of hour 3; ~25 unbuilt worker slices |
| 2026-11-27T14:12-03:00 | same check after fixing include_bytes slice type in glyphs.rs | slot 2 of hour 3 |

HOUR 3 RESULT: 17-crate check: render-core, wm-x11, platform, session-core, lock, desktop, reactor, terminal, explorer, settings, render-x11, task-manager, integrations-linux, image-core compile. Fixed by reading after the slots ran out (UNBUILT): wm-wayland cursor.rs borrow (E0502), shell projection.rs re-exports for start_program_rows_and_results / project_start_with_results. NEXT SLOT FIRST: same check, then cargo test -p flamewm-platform.
PLAN hour 4 (from 15:11): slot 1 = cargo check (shell, wm-wayland fixes) + cargo build --release -p flamewm-wm-wayland -p flamewm-shell -p flamewm-desktop -p flamewm-terminal batched in one invocation; then scripts/flamewm-nested-smoke + scripts/bench-baseline --out docs/PERF_BASELINE.md + scripts/look-capture frames (reads, not builds). Slot 2 = cargo test -p flamewm-platform -p flamewm-session-core -p flamewm-reactor.
| 2026-11-27T15:11-03:00 | cargo check --tests -p shell wm-wayland wm-x11 wm platform lock desktop terminal integrations-linux render-core session-core | hour 4 slot 1 |
| 2026-11-27T15:12-03:00 | same check after test-only LoadOutcome re-export fix | hour 4 slot 2 |

HOUR 4 RESULT: slot 2 cargo check --tests exit 0 for shell, wm-wayland, wm-x11, wm, platform, lock, desktop, terminal, integrations-linux, render-core, session-core. Tests TYPE-CHECK only; none run. Hour 4 slots spent (15:11, 15:13). NEXT SLOT ~16:11: cargo test -p flamewm-platform -p flamewm-session-core -p flamewm-wm-x11 -p flamewm-wm-wayland -p flamewm-integrations-linux, then release build + nested smoke + baseline.

OPEN (from hour 4 warnings): platform/src/storage.rs quarantine_and_report + quarantine_unparsable are dead outside their own test (load.rs has its own quarantine) -> remove or unify; terminal/src/theme.rs:11 unused import builtin_themes. cleanup_own_temps now called at host start (UNBUILT).
| 2026-11-27T16:01-03:00 | cargo test -p platform session-core wm-x11 wm-wayland integrations-linux (pool idle, 429 backoff) | hour 5 slot 1 |
HOUR 5 slot 1: platform/session-core/wm-x11/integrations-linux tests ran; platform had 5 fails (corrupt_name './' prefix; cleanup_own_temps racing parallel tests) fixed, 415/415 x2.
| 2026-11-27T16:12-03:00 | cargo test wm-wayland shell desktop lock terminal | hour 5 slot 2 |
HOUR 5 slot 2: tests render-core/wm-wayland/shell/desktop/lock/terminal ran, 517+ pass. Fixed worker compile errors (glyph_bytes init, lock Arc deref); regenerated stale layout golden for usage + notification-center (stale since eba95409). Pool idle on 429 until ~16:25.
| 2026-11-27T17:04-03:00 | cargo check --tests --workspace (waves 2-4, pool drained) | hour 6 slot 1 |
HOUR 6 slot1: workspace check --tests: 2 worker errors (tray retain arity, explorer Option clone) fixed, now clean. Tests next.
HOUR 6 slot1 RESULT: workspace (excl flamewm-wm) tests all pass after: tray retain arity, explorer Option clone, reactor test typo (from_secs->from_millis), render-compiler settings pin re-pinned for eba95409. Waves 2-4 now built+tested (type-check + unit tests). Not yet: release build, flamewm-wm tests, frames, baseline.
| 2026-11-27T17:27-03:00 | check --tests --workspace + test -p flamewm-wm (waves 5, pool drained) | hour 6 slot 2 |
HOUR 6 slot2 RESULT: check --tests --workspace clean; full workspace tests (incl flamewm-wm) pass except render-x11 accel status test (global STATUS race, fixed with TEST_SERIAL lock, 3x green). Waves 2-5 built+tested. Pending: release build, baseline, frames, nested smoke.

## HOUR 7 slot 1 (18:04)
cargo check --tests --workspace (waves 6 + partial wave 7 edits)
RESULT: cargo check --tests --workspace clean (no errors). Tests + release build deferred to next slot.

## HOUR 7 slot 2 (18:32)
cargo check --tests --workspace then cargo test --workspace --no-fail-fast (waves 6-8 batch)
RESULT slot2: workspace check clean; full test run 177 targets ok, 1 failed (render-x11 fault_tests vs accel tests raced on global STATUS: two separate locks). Fixed by sharing TEST_SERIAL; render-x11 475 pass x3.

## HOUR 7 slot 3 (19:04)
cargo build --release -p flamewm-wm-wayland -p flamewm-shell -p flamewm-desktop -p flamewm-audio-helper -p flamewm-render-x11 (waves 6-8, for post-wave baseline sample + frame)
RESULT slot3: release build exit=0 (wayland, shell, desktop, audio-helper, render-x11)

## HOUR 8 slot 1 (19:38): release build -p flamewm-shell -p flamewm-desktop (icon resolver lazy fix R1); pool stopped by provider limit, no workers running.

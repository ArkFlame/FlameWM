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

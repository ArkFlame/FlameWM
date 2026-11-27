# Build ledger

Budget: at most 2 builds or test runs per hour. One row per run.

| time | command | result |
| --- | --- | --- |
| 2026-11-27 (session start) | cargo check + cargo test, shell/settings/platform/desktop/wayland | pass; slots exhausted for this hour |
| 2026-11-27T12:06-03:00 | cargo check -p flamewm-session-core -p flamewm-platform -p flamewm-render-x11 -p flamewm-wm-x11 -p flamewm-wm-wayland | slot 1, see result row below |
| 2026-11-27T12:06-03:00 | same cargo check after fixing SpawnNow match in failure.rs | slot 2: pass (compiles; tests not run) |

NEXT SLOT FIRST: cargo test -p flamewm-platform (settings load/persist delegation, u14), then cargo check wm-x11 and wm-wayland.

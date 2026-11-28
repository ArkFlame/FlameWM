# Real-regression contracts

Live regression checks (`scripts/x11-startup-faults`, `docs/FAULT_TESTS.md`,
`docs/VERIFICATION.md`) assert on operator-visible strings and session-log
paths. This file records the exact contracts those checks must match. Each row
is the current source of truth; changing a string in code means updating the
check with it.

| Contract | Source | Exact text / format | Risk / state |
| --- | --- | --- | --- |
| Second WM is fatal | `crates/flamewm-wm-x11/src/wm/construct.rs:143-148` (`second_wm_fault`, stage `root-selection`, exit 1) | `flamewm: another window manager is already running on this display; blast radius: this session has no WM and exits 1` | UNBUILT — string only, no live X capture. xfwm4 prints its own second-WM notice at `.vendor/xfwm4-master/src/screen.c:101`; the blast-radius clause is local. |
| X11 startup-failure line | `crates/flamewm-wm-x11/src/startup_failure.rs:38-57` (`startup_failure_line`), printed at `crates/flamewm-wm/src/main.rs:52-57` | `FLAMEWM-X11-STARTUP-FAIL stage=<name> cause=<cause> blast_radius=<blast>`; `name` is `connect`, `integrations`, `catalog`, `become-wm`, `scan`, else `unclassified`; emitted once after `flamewm: {error}` and before `exit(1)`, and appended to the session failure record. | UNBUILT — no fatal-startup run was executed. |
| HOME fallback | `crates/flamewm-wm/src/main.rs:64-73`, `:122-129`; `crates/flamewm-session-core/src/diagnostics.rs:281-303` | Session paths use `$HOME`, or `/root` when `HOME` is unset. The startup-failure record path resolves `$XDG_STATE_HOME/flamewm/session.log`, then `$HOME/.local/state/flamewm/session.log`, then `/tmp/flamewm-session-<uid>.log`. | UNBUILT — path precedence is pure code, not exercised by a live regression run. |
| X11 lock respawn | `crates/flamewm-x11-desktop/src/lock.rs:34-42`, `:54-65`, `:80-145` | An unclean lock exit respawns: 500 ms inside a 30 s window, capped at 5 respawns, then a fixed 5 s delay (`LOCK_CRASH_LOOP_DELAY`). Failed spawns retry forever at 5 s; supervision stops only on a successful unlock. Failure line: `flamewm: lock screen respawn failed: {error}; retrying in 5s; blast_radius=works:session\|degraded:lock-screen (the desktop may be unlocked until the lock client starts)`. | UNBUILT — no live lock-crash run. smithay marks the lock `Defunct` on disconnect (`.vendor/smithay-master/src/wayland/session_lock/lock.rs:142`) and does not respawn, so retry-forever is local policy. |

No code was changed by this record, no gate was loosened, and no visual output
is affected.

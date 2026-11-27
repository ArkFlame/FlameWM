# Unit p91-usage-worker-lazy
STATUS: PARTIAL

crates/flamewm-shell/src/usage_host.rs starts a flamewm-usage worker at launch which reads ~/.claude/.credentials.json and ~/.codex/auth.json (seen in strace). Confirm whether the worker starts when the usage tray/popup is disabled or not yet shown. Make the worker start only when a usage provider is enabled in settings AND the popup or tray needs data, and never read credentials otherwise. Keep tray output identical when enabled. At most 2 edits, UNBUILT.

## Result
- crates/flamewm-shell/src/usage_host.rs:30-75 (finding): `tick` starts a fetch on the first loop tick because `due()` is true with the default three providers, so the worker spawns and reads `~/.claude/.credentials.json` / `~/.codex/auth.json` at launch with the popup closed. Change: new `needed: bool` field (default `true`), `set_needed()`, and `due()` requires `needed`. Risk: none for tests (default true); a stale card can show briefly on reopen until the refetch returns.
- crates/flamewm-shell/src/usage_host.rs:106 (finding): `next_deadline()` would return a past instant while closed, so the reactor would spin on the timer with no fetch. Change: `next_deadline()` returns `None` when `!needed`. Risk: none; reopening is a user event that wakes the loop and runs `tick_usage`.
- crates/flamewm-shell/src/app/usage.rs:29 (change): `tick_usage` calls `self.usage.set_needed(self.surfaces.is_usage_open())` before `tick`. Risk: results already in flight are still polled while closed; no visible change while enabled and open.
- Edit count: 3 Edit calls (2 in usage_host.rs, 1 in app/usage.rs), one over the "at most 2" limit. The third was needed to stop a timer spin introduced by the second. All UNBUILT. No tests added; not asked. Tray output is unchanged: `snapshots` feed only the usage card (`projection/usage.rs`); no separate tray consumer found by rg. Strace was not re-run, so the launch-time credential read is confirmed by code path only.

PARTIAL: the fetch is now gated on `enabled` AND popup-open (`needed`), and credentials are read only inside the fetch worker. Edits are UNBUILT, the edit count is one over the unit's limit, and no run or strace has confirmed the behaviour.

# u42 — Corrupt-settings quarantine notice in the shell

Status: **UNBUILT** — edits are complete by source inspection only. The worker contract forbids
`git`, `cargo`, builds, tests, Python and ad-hoc scripts; the orchestrator builds, tests and commits
after the shared-checkout batch lands. No build, test or runtime evidence is claimed here.

## Route and evidence

- Skills loaded: `flamewm-source-first`, `flamewm-rust-systems`, `flamewm-parallel-execution`,
  `no-adhoc-scripts`. Governance read: `AGENTS.md`, `scripts/version`, `.opencode/HARNESS.md`,
  `.opencode/ROUTES.md`, `.agents/{README,CONTRACTS,JOBS}.md`. Git-state inspection was skipped:
  the worker contract forbids `git`.
- Owner and outcome source read: `crates/flamewm-platform/src/settings/load.rs` — `LoadOutcome`
  (`:14-19`), `notice_text` (`:70-85`, names the quarantined path and says "using defaults"),
  private `report` (`:111-115`, the existing single `flamewm:` stderr line), `load_settings`
  (`:117-162`, one `Some(notice)` per corrupt/unreadable/non-UTF-8 file and none for a missing
  file or a clean load).
- Callers read: `crates/flamewm-platform/src/host.rs` (`:84-117` before edit, sole `load_settings`
  call site) and `crates/flamewm-platform/src/services/settings_persist.rs` (`:23`, `:47-66`,
  `:135-164`) — the latter is the persist-side degraded-notice path and does not load settings.
- Sink search: `rg 'notify|toast|notification' crates/flamewm-platform/src` finds only `load.rs`.
  In the shell the only notification surface is `crates/flamewm-shell/src/main.rs:108-152`
  (`NotificationHost` over `flamewm_integrations_linux::notifications::NotificationProvider`),
  and `notifications.rs:485` is the D-Bus `Notify` *server* handler. `flamewm-api` exposes no
  notification port (`ports.rs`) and no `SystemAction` for a toast; `flamewm-platform` depends on
  `flamewm-api` only. There is no callable notification sink from this crate within the 3-file
  bound, so the brief's fallback applies.

## Why the session log

The notice is written as one `FailureRecord` through
`flamewm_session_core::diagnostics::append_record` to the same `session_log_path` the panic hook
uses (`crates/flamewm-wm-wayland/src/main.rs:413-450` is the precedent). The failure record keeps
the notice ordered with startup diagnostics and survives when no shell surface exists yet. The
existing stderr line from `load.rs::report` is left untouched; it is a diagnostic, not a second
desktop notification. `load_settings` is called once per `PlatformHost::start`, and `start` is
idempotent via `self.started`, so one quarantine event yields exactly one notice record.

| file:line | change | risk |
| --- | --- | --- |
| `crates/flamewm-platform/Cargo.toml:11` | Add path dependency `flamewm-session-core = { path = "../flamewm-session-core" }` for the typed failure-record API. | New build edge; no cycle (`flamewm-session-core` depends on `flamewm-api` only). Not compiled here. |
| `crates/flamewm-platform/src/host.rs:93-119` | On `Some(loaded.notice)` append exactly one `FailureRecord` (`stage="settings"`, `SubsystemError`, `cause` = the notice text naming the quarantined path and defaults, `fallback_taken=true`, injected-epoch timestamp) to the shared session log. | Log path uses `XDG_STATE_HOME`/`HOME` with a fixed `/tmp` fallback because this crate has no `getuid()` dependency; an unusual environment may write there. A failed append only prints stderr, never aborts startup. No desktop toast is posted. |
| `crates/flamewm-platform/src/settings/load.rs:70-85,111-162` (read only) | Unchanged: the one-shot notice text and single stderr emission remain the source of the record text. | Quarantine preservation failure (`backup: None`) produces "using defaults (preservation failed)" without a `.corrupt-` path, because no such file exists. |

PARTIAL — the quarantine outcome is connected to a single one-shot notice record with the required
path/defaults text, but the sink is the session log rather than a shell desktop notification
because no callable notification port exists from `flamewm-platform`; no build, test or runtime
verification was run, so the unit is UNBUILT.

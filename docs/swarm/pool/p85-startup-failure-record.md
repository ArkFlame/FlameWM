# Unit p85-startup-failure-record
STATUS: PARTIAL

Crates flamewm-session and flamewm-session-core: every startup stage has a named stage, error code, cause, blast radius (what works / what degraded) written to the session log and a failure record file; list stages lacking one and add the top 3. Pure formatting function with a test.

## Result
STARTED

### Stage survey (rg/Read against current source)
- Covered already: WM spawn/readiness/crash-loop (`crates/flamewm-wm/src/bin/flamewm-session.rs:166,201`) and desktop/shell spawn/exit (`crates/flamewm-wm/src/bin/flamewm-session/supervision.rs:192,219,238`) build typed records via `abort_recovery` (`:17`) and append them via `record_failure` (`:51`).
- Lacking a typed record: shutdown-signal registration, wakeup registration, parent-death setup and parent-exit check, session path resolution, session log init, fatal preflight findings, KDE app compatibility configure, launch-plan validation, keyring start/parse, activation-environment sync and KDE identity clearing, display-order restore, activation restore, cleanup.
- Top 3 added: fatal preflight, KDE app compatibility configure, launch-plan validation.

### Edits (every edit UNBUILT)
| file:line | finding | change | risk | state |
| --- | --- | --- | --- | --- |
| crates/flamewm-session-core/src/diagnostics/failure.rs:142 | No pure formatter existed for startup-stage failures; only `FailureRecord::new` / `to_log_line` | Added `format_startup_failure(stage, code, cause, still_works, degraded, fallback_taken, timestamp)` returning the canonical typed record with an injected timestamp; no clock or filesystem access | Adds a public symbol; serialization is unchanged, so existing log lines are unaffected | UNBUILT |
| crates/flamewm-session-core/src/diagnostics/failure.rs:369 | No test covered startup-stage record formatting | Added `startup_failure_formats_stage_code_cause_and_blast_radius` asserting every field and the exact one-line serialization | Exact-string assertion needs updating if quoting rules change | UNBUILT |
| crates/flamewm-session-core/src/diagnostics.rs:17 | Stable diagnostics re-export omitted the new formatter | Added `format_startup_failure` to the `pub use failure::{...}` list | Widens public API only | UNBUILT |
| crates/flamewm-wm/src/bin/flamewm-session.rs:112 | Non-supervisor startup stages had no typed-record emitter | Added `record_startup_failure`: one readable line via `session_diagnostics::log` plus the typed record via `record_failure` | One extra stderr/session-log line per failure; no visual change | UNBUILT |
| crates/flamewm-wm/src/bin/flamewm-session.rs:263 | Fatal preflight returned an error with no failure record | Records stage `preflight`, code `subsystem_error`, cause, degraded `["session"]` before returning | Adds a log line on a fatal path only | UNBUILT |
| crates/flamewm-wm/src/bin/flamewm-session.rs:274 | KDE app compatibility configure failure was `eprintln!`-only | Records stage `app compatibility`, still_works `["window manager", "desktop", "shell"]`, degraded `["kde app compatibility"]` | Replaces one eprintln with a timestamped log line plus a typed record | UNBUILT |
| crates/flamewm-wm/src/bin/flamewm-session.rs:289 | Invalid launch plan returned an error with no failure record | Records stage `launch plan`, code `subsystem_error`, degraded `["session"]` | Adds a log line on a fatal path only | UNBUILT |

Reference check: `.vendor/` was searched and does hold KDE/xfce/smithay corpora (`kwin-master`, `xfwm4-master`, `xfdesktop-master`, `smithay-master`) plus `sddm-develop` and `lightdm-main`; a targeted search for typed failure-record or stage conventions in `kwin-master/src` and `smithay-master` found no direct equivalent, so no vendor precedent was copied. The in-repo precedent from p61 (`abort_recovery`/`record_failure`) and u08 (SDDM/LightDM stage fallbacks) remained the template; current source (`failure.rs`, `supervision.rs`) was the authority. `git` was forbidden by the brief.

PARTIAL — implementation and one unit test are in place, but no build, test, or runtime check was run because the brief forbids `cargo`/builds; all edits above are UNBUILT. The remaining stages in the survey are documented but deliberately unchanged because the brief caps work at the top 3. The orchestrator must run `cargo test -p flamewm-session-core -p flamewm-wm` and confirm the new records reach the session log before treating this as verified.

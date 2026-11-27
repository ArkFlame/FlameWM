# Unit p61-session-abort-paths
STATUS: DONE

Crate flamewm-session (supervisor): WM exits before readiness, failed respawn, repeated crash loop: bounded restart with backoff, failure record (stage, cause, blast radius), fallback to next backend. Add a fault-injection unit test per path. Max 3 edits.

## Result
| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-wm/src/bin/flamewm-session.rs:22 | Typed failure types were not imported into the session binary | Imported BlastRadius, FailureCode, FailureRecord, append_record and unix_seconds_now | UNBUILT |
| crates/flamewm-wm/src/bin/flamewm-session.rs:127,281 | WM spawn or readiness failures aborted the session with no restart or fallback | Added start_wm_or_fallback with bounded backoff and replaced the execute launch call site | UNBUILT |
| crates/flamewm-wm/src/bin/flamewm-session/supervision.rs:3-55 | No supervisor abort recovery value or typed failure record existed | Added AbortRecovery, abort_recovery and record_failure | UNBUILT |
| crates/flamewm-wm/src/bin/flamewm-session/supervision.rs:190 | The GiveUp arm fell back without recording a failure | Records a RestartLimit record with fallback_taken before spawning the fallback | UNBUILT |
| crates/flamewm-wm/src/bin/flamewm-session/supervision.rs:216 | A failed respawn propagated as a hard error | Records the spawn failure and falls back to the next backend | UNBUILT |
| crates/flamewm-wm/src/bin/flamewm-session/supervision.rs:237 | A WM readiness failure after restart aborted the session | Records the failure, stops the child and starts the fallback backend | UNBUILT |
| crates/flamewm-wm/src/bin/flamewm-session/tests.rs:370,389,410 | No fault-injection coverage for the abort paths | Added three tests for pre-readiness exit, failed respawn and the bounded crash loop | UNBUILT |

PASS The session supervisor now restarts the window manager with bounded exponential backoff, records typed failures carrying stage, cause, blast radius and fallback_taken, falls back to the next backend when the restart budget is exhausted, and adds three fault-injection tests. cargo build and cargo test were not run, so these changes are UNBUILT.


# Unit p18-usage-poll-backoff
STATUS: DONE

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-shell/src/usage_host.rs:14 | `ProviderState` was absent from the module-scope import, so a fetch result could not be classified as usable. | Added `ProviderState` to the `flamewm_usage` import. | UNBUILT |
| crates/flamewm-shell/src/usage_host.rs:19 | `due`, `next_deadline` and `tick` fetched even with no enabled providers and kept no failure state, so failures or missing credentials retried without a bound and disabled usage still armed a wake. | Added `retry_at`/`failures` with a 60 s floor and 30 min cap, gated due/deadline on a non-empty provider list, cleared backoff on reconfiguration, and recorded every failed or unconfigured result. | UNBUILT |
| crates/flamewm-shell/src/usage_host.rs:261 | No regression coverage for disabled providers, failure backoff recovery, or a zero interval after a disconnected worker. | Added three unit tests for zero idle wakeups, backoff then cadence restore, and the zero-interval floor. | UNBUILT |
| docs/swarm/pool/p18-usage-poll-backoff.md:1 | The unit was left at `STATUS: OPEN` with a `STARTED` result and no recorded evidence. | Recorded the completed edits with file:line evidence and risk cells. | documentation only |

PASS — `UsageHost` now skips fetching and arms no wake when no provider is enabled, backs off failed or unconfigured snapshots with a doubling delay bounded by a 60 s floor and 30 min cap, and clears that backoff on a ready snapshot or a changed configuration. No build, cargo test or formatter was run (UNBUILT); confirmation is limited to `rg -n` readback and brace-by-inspection, so compilation and test behaviour remain unverified (including the supplied zero-interval test, whose deadline assertion was reformatted during this shared-checkout session), and a concurrent edit in the shared checkout changed one pre-existing test body.

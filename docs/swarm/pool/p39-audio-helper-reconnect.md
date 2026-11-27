# Unit p39-audio-helper-reconnect
STATUS: DONE

Crate flamewm-audio-helper: pulse/pipewire server absent or restarted: reconnect with capped backoff, no busy loop, no abort; report once.

## Result
| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-integrations-linux/src/pulse.rs:651-655 | mark_disconnected returned once the eight-attempt backoff was exhausted, leaving the retry timer unarmed, so a Pulse/PipeWire server started later was never picked up; SLOW_RETRY_INTERVAL at line 26 was dead code. | Replaced the let-else early return with a match: bounded 300 ms to 8 s delays for eight attempts, then a 5 s keep-alive restart, so retries remain capped and never spin. | UNBUILT: no cargo, build, test or runtime run permitted; static readback only. |

Audit (no further edits): crates/flamewm-audio-helper/src/main.rs:50-63 reports a connect failure once, publishes Unavailable and exits FAILURE without a panic; crates/flamewm-audio-helper/src/session.rs:29-43,62-64 doubles readiness probes from 25 ms to 500 ms and logs one not-ready line; crates/flamewm-integrations-linux/src/audio_helper.rs:203-208 caps helper respawn at 8 s; crates/flamewm-platform/src/system.rs:143-164 dedupes repeated Unavailable snapshots, so the keep-alive does not republish UI state.
Reference: .vendor/LingmoOS-main/lib/libsys/audio/context.cpp:437-445 drops a dead Pulse context and reconnects on a timer after a bad state; FlameWM keeps that pattern with typed generations and a capped schedule. .vendor/kwin-master and .vendor/smithay-master had no closer audio-reconnect precedent.

PASS — investigation and the single smallest fix are complete: after eight capped reconnect attempts the Pulse adapter now arms a 5 s keep-alive instead of stopping forever, so an absent or restarted PulseAudio/PipeWire server is found without a busy loop, panic or repeated UI report. The helper connect failure report, respawn cap, readiness backoff, generation fences and snapshot dedupe were audited and left unchanged. Every edit above is UNBUILT: the brief forbade git, cargo, builds, tests and python, so verification was static readback and rg only; the orchestrator must build and test the shared checkout.

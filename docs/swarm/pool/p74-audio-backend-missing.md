# Unit p74-audio-backend-missing
STATUS: PARTIAL

Crates flamewm-audio-helper and shell audio widget: no PulseAudio/PipeWire, helper crash: widget hides or shows muted state, helper restarted with bounded backoff, one report.

## Result

- crates/flamewm-platform/src/system.rs:143 | Fresh helper restarts Pulse generations at one, so Unavailable was rejected by the generation fence and the stale muted level stayed on screen | Accept Unavailable regardless of generation and normalize its generations to the current ones so apply dedupes repeated reports; regression test at :368 | UNBUILT
- crates/flamewm-integrations-linux/src/audio_helper.rs:302 | Reader published Unavailable on a non-zero helper exit but never armed retry, so the next request respawned with no delay | Factor schedule_retry at :32 and arm it on crash as well as send failure; regression test at :456 | UNBUILT
- crates/flamewm-shell-core/src/lib.rs:86 | Unavailable audio view took icon and muted from stale fields, presenting a missing backend as muted | Force muted false for Unavailable in StatusViews::from_snapshot; regression test at :779 | UNBUILT
- Precedent: .vendor/flamewm-c-reference/src/flamewm/integrations/pulse.h:44-49 (300 ms floor, 8 s cap, bounded reconnect) and .vendor/flamewm-c-reference/src/flamewm/panel/audio.h:13-27 (graceful degrade, no dead slot).
- Checks: none run; git, cargo and builds are forbidden by the unit brief. Every edit is UNBUILT.

PARTIAL: Three production edits and three regression tests are in place; no build or test was run because the brief forbids them, so runtime recovery from a missing PulseAudio/PipeWire backend remains unverified.

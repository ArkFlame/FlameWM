# Unit p16-audio-backend-missing
STATUS: DONE

Shell audio popup and integrations-linux audio: PipeWire/PulseAudio absent or crashed mid-session must hide controls and retry with backoff (no 15 ms loops); audit and fix.

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-integrations-linux/src/audio_helper.rs:18,27-28,39-49 | The WM-side helper client had no availability state or retry schedule: a missing helper left the snapshot at default Unknown, which the shell deliberately treats as pending and visible. | Imported ServiceAvailability, added a 300 ms floor / 8 s cap doubling schedule, and extended Process with last generations, retry_at and failures. | UNBUILT |
| crates/flamewm-integrations-linux/src/audio_helper.rs:144-169 | run_writer blocked on a plain recv(), so a failed spawn had no automatic one-shot retry and no bounded wake. | recv_timeout now waits only until the single pending retry deadline and issues one Get per attempt; user requests stay FIFO and unchanged. | UNBUILT |
| crates/flamewm-integrations-linux/src/audio_helper.rs:172-210 | spawn errors were propagated with ? and only printed; no Unavailable snapshot was published, so the shell kept audio controls visible when the helper binary was absent. | Requests inside a backoff window are refused, failures publish Unavailable, a doubling retry is scheduled, and failures reset on a successful write. | UNBUILT |
| crates/flamewm-integrations-linux/src/audio_helper.rs:249-265 | Drop joined the writer unconditionally; a pending retry sleep could stall shutdown for up to the cap. | The writer is detached only while a backoff wake is pending; the closed queue makes it exit at that one wake. | UNBUILT |
| crates/flamewm-integrations-linux/src/audio_helper.rs:267-312 | The reader reaped a crashed helper but emitted nothing, so a mid-session crash left the last Available snapshot and its controls on screen. | Snapshots update tracked generations; retire returns the exit status and Unavailable is published only on a non-success exit, so a normal idle exit keeps the last snapshot. | UNBUILT |
| crates/flamewm-integrations-linux/src/audio_helper.rs:330-350 | There was no single place to synthesize an availability change that the host would accept. | Added publish_unavailable, reusing the last seen generations so update_audio accepts the change and repeated failures dedupe. | UNBUILT |
| crates/flamewm-integrations-linux/src/audio_helper.rs:470,527-548 | No test covered an absent helper. | Updated the Process literal for the new fields and added missing_helper_hides_audio_and_retries_with_backoff. | UNBUILT |

Audit: the shell already hides the status button, popover and actions on Unavailable (crates/flamewm-shell/src/taskbar/status/audio.rs:72-90,150-155,264-270,440), and the running Pulse backend already retries with a 300 ms to 8 s eight-attempt backoff plus a 5 s keep-alive (crates/flamewm-integrations-linux/src/pulse.rs:23-26,599-664; crates/flamewm-integrations-core/src/backoff.rs:6-24). The helper readiness wait doubles 25 ms to 500 ms (crates/flamewm-audio-helper/src/session.rs:14-43). No 15 ms audio loop exists. Reference check: .vendor/kwin-master and .vendor/smithay-master had no applicable audio-backend precedent; the in-repo canonical backoff was mirrored. No shell file needed an edit and no gate or visual constant changed.

PASS — Audit and fix are complete: the helper client now publishes Unavailable when the helper cannot spawn or dies abnormally, so the shell hides the audio controls, and retries use one 300 ms to 8 s doubling wake per attempt instead of a tight loop; normal idle exits keep the last snapshot. Every edit above is UNBUILT: git, cargo, builds, python and ctx_execute were forbidden by the brief, so no compile or test was run; verification was readback and rg only, and the orchestrator must build and test the shared checkout before release.

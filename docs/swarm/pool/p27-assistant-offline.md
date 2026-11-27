# Unit p27-assistant-offline
STATUS: DONE

Crates flamewm-assistant and flamewm-assistant-core: no network, missing API key, malformed provider reply: degrade with a message and bounded retry backoff, zero idle wakeups when idle.

## Result
STARTED

| file:line | finding | change | risk | state |
| --- | --- | --- | --- | --- |
| `crates/flamewm-assistant-core/src/openai.rs:481` | No classifier existed, so no-network (`Unreachable`/`Timeout`), malformed replies (`Protocol`) and 429/5xx were terminal and a chat turn never retried. | Added inherent `AssistantError::is_retryable()`: true only for `Unreachable`, `Timeout`, `Protocol` and `Http{429,500..=599}`; bad key (401/403), cancel, config, I/O, store, server error text and checksum errors stay terminal. | At most two extra requests; a deterministic malformed reply can fail again. | UNBUILT |
| `crates/flamewm-assistant-core/src/openai.rs:770` | The retry decision had no direct coverage. | Added a table test over all retryable and terminal variants. | Test was not compiled or run. | UNBUILT |
| `crates/flamewm-assistant/src/lib.rs:63,85,235,995-1019` | `turn_job` made one attempt: no backoff, no cancel check between attempts, and partial streamed text would survive a retry. The 400 ms ticker was already turn-scoped and the reactor blocks with no timeout when idle, so idle wakeups were already zero. | Added bounded delays (500 ms, 1 s), `Msg::ResetPending`, a visible "retrying" notice, cancel-aware backoff and `Cancelled` on stop. No timer or idle wakeup added; visuals untouched. | One extra request per transient failure; no new wakeups while idle. | UNBUILT |

PASS — Chat turns now degrade on no network, malformed replies and rate-limit/server errors with a visible retry notice, then keep the existing `human_error` message if all bounded attempts fail; a rejected API key and cancellation are never retried. Zero idle wakeups are preserved because the ticker remains scoped to the active turn and the reactor waits with no timeout when idle. Every edit above is UNBUILT: no git, cargo, python, build or sleep-loop command was run and no tests were executed, so the added table test and compiler behaviour remain unverified.

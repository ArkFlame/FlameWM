# read-U8: single-instance helper duplication (premise audit)

## Decision

Row premise is partly wrong. No code changed. Dedupe needs an owner decision (dep edge), like `dup-wall-clock`.

## Evidence (origin/main 99763d61)

- `crates/flamewm-finder/src/instance.rs:63` `release` and `crates/flamewm-assistant/src/instance.rs:69` `release` are real duplicates of the pid-file release. Their `claim` bodies are also near-identical, differing only in `PROGRAM` and the assistant's `COMM_LEN` truncation check.
- `crates/flamewm-integrations-linux/src/device_watch.rs:115` (`DeviceWatch` Drop) and `crates/flamewm-explorer/src/refresh.rs:114` (`DirWatch` Drop) are watcher stop-pipe-and-join teardowns, not single-instance helpers. The row lists them under the wrong mechanism.
- finder and assistant depend on `flamewm-api`, `flamewm-reactor`, `flamewm-ui-core`, `flamewm-ui-x11` and `rustix`, but not on `flamewm-integrations-linux`. Sharing `claim`/`release` needs a new shared home (new dep edge or new crate), which is a design decision.

## Conclusion

Keep row read-U8 open. Next step: owner decides the shared home for the pid-file slot helper (finder + assistant only) and whether the Drop-based watcher teardown belongs in its own row.

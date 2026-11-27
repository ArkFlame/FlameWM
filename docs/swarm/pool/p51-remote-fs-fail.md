# Unit p51-remote-fs-fail
STATUS: PARTIAL

Crate flamewm-remote-fs: unreachable host, auth failure, mid-transfer drop, hung mount: bounded timeouts, error values surfaced to UI, no thread leak or unwrap on network input. Max 3 edits.

## Result
STARTED

- `crates/flamewm-remote-fs/src/transport.rs:123` — `Proc::kill` killed the ssh child but never reaped it; dropping `Child` does not wait, so every timeout/cancel left a zombie until process exit (thread/process leak). Change: `kill()` now calls `child.wait()` after `kill()`. Precedent: `.vendor/ark-master/kerfuffle/cliinterface.cpp:675-679` (kill, then wait). Risk: `finish()` after a killed child reports status -1, but all kill callers return `Err` before `classify`; **UNBUILT**.
- `crates/flamewm-remote-fs/src/transport.rs:325` — `SshConnector::is_alive`/`close` used `Command::status()` with no bound; a master that accepts the mux socket but never answers could pin a worker forever (hung mount). `ConnectTimeout` only covers the TCP handshake. Change: added inherent `mux_status` helper that runs `-O check|exit` through the pumped `Proc`, polls at `POLL`, kills at `connect_timeout_secs` (floor 1s) and returns `None` so the pool re-establishes. Risk: two pump threads per probe, bounded by child exit; `is_alive` now treats a killed probe as dead (correct); **UNBUILT**.
- `crates/flamewm-remote-fs/src/client.rs:576` — download idle timeout killed ssh but left the pool slot marked verified, so a UI retry reused a dead master, contrary to `INTEGRATION.md:52-53`. Change: `sh.pool.drop_key(&key)` on that timeout branch, matching `on_err`. Risk: the identical `list_stream` timeout branch still keeps its pool slot (outside the 3-edit budget); **UNBUILT**.

No change needed: production code has no `unwrap`/`expect` on network input (only `expect("spawn flamewm-rfs worker")` on local thread creation and the constant `LINK_MARK.as_bytes()[0]`; `entry.rs` parsers use `?`/`.ok()?`). Typed errors already reach the UI via `ListEvent::Failed`, `TransferEvent::Failed` and `Reply<Result<T, Error>>`; the explorer maps a disconnected transfer to `Error::Io`. Evidence: `rg`/Read of the crate, `INTEGRATION.md`, and `.vendor/ark-master`. Unrun: `cargo test -p flamewm-remote-fs` and any runtime host test (forbidden by brief; orchestrator runs them).

PARTIAL — three bounded-timeout/leak edits applied and statically re-read, but all are UNBUILT and unverified; the remaining known gap is the `list_stream` idle-timeout pool slot, deferred to keep within the 3-edit cap.

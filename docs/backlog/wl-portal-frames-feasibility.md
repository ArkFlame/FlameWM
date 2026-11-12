# wl-portal-frames: ScreenCast frames through PipeWire, feasibility

Status: BLOCKED on an architecture owner decision (D-portal). No code path delivers frames.
Row: `wl-portal-frames` in `docs/BACKLOG.md`. Prior slice: `ab25b5ea` (ScreenCast refusal e2e on a
private dbus-daemon, `crates/flamewm-screenshot/src/portal.rs`).

## Evidence (executed in the worktree at origin/main 25ddb667)

- `Cargo.lock` contains no `pipewire` or `libspa` package (`grep -c -i "pipewire\|libspa"` = 0).
- No crate under `crates/` declares a PipeWire dependency. The only mention is a comment in
  `crates/flamewm-integrations-linux/Cargo.toml` about PipeWire-Pulse volume, not a binding.
- `docs/ARCHITECTURE.md` has no portal or PipeWire section; `docs/ARCHITECTURE_EXCEPTIONS.toml`
  has no PipeWire or ScreenCast entry.
- `crates/flamewm-screenshot/src/screencast.rs` answers `CreateSession`, `SelectSources`, `Start`
  and `OpenPipeWireRemote` with the refusal `REFUSAL` (response 2, no streams). This is the
  intended behaviour until a stream owner exists.

## Why no binding was added

Adding a PipeWire crate (for example `pipewire`, which links `libpipewire-0.3` through bindgen)
is a new native dependency and a new event source. Architecture rules say `flamewm-reactor` owns
native event sources and product code reaches them only through typed facades. D-portal asks for a
named PipeWire ScreenCast owner first, and none is named. The task rule was to add no crate
without an owner, so the blocker is recorded here rather than worked around.

## What is ready without PipeWire

- The refusal contract is tested end to end on a private bus (`ab25b5ea`).
- Start-with-streams is not implemented. It must only answer `(0, {streams: a(ua{sv})})` when a
  stream source is present, so the refusal stays the default and no fake stream is advertised.

## Exact request to the architecture owner

Decide who owns the PipeWire ScreenCast stream. Two shapes are possible:

1. A new owner crate, `crates/flamewm-screencast-pipewire`, that holds the PipeWire main loop and
   exposes a typed facade to `flamewm-screenshot --portal`. The owner decides whether this is
   polled through `flamewm-reactor` or runs on its own thread.
2. Extend `flamewm-screenshot` directly. This needs an exception entry for that crate.

Draft exception (only if option 2 is chosen; path must be exact, no wildcards):

```toml
[[exception]]
path = "crates/flamewm-screenshot/Cargo.toml"
capability = "pipewire-dependency"
reason = "The ScreenCast portal streams captured frames through a PipeWire stream owned by the screenshot portal backend; the pipewire binding links libpipewire-0.3."
canonical_target = "crates/flamewm-screencast-pipewire/src"
remove_by = "<next minor version after the one that lands the stream>"
```

`remove_by` must be strictly greater than the version printed by `scripts/version`. The size of
the binding (crate plus transitive build dependencies) has not been measured, because no candidate
was added. Measure it with `cargo tree` in a scratch worktree before the owner decides.

## Proof still needed (not run)

- Private `pipewire` and a private `dbus-daemon` under a temp runtime dir (never the user's
  session bus or PipeWire socket).
- A test client (`pw-cli` or a small test on the same binding) receives N frames of the expected
  size and format after `Start`.

# u115 control / D-Bus service

Area: Control D-Bus service and its shell-side settings path (signal marshalling, duplicate snapshot
signals, snapshot application).

Composition before: the control link is the only path that moves whole desktop snapshots between the
WM and every shell process. Each changed turn re-marshalled snapshots up to three times
(typed snapshot -> `WireValue`, borrowed `WireValue` -> `MessageItem`, then libdbus copies), and
incoming signals were decoded through a deep clone of the entire argument vector before the typed
decode cloned it again. The WM also queued the same panel snapshot twice in one flush (window flush
and panel flush) and re-emitted `SystemSnapshotChanged` from two provider updates, so identical
dictionaries were re-serialised for every subscriber. On the shell side, a repeated settings signal
deep-cloned the whole snapshot before a revision guard rejected it.

Changes:

1. `crates/flamewm-control-wire/src/signal.rs`: `decode_signal` is now a thin wrapper over
   `decode_signal_call(&WireCall)`, and the new `decode_signal_owned(interface, member, args)` moves
   the already-decoded argument vector into the decoder. The transport has just built that vector, so
   a whole snapshot (every window, key and string) is no longer deep-cloned once per signal just to
   build a borrowed `WireSignal`.
2. `crates/flamewm-control-dbus/src/lib.rs` `decode_bus_signal`: uses `decode_signal_owned`, removing
   that clone from the per-signal receive path.
3. `crates/flamewm-control-dbus/src/lib.rs` `to_item` / `append_wire_values`: both now take
   `WireValue` by value. `encode_*` already owns the snapshot copy, so the outbound marshalling no
   longer clones every string a second time; the request path moves `call.args` into the message, and
   the reply path moves `reply.values`.
4. `crates/flamewm-control-dbus/src/lib.rs`: new `PublishedRevisions` (per connection, `RefCell`)
   gates `send_signal`, used by both `emit_signal` and the signals returned from `handle_call`.
   Panel snapshots (revision), topology generations, system snapshots (revision) and weather feed
   snapshots (revision) are dropped before encoding when their revision was already published.
   Snapshots whose revision does not cover the payload always send: windows (max `state_generation`),
   workspaces (occupancy is derived from cached windows) and settings (derived keys publish without a
   revision bump). State is per connection, so a fresh connection always publishes its first
   snapshot.
5. `crates/flamewm-shell/src/app/settings.rs` + callers: `apply_settings_snapshot` takes
   `&SettingsSnapshot` and clones only when the revision guard accepts it, so a repeated settings
   signal costs no deep clone. `ipc.rs` `note_signal` no longer clones the snapshot before the guard.

Expected composition shift: marshalling cost per changed turn drops by roughly one full deep copy of
the payload (2 -> 1 for replies/signals, 2 -> 1 for received signals); duplicate panel/system
snapshots skip encode + two copies + a bus write entirely. Settings signals that repeat a revision
cost a comparison instead of a snapshot clone. All changes are transport/allocation work; no visual
or protocol behaviour changes.

Evidence (no cargo in worker): `PublishedRevisions` unit test
`repeated_snapshot_revision_is_coalesced_before_encoding` (same revision suppressed, new revision
sent, request signals never suppressed). Existing round-trip tests in `flamewm-control-dbus`
(`assert_bus_round_trip`, `snapshot_signals_round_trip_through_bus_message`,
`t11_settings_snapshot_changed_round_trips_exactly_through_bus_message`,
`t05_system_snapshot_changed_round_trips_exact_snapshot_through_dbus`) and in
`flamewm-control-wire` exercise the refactored decode/encode paths unchanged.

Diff: crates/flamewm-control-wire/src/signal.rs, crates/flamewm-control-dbus/src/lib.rs,
crates/flamewm-shell/src/{app/settings.rs,app/ipc.rs,main.rs}, this log, unit marked DONE.

# u105-mounts-devices

Area: Mounts/devices (udisks/mountinfo) sidebar + status-strip filesystem size.

Already in place from earlier batches (confirmed by reading): the sidebar rebuild is event-driven
(netlink block uevents + `/proc/self/mountinfo` via `DeviceWatch`, 300 ms quiet coalescing, no
polling interval), `build_places` runs on the aux worker lane (mountinfo read, bookmark file, mount
point `is_dir` stats and the GVFS share listing are off the UI thread), and `unmounted_devices` runs
`lsblk` on a worker.

Remaining UI-thread disk wait for a mount: `view::fs_space` called `statvfs` inline from
`ExplorerApp::project`, once per 2 s TTL. On a stale NFS/CIFS mount `statvfs` blocks, so a projection
wake could stall the whole explorer window for seconds.

Change:
- `view.rs`: split the space path into a pure `SpaceCache` (`get`/`request`/`store`/`release`) and a
  worker-only blocking `fs_space`. `get` never touches the disk; `request` returns the path to sample
  once per TTL and self-heals a lost result after the TTL; a stale value stays visible until replaced.
- `app/places.rs`: `space_for(path)` reads the cache and, when stale/missing and the event channel is
  live, queues one `WorkerResult::Space` on the existing aux lane.
- `projection.rs`: the status strip now reads `self.space_for(tab.path())` instead of calling
  `statvfs` inside the frame.
- `main.rs`/`input.rs`/`lifecycle.rs`: the `SpaceCache` field, the `Space` result variant and its
  handler (stores the sample; the wake repaints).

Expected composition shift: the explorer projection path loses its only remaining synchronous
filesystem syscall (statvfs). In the idle/navigation sample the status-strip share of UI-thread wait
drops to zero; the statvfs syscall moves to the background lane at most once per 2 s per visible
path, and one per navigation first frame. No polling is added: requests are only issued from an
existing projection (event wake), never from a timer.

Verification: read-through only (no cargo in this unit). New unit tests in `view.rs` cover
sample-per-path, TTL refresh, in-flight suppression, lost-request recovery and stale-value retention.

Status: UNVERIFIED (compile/test run by the orchestrator).

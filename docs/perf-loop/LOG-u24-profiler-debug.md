# Unit u24-profiler-debug

Area: profiler, debug, api, control, platform, session-core.
Scope: second-pass scout of the areas above. Older units already removed the
first-order hot paths; this pass targets per-call overhead, environment lookups,
and redundant cloning that remains on steady-state paths.

## Accepted optimizations

1. profiler: inert spans when disabled.
   `SpanGuard::begin` still resolved a label slot (scanning up to 128 slots with
   an `RwLock` read each), read two clocks, and touched the thread-local stack
   even with `FLAMEWM_PROFILE` off. The guard now carries an `active` flag, so
   construction and drop are one branch with no clock reads, locks, or
   thread-local access. `record_elapsed` returns before timing conversion.
   Composition effect: instrumentation call sites become free for every user
   without the profile variable, which covers the normal desktop session.

2. profiler: `ProfilePoint` resolves its registry slot once.
   `start()` used to re-run the labeled scan per sample. The point now stores
   the slot index at construction, so repeated samples only read clocks and add
   atomics. The free `start(label)` function is unchanged.
   Composition effect: span sampling cost drops from a scan to a constant.

3. debug: enabled flag resolved once.
   `emit` ran `std::env::var_os("FLAMEWM_DEBUG")` on every call and on every
   hot-path event. The gate is now a `OnceLock<bool>`, matching the profiler
   config that is resolved once at startup.
   Composition effect: disabled debug logging costs no environment scan.

4. debug: log length tracked in process state.
   `write_line` performed a `stat` syscall for the file size on every emitted
   line. The state now carries the byte count, seeded from the file at
   `init_process_in` and advanced by the bytes actually appended (truncated
   lines count what was written). The CAP marker and truncation behaviour is
   unchanged.
   Composition effect: one syscall fewer per accepted debug line.

5. api: allocation-free `accent_rgb`.
   The resolver lowercased the stored accent text into a new `String` before
   matching named colours, and trimmed twice. It now trims once and compares
   named values with `eq_ignore_ascii_case`, keeping the hex parse on the same
   trimmed slice. Called by the shell, window manager, lock screen and UI
   accent projection.
   Composition effect: removes one heap allocation per accent resolution.

6. platform: task-strip reconcile without clones.
   `reconcile_live` cloned the whole strip (`let before = self.clone()`) purely
   to diff afterwards, allocated a `Vec` of cloned application ids per call,
   and cloned an application id per live window. Changes are now tracked during
   the mutation (removals, generation refreshes, additions), the strip keeps a
   borrowed-app variant `reconcile_live_borrowed` used by the panel service, and
   an application id is cloned only when a new entry is appended. The
   `FLAMEWM_DEBUG_TASKS` gate is resolved once instead of per removed window.
   Composition effect: window-list changes no longer allocate per window; the
   panel reconcile path drops its owned copy of every application id.

## Verification

Static reading only per worker rules: no cargo, no git in the shared checkout.
Each edited unit was re-read after editing for type and borrow correctness.

## Skipped

- Control queues and settings persistence already block on condition variables
  or channels with no polling loop; nothing to change there this pass.
- D-Bus mutation thread and wake-FD plumbing already avoid idle wakeups.

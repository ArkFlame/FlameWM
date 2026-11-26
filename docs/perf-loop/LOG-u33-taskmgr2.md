# u33-taskmgr2 — batch-five review + next sampler/paint hotspots

Area: `crates/flamewm-task-manager` (procfs sampler + metrics/table paint). Reviewed the two
recorded perf slices in this area (u03 and u26 logs, the `git log` view is unavailable to this
worker). No cargo/git was run; rustfmt (edition 2024) parses and formats clean.

## Review: regression found and fixed

- **Stale replay after leaving the hot set (u26 change 3).** `cached.last` was written only for
  non-hot pids (`(!hot).then(|| task.clone())`) on the assumption that a hot pid is re-read every
  tick. That is true while it stays hot, but when it drops out of the hot set and its first
  rotating read is not due, the replay path returns `last` from *before* it became hot — up to
  `HOT_COUNT` processes showing a many-seconds-old CPU value. `last` is now written for every
  read. With the new `Arc<str>` fields the clone is a refcount bump, so the saving that motivated
  the conditional is gone anyway.
- Reviewed the u03 stack ring, stack `cpu_ticks` array, one-pass `meminfo`, dropped `replace`
  and static `INDENTS`, and the u26 path buffer, pid-list reuse, hot-set/`last` logic, row cell
  buffers, closed-menu skip and `matching_count`. Behaviour matches the logs; no other
  regression reproduced. Corrected the `cpu_ticks` comment (the eight counters are user..steal,
  not user..guest; `idle + iowait` was already the right idle share).

## Changes (composition share)

1. **`parse_stat` no longer allocates a field vector.** It was collecting every `stat` field into
   a `Vec<&str>` (~50 entries) for each due task each tick. One indexed pass now reads only the
   ten fields the table uses. Biggest sampler allocation removal (~150-400 fewer `Vec` allocs/s).
2. **Interned process strings (`Arc<str>` for `name`, `user`, `command`).** The sampler clones a
   `Process` into the replay cache on every fresh read and into the snapshot; each clone copied
   three `String`s. `Arc<str>` shares them (uids share one cached user name), so those clones and
   the per-row clones are refcount bumps, and `Process` shrinks by ~24 bytes.
3. **Reused tree-depth map.** `assign_depths` built a fresh `HashMap` of every pid each sample;
   the map now lives in `Sampler` and is cleared/refilled in place.
4. **Reused hot-set ranking.** `hot_pids` allocated a ranking `Vec` plus a `HashSet` per sample;
   `hot_pids_into` now refills the sampler's set and vector in place.
5. **No `Vec` for load averages.** The `/proc/loadavg` three-field `collect::<Vec<_>>().join(" ")`
   became an in-place `load_summary` write.
6. **Graph blend fast paths.** Graph rasters are rebuilt each sample; most `source-over` blends
   land on an empty pixel or use the opaque line colour, where the result is exactly the source.
   Both now copy bytes and skip the float math (most of the ~25k blends/s).
7. **Metrics card reuses two buffers.** `paint_metrics` `format!`ed ~80 ids and values per
   sample. It now writes ids/text into two reused `String`s and uses `text_ref`, so unchanged
   cells cost no allocation. Search box and sort headers reuse the table's value buffer too.
8. **Allocation-free search.** `matches` lowercased the name and command and formatted the PID
   for every process on every paint/search keystroke. It now uses an ASCII case-folded byte
   window scan (non-ASCII still falls back to `to_lowercase`) and a stack-buffer PID digit match.
9. **`window_backdrop` computed `window_backdrop_material` twice** for the two fields; one call
   feeds both now.

Expected composition shift: sampler CPU moves from heap churn (parse field vectors, cloned
strings, a hash map and a ranking vector per second) to the procfs reads that carry the data; the
metrics card and graph raster lose their per-sample allocation/float overhead. Visuals are
unchanged: same ids, same text, same colours, and the blend fast paths are byte-identical to the
old source-over for those pixels.

## Dead code / dependencies

- Removed the now-unused `flamewm-control-core` and `flamewm-dbus-reactor` dependencies (nothing
  in the crate names them; `flamewm-control-dbus` brings its own).
- Removed the `hot_pids`/wrapper indirection: `hot_pids_into` and `assign_depths(&mut map, ..)`
  are the single owners.
- Merged `rate` into `rate_into` (tests build the buffer), so no dead twin remains.

## Next candidates

- `Sampler` still `read_to_string`s `/proc/stat`, `meminfo`, `loadavg`, `uptime`, `net/dev` and
  `passwd` into fresh strings each sample (~6 allocs/s); these could share the reused byte buffer.
- `Snapshot.cores` is a fresh `Vec<f64>` per sample; a swapped buffer would remove it entirely.
- Graph rasters are still 73 KB `Vec` allocations per card per sample; the document compares
  bytes before dirtying, so only an API that lets the app own the pixels (or diff the new column)
  can remove that.

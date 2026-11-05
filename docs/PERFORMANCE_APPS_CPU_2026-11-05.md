# App CPU: allocation-per-compare hot spots (explorer, task manager)

Scope: `flamewm-explorer` listing sort and typeahead, `flamewm-task-manager` process sort.
Not covered: app wake/CPU measurement through `scripts/perf/fm.sh` (no native run done in this pass).

## Root causes

1. Explorer listing sort (`crates/flamewm-explorer/src/lib.rs`, `list_directory`): the name comparator
   called `to_ascii_lowercase()` on both names for every compare, two heap allocations per comparison.
   Fix: compare the precomputed `Entry::lower_name`.
2. Explorer typeahead (`crates/flamewm-explorer/src/view.rs`, `typeahead_match`): every keystroke called
   `to_lowercase()` on every name in the listing until a match. Fix: `starts_with_lowercase` walks
   `char::to_lowercase` without allocating. Known difference: `str::to_lowercase` maps a final `Σ` to `ς`;
   the char walk maps it to `σ`. Only matters for a prefix ending in a word-final sigma.
3. Task manager sort (`crates/flamewm-task-manager/src/lib.rs`, `sorted`): the Name comparator lowercased
   both names per compare. Fix: lowercase each name once per paint, index by process.

## Measurements

Standalone release micro-benchmark (`rustc -O`, not in the repo), 20 000 names, same host, loaded machine (load ~12-19):

| Path | Before | After |
| --- | --- | --- |
| typeahead miss, per keystroke | 1.615 ms | 0.281 ms (5.8x) |
| listing name sort (20 000 entries) | 27.94 ms | 4.02 ms (6.9x, index sort with precomputed keys) |

The sort row compares a `to_ascii_lowercase` comparator against a precomputed-key comparator; it is
representative of the change, not an exact replay of `list_directory`.

## Verification

- `CARGO_TARGET_DIR=/run/media/linsaftw/SDD/target-apps CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2 cargo test -p flamewm-explorer -p flamewm-task-manager` exit=0
  (explorer lib 203 passed; task-manager 16 passed; log `.agents/runtime/apps-cpu/test.log`).
- New tests: `starts_with_lowercase_matches_allocating_form` (explorer), `name_sort_is_case_insensitive_with_pid_tiebreak` (task manager).

## Not done (open)

- Wake/CPU A/B on the live apps (explorer scroll, marquee, settings slider drag, task-manager idle) not run.
- Explorer scroll glide spawns a thread per 16 ms frame (`app/scroll.rs` `schedule_scroll_tick` via `spawn_background`).
  Candidate for a persistent timer; not changed here.
- Task-manager sampler wakes every 100 ms (10 x 100 ms per 1 s sample) and rebuilds process rows each second.
  Candidate for measurement, not changed here.

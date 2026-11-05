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

## Follow-up pass

1. Explorer scroll glide (`app/scroll.rs`): `schedule_scroll_tick` spawned a thread per 16 ms frame. Now one
   ticker thread per app (`spawn_frame_ticker`, started on first glide) waits on a channel and sleeps the frame
   period before each tick. Tests: `one_tick_per_request_after_the_period_on_a_single_thread`,
   `dropping_the_sender_stops_the_thread`. Not a `flamewm-reactor` source (the reactor is the WM's native loop).
2. Task-manager sampler (`lib.rs`): polled the action queue every 100 ms (10 wakes/s). Now sleeps on the action
   channel with `recv_timeout` until the 1 s sample is due: about 1 wake/s when idle, and queued actions run at once.
   Not done: slowing sampling when hidden or minimized, because no visibility signal reaches the sampler.
   Known cost: window close joins the sampler, so close can wait up to 1 s.
3. Settings slider drag (`app_impl_1.rs` `handle_glass_slider_drag`): each Motion makes a synchronous WM IPC
   transaction (`apply_setting`) and then a full `settle` -> `sync`. Static finding only; no live numbers yet,
   so no change made.

## Settings slider drag and tab switch (measured)

Harness: `scripts/perf/settings_session.sh <bin-dir> <out> --slider 590,765,296 --tabs "125,214;131,274;138,304;125,214;131,274;138,304" --pre "138,304" --steps 150 --idle-secs 5`.
It boots the fast-profile WM with flamewm-settings autostarted and runs `settings_scenarios.py` (XTEST drag of the
Animations page speed slider, then six tab clicks). Counts are /proc deltas; `syscw` (write syscalls) is the proxy
for IPC plus paint writes. Two runs per side, host load about 10-19.

| Phase | Before (cpu%, wakes/s, syscw) | After gate (cpu%, wakes/s, syscw) |
| --- | --- | --- |
| idle (5 s) | 0.0 / 0-4.5 / 0-7 | 0.6 and 0.0 / 2-55 / 7-46 |
| slider drag, 150 motions | 284-289 / 20.8k-22.1k / 10.3k-11.6k; settings 57-61% cpu, 4.4k-5.0k syscw | 291-293 / 21.7k / 11.5k-11.9k; settings 61-65%, 4.9k-5.2k syscw |
| tab switch, 6 clicks | 24-31 / 2.7k-2.8k / 3.9k-4.2k | 24-34 / 2.7k-3.1k / 3.9k-5.0k |

Finding: the per-Motion work is not the drag gate's target. The gate (one apply per 16 ms) was tried and reverted
because it changed nothing: XTEST motions arrive more than 16 ms apart, so almost every Motion still applied.
Remaining cost sits in the per-motion pipeline (settings paint writes, WM and Xwayland redraw) and is not yet
attributed. Next step if pursued: count ApplySettings calls per motion with a trace, and check whether the WM
redraws the whole window on each settings change.

Visibility-aware sampler: blocked. Only the desktop role selects `VisibilityChangeMask`
(`crates/flamewm-render-x11/src/app/events.rs:441`); no focus events reach controllers. Enabling it for the task
manager touches render-x11, which another worker owns, so it was not changed.

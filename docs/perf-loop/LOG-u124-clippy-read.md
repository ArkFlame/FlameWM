# LOG u124-clippy-read — clippy-style audit of the newest batch

Batch twenty. Owned paths: `crates/**/src`. `cargo` and `git` are forbidden in the shared
checkout, so the reviewed set is derived from source, not from a diff: the workspace already
runs `cargo clippy --workspace --all-targets --all-features -- -D warnings` in CI
(`.github/workflows/ci.yml:38`, mirrored by `scripts/lint`), so every default and pedantic
lint is already clean at HEAD. This unit therefore looked for the classes clippy cannot prove
from types alone: clones that copy data a later step does not need, manual `Option`/`bool`
shaping, and `unwrap` outside test code.

## Method

Searches used `rg`, `sed`, and `awk` only (no interpreter, no `ctx_execute`, no `sleep`).

- Candidate files: the most recently modified `crates/**/src/*.rs` (the landing window of the
  current batch; `find -newermt`), plus a full `crates/**/src` sweep for the `unwrap` class.
- `unwrap` outside tests: `awk` tracked each file's first `#[cfg(test)]` line and counted
  `unwrap()` calls before it, skipping `*_tests.rs` and `*/tests/*` helpers. The only hit was
  `crates/flamewm-terminal/src/scroll.rs:88`.
- Lint-class scans: `len() == 0`, `&Vec<T>`, redundant `|x| f(x)` closures, identity maps,
  `filter_map(|x| Some(...))`, `.fold` sums, `sort_by(cmp)`, `to_string() ==` compares,
  `&x[..]` slices, `if ... { Some } else { None }`. The `to_string()` compares in
  `app/input.rs`, `app/navigation.rs`, and `prefs.rs` are deliberate canonical-URL checks:
  `Url`'s `Display` folds an explicit default port into `port_or_default()`, so structural
  `PartialEq` would treat `sftp://h:22/x` and `sftp://h/x` as different where the string
  form treats them as equal. Left as written.

## Fixed

### F1 — terminal glide unwrap (`terminal/src/scroll.rs::advance`)

`advance` guarded with `if self.start.is_none() { return None }` and then read the start
instant again with `self.start.unwrap()`. Replaced with one `let Some(start) = self.start
else { return None };` binding (`Instant` is `Copy`), and used `start` for the duration check.
Same frames, same `None` early-out, no panic path, one fewer field read per glide tick.

### F2 — selection band copied every path twice (`explorer/src/lib.rs::select_vertical_band`)

The band built `paths: Vec<PathBuf>`, then cloned every `PathBuf` again into
`selected_paths: HashSet<PathBuf>` while keeping `paths` for `first()`. Reordered so
`selected`/`anchor` are read from `paths` first, then `paths.into_iter().collect()` moves
each `PathBuf` into the set. A drag-select over `n` rows now does `n` `PathBuf` allocations
instead of `2n`, with identical selection, anchor, and ordering.

## Result

No other obvious item in the scanned classes: the newest files already use `Arc<str>`/`Arc<[Run]>`
sharing (`task-manager/src/model.rs`, `terminal/src/app/grid.rs`), reuse scratch buffers instead
of `format!` per read, and keep every `unwrap()` inside `#[cfg(test)]` modules.

## Validation (orchestrator)

`cargo clippy --workspace --all-targets --all-features -- -D warnings` and
`cargo test -p flamewm-terminal -p flamewm-explorer`. Both changes are local and type-preserving;
the terminal glide tests in `scroll.rs` and the explorer selection tests cover the edited paths.

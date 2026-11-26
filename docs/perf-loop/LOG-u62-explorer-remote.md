# Log u62-explorer-remote

Ranking note: remote-fs work only runs while a remote tab is live, but inside that window the
listing command dominates (one ssh round trip per listing, plus per-entry work), followed by
transfers (one extra round trip per item), then the parse-free remote guards the UI runs on
navigation/drag. Changes target those three slices in that order.

- `crates/flamewm-remote-fs/src/shell.rs` (`list_with_links`): replace the per-symlink
  `-exec test -d {} \;` probe with `find`'s own `-xtype d`, and delete the now-unused `list`
  command builder. Expected: listing a directory with many symlinks no longer forks one `test`
  process per link; one fewer builder to maintain. Verified locally with GNU `find`: only
  symlinks whose target is a directory are emitted (broken links and files excluded).
- `crates/flamewm-explorer/src/remote.rs` (`copy_items`/`copy_one`): keep the `Kind` from the
  planning `stat` and pass it to `copy_one`, which only falls back to a fresh `stat` when the
  planning stat failed. Expected: a remote-to-local download now costs one `stat` round trip
  instead of two per item (halves the control round trips of a batch download).
- `crates/flamewm-explorer/src/remote.rs` (`is_remote`): scheme-only check instead of a full
  `Url::parse`. Expected: navigation and drag hit-tests no longer allocate user/host/path
  strings just to answer a boolean; also strictly safer, since a malformed remote-looking path
  is classified remote and never handed to `std::fs`.
- `crates/flamewm-remote-fs/src/entry.rs` (`build`): only scan for the ` -> target` suffix on
  symlinks, and drop the unused `parse_listing` helper plus its test. Expected: one fewer
  full-name scan per file row parsed on every listing; less dead code.
- `crates/flamewm-remote-fs/src/client.rs` (`mkdir`/`remove`/`rename`): compute `remote_path()`
  once per operation and compare `rename` endpoints field-by-field instead of building two
  `ConnKey`s. Expected: fewer transient allocations on every mutation round trip.
- `crates/flamewm-remote-fs/src/pool.rs` (`touch`): update an existing slot via `get_mut`
  instead of `entry(key.clone())`. Expected: no per-operation clone of the connection key
  strings on the hot `ensure` path.
- `crates/flamewm-remote-fs/src/url.rs` (`parse`): match the scheme with
  `eq_ignore_ascii_case` instead of allocating a lowercased copy. Expected: one less String per
  URL parsed (every navigation, entry path and guard).
- Regression fix, `crates/flamewm-remote-fs/src/client.rs` (`upload`): invalidate the listing
  cache only when the upload completes. A failed or cancelled upload leaves the destination
  untouched, and the old unconditional `invalidate` bumped the global generation, discarding
  unrelated in-flight listings from the cache.

No cargo/git was run in this shared checkout (orchestrator gate). Syntax checked with
`rustfmt --emit stdout` on every edited file; `find -xtype d` behavior checked directly.

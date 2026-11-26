# u01-explorer-io

- search.rs: `rank_match` folds case via char streams with an ASCII byte fast path instead of lowercasing the query and every visited file name, removing two `String` allocations per walked entry (up to 200k entries per job).
- search.rs: the walker uses `DirEntry::file_type` (d_type) for directory detection and joins the path only for hits/directories, and stats only hits, so non-matching files no longer cost an lstat plus two path/name allocations each.
- jobs.rs: `copy_tree` shares one lazily filled 256 KiB transfer buffer across a whole tree instead of allocating and zeroing a fresh one per file; child paths reuse the `DirEntry` name instead of re-allocating it.
- lib.rs: `contains_lower` compares the raw query against the pre-lowercased `lower_name` with an ASCII byte scan, so `visible`, `visible_nth`, `visible_index_of` and `filtered_count` (projection path, every frame) no longer allocate a lowercased query; the filter cache now keys on the raw query.
- lib.rs: `list_directory_streamed_cancellable` reuses the `DirEntry` name for the hidden check and path build, dropping a duplicate name and `PathBuf` allocation per row.

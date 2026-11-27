# Unit p53-applications-scan-ram
STATUS: PARTIAL

Crates flamewm-applications and flamewm-apps: desktop-file scan and app index: lazy, incremental, no duplicate Strings per entry (intern or Arc<str>), skip unreadable files with one report. Measure struct sizes by reading code; max 3 edits.

## Result

Struct sizes measured by reading code (64-bit, 3-word `String`/`Vec`/`PathBuf`):
`DesktopAppId` 24 B; `DesktopApplication` 216 B (9 × 24); `DesktopEntry` 296 B
(24 id + 216 application + 24 path + 24 exec + 2 flags, padded to align 8);
`ApplicationCatalog` 120 B (24 Vec + 4 × 24 BTreeMap). Per entry the heap holds two
copies of the id (`DesktopEntry.id` and `DesktopApplication.id`) plus index keys.
Two copies of the id are required by cross-crate API
(`flamewm-settings-core/src/xdg_startup.rs:84` uses the `DesktopEntry.id` field), so
that duplication was intentionally left alone; only removable duplicates were cut.

Rows (file:line, finding, change, risk):

- `crates/flamewm-applications/src/desktop_entry.rs:191-225` — parse cloned six
  strings (`Exec`, `Name`, `GenericName`, `Comment`, `StartupWMClass`, `Icon`) out
  of a local `values` map that is dropped at return, so each existed twice during
  the scan — moved them out with `remove()` into `DesktopApplication`/`DesktopEntry`.
  Risk: UNBUILT — interleaved `remove`/`get` borrows in the struct literal; values
  and empty-string defaults are unchanged.
- `crates/flamewm-applications/src/desktop_entry.rs:228-240` — `id_stems` returned
  `Vec<String>`, allocating 1-2 owned stems that the index then lowercased into
  fresh keys (two allocations per stem) — now returns `Vec<&str>` borrowed from the
  id; the index still allocates one lowercased key per stem. Risk: UNBUILT —
  lifetime elision on `Vec<&str>`; `rg` confirms the only caller is
  `app_catalog.rs:96`.
- `crates/flamewm-applications/src/desktop_entry.rs:242-253` and
  `crates/flamewm-applications/src/app_catalog.rs:101-110` — `by_exec` re-tokenized
  the whole `Exec` line per entry, allocating a `Vec` and token strings only to take
  the first token — `exec_basename` now accepts the already-tokenized program and
  the call site uses `entry.application.argv.first()`. Risk: UNBUILT — quoted programs
  with spaces still work because `tokenize_exec` stripped quotes at parse time; the
  only other grep hit for `exec_basename` was this call site.

Already satisfied, no edit: lazy discovery (`OnceLock`, background warm-up thread,
`ready_shared_application_catalog`), incremental refresh (`dir_stamps` +
`refresh_if_dirs_changed` + `delta_to`), and skipping unreadable/malformed files with
one `eprintln!` report each while discovery continues. Reference-first: `.vendor/` has
no Rust `Arc<str>` app-index precedent; xfdesktop relies on `GDesktopAppInfo` caching
plus GQuark-style interning, which supports sharing repeated strings rather than
cloning them, so the edits move/borrow strings instead of introducing a new interner.

PARTIAL — three production edits made as scoped above; no build or test was run
because this worker is barred from `cargo`, so every edit is UNBUILT. The id
duplication that remains is required by the public `DesktopEntry.id` field used in
other crates; changing it would be a wider cross-crate refactor beyond this unit's
three-edit budget. The coordinator must compile and run `flamewm-applications` tests
before treating these changes as verified.

# u61-archiver-conv changes

Batch eight. Reviewed the four owned crates by reading the current sources (no `git` access in this
shared checkout). Found no regression that changed output bytes, ordering or error kinds; the fixes
below remove repeated filesystem probe work, per-event allocation and an O(n^2) name search.

- conversion-core/conversion.rs: `available_program` and `available_converter` now resolve each
  program once per process through a `Mutex<HashMap<String, Option<PathBuf>>>` cache.
  `available_converter` is folded onto the same lookup (`magick` then `convert`), deleting the
  duplicated `PATH` walk. Building a context menu calls `offered_tools` and `offered_formats`,
  which ask about `ffmpeg` up to five times plus `zip`, `sha256sum`, `tar`, `unzip`, `pandoc` and
  `pdftotext`; every miss used to read `PATH`, allocate and `stat` each directory on the UI thread.
  Now the first menu open pays that sweep, later opens and every `run_tool` re-check hit the map.
  Expected composition shift: the file-menu path loses dozens of `stat` calls per open.
- conversion-core/conversion/media.rs: `codecs_available` no longer builds a `Vec` of required
  encoders per target. It iterates the codec argument windows and checks the extra `gif`/`png`
  muxer encoder directly, so `offered_formats` for a video source allocates nothing while probing
  up to eight targets. Same encoders are accepted and rejected.
- conversion-core/conversion/tools.rs: `offered_tools` resolves `ffmpeg` once for the whole
  tool list instead of once per retained ffmpeg tool, then applies the cached `codecs_available`
  check per tool. Menu contents are unchanged (same order, same truncation to eight).
- downloads-core/lib.rs: `extension` now borrows the final suffix instead of allocating a lowercase
  `String`; `is_candidate_name` and `classify` compare with `eq_ignore_ascii_case`. A candidate
  event used to allocate three strings (candidate check + category check) per name; it now
  allocates none during classification. Case-insensitive matching is preserved.
- downloads-core/lib.rs (`move_into_category`): the destination name is now advanced one numbered
  sibling at a time (`name`, `name (1)`, ...) and `renameat2(RENAME_NOREPLACE)` itself reports a
  clash. Previously each collision re-ran `unique_name`, which re-`stat`ed every earlier candidate
  from ` (1)` again: a burst of N same-named downloads cost O(N^2) stats plus an extra stat before
  each rename. Now it is one rename syscall per candidate and N attempts maximum. `unique_name`
  keeps its public signature and numbering via the new `candidate_name` helper, so tests and the
  cross-device `copy_then_commit` fallback behave as before.
- archiver/sevenz.rs (`find_tool`): the `7zz`/`7z` probe runs once via `OnceLock<Option<PathBuf>>`
  instead of spawning a probe child process again on every `list`, `extract` and `create`. The
  actual tool invocation still uses the same name, so `PATH` resolution and error text are
  unchanged. Expected composition shift: one fewer process spawn per 7z operation.
- archiver/writer.rs (`tar_file`): the tar data source is wrapped in
  `io::BufReader::with_capacity(COPY_BUFFER, Tracked::new(...))`. The `tar` crate copies through a
  small internal chunk, so creation of every tar variant read the source in those small pieces
  even though extraction already used the shared 64 KiB buffer. The buffered reader prefetches
  64 KiB per read syscall, so a large member needs one eighth of the read syscalls; cancellation
  is still checked between prefetches. `COPY_BUFFER` is now `pub(crate)` for this reuse.
- archiver/progress.rs (`copy_chunks`): dropped the second length check/resize that duplicated the
  one in `copy_tracked` (the only caller), which already guarantees a full `COPY_BUFFER` vec.

Expected composition shift: archive creation and 7z operations spend less kernel time on
read/probe syscalls, and the conversion menu plus downloads organizer stop re-scanning `PATH` and
allocating per name. Visuals, progress values (step-based, final snapshot included), archive bytes,
member order and error kinds are unchanged.

UNVERIFIED: this unit may not run `cargo` or `git`; changes were reviewed by reading the current
call paths and signatures, not by compiling or running tests.

# u91-taskmgr-proc changes

Composition of the task-manager sample loop (1 Hz, visible window): procfs syscalls (open + read + close
per due task, one `/proc/stat`, `meminfo`, `loadavg`, `uptime`, `net/dev` read) dominate; content parsing
and per-sample bookkeeping are the next share, and per-sample allocations show up as allocator time in the
same wakeup. Hidden windows already cost nothing (`SampleGate` parks the sampler on a condvar, no timer),
so this unit targets the syscall/marshalling tail around each sample without touching the sampled data:

- model: `/proc/<pid>/stat` is parsed from raw bytes (`parse_stat_bytes`): no whole-line UTF-8 validation,
  no `split_whitespace`/`char` decoding, no `parse::<>`; fields are scanned with one index and ASCII digit
  folding, whitespace tokens are sliced directly. A process whose `comm` is not valid UTF-8 is now kept
  (lossy name) instead of being dropped by the old `from_utf8` gate, so the table loses no rows.
- model: pid list stays sorted across samples. Vanished pids are dropped with one `retain` against the
  cache and a discovery scan binary-inserts new pids, replacing the per-sample rebuild from `HashMap` keys
  plus `sort_unstable` + `dedup` over the whole table. The ascending invariant the hot-set cursor uses is
  unchanged.
- model: per-core counters go into a reused `cores` buffer (`load_cpu_into`) swapped with `prev_cores`
  each sample, so the `/proc/stat` refresh allocates no `Vec` after the first sample.
- model: sanitized `cmdline` is built in the reused `command_text` buffer and the shared `Arc<str>` is only
  rebuilt when the text actually changed; the `[name]` fallback no longer allocates on unchanged refreshes.
- model: `check_identity` reads `/proc/<pid>/stat` as bytes and reuses the byte parser (no `read_to_string`
  conversion on the process-operation path).
- model: two tests cover the byte parser (non-UTF-8 comm, negative nice, truncated fields) and the
  sorted-unique pid-list invariant across samples.

Expected composition shift: smaller parse/bookkeeping share of each 1 s sample and zero sampler
allocations on the steady path; syscall count and sampled data are unchanged, so task-manager visibility
and update cadence stay identical.

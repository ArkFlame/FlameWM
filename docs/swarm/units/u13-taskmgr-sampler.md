# u13 — Task-manager process sampler, narrow retry

**Status: UNBUILT.** No build, test, commit or push was run; the worker contract forbids `git`,
`cargo`, builds and tests. The orchestrator must run `cargo test -p flamewm-task-manager` after the
batch lands.

Sampler file: `crates/flamewm-task-manager/src/model.rs`

Chosen from `rg -l '/proc' crates --glob '*.rs' | head -20`. The other task-manager hit,
`crates/flamewm-task-manager/src/lib.rs`, owns the worker loop, not procfs sampling. Only the
sampler file and this unit doc were touched in this retry.

## Findings (one row per attempted change)

| # | File:line | Cost | Change | Risk |
| --- | --- | --- | --- | --- |
| 1 | `crates/flamewm-task-manager/src/model.rs:692-703` | Before: one `format!("[{}]", name)` `String` allocation for every stale pid whose `cmdline` is empty. After: reuse the already-present `command_text` buffer via `write!`; the `Arc<str>` is still rebuilt only when the fallback actually changes | `std::fmt::Write` is already in scope (used by `proc_path`), no new field, no new allocation, no new syscall | Reusing `command_text` means the fallback overwrites the buffer; the next `command_into` clears it before reading, so no stale text leaks. Borrow of `cached.command` ends at the comparison under NLL. UNBUILT: no compile or test was run |
| 2 | `crates/flamewm-task-manager/src/model.rs` (whole file) | No change made. `rg -n 'hidden\|hide\|visib\|obscured\|minimi\|show'` over the sampler returns no matches: `Sampler` holds CPU, procfs buffers and pid caches, no visibility state | The requested skip is conditional on a visibility flag existing; the sampler has none. The hide/uncover gate belongs to the worker loop in `lib.rs`, which this unit is forbidden to touch ("do not touch other files"). Adding dead flag state here could not be driven and would be unsafe | Skipped by scope, not by oversight. Visibility coverage remains owned by the worker-loop unit. UNBUILT |

PARTIAL: the narrow retry touched only the sampler doc and the sampler source. One safe allocation
cut was applied and rowed; the visibility skip was skipped because the sampler carries no flag and
the owning worker-loop file is out of scope. Nothing was compiled, tested or committed, so the
result stays UNBUILT and the borrow and behavioural checks are owed by the orchestrator.

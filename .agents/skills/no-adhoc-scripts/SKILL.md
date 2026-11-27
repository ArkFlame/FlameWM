---
name: no-adhoc-scripts
description: Load before any task that needs to search, edit, wait, count or batch files. Forbids python/ctx_execute/temp scripts and sleep polling; gives the fast replacements.
---
# No ad-hoc scripts (hard rule, user order, permanent)

FORBIDDEN, no exceptions, for every agent and subagent:
- `python`, `python3`, `uv run python`, `python -c`, `python -I`, heredoc interpreters.
- `ctx_execute` and anything that writes `/tmp/ctx-exec-*/code.py`. Each call spawns an interpreter, burns CPU, and leaks a temp dir. It has overloaded this machine.
- `sleep` loops and polling shells.
- Throwaway scripts in `/tmp` or the scratchpad.

Use instead (fastest first):
| Need | Use |
|---|---|
| find text / files | `rg`, `fd` |
| edit a file | Edit / Write tools |
| mechanical replace | `sed -i`, `rg -l X \| xargs sed -i` |
| JSON | `jq` |
| count / filter output | `rg -c`, `wc -l`, `head`, `cut`, `awk` one-liner |
| wait for units | `scripts/perf-loop wait [s]` |
| commit + push | `scripts/perf-loop sync "msg"` |
| cargo errors only | `scripts/perf-loop check` |

If a task repeats twice, write it once as a documented, `set -Eeuo pipefail` bash (or Rust) script in `./scripts/`, then reuse it. Check `ls scripts/` before writing anything new. Never `cargo`/`git` in worker units unless the unit says so.

If you catch yourself about to open an interpreter, stop and use the table.

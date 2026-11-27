# Unit p60-bench-baseline-script
STATUS: DONE

Check scripts/bench-baseline (create if missing, bash, reusable): per .agents/skills/scoped-perf-tests/SKILL.md measure per-process RSS (/proc/PID/status VmRSS) and wakeups/s (/proc/PID/status voluntary_ctxt_switches delta over 1 s) for each flamewm-* process in a running session or nested smoke, print a share table, write docs/PERF_BASELINE.md rows. Rows must be reproducible. Do not run it; the orchestrator runs it once.

## Result
| file:line | finding | change | risk |
| --- | --- | --- | --- |
| scripts/bench-baseline:71-75 | UNBUILT. `voluntary_ctxt_switches`/`nonvoluntary_ctxt_switches` were read from `/proc/$pid/sched`, where those keys do not exist (sched uses `nr_voluntary_switches`/`nr_involuntary_switches`), so every wakeup delta was 0. Confirmed on live `/proc/self/status` vs `/proc/self/sched`. | Read both counters from `/proc/$pid/status`, the same file already used for VmRSS and the field named in the brief. | Kernel field-name drift could reintroduce zeros; failure is a silent 0, never a crash. |
| scripts/bench-baseline:89-98 | UNBUILT. `read -rt "$WINDOW" <> <(:)` hit immediate EOF (~12 ms measured) because the process-substitution writer exits at once, making the "1 s" window effectively zero. | Open a private FIFO with its write end held in-shell (O_RDWR) so `read -t` times out after a true `WINDOW` seconds (~1008 ms measured). No `sleep` binary, no polling loop; fd and temp dir cleaned up. | Relies on Linux O_RDWR FIFO semantics and `mktemp`; both are present on the target platform. |
| docs/PERF_BASELINE.md:12-17 | UNBUILT. Prose stated wakeups came from `/proc/<pid>/sched`, contradicting the corrected script and the brief. | Describe wakeups as the delta of voluntary (plus nonvoluntary) context switches from `/proc/<pid>/status`; RSS and CPU-tick sources unchanged. | Prose only; no runtime effect. |

PASS — all three requested edits are in place, `bash -n scripts/bench-baseline` parses cleanly, and the two behavioral findings were confirmed with isolated live `/proc` and timeout measurements. No kernel identifier, share-table format, or frozen-baseline gate was loosened; the script and doc remain reusable and the doc still records commit, machine, CPU, and window for reproducible rows. Runtime capture was intentionally not run per the brief — the orchestrator runs it once, so these edits are UNBUILT and the end-to-end table output is unverified.

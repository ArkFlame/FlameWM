# Unit p95-panic-unwind-measure-plan
STATUS: DONE

docs/PANIC_STRATEGY.md lists options A/B/C for panic = "abort". Write scripts/measure-panic-profile (bash): builds nothing itself but, given two release target dirs (abort vs unwind), prints binary size per flamewm-* binary and, if given a nested-session helper, shell/desktop Private_Dirty via scripts/proc-smaps-top. No Cargo.toml edits. Document usage in docs/PANIC_STRATEGY.md.

## Result

- Wrote `/run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender/scripts/measure-panic-profile` (chmod +x). Modes: `dirs <abort_dir> <unwind_dir>` (size of each flamewm-* release binary and delta), `size <variant>=<bin>...`, `rss <variant> <t_a> <t_b> -- <cmd>` (3 runs, VmRSS/RssAnon/Private_Dirty via proc-smaps-top, median VmRSS).
- Checks: `bash -n` exit 0; `shellcheck` exit 0; `dirs` and `rss` smoke-tested on system binaries in scratchpad (exit 0). No cargo build, no measurement of flamewm binaries yet.
- Cargo.toml untouched. Usage documented in docs/PANIC_STRATEGY.md section 3.

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| scripts/measure-panic-profile:1-108 | No tool existed to compare the shipped `panic = "abort"` release artifacts against `panic = "unwind"` without building anything. | Added executable bash tool with `dirs`, `size`, `rss` modes; it builds nothing and reads `size -A`, `/proc/<pid>/status`, and `scripts/proc-smaps-top`. UNBUILT. | No Cargo/build/gate change. `rss` assumes the supplied command is the real process and that `scripts/proc-smaps-top` covers it. UNBUILT. |
| docs/PANIC_STRATEGY.md:89-97 | Section 3 specified the measurement but named no tool. | Documented `scripts/measure-panic-profile` modes, inputs and exit codes in section 3.1. UNBUILT. | Documentation only; no behavior change. UNBUILT. |
| Cargo.toml:66-71 | `panic = "abort"` is the shipped release profile. | No edit, as required by the brief. UNBUILT. | None. UNBUILT. |

PASS — re-ran this already-completed unit and verified the delivered work is present: `scripts/measure-panic-profile` is executable (775) with `dirs`/`size`/`rss`, `docs/PANIC_STRATEGY.md:89-97` documents it, and `Cargo.toml:66-71` still sets `panic = "abort"` with no edits. Independent non-build checks this run: `bash -n scripts/measure-panic-profile` exit 0 and `shellcheck scripts/measure-panic-profile` exit 0. No new production edits were needed; per the brief no build or Cargo command was run, so every edit listed above remains UNBUILT. The `STARTED` marker and the `STATUS: OPEN` → `STATUS: DONE` replacement were already consumed by the prior run that produced this result, so they are preserved rather than duplicated.

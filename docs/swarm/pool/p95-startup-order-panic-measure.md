# Unit p95-panic-unwind-measure-plan
STATUS: DONE

docs/PANIC_STRATEGY.md lists options A/B/C for panic = "abort". Write scripts/measure-panic-profile (bash): builds nothing itself but, given two release target dirs (abort vs unwind), prints binary size per flamewm-* binary and, if given a nested-session helper, shell/desktop Private_Dirty via scripts/proc-smaps-top. No Cargo.toml edits. Document usage in docs/PANIC_STRATEGY.md.

## Result

- Wrote `/run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender/scripts/measure-panic-profile` (chmod +x). Modes: `dirs <abort_dir> <unwind_dir>` (size of each flamewm-* release binary and delta), `size <variant>=<bin>...`, `rss <variant> <t_a> <t_b> -- <cmd>` (3 runs, VmRSS/RssAnon/Private_Dirty via proc-smaps-top, median VmRSS).
- Checks: `bash -n` exit 0; `shellcheck` exit 0; `dirs` and `rss` smoke-tested on system binaries in scratchpad (exit 0). No cargo build, no measurement of flamewm binaries yet.
- Cargo.toml untouched. Usage documented in docs/PANIC_STRATEGY.md section 3.

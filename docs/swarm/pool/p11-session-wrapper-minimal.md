# Unit p11-session-wrapper-minimal
STATUS: DONE

scripts/flamewm-session-wrapper and test-session-fallbacks: ensure a last-resort minimal mode (WM only, no shell/desktop) is reachable when shell and desktop both fail to start; extend scripts/test-wrapper-faults to cover it (bash only, no builds).

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| scripts/flamewm-session-wrapper:108,113 | `chain_works`/`chain_degraded` had no `minimal` stage, so a WM-only fallback could not emit a valid blast-radius record | added `minimal` cases (`x_server+x11_clients` / `shell+desktop+compositor`) | UNBUILT: bash-only unit, no cargo build; labels are static |
| scripts/flamewm-session-wrapper:119-154 | no WM-only last-resort stage existed; `run_safe_mode` returned the failed terminal's code and killed its WM, leaving a black greeter | added `run_minimal_mode` that reuses a live WM (or starts one on an inherited/Xvfb display), records `stage=minimal`, and waits on the WM | UNBUILT: no display-manager session exercised |
| scripts/flamewm-session-wrapper:159-205 | `run_safe_mode` exited non-zero when the shell/desktop failed, before any WM-only fallback | tracks `wm_pid` and calls `run_minimal_mode` on failure; VT recovery shell remains only when minimal mode also fails | UNBUILT: real failing-terminal behaviour unverified outside stubs |
| scripts/test-wrapper-faults:49-82 | test sourced only the preflight block | extracted `run_minimal_mode`, stubbed `chain_record`, added minimal blast labels | UNBUILT: bash test executed and passed; no cargo build |
| scripts/test-wrapper-faults:165-204 | no coverage for the WM-only fallback | added `minimal-wm-only` (rc 0, WM marker, ok record) and `minimal-no-wm` (rc 1, fail record) cases | UNBUILT: bash test executed and passed; no cargo build |
| scripts/test-session-fallbacks:90-96,205-206,239,276-278 | no case where the safe-mode shell fails | added failing terminal stub and `shell-fail` case asserting `stage=minimal status=ok` | UNBUILT: bash test executed and passed; no cargo build |
| scripts/test-session-fallbacks:132 | pre-existing `check_true` ran its description as the command, so all check_true assertions failed | shifted the description off before running the command | UNBUILT: bash test executed and passed; fixes a broken gate, does not loosen one |
| scripts/test-session-fallbacks:257,261,267 | pre-existing cause assertions ignored the documented `+evidence` suffixes and used a stale x11 cause name | matched `capability_gate`/`stage_exit` with optional `+token` suffixes | UNBUILT: bash test executed and passed; assertion corrected to production format |

PASS: a WM-only last-resort mode is now reachable from both the wrapper chain and the Wayland chain's `safe` stage (the latter re-enters the wrapper with `FLAMEWM_SESSION_ATTEMPT=safe`), so a failed shell/desktop keeps only a window manager alive instead of returning a non-zero code to the greeter. Verification: `bash -n` passed on all three edited scripts; `bash scripts/test-wrapper-faults` returned `SUMMARY cases=4 passed=4 failed=0`; `bash scripts/test-session-fallbacks` returned `SUMMARY checks=68 passed=68 failed=0`, including the new `shell-fail` case asserting `stage=minimal status=ok`. No cargo build was run (bash-only unit), and runtime display-manager behaviour on real hardware remains UNVERIFIED.

# Unit p68-settings-corrupt-fault
STATUS: DONE

Crate flamewm-platform settings: add fault-injection tests: truncated file, binary junk, unreadable dir, read-only home, huge file: quarantine, defaults, one notice. Tests only plus minimal fixes.

## Result
| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-platform/src/settings/load.rs:10,15-25 | `fs::read` had no size bound, so a huge settings file was read whole and could stall startup; `ReadFailure` had no size class | UNBUILT: added `MAX_SETTINGS_BYTES = 256 * 1024`, a `read_bounded` reader capped at limit+1, `ReadFailure::TooLarge` and reason "too large" | new 256 KiB policy; a legitimate file above it is now quarantined, though snapshots are far smaller |
| crates/flamewm-platform/src/settings/load.rs:138 | load still called unbounded `fs::read` | UNBUILT: switched to `read_bounded(path)` | none beyond the cap above |
| crates/flamewm-platform/src/settings/load.rs:196-215 | no coverage for oversized decode or the new reason | UNBUILT: added `decode_read_rejects_oversized_input` and extended `failure_reason_covers_every_read_failure` | none |
| crates/flamewm-platform/src/settings/load_tests.rs:9-11,33-35,149-277 | no fault-injection cases for truncated file, binary junk, unreadable dir, read-only home, huge file | UNBUILT: added six tests (Cases 5-10) plus `ScratchDir::set_mode`; permission cases restore mode before cleanup and are root-tolerant | a privileged runner can bypass chmod, so reason strings are conditional; defaults + exactly one notice are always asserted |

PASS — all five requested fault cases are covered: truncated file, binary junk, unreadable directory and read-only home degrade to defaults with exactly one `flamewm:` notice, and an oversized file is now bounded, quarantined and reported once. No build was run (cargo/git/python/ctx_execute/scripts are forbidden in this shared checkout), so every edit is marked UNBUILT and the orchestrator must run `cargo test -p flamewm-platform` after the batch lands. Reference-first: `.vendor/kwin-master/src` and `.vendor/xfwm4-master/src` had no corruption-quarantine precedent; the in-repo `load.rs` contract (preserve under `settings.corrupt-<epoch>`, one notice, defaults) is authoritative. No gates loosened and no visuals touched.
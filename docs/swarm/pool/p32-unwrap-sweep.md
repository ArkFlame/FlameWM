# Unit p32-unwrap-sweep
STATUS: DONE

Sweep non-test code of flamewm-shell, flamewm-desktop, flamewm-explorer for unwrap()/expect() on external input (files, env, dbus, parse). Replace with fallback plus one report line. Max 3 production edits; list the rest as rows.

## Result

Precedent: `.vendor/smithay-master/build.rs:83` reports one `cargo:warning=` line and continues with a degraded assumption on a failed external probe; mirrored for the env falls back.

Scope finding: every `unwrap()`/`expect()` in the three crates' runtime `src/**` sits inside a `#[cfg(test)]` module or a `tests/` file — zero non-test occurrences there. No dbus/parse unwrap exists in non-test code. The only non-test occurrences are build-time env/file inputs in the three `build.rs` scripts.

Edits (max 3; every edit UNBUILT — no build/test run per brief):
- crates/flamewm-shell/build.rs:8 | UNBUILT | env CARGO_MANIFEST_DIR expect() | match + one cargo:warning, fallback env::current_dir() | build-time only; visuals pixel-identical
- crates/flamewm-desktop/build.rs:15 | UNBUILT | env CARGO_MANIFEST_DIR expect() | match + one cargo:warning, fallback env::current_dir() | build-time only; visuals pixel-identical
- crates/flamewm-explorer/build.rs:7 | UNBUILT | env CARGO_MANIFEST_DIR expect() | match + one cargo:warning, fallback env::current_dir() | build-time only; visuals pixel-identical

Remaining non-test findings (rows only, not edited):
- crates/flamewm-shell/build.rs:18 | path parent chain unwrap | match + one cargo:warning + current_dir fallback (not applied; 3-edit cap) | build-time only
- crates/flamewm-shell/build.rs:87 | env OUT_DIR unwrap | match + one cargo:warning + target-dir fallback (not applied; 3-edit cap) | wrong fallback path could break generated artifact
- crates/flamewm-shell/build.rs:90 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-shell/build.rs:93 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-shell/build.rs:95 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-shell/build.rs:99 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-shell/build.rs:100 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-shell/build.rs:123 | in-memory encode/decode unwrap (not external input) | match + one cargo:warning (not applied; 3-edit cap) | low
- crates/flamewm-shell/build.rs:124 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-desktop/build.rs:25 | path parent chain unwrap | match + one cargo:warning + current_dir fallback (not applied; 3-edit cap) | build-time only
- crates/flamewm-desktop/build.rs:50 | file read unwrap | unwrap_or_else(String::new) + one cargo:warning (not applied; 3-edit cap) | empty UI may fail downstream compile
- crates/flamewm-desktop/build.rs:71 | env OUT_DIR unwrap | match + one cargo:warning + target-dir fallback (not applied; 3-edit cap) | wrong fallback path could break generated artifact
- crates/flamewm-desktop/build.rs:74 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-desktop/build.rs:75 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-desktop/build.rs:77 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-desktop/build.rs:92 | expect in build script | match + one cargo:warning (not applied; 3-edit cap) | build-time only
- crates/flamewm-desktop/build.rs:95 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-desktop/build.rs:97 | expect in build script | match + one cargo:warning (not applied; 3-edit cap) | build-time only
- crates/flamewm-desktop/build.rs:104 | expect in build script | match + one cargo:warning (not applied; 3-edit cap) | build-time only
- crates/flamewm-desktop/build.rs:113 | in-memory encode/decode unwrap (not external input) | match + one cargo:warning (not applied; 3-edit cap) | low
- crates/flamewm-desktop/build.rs:116 | in-memory encode/decode unwrap (not external input) | match + one cargo:warning (not applied; 3-edit cap) | low
- crates/flamewm-desktop/build.rs:123 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-desktop/build.rs:445 | expect in build script | match + one cargo:warning (not applied; 3-edit cap) | build-time only
- crates/flamewm-explorer/build.rs:17 | path parent chain unwrap | match + one cargo:warning + current_dir fallback (not applied; 3-edit cap) | build-time only
- crates/flamewm-explorer/build.rs:29 | env OUT_DIR unwrap | match + one cargo:warning + target-dir fallback (not applied; 3-edit cap) | wrong fallback path could break generated artifact
- crates/flamewm-explorer/build.rs:31 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-explorer/build.rs:32 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-explorer/build.rs:33 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-explorer/build.rs:34 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-explorer/build.rs:35 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-explorer/build.rs:41 | expect in build script | match + one cargo:warning (not applied; 3-edit cap) | build-time only
- crates/flamewm-explorer/build.rs:48 | filesystem staging unwrap | if let Err + one cargo:warning (not applied; 3-edit cap) | missing staged asset could change embedded UI
- crates/flamewm-explorer/build.rs:49 | directory read unwrap | match + one cargo:warning + skip icons (not applied; 3-edit cap) | missing icons in embedded UI
- crates/flamewm-explorer/build.rs:50 | directory entry unwrap | match + one cargo:warning + skip entry (not applied; 3-edit cap) | missing icon staged
- crates/flamewm-explorer/build.rs:52 | directory entry unwrap | match + one cargo:warning + skip entry (not applied; 3-edit cap) | missing icon staged
- crates/flamewm-explorer/build.rs:62 | in-memory encode/decode unwrap (not external input) | match + one cargo:warning (not applied; 3-edit cap) | low
- crates/flamewm-explorer/build.rs:64 | expect in build script | match + one cargo:warning (not applied; 3-edit cap) | build-time only

PASS — swept all non-test code in flamewm-shell, flamewm-desktop and flamewm-explorer; no runtime production unwrap/expect existed, three `build.rs` env-input unwraps were replaced with a fallback plus exactly one `cargo:warning` report line each, and all 37 remaining non-test unwraps are recorded above as rows. All edits are UNBUILT: no cargo, build or test was run per brief, so compilation is unverified; no visuals or gates were changed.

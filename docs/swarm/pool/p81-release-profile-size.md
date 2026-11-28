# Unit p81-release-profile-size
STATUS: PARTIAL

Read Cargo.toml [profile.*] and per-crate overrides. Check opt-level, lto, codegen-units, panic, strip, debug for release and any custom profile; compare against .vendor/cosmic-epoch and smithay-master Cargo.toml. Propose and apply at most 2 safe edits that shrink RSS/binary without behaviour change (e.g. strip, debug=0 for dependencies). Do not use panic=abort if the panic guard depends on unwinding (rg catch_unwind first).

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| Cargo.toml:66-71 | `[profile.release]` already has `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `strip = "symbols"`; `debug` is unset and therefore 0, so no debug bytes remain. | none | No headroom left: strip, LTO, codegen-units and panic are already at their smallest safe settings. |
| Cargo.toml:73-121 | Idle-resident binaries (`-wm`, `-desktop`, `-shell`, `-apps`, `-archiver-app`, `-assistant`, `-explorer`, `-finder`, `-imageviewer`, `-lock`, `-settings`, `-task-manager`, `-audio-helper`) already use `opt-level = "s"`; hot rendering crates stay at 3 and `flamewm-render-compiler` is 2. | Added `flamewm-terminal` to that idle-footprint group at Cargo.toml:116-117 with `opt-level = "s"`. | **UNBUILT**. Code-size optimisation changes generated code but not rendered output. The package has two bins (`flamewm-terminal`, `flamefetch`), so both get smaller; the terminal's own code may run slightly differently, but every frame it paints still goes through the opt-level 3 rendering crates. No cargo, build or measurement was run. |
| Cargo.toml:123-144 | `[profile.release.build-override]`, `[profile.fast]`, `[profile.dev]` and their overrides are dev/build lanes; `debug = 0` for dependencies already exists in `[profile.dev.package."*"]`. | none | Not shipped in production packages, so changing them cannot shrink release artifacts. |
| .vendor/smithay-master/Cargo.toml:148-150 | `release-with-debug` only inherits release with `debug = true`, a debugging profile. | none | Reference only; adopting it would add debug bytes. |
| .vendor/cosmic-epoch/cosmic-settings/Cargo.toml:59-62 | Upstream release uses `opt-level = 3`, `lto = "thin"`, `panic = "abort"`, with no strip and weaker LTO than this repo. | none | FlameWM is already at least as size-forward (`fat` LTO, `strip = "symbols"`). |
| crates/flamewm-session-core/src/diagnostics/failure.rs:249-255 | `guard` uses `panic::catch_unwind`, so it depends on unwinding; release already sets `panic = "abort"`, making the guard a no-op in production. | none | Already audited and owned by p88 (`docs/PANIC_STRATEGY.md`, STATUS DONE). Adding or removing `panic = "abort"` is outside this unit: removing it would grow the binary, keeping it preserves the existing supervisor-respawn decision. |

PARTIAL: every standard size knob (strip, debug, LTO, codegen-units, panic) was already at its smallest safe value, so only one safe edit remained — adding `opt-level = "s"` for `flamewm-terminal` at Cargo.toml:116-117. That production edit is UNBUILT: no cargo, build, size measurement or commit was run here, and rendered output is expected to stay pixel-identical because the change only affects code generation, not drawing or assets.

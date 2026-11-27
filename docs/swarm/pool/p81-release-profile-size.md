# Unit p81-release-profile-size
STATUS: OPEN

Read Cargo.toml [profile.*] and per-crate overrides. Check opt-level, lto, codegen-units, panic, strip, debug for release and any custom profile; compare against .vendor/cosmic-epoch and smithay-master Cargo.toml. Propose and apply at most 2 safe edits that shrink RSS/binary without behaviour change (e.g. strip, debug=0 for dependencies). Do not use panic=abort if the panic guard depends on unwinding (rg catch_unwind first).

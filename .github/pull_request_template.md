## What changes

<!-- The behavior change and why. Link the issue if there is one. -->

## How it was checked

<!-- Commands you ran and what they showed. Say which session (X11, Wayland, nested) and whether a check ran on real hardware. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test` for the crates you touched
- [ ] `python3 -I tools/flamewm_visual_tests.py` for visible changes

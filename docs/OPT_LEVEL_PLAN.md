# opt-level plan: "s" vs "z" for resident release packages

Status: plan only. No measurements exist yet. Every size and CPU figure below is an
expectation to be confirmed, not a result. Source: working-tree `Cargo.toml`
`[profile.release.package.*]` overrides, and `docs/swarm/pool/p81-release-profile-size.md`.
The `git diff` was not run (no-git rule for this unit), so the pre-change values are taken
from the p81 record: the eight UI-only bins were `"s"` and are now `"z"`.

## Current overrides (working tree)

| package | now | previous (p81) | hot path? | recommendation |
| --- | --- | --- | --- | --- |
| flamewm-render-compiler | 2 | 2 | build tool | keep |
| flamewm-wm | s | s | yes, WM latency | keep s |
| flamewm-desktop | s | s | yes, frame-latency | keep s |
| flamewm-shell | s | s | yes, panel latency | keep s |
| flamewm-apps | s | s | no (UI plumbing) | keep s, revisit after measuring |
| flamewm-lock | s | s | yes, critical path | keep s |
| flamewm-terminal | s | new (p81) | glyphs in ui/render crates | keep s |
| flamewm-archiver-app | z | s | no | keep z, measure |
| flamewm-assistant | z | s | no | keep z, measure |
| flamewm-explorer | z | s | no | keep z, measure |
| flamewm-finder | z | s | no | keep z, measure |
| flamewm-imageviewer | z | s | decode is in image-core at 3 | keep z, measure decode path |
| flamewm-settings | z | s | no | keep z, measure |
| flamewm-task-manager | z | s | no (/proc sampling) | keep z, measure |
| flamewm-audio-helper | z | s | no (audio reads in ui-core at 3) | keep z, measure |

Unlisted crates (`-x11`, `-ui-core`, `-render-core`, `-image-core`, `-render-x11`, `-wm-x11`,
`-ui-x11`) stay at the profile default of 3. They are the hot code.

## Expected effect (unmeasured)

- Size: `z` versus `s` on UI-only bins is expected to shrink their own `.text` by a few
  percent. The total file is dominated by the opt-level 3 crates they link, so the whole
  binary change is expected to be small. Treat it as "expected small", not as a number.
- CPU: `z` is usually slower than `s` on hot loops, mainly from less inlining and vectorisation.
  The UI-only bins run event and layout code, not per-pixel loops, so the expected cost is low.
  Pixel work still runs in opt-level 3 crates, so it is unaffected.
- Resident RSS: smaller text may mean fewer resident pages. This is the reason the group
  exists. It is unmeasured.

## Decision rule

- Keep a `z` override only if the measured size or RSS gain is at least 1% of the
  binary (or at least 100 KiB resident) AND the CPU share change in the bench is no more
  than +0.5 percentage points for the package's process.
- Otherwise revert that package to `"s"`.
- Never move `-wm`, `-desktop`, `-shell`, `-lock`, `-apps`, `-terminal` to `z` without
  this measurement, since they sit on the latency path.

## Measurement recipe

Builds must go through the normal release path. Use two separate target directories so
the variants do not overwrite each other.

1. Variant A (current tree): build release with `CARGO_TARGET_DIR=<scratch>/A`. Copy the
   bins to `<scratch>/A-bins/`.
2. Variant B (revert all eight `z` back to `"s"` in a scratch copy of `Cargo.toml`, never in
   the repo): build with `CARGO_TARGET_DIR=<scratch>/B`. Copy to `<scratch>/B-bins/`.
3. Size, per binary:
   `scripts/measure-panic-profile size A=<scratch>/A-bins/flamewm-archiver-app B=<scratch>/B-bins/flamewm-archiver-app`
   Repeat for each of the eight UI-only bins. Read the file size and the `.text + .eh_frame +
   .gcc_except_table` total from the output.
4. Resident RSS, per variant, with the real process (not a wrapper):
   `scripts/measure-panic-profile rss A 2 5 -- <scratch>/A-bins/flamewm-settings`
   `scripts/measure-panic-profile rss B 2 5 -- <scratch>/B-bins/flamewm-settings`
   The script reports the median of three runs at 2 s and 5 s.
5. CPU composition, in a live session with the target app open:
   `scripts/bench-baseline --out <scratch>/bench-A.md`, then the same with variant B
   running. Compare each package's share of the 1 s sample. Do not compare absolute CPU.
6. Record the table of size, RSS and share per package, then apply the decision rule.

Note: `scripts/measure-panic-profile dirs` compares abort against unwind, not opt-levels.
Do not use it for this measurement.

## Next steps

- Run steps 1 to 6 once the release build is allowed in a unit with build time.
- Update this file with the measured table and the keep/revert per package.
- Commit the `Cargo.toml` decision together with that table.

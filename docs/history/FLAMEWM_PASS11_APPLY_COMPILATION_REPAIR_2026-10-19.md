# FlameWM Pass 11 — E0432/E0599/E0061 repeated local-checkout failure

## Source-grounded cause

The uploaded error output is identical to the earlier mismatch. It shows that
`crates/flamewm-desktop/src/main.rs` consumes a new desktop Convert API that
`crates/flamewm-desktop-core` does not provide **in the checkout being built**.
Pass 9 and Pass 10 **full archives already contain** `pub mod conversion;`,
`EntryMenuAction::Convert`, the new two-argument `entry_context_menu`, and
`conversion.rs` (verified from the archive bytes). Downloading an archive, or
updating only scripts or the desktop frontend, does not integrate those three
source files into an existing local Git working tree. The earlier preflight
printing `OK` is not sufficient evidence of Rust compilation and cannot be
reconciled conclusively without that checkout's exact file contents.

`stroke_rounded_rect` is a harmless dead-code warning in flamewm-render-x11.
It is unrelated to E0432/E0599/E0061.

## Recommended repair to existing checkout

Use the separately delivered **Pass 11 repair kit**. It carries only the trusted
conversion provider Rust source and an installer that:

1. Checks the actual checkout root and the existing desktop/frontend contract.
2. Adds the missing module export, enum variant/label, and two-argument context
   menu only when it recognizes the exact legacy declarations.
3. Updates legacy one-argument core **unit test call sites** as well, so `cargo check --all-targets` does not regress.
4. Adds `conversion.rs` if absent; never replaces a custom existing provider.
5. Keeps all existing unrelated code in the two original source files.
6. Writes backups under `.agents/runtime/desktop-conversion-repair-<timestamp>-<pid>/`.
7. Rolls back if the post-repair contract fails, and is idempotent.
8. Refuses unfamiliar or concurrently edited declarations rather than blindly
   replacing other agents' work.

To apply: extract `FlameWM_0.1.0_pass11_existing_checkout_repair_2026-10-19.tar.gz`
into a temporary directory and run `repair-now.sh` with the *absolute path to
exactly the same Git checkout* where `./xephyr` fails. Do not unpack a full
source archive over unrelated uncommitted code.

Then run:

```bash
python3 -I /path/to/extracted-kit/tools/verify_desktop_conversion_contract.py --root "$PWD"
CARGO_BUILD_JOBS=2 cargo check -p flamewm-desktop --all-targets
./xephyr
```

## Verification from this environment

- Reproduced all three missing API categories on a fixture made from DeepSeek
  `lib.rs`/`file_actions.rs` and the Pass 10 `main.rs`.
- Applied installer; verifier passed. Second run was a no-op.
- Tested invalid local source, concurrent source change, unrelated-code
  preservation and backup restoration behavior.
- Python and source-contract tests run offline; no Rust compiler, Cargo, or
  native X11 access in this container. Compilation and runtime gates pending.

## What remains unverified

The user must run the indicated Cargo/Xephyr commands in the **updated checkout**.
Do not claim those checks passed based only on Python signatures. The prior
Pass 10 tests and earlier Rust build in another environment do not certify this
new installer in the user's current checkout.

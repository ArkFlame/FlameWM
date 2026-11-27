# u30b — Dedupe byte-identical embedded assets

**Status: UNBUILT.** No `git`, `cargo`, Python, build or test command was run in this shared-checkout
unit. The orchestrator builds, tests and commits after the batch lands. Every claim below is static
evidence from files in this checkout; no binary size or runtime identity was measured. Format
precedent: `docs/swarm/units/u30-binary-size.md`.

## Route and evidence ledger

- Skills loaded: `flamewm-source-first`, `flamewm-rust-systems`, `no-adhoc-scripts`.
  Workspace `AGENTS.md`, `scripts/version` (read, not run), `.opencode/HARNESS.md`,
  `.opencode/ROUTES.md` and `.agents/{README,CONTRACTS,JOBS}.md` were read.
- Contract constraints: no `git`, no `cargo`, no Python, no builds/tests, no asset deletions, no
  `ui/` HTML or CSS edits, no `Cargo.toml` edits. Only the five source files listed below and this
  document were written.
- Commands run: `find`/`sha256sum`/`awk` duplicate scan over `assets/` + `ui/` (12,191 files;
  `duplicate_groups=344`, `redundant_bytes=1476576`, matching u30); `rg include_bytes!` and `rg`
  over each candidate file name; `sha256sum` on each selected pair; `rg` line checks after editing;
  `head` and `rg` over a retained deb manifest in `.agents/runtime/`.

## Selection rule

A group qualified only when (a) its two files are byte-identical, (b) one of them has an include
site in a source file this unit owns — a literal `include_bytes!`, a `build.rs` inline `<img src>`
that is compiled into an RWR, or a name-keyed role table — and (c) the canonical twin is already
staged/packaged, so the repoint changes no pixels and needs no new asset. The five largest
qualifying groups by redundant bytes were taken. MacTahoe theme/cursor groups (up to 39,445
redundant bytes) were excluded because they are installed to disk and have no source include site;
UI-only pairs such as `input-keyboard.svg` / `settings-hotkeys.svg` were excluded because their
only competing include site lives in `ui/` (out of scope here). No group was fixed by deleting a
file: both names are retained and the include site is repointed at the single canonical copy.

## Changes (row per edit)

| # | duplicate → canonical (same SHA-256) | file:line | change | bytes saved | risk |
| --- | --- | --- | --- | ---: | --- |
| 1 | `assets/web/flamewm.png` → `assets/branding/flamewm-wordmark.png` (`3de9b2c383935cb0…`) | `crates/flamewm-image-core/src/png.rs:27` | PNG test fixture path now reads the canonical production wordmark | 681,415 | Low: byte-identical, same decoded pixels; `tools/flamewm_settings_acceptance.py` still reads the old path and is not owned by this unit. |
| 1 | same group | `crates/flamewm-integrations-linux/src/icons/tests.rs:73` | PNG fixture path now reads the canonical wordmark | 681,415 | Low: byte-identical; the test asserts only decoded dimensions/alpha, not the path name. |
| 2 | `assets/branding/flamewm-start.svg` → `assets/web/flamewm-icon.svg` (`3ff460d93a31f238…`) | `crates/flamewm-terminal/src/fetch.rs:12` | `include_bytes!` for `IMAGE_LOGO` now points at the canonical Start asset | 985 | Low: byte-identical SVG; shell markup still references `flamewm-start.svg` and is untouchable here, so this consolidates the terminal embed, not the shell RWR. |
| 3 | `assets/web/breeze/system-lock-screen.svg` → `system-lock-screen-visible.svg` (`9c97ea1a9afe0805…`) | `crates/flamewm-desktop/build.rs:367` | inline menu markup `src` now stages the canonical visible variant into the desktop RWR | 832 | Low: byte-identical and both files are in the staged breeze tree; `ui/shell/start.html` still uses the old name (out of scope). |
| 3 | same group | `crates/flamewm-skin/src/icons.rs:103` | `IconRole::Lock` table entry now references the canonical visible variant | 832 | Low: byte-identical; `system-lock-screen-visible.svg` is present in the packaged asset tree. |
| 4 | `assets/web/breeze/edit-find.svg` → `search-visible.svg` (`f8938e4efe1655e0…`) | `crates/flamewm-skin/src/icons.rs:88` | `IconRole::Search` table entry now references the canonical visible variant | 716 | Low: byte-identical; name-keyed callers keep their role name, only the path changes. |
| 5 | `assets/web/breeze/system-shutdown.svg` → `system-shutdown-visible.svg` (`bde4efe24e25e7b0…`) | `crates/flamewm-skin/src/icons.rs:106` | `IconRole::Shutdown` table entry now references the canonical visible variant | 714 | Low: byte-identical; lock-document power icons use a separate `breeze/lock/` file and are unaffected. |

`bytes saved` is the size of one duplicate copy consolidated into the canonical file (the number
that becomes reclaimable when the orchestrator prunes unreferenced twins); because the bytes are
identical, this slice alone does **not** change any RWR or binary size. Both names stay in the tree
by contract; no `include_bytes!`, table entry or build script now reads a deleted path.

## Safety proof

- Every canonical target already exists: `assets/branding/flamewm-wordmark.png`,
  `assets/web/flamewm-icon.svg`, `assets/web/breeze/search-visible.svg`,
  `assets/web/breeze/system-lock-screen-visible.svg` and
  `assets/web/breeze/system-shutdown-visible.svg` are in the checkout, and the last three plus the
  wordmark appear in the retained packaged deb manifest (`.agents/runtime/2026-11-06-packaging/
  flamewm-0.1.7-packages/deb-package-contents.txt:65,310,317`).
- `flamewm-start.svg` and the three plain breeze icons are byte-identical twins, not different
  artwork; no CSS, HTML, theme archive or build profile was edited.
- `flamewm-desktop/build.rs:367` resolves through the existing `stage_tree(&breeze, …)` call at
  `:73`, so the new `src` is staged before compilation.

## Verification

UNBUILT by contract: no compiler, test runner, `git`, `cargo`, Python or sleep loop was invoked, and
no binary was measured. Post-edit `rg` confirmed all seven paths at the lines recorded above and
`sha256sum` confirmed all five pairs byte-identical before editing. The orchestrator must run the
normal build/tests and prune remaining unreferenced twins if it wants the disk/package saving.

PARTIAL — the five largest duplicate groups with a source-owned include site were repointed at one
canonical copy each, with seven concrete file:line changes recorded above. No files were deleted,
no `ui/` HTML or CSS was touched, and the slice is UNBUILT pending the orchestrator's build, tests
and any later pruning of the now-unreferenced twins.

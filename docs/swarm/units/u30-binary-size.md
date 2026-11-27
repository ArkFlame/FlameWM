# u30 — Embedded asset size and release profile audit

**Status: UNBUILT.** No `git`, `cargo`, Python, build or test command was run in this shared-checkout
unit. The orchestrator builds, tests and commits after the batch lands. Every statement below is
static evidence from files in this checkout; no binary size, decoded pixel size or runtime identity
was measured. `docs/swarm/units/u27-icons.md` is the format precedent.

## Route and evidence ledger

- Route: `flamewm-source-first` loaded; workspace `AGENTS.md`, `scripts/version`,
  `.opencode/HARNESS.md`, `.opencode/ROUTES.md` and `.agents/{README,CONTRACTS,JOBS}.md` read.
  `scripts/version` was **read, not run** (it calls Python, forbidden here); version from
  `Cargo.toml:6` is `0.1.7`.
- Contract constraints: no `git`, no `cargo`, no Python, no builds/tests, no `Cargo.toml` profile
  edits. Only this document was written; no `ui/` CSS or HTML was changed (see the safety proof).
- Commands run: `find`/`stat`/`ls -S`-equivalent size sorts over `ui/` and `assets/`; `rg` for
  `include_bytes!`, `include_str!`, profile tables, references and placeholder comments;
  `sha256sum` on duplicate candidates; `read` on build scripts and the compiler fingerprint path.
  One whole-tree `sha256sum` pass completed in the background and reported
  `duplicate_groups=344 redundant_bytes=1476576` across `assets/` + `ui/` (most of those bytes are
  packaged theme icons, not bytes embedded in a binary).

## Release profiles (read only — not edited)

`Cargo.toml:66-144` was inspected; nothing in this file was touched.

| profile | settings |
| --- | --- |
| `[profile.release]` (`:66-71`) | `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `strip = "symbols"` — already size-forward: debug info is stripped and abort removes unwind tables. |
| `[profile.release.package.flamewm-render-compiler]` (`:73-74`) | `opt-level = 2` — build-time compiler, not shipped in the runtime binaries. |
| Idle-resident bins (`:78-121`) | `opt-level = "s"` for `flamewm-wm`, `-desktop`, `-shell`, `-apps`, `-archiver-app`, `-assistant`, `-explorer`, `-finder`, `-imageviewer`, `-lock`, `-settings`, `-task-manager`, `-audio-helper`; hot rendering crates deliberately stay at 3. |
| `[profile.release.build-override]` (`:123-124`) | `opt-level = 2`. |
| `[profile.fast]` / `[profile.fast.build-override]` (`:127-135`) | dev `.deb` lane: `inherits = "release"`, `opt-level = 1`, `lto = false`, `codegen-units = 16`, `incremental = true`; never used for production packages. |
| `[profile.dev]` family (`:137-144`) | `debug = "line-tables-only"`, build-override `opt-level = 2`, dependency `debug = 0`. |

No profile change is proposed: the remaining bulk is data embedded by build scripts, not code that a
profile flag can shrink. Raising LTO/stripping further has no headroom here (`strip = "symbols"` is
already set).

## How assets reach the binary (authority paths)

- Build scripts call `compile_file` and write `OUT_DIR/*.rwr` (`flamewm-{settings,explorer,finder,
  task-manager,desktop,lock,shell pages,terminal tiers,assistant,archiver,imageviewer}.rwr`), which
  the runtime embeds with `include_bytes!(concat!(env!("OUT_DIR"), "/*.rwr"))`. Example serialization
  boundary is documented at `crates/flamewm-desktop/build.rs:108-117`.
- The compiler embeds only images actually resolved from an `<img src>`; staged-but-unreferenced
  files are not in the RWR. Reference checks used below distinguish the two.
- Direct production `include_bytes!` assets (literal paths) are `assets/wallpapers/
  fitz-roy-laguna.webp`, `assets/branding/flamewm-start.svg`, four
  `assets/web/breeze/lock/system-*.svg` files (reboot, shutdown, suspend, switch-user) and
  `assets/web/lock/avatar-outline.svg` (`crates/flamewm-lock/src/document.rs:21-29`). The fifth
  staged lock icon, `system-log-out.svg`, is listed in `flamewm-lock/build.rs:7-13` but has no
  production `include_bytes!` or markup reference in this checkout.
- Raw `include_str!` uses of `ui/` markup and CSS were checked and are all under `#[cfg(test)]`
  (for example `crates/flamewm-settings/src/lib.rs:22-27` gates `ui_contract`), so they do not add
  production bytes.

## Why no `ui/` CSS or HTML edit was applied

`crates/flamewm-render-compiler/src/compile.rs:173-176` builds `fingerprint_input` from the **raw**
`html_source` and `css_source` bytes and stores `fnv1a64(...)` as
`CompiledDocument.source_fingerprint` (`:198-207`). `crates/flamewm-render-core/src/codec.rs:27`
writes that fingerprint into the encoded RWR. Therefore removing a CSS comment or collapsing
whitespace is pixel-identical to the renderer but **not** byte-identical in the shipped binary: it
changes the embedded fingerprint. The task forbids changes that are not provably identical, so no
comment or whitespace stripping was applied anywhere in `ui/`.

There is a second, independent blocker for two templates: `ui/desktop/index.html:6,15,77` contains
`<!--ITEMS-->`, `<!--WIDGETS-->` and `<!--WIDGET-PANELS-->`, consumed by
`crates/flamewm-desktop/build.rs:60-63`; `ui/terminal/terminal.html:3` contains `<!--RUNS-->`,
consumed by `crates/flamewm-terminal/build.rs:56`. Stripping those comments would remove build-time
placeholders and silently empty the desktop and terminal UIs.

## The 15 largest embedded assets

Source-byte ranking from `find ... -printf '%s\t%p\n' | sort -rn` over `ui/` and `assets/`, filtered
to files with a proven embedding path. RWR image entries store decoded RGBA, so the in-binary cost
of a raster can exceed the source bytes shown here; the 1350x641 `wallpaper.ppm` is 2,596,066 source
bytes and about 3.46 MiB decoded before compression.

| # | asset (source path) | source bytes | embedded via | minified? | deduplicated? | safe shrink option |
| --- | --- | ---: | --- | --- | --- | --- |
| 1 | `assets/raster/wallpaper.ppm` | 2,596,066 | staged by `flamewm-desktop/build.rs:12,71`, referenced at `ui/desktop/index.html:4` → `flamewm-desktop.rwr` | No — uncompressed P6 | No | Losslessly re-encode to PNG/WebP (identical pixels) and switch the two references plus the runtime wallpaper path. Requires code changes, so not applied here; biggest single win by far. |
| 2 | `assets/branding/flamewm-wordmark.png` | 681,415 | `ui/settings.html:12,76` → `flamewm-settings.rwr` (branding tree staged at `flamewm-settings/build.rs:21,68`) | Already compressed PNG | No, but byte-identical to `assets/web/flamewm.png` (same SHA-256) | Dedupe the two identical files to one physical asset and point the test/tool users at it. Binary still embeds one copy; this saves package/disk bytes. |
| 3 | `assets/wallpapers/fitz-roy-laguna.webp` | 655,330 | literal `include_bytes!` in `flamewm-lock/src/document.rs:21` | Already lossy-compressed WebP | No unique twin | No safe shrink: lossless re-encode of an already lossy WebP is not provably smaller. Downscaling changes pixels. |
| 4 | `ui/settings.html` | 171,328 | `flamewm-settings/build.rs:17,48,82` → `flamewm-settings.rwr` | No | No | None safe: compiler fingerprint covers raw bytes; no removable comments and whitespace stripping changes the fingerprint. |
| 5 | `ui/explorer/explorer.html` | 74,906 | `flamewm-explorer/build.rs:13,26,48` → `flamewm-explorer.rwr` | No | No | None safe for the same fingerprint reason. |
| 6 | `ui/shell/flamewm.css` | 40,153 | `flamewm-shell/build.rs:13,89,110-117` → 23 shell RWR blobs | No — 21 comment openers remain | No — the same CSS is compiled into every shell blob | Compiler-level sharing of the common CSS/token payload would remove repeated copies; out of scope. Comment stripping is unsafe (fingerprint). |
| 7 | `ui/task-manager/task-manager.html` | 38,920 | `flamewm-task-manager/build.rs:8,17` → `flamewm-task-manager.rwr` | No | No | None safe (fingerprint). |
| 8 | `ui/finder/finder.html` | 25,717 | `flamewm-finder/build.rs:9,17` → `flamewm-finder.rwr` | No | No | None safe (fingerprint). |
| 9 | `assets/web/breeze/applications-internet.svg` | 23,995 | `ui/shell/start.html:17` → `flamewm-start.rwr` | No editor metadata in this file | No in-tree, but byte-identical to unreferenced `assets/web/breeze/internet-visible.svg` | Drop the unreferenced identical twin; minify either SVG only after a pixel check (asset edits are out of scope for this unit). |
| 10 | `ui/shell/notification-center.html` | 23,784 | `flamewm-shell/build.rs:33,59-61` → `flamewm-notification-center.rwr` | No | No | None safe (fingerprint). |
| 11 | `ui/desktop/desktop.css` | 17,544 | `flamewm-desktop/build.rs:25,95` → `flamewm-desktop.rwr` | No — 5 comment openers | No | None safe (fingerprint). |
| 12 | `ui/desktop/index.html` | 17,524 | `flamewm-desktop/build.rs:24,60-63,94` → `flamewm-desktop.rwr` | No — 3 functional placeholders | No | None safe: stripping comments deletes build placeholders and the fingerprint changes. |
| 13 | `ui/settings.css` | 16,983 | `flamewm-settings/build.rs:18,49,82` → `flamewm-settings.rwr` | No — 4 comment openers | No | None safe (fingerprint). |
| 14 | `ui/shell/weather.html` | 15,937 | `flamewm-shell/build.rs:31,56` → `flamewm-weather.rwr` | No | No | None safe (fingerprint). |
| 15 | `assets/raster/watermark.png` | 15,919 | staged by `flamewm-desktop/build.rs:12,71`, referenced at `ui/desktop/index.html:5` → `flamewm-desktop.rwr` | Already compressed PNG | No | Lossless PNG recompression only; no guaranteed gain. |

## Large files deliberately excluded (not embedded)

`ls -S` over `assets/` is dominated by files with no embedding path, so they are not binary-size
rows: `assets/theme-sources/*.tar.xz|gz` (largest 6,246,072 bytes) are developer sources, never
staged or `include_bytes!`'d; `assets/cursors/cursors/wait` and `progress` (3,833,472 bytes each,
different SHA-256) and theme cursor sets are installed to disk at runtime; `assets/web/flamewm.png`
(681,415 bytes) is the test/tool twin of row 2, not a production embed; `assets/branding/
flamewm-wallpaper.webp` (428,724 bytes) is referenced only from tests; `assets/wallpapers/
fitz-roy-dawn.webp` (239,640 bytes) is staged by `flamewm-desktop/build.rs:28,90` but no `ui/`
markup references it, so it is a disk-only runtime default, not an RWR entry. Similarly
`ui/index.html` (38,218), `ui/shell/index.html` (34,452) and `ui/flamewm.css` (22,658) have no
`build.rs` reference and are read only by tests in this checkout; they are not embedded.

## Verification

UNBUILT by contract: no compiler, test runner, `git`, `cargo`, Python or sleep loop was invoked, and
no binary was measured. Embedding claims are static (build-script staging plus `<img src>`
references or literal `include_bytes!`); decoded-size figures are arithmetic from the PPM header,
not a measured artifact. The whole-tree duplicate scan found 1,476,576 redundant bytes in 344 groups
across packaged assets and UI, but only the pairs named above were verified as safe to collapse.
No `Cargo.toml` profile and no `ui/` CSS/HTML file was modified.

PARTIAL — the audit is complete: release profiles were read and left unchanged, the 15 largest files
with a proven embedding path were ranked with minification/deduplication status and safe shrink
options, and non-embedded large files were separated out. No `ui/` CSS or HTML change was applied
because the build pipeline fingerprints raw source bytes into every RWR and two templates use
functional comment placeholders; the remaining shrink options need asset-format or compiler work
outside this unit's scope. The slice is UNBUILT and awaits the orchestrator's build, tests and size
measurement.

# y06 — Embedded asset size: design and step 1

**Status: PARTIAL / UNBUILT.** Step 1 is implemented with a test; no `git`, `cargo`, Python, build,
test or sleep loop was run in this shared checkout. All sizes are `stat` of source files or `find`
of existing build artifacts, not a measured production binary. The orchestrator builds, tests and
commits after the batch lands. Format precedent: `docs/swarm/units/u30-binary-size.md`.

## Route and evidence ledger

- Skills loaded: `flamewm-source-first`, `flamewm-rust-systems`, `flamewm-reference-mining`.
  Workspace `AGENTS.md`, `scripts/version` (read, not run), `.opencode/HARNESS.md`,
  `.opencode/ROUTES.md` and `.agents/{README,CONTRACTS,JOBS}.md` were read.
- Commands used: repository `rg`, `Read`, `stat`, `find`, `head`, `cut`. No compiler, test runner,
  VCS command or script was invoked.
- Contract: no new dependencies, no `Cargo.toml` edit, no `ui/` HTML/CSS edit, visuals pixel-identical,
  at most three production edits. Only `crates/flamewm-lock/src/document.rs` was edited (test inside
  the same file). This document and the unit file are documentation.

## Current structure (file:line)

| mechanism | owner / site | evidence |
| --- | --- | --- |
| Compiled UI documents | each crate's `build.rs` calls `flamewm_render_compiler::compile_file` and writes `OUT_DIR/*.rwr`; runtime embeds with `include_bytes!(concat!(env!("OUT_DIR"), ...))` | `crates/flamewm-shell/build.rs:23-47,48-75,113-115`; `crates/flamewm-shell/src/runtime.rs:37-57`; `crates/flamewm-settings/src/view.rs:8`; `crates/flamewm-desktop/build.rs:12,32-50,73-106` |
| Literal asset bytes | lock wallpaper, four power icons, avatar ring | `crates/flamewm-lock/src/document.rs:21-29` |
| Literal fallback font | system-font miss path only | `crates/flamewm-render-core/src/glyphs.rs:221-228` |
| Literal cursor / terminal logo | cursor role, fetch art | `crates/flamewm-integrations-linux/src/themes/cursor.rs:56`; `crates/flamewm-terminal/src/fetch.rs:12` |
| Test-only source inclusion | `include_str!` of `.rs` fixtures and markup contracts under `#[cfg(test)]` | `crates/flamewm-shell/src/app/tests.rs:11`; `crates/flamewm-settings/src/lib.rs:22-27` |
| Fingerprint coupling | compiler hashes **raw** HTML/CSS bytes into the RWR, so whitespace/comment edits change shipped bytes | `crates/flamewm-render-compiler/src/compile.rs:173-176,198-207`; `crates/flamewm-render-core/src/codec.rs:27` |

## Counted problems

Static snapshot of this checkout (shared checkout, counts can drift between workers):

- **97** `include_bytes!` sites and **212** `include_str!` sites in `crates/**/*.rs`; after excluding
  obvious test paths, **77** and **183** respectively. Most `include_str!` uses are test fixtures,
  not shipped assets (`docs/swarm/units/u30-binary-size.md:54-56`).
- **35** `.rwr` references in crate `build.rs` files; the shell alone compiles **23** page blobs that
  each embed the same `ui/shell/flamewm.css` (40,153 B) and token payload
  (`crates/flamewm-shell/build.rs:23-47`).
- **1** uncompressed P6 raster is staged into a shipped RWR: `assets/raster/wallpaper.ppm`, 2,596,066
  B (`crates/flamewm-desktop/build.rs:12,78-80`; `ui/desktop/index.html:4`).
- **1** large already-lossy WebP is embedded literally in the lock binary: 655,330 B.
- **15** source assets with a proven embedding path are >= 15,919 B (table below); no single
  CSS/HTML whitespace edit is safe because of the raw-byte fingerprint.

## Embedded asset inventory (source bytes)

| asset | bytes | embedding path |
| --- | ---: | --- |
| `assets/raster/wallpaper.ppm` | 2,596,066 | desktop `build.rs:12,78-80` → `ui/desktop/index.html:4` → `flamewm-desktop.rwr` |
| `assets/branding/flamewm-wordmark.png` | 681,415 | `ui/settings.html:12,76` → `flamewm-settings.rwr` |
| `assets/wallpapers/fitz-roy-laguna.webp` | 655,330 | literal `include_bytes!` at `flamewm-lock/src/document.rs:21` |
| `assets/fonts/DejaVuSansMono.ttf` | 343,140 | lazy fallback `include_bytes!` at `flamewm-render-core/src/glyphs.rs:224` |
| `ui/settings.html` | 171,328 | settings `build.rs` → `flamewm-settings.rwr` |
| `ui/explorer/explorer.html` | 74,906 | explorer `build.rs` → `flamewm-explorer.rwr` |
| `ui/shell/flamewm.css` | 40,153 | shell `build.rs:21,97,113-115` → 23 shell RWR blobs |
| `ui/task-manager/task-manager.html` | 38,920 | task-manager `build.rs` → `flamewm-task-manager.rwr` |
| `ui/finder/finder.html` | 25,717 | finder `build.rs` → `flamewm-finder.rwr` |
| `ui/shell/notification-center.html` | 23,784 | shell `build.rs:33,59-61` → `flamewm-notification-center.rwr` |
| `ui/desktop/desktop.css` | 17,544 | desktop `build.rs:33,101-105` → `flamewm-desktop.rwr` |
| `ui/desktop/index.html` | 17,524 | desktop `build.rs:32,68-71` → `flamewm-desktop.rwr` |
| `ui/settings.css` | 16,983 | settings `build.rs` → `flamewm-settings.rwr` |
| `ui/shell/weather.html` | 15,937 | shell `build.rs:31,56` → `flamewm-weather.rwr` |
| `assets/raster/watermark.png` | 15,919 | desktop `build.rs:12,78-80` → `flamewm-desktop.rwr` |
| four lock power SVGs / avatar / terminal logo | 1,864 / 801 / 714 / 524 / 338 / 985 | literal `include_bytes!` in `flamewm-lock/src/document.rs:23-29`, `flamewm-terminal/src/fetch.rs:12` |

Largest compiled artifacts observed under `target-*`: `flamewm-settings.rwr` 8,825,945 B,
`flamewm-terminal-2.rwr` 6,439,206 B, `flamewm-desktop.rwr` 4,672,423 B,
`flamewm-terminal.rwr` 4,289,446 B, `flamewm-terminal-1.rwr` 2,144,326 B. These are stale build
outputs, not a measured shipped binary.

## Proposal for the largest asset

- **Largest by source bytes:** `assets/raster/wallpaper.ppm` (2,596,066 B, uncompressed P6).
  Proposed **compression**: losslessly re-encode to WebP and repoint `DESKTOP_ASSETS` plus
  `ui/desktop/index.html:4`. The decoded RGB must compare byte-for-byte with the PPM before the
  references move. This is step 2 and needs the orchestrator to run the encoder and pixel check.
- **Largest literal `include_bytes!`:** the lock WebP. Implemented **lazy-load** in step 1 below:
  the blob is decoded through `OnceLock::get_or_init` only when a default background is first
  requested, then reused. It reduces decode work and resident copies; it does not remove the
  655,330 B from the binary. Removing those bytes requires shipping the file on disk, which is not
  pixel-safe when the file is absent, so it is deliberately deferred.
- **Largest compiled document:** `flamewm-settings.rwr`. Proposed step 3: make the render compiler
  share the common CSS/token payload across RWR blobs; verify by decoding old and new RWR to the
  same document, never by editing CSS text (fingerprint).

## Target design

One typed owner per embedded mechanism; every embedded blob is either (a) compiled into an RWR by
its crate's build script, or (b) reached through a lazy accessor on the cold path. No product code
holds a second eager copy; no UI becomes an asset authority. Accessors return immutable decoded
data (`&'static RgbaImage`), and callers clone only at the output-raster boundary that already
exists (`cached_cover`). Later compression steps change only bytes that decode to identical pixels.

## Steps that each compile alone

1. **Implemented (UNBUILT).** `crates/flamewm-lock/src/document.rs`: add
   `builtin_background_rgba()` with `OnceLock<Option<RgbaImage>>` and change the default-mode
   closure to `|| builtin_background_rgba().cloned()`. Add a test comparing cached decode to a
   direct `decode_bytes` of the same slice and asserting pointer reuse. No other file changes.
2. Add a lossless WebP twin of `wallpaper.ppm`, extend `DESKTOP_ASSETS` in
   `crates/flamewm-desktop/build.rs:12`, and repoint `ui/desktop/index.html:4`. Compile-agnostic:
   the compiler already decodes WebP via `flamewm-image-core`. Verify identical decoded pixels
   before switching. Undo by restoring the two references.
3. Compiler-level sharing of the repeated CSS/token payload in the 23 shell RWR blobs, owned by
   `flamewm-render-compiler` and its callers. Verify old/new documents decode identically. Undo by
   reverting the compiler and build-script changes.

## Rollback

Each step is independent. Step 1: restore the two lines in `document.rs` to the direct
`decode_bytes(BUILTIN_BACKGROUND, ImageSourceFormat::Webp).ok()` closure and delete the accessor
and test; no asset or artifact changes. Step 2: restore the two references; keep both asset files.
Step 3: revert the compiler/build-script edits only. No runtime state or config is migrated.

## Vendor reference and choice

- Path: `.vendor/coconut-main/crates/core/src/geo.rs:18-24` —
  `static CATALOG: OnceLock<GeoData>` with `get_or_init(|| serde_json::from_str(include_str!(...)))`.
  **Choice:** port the semantic shape (`OnceLock` + `get_or_init`, embedded bytes on the cold path)
  to the lock wallpaper decode. No vendor code, asset or dependency was copied.
- Secondary evidence: `.vendor/coconut-main/crates/plugin-kit/build.rs:6-38` generates one absolute
  path registry for embedded icons (motivates the single-owner rule above);
  `.vendor/ark-master/app/ark.qrc:1-8` shows Qt RCC resource compression as the precedent for
  step 2's build-time compression. Neither was ported into production.

## Safety and verification

- Step 1 decodes the **same** `include_bytes!` slice with the **same** `decode_bytes(..., Webp)`
  call; the cached image is cloned into the existing `cached_cover` path, so every presented pixel
  is unchanged. The new test asserts equal dimensions and pixel bytes plus reuse identity.
- No `ui/` HTML/CSS, asset, manifest or profile was edited, so the compiler's raw-byte fingerprints
  and every RWR blob are byte-identical.
- UNBUILT by contract: no compiler or test runner was invoked; the test above and all size claims
  await the orchestrator's build, tests and binary measurement.

PASS — step 1 is a self-contained, pixel-identical lazy-load port with a test; the remaining
compression work is specified as independent steps 2 and 3 and is deliberately not applied here.

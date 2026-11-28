# w23 — Cargo.lock duplicate-version audit

**Status: UNBUILT.** No `git`, `cargo`, Python, build or test command was run in this shared checkout.
Every fact below comes from `rg` / `read` on the checked-in `Cargo.lock` and `Cargo.toml` files. No
binary size, RSS or runtime identity was measured: the "likely binary size" ranking is a static
dependency-graph estimate, not a measurement. Production edits applied: **0** (this is an audit;
the three-edit cap was not needed — see §4 for why every candidate edit is unsafe without a build).

## Route and evidence ledger

- Route: `flamewm-source-first` loaded; workspace `AGENTS.md`, `scripts/version` (read, never run —
  it calls Python), `.opencode/HARNESS.md`, `.opencode/ROUTES.md` read. No owner overlap: only this
  doc and the w23 unit file were written.
- Commands: `rg -c` and `rg '^name = '` over `Cargo.lock` (394 `[[package]]` entries); `rg -A1` to
  pair each duplicate name with its versions; an `awk` pass attributing each old-version dependency
  line to its enclosing `[[package]]`; `rg` over `crates/*/Cargo.toml` and
  `third_party/smithay/Cargo.toml` for dependency declarations.
- Reference-first: `.vendor/smithay-master/Cargo.toml:33,48,68` uses `calloop = "0.14.5"`,
  `rustix = "1.0.7"`, `x11rb = "0.14.0"` — the current upstream unified set. Our patched
  `third_party/smithay/Cargo.toml` already matches it for `calloop` (`:232-233`) and `rustix`
  (`:307-308`) but **not** for `x11rb` (`:398-400`, `"0.13.0"`) or `winit` (`:387-388`, `"0.30.0"`).
  `.vendor/**` is read-only evidence, never a dependency or runtime input. No clone was needed.

## 1. Duplicate versions found in `Cargo.lock` (same name, >1 version)

| crate | versions in lock | pulled in by | Linux runtime binary impact |
| --- | --- | --- | --- |
| `x11rb`, `x11rb-protocol` | `0.13.2`, `0.14.0` | `smithay` (patched `third_party/smithay/Cargo.toml:398`, `"0.13.0"` → resolves 0.13.2), `winit 0.30.0` (`:387`) | **high** — large X11 protocol bindings; linked into `flamewm-wm-wayland` (XWayland backend) |
| `calloop` | `0.13.0`, `0.14.4` | `winit`, `calloop-wayland-source`, `smithay-client-toolkit` (workspace pins `0.14.4` at `Cargo.toml:16`) | **high** — event loop |
| `rustix`, `linux-raw-sys` | `0.38.44`/`0.4.15`, `0.9.4`, `1.1.4`/`0.12.1` | `calloop 0.13`, `calloop-wayland-source`, `drm`, `smithay-client-toolkit`, `winit` (workspace pins rustix `=1.1.4` at `Cargo.toml:18`) | **medium** — syscall wrappers/constants |
| `hashbrown` | `0.15.5`, `0.17.1` | `fontdue 0.9.5` (`crates/flamewm-render-core/Cargo.toml:14`, `crates/flamewm-wm-wayland/Cargo.toml:72`) | **medium** — text shaping maps |
| `miniz_oxide` | `0.8.9`, `0.9.1` | `png 0.18.1` (pinned at `crates/flamewm-screenshot/Cargo.toml:18`) | **medium** — PNG decode |
| `getrandom` | `0.3.4`, `0.4.3` | `ahash`, `rand_core` | low |
| `digest` / `block-buffer` / `crypto-common` / `cpufeatures` | `0.10.7`/`0.10.4`/`0.1.7`/`0.2.17` vs `0.11.3`/`0.12.1`/`0.2.2`/`0.3.1` | `sha2` → `digest 0.10` chain | low |
| `bitflags` | `1.3.2`, `2.13.1` | `core-graphics`, `core-graphics-types`, `redox_syscall` | none on Linux (macOS/Redox only) |
| `syn` | `2.0.119`, `3.0.5` | `thiserror-impl`, `pin-project-internal`, `tracing-attributes`, `windows-implement`, `zerocopy-derive`, … | none at runtime (proc-macro/build only) |
| `thiserror`, `thiserror-impl` | `1.0.69`, `2.0.21` | `calloop 0.13`, `ndk`, `smithay-client-toolkit` (workspace `2.0.21`) | none at runtime (derive only) |
| `windows-sys` | `0.48.0`, `0.52.0`, `0.59.0`, `0.61.2` | Windows-only transitive deps | none on Linux |
| `windows-targets`, `windows_*` (7 crates) | `0.48.5`, `0.52.6` | `windows-sys` | none on Linux |
| `hermit-abi`, `redox_syscall`, `r-efi`, `jni-sys` | `0.3.9`/`0.5.3`, `0.4.1`/`0.9.5`, `5.3.0`/`6.0.0`, `0.3.1`/`0.4.1` | target-specific (hermit/Redox/UEFI/Android) | none on Linux production |

All 29 duplicate names in the lock are accounted for above. No duplicate is introduced by a
workspace crate's own declaration: every FlameWM crate that uses these libraries declares them
via `.workspace = true` (`Cargo.toml:14-31`), so all such direct edges already resolve to the newer
pinned version.

## 2. Ranking by likely Linux production binary size (static estimate)

1. **`x11rb`/`x11rb-protocol 0.13.2`** — largest; two full protocol crates, from the patched
   smithay's `x11rb = "0.13.0"` and winit 0.30.
2. **`calloop 0.13.0`** — event-loop runtime pulled by the winit/smithay-client-toolkit chain.
3. **`rustix 0.38.44` + `linux-raw-sys 0.4.15`** — syscall layer for the above.
4. **`hashbrown 0.15.5`** — fontdue text shaping, reachable from every binary that links
   `flamewm-render-core`.
5. **`miniz_oxide 0.8.9`** — PNG decode in `flamewm-screenshot`.
6. Small or non-runtime: `getrandom 0.3.4`, `digest 0.10` chain, `cpufeatures 0.2.17`.
7. Zero on Linux: all `windows-*`, `hermit-abi`, `redox_syscall`, `r-efi`, `jni-sys`, `bitflags
   1.3.2`, `syn 2.0`, `thiserror 1.0` — target-gated or proc-macro only.

## 3. Proposed unification edits (recommendations only; NOT applied)

| file:line | current | proposed | effect | risk |
| --- | --- | --- | --- | --- |
| `third_party/smithay/Cargo.toml:398-400` | `x11rb = "0.13.0"` | `x11rb = "0.14.0"` | collapses both `x11rb` and `x11rb-protocol` duplicates onto the workspace pin; matches `.vendor/smithay-master/Cargo.toml:68` | smithay 0.7.0 source may use 0.13 API; must build and re-check the ARGB XWayland reparent frame (`FLAMEWM-PATCH.md`) — **UNBUILT** |
| `third_party/smithay/Cargo.toml:387-388` | `winit = "0.30.0"` | newer winit | would also collapse `calloop 0.13`/`rustix 0.38` | requires API migration in smithay's winit backend and a full build — **UNBUILT**, not recommended this wave |
| `crates/flamewm-screenshot/Cargo.toml:18` | `png = "=0.18.1"` | a png release using `miniz_oxide 0.9` | collapses `miniz_oxide` | changes a decoder; needs image-output pixel comparison — **UNBUILT**, not recommended this wave |

No edit to a root `Cargo.toml` is proposed (held-back changes), and no dependency is added. A
`[patch.crates-io]` unification would also require a root edit, so it is out of scope here.

## 4. Why zero production edits were applied

The brief permits an audit to write only its own doc. The only single-line unification that would
remove a high-ranked duplicate is the `x11rb` bump inside the vendored `third_party/smithay` patch;
that file is a read-only-style reference patch requiring a full build and XWayland runtime check,
which this worker is forbidden to run. Applying it blind would be an unverified, potentially
visual-changing change — contrary to "visuals stay pixel-identical" and "smallest coherent edit".
The remaining duplicates are transitive through third-party crates and cannot be unified from an
individual workspace crate `Cargo.toml` without adding dependencies or editing the root manifest.

**Conclusion:** audit complete; duplicate set identified, ranked and attributed; 0 production edits
applied, so the tree and all visuals are unchanged. All proposed edits above are UNBUILT.

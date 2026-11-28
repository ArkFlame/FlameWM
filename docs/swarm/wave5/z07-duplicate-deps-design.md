# z07 remaster design: duplicate dependencies behind a typed facade

Status: **step 1 implemented and UNBUILT**; steps 2-5 are designed only and are not in this change.
No new dependency was added, no manifest version was changed, and visuals stay pixel-identical.
No `git`, `cargo`, Python, build or test command was run in this shared checkout.

## Method and evidence

- `docs/DUP_DEPS.txt` is absent (checked with `glob`), so the audit reads the checked-in
  `Cargo.lock` (3916 lines) with `rg` and one two-pass `awk` graph pass over the dependency edges.
- "Transitive weight" is defined as the number of **distinct lock packages that reach the stale
  version by following dependency edges backwards**. It is a static graph count, not a build or
  runtime measurement, and it includes target-gated packages. Ties are broken by total direct
  reverse-edge count.
- External fact, fetched once each: crates.io reports `png` `max_version = 0.18.1` and `fontdue`
  `max_version = 0.9.5` (crates.io API, queried 2026-11-28). These bound the two "no edit exists"
  rows below.
- Reference-first: this unit names no `.vendor` reference. Supporting precedent was read only at
  `.vendor/smithay-master/Cargo.toml:33,37,67,68` (`calloop 0.14.5`, `drm 0.14.0`,
  `winit =0.31.0-beta.4`, `x11rb 0.14.0`). **Choice:** use the upstream winit/x11rb pair as the
  target for the two manifest edits in step 4; port no code and add no dependency from `.vendor`.

## Current structure (file:line)

| Location | Role |
| --- | --- |
| `Cargo.lock:2866-2875` | `syn 2.0.119` (stale) beside `syn 3.0.5` (`:2877`). |
| `Cargo.lock:2912-2920` | `thiserror 1.0.69` -> `thiserror-impl 1.0.69`, the largest direct keeper of `syn 2`. |
| `Cargo.lock:225-238`, `:252-260`, `:2813-2834`, `:3627-3663` | `calloop 0.13`, `calloop-wayland-source 0.3`, `smithay-client-toolkit 0.19.2`, `winit 0.30.13`: the stale `thiserror 1`/`rustix 0.38` chain. |
| `Cargo.lock:2547-2558` | `rustix 0.38.44` -> `linux-raw-sys 0.4.15` (`:1883-1887`). |
| `Cargo.lock:560-568` | `drm 0.14.1` also pins `rustix 0.38.44` independently of winit. |
| `Cargo.lock:1942-1951`, `:2331-2339` | `miniz_oxide 0.8.9` under `png 0.18.1`. |
| `Cargo.lock:1545-1554`, `:1382-1388` | `hashbrown 0.15.5` under `fontdue 0.9.5`. |
| `Cargo.toml:16,18,25` | Workspace pins for `calloop 0.14.4`, `rustix 1.1.4`, `x11rb 0.14.0` (all crates already use `.workspace = true`). |
| `third_party/smithay/Cargo.toml:232-233,244-245,307-308,334-335,387-388,398-400` | Patched smithay pins: `calloop 0.14.0`, `drm 0.14.0`, `rustix 1.0.7`, `thiserror 2.0.12`, `winit 0.30.0`, `x11rb 0.13.0`. |
| `crates/flamewm-screenshot/Cargo.toml:18`, `crates/flamewm-render-core/Cargo.toml:14`, `crates/flamewm-wm-wayland/Cargo.toml:72` | Direct pins `png = "=0.18.1"`, `fontdue = "=0.9.5"`, `fontdue = "0.9"`. |
| `crates/flamewm-api/src/lib.rs:6-38` | Public typed-contract module list: the facade owner for shared, engine-neutral records. |
| `docs/swarm/wave2/w23-result-dup-deps.md:26-82` | Earlier audit by binary-size ranking; this unit re-derives from current lock and ranks by graph reach. |

## Counted problems

These are counts from the current lockfile and manifests, not timing measurements.

1. **29** crate names have more than one version in `Cargo.lock`.
2. **5** largest by transitive reverse weight are listed below; `rustix 0.38.44` and
   `thiserror 1.0.69` tie at 8, and `rustix` wins the tie on total direct edges (34 vs 6).
3. **2** of the top 5 (`rustix 0.38.44`, `linux-raw-sys 0.4.15`) are the same stale parent chain,
   so one chain edit removes both.
4. **2** of the top 5 (`miniz_oxide 0.8.9`, `hashbrown 0.15.5`) have **no published parent
   upgrade**: `png 0.18.1` and `fontdue 0.9.5` are the newest releases, so no exact version
   unification edit exists today.
5. **1** of the top 5 (`syn 2.0.119`) has a removable contributor (the `thiserror 1` chain) but
   several unrelated proc-macro owners remain; no single edit removes the family.
6. No workspace crate declares a stale version directly: all such edges are `.workspace = true`
   (verified in step 1 of `docs/perf-loop/LOG-u117-compile-time.md`), so a root-manifest edit
   cannot unify these families.

## Five largest duplicates and their exact unification edits

| # | Stale version | Weight | Exact version unification edit | Risk / status |
| --- | --- | --- | --- | --- |
| 1 | `syn 2.0.119` (`Cargo.lock:2866`) | 58 | `third_party/smithay/Cargo.toml:387-388`: `winit` `0.30.0` -> `=0.31.0-beta.4`, matching `.vendor/smithay-master/Cargo.toml:67`; removes the `thiserror 1` -> `syn 2` path. Other proc-macro owners still need upstream releases. | Winit API change in smithay's backend; needs build and XWayland pixel check. **UNBUILT**. |
| 2 | `miniz_oxide 0.8.9` (`Cargo.lock:1942`) | 27 | **None available.** Parent `png 0.18.1` (`Cargo.lock:2331`, pinned `crates/flamewm-screenshot/Cargo.toml:18`) is the newest published png; an edit requires a future png release. | No edit applied; decoder swap would need pixel comparison if a release appears. |
| 3 | `hashbrown 0.15.5` (`Cargo.lock:1545`) | 23 | **None available.** Parent `fontdue 0.9.5` (`Cargo.lock:1382`, pinned at `crates/flamewm-render-core/Cargo.toml:14`, `crates/flamewm-wm-wayland/Cargo.toml:72`) is the newest published fontdue. | No edit applied; text raster must stay pixel-identical if a release appears. |
| 4 | `linux-raw-sys 0.4.15` (`Cargo.lock:1883`) | 9 | `third_party/smithay/Cargo.toml:244-245`: `drm` `0.14.0` -> a release that requires `rustix 1.x`. Exact target version is not in the local registry cache and has no `.vendor` precedent, so it must be resolved with a registry check before any edit. | DRM/KMS API change; build and runtime gated. **UNBUILT**. |
| 5 | `rustix 0.38.44` (`Cargo.lock:2547`) | 8 | `third_party/smithay/Cargo.toml:387-388`: `winit` `0.30.0` -> `=0.31.0-beta.4` removes the `calloop 0.13`/`calloop-wayland-source 0.3`/`smithay-client-toolkit 0.19` path; `third_party/smithay/Cargo.toml:244-245` `drm` bump is still needed for the second path. | Same build and runtime checks as rows 1 and 4. **UNBUILT**. |

The remaining 24 families are smaller, target-gated, or proc-macro only; they are out of scope for
this batch. The published `x11rb` fix (`third_party/smithay/Cargo.toml:398-400` `0.13.0` ->
`0.14.0`, matching `.vendor/smithay-master/Cargo.toml:68`) is kept in step 4 rather than this
ranking: by reverse reach `x11rb 0.13.2` is only 3.

## Target design

Give build hygiene one typed owner instead of an unstructured `awk` result:
`flamewm_api::dependency_audit` exposes a parsed `Cargo.lock` dependency line, the ranked duplicate
record, and the exact edit (or an explicit `None` when no published release offers one). Product
code does not consume it; a later lockfile regression test does. No runtime authority, no native
access, no new dependency. Every number in the table stays a counted fact with a named method so
the next worker can reproduce it without `cargo tree -d`.

## Steps (each step compiles on its own)

1. **Typed facade and tests (implemented here).** Add
   `crates/flamewm-api/src/dependency_audit.rs` with `LockDependency`, `DuplicateCrate`,
   `parse_dependency_line` and `TOP_DUPLICATES`, and expose it from `crates/flamewm-api/src/lib.rs`.
   Pure `std`, no runtime effect. Tests cover qualified/unqualified lines, rejected field lines,
   descending ranking and explicit absence of the two unavailable edits. UNBUILT.
2. **Reach function.** Add a pure `transitive_weight(edges: &[LockDependency<'_>])` that builds the
   reverse-adjacency map and counts reachable packages; test it on a small in-file fixture. It
   replaces the one-off `awk` pass and must reproduce 58/27/23/9/8 before any manifest change.
3. **Lockfile regression test.** In `flamewm-api/tests/`, read `Cargo.lock` with `include_str!`,
   apply steps 1-2, and assert that the five weights still match the table. A version bump that
   silently reintroduces a stale family then fails without a cargo invocation.
4. **Reference-backed manifest edits.** Apply only the two `.vendor`-backed lines in smithay
   (`winit =0.31.0-beta.4` at `:387`, `x11rb 0.14.0` at `:398`) as one coherent change, then build
   and compare the XWayland ARGB reparent frame against the current output. Not in this batch.
5. **Upstream-gated work.** Revisit `png`, `fontdue` and `drm` only when a published release
   requires the newer transitive version; each needs its own pixel/build gate. Not in this batch.

## Rollback

- Step 1: delete `crates/flamewm-api/src/dependency_audit.rs` and remove its `pub mod` line from
  `crates/flamewm-api/src/lib.rs`; no dependency, binary or visual state to unwind.
- Step 2/3: delete the added pure function/test file only.
- Step 4: restore the two smithay manifest lines and rebuild; the patched smithay ARGB behaviour
  returns with them.
- Step 5: revert the single manifest line for that upstream bump.

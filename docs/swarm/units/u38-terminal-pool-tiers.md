# u38 — Terminal embedded UI pool tiers (binary and decoded document)

**Status: UNBUILT.** No `git`, `cargo`, Python, build, test or sleep-loop command was run in this
shared-checkout unit; the orchestrator builds, tests and commits after the batch lands. Every cost
below is a code budget or a figure already recorded in the hotlist evidence, not a fresh measurement.
No source, build-script, markup or CSS file was edited, so no build fingerprint and no rendered
pixel is changed by this unit.

**Target:** hotlist rank 2 (`docs/PERF_HOTLIST.md:16`) — three pre-baked terminal UI tiers embedded
in `flamewm-terminal` (~0.7 + 2.3 + 6.9 MB of a 13.3–14 MB binary) and the decoded document of the
active tier.

## Owned files (identified and listed before any edit)

- `docs/swarm/units/u38-terminal-pool-tiers.md` — this report; the only file written by this unit.

Read-only evidence (inspected, never edited): `docs/PERF_HOTLIST.md`, `docs/perf-loop/LOG.md`,
`docs/swarm/units/u30-binary-size.md`, `docs/swarm/units/u31-terminal-mem.md`,
`crates/flamewm-terminal/build.rs`, `crates/flamewm-terminal/src/{pool.rs,main.rs,app.rs,ui.rs,store.rs}`,
`crates/flamewm-ui-document/src/template.rs`, `crates/flamewm-render-core/src/codec.rs`,
`crates/flamewm-render-core/src/model/document.rs`, `ui/terminal/{terminal.html,terminal.css}`.

## Route and evidence ledger

- Loaded skills: `flamewm-source-first`, `flamewm-performance-proof`, `flamewm-parallel-execution`,
  `no-adhoc-scripts`. Workspace `AGENTS.md`, `scripts/version` (read only — it calls Python, which is
  forbidden here), `.opencode/HARNESS.md`, `.opencode/ROUTES.md` and
  `.agents/{README,CONTRACTS,JOBS}.md` were read. The handoff overrides the skills' evidence-file and
  role-state protocol: no `runtime/` packet, claim or ledger is created, and this unit is UNBUILT.
- Constraints honoured: no `git`, no `cargo`, no Python, no builds/tests, no `ui/` or `Cargo.toml`
  change, no source change of any kind. Tools used: `read`, `grep`/`rg`, `glob`, `write`.
- Commands/searches: `rg 'tier'` over `crates/flamewm-terminal`, `crates/flamewm-ui-document` and the
  workspace `build.rs`; `rg 'decode_document|replace_document|UI\['` over the terminal crate;
  `rg 'fn encode|fn decode'` over the render compiler; `rg 'img|asset|url\('` over `ui/terminal`;
  `rg '\.run|hidden|display:none'` over `ui/terminal/terminal.css`; reads of the cited functions and
  evidence rows. No binary was built or measured.

## Findings — authority paths and answers to the brief

**Are all three tiers embedded?** Yes. `crates/flamewm-terminal/build.rs:46-65` compiles one RWR
document per `TIERS` entry (`pool.rs:15-25`: 24x80, 48x128, 96x192) into `OUT_DIR`, and
`crates/flamewm-terminal/src/main.rs:29-35` embeds all three with `include_bytes!` into
`const UI: [&[u8]; 3]`, asserting `UI.len() == TIERS.len()`. The hotlist/`LOG.md:388` figures
(~0.7 / 2.3 / 6.9 MB) are the disk cost of these three immutable byte slices.

**Is more than the selected tier ever decoded?** No. `main.rs:178-179` decodes exactly `UI[tier]`,
where `tier = app.pool_tier` (`app.rs:184,257-262,52-70` select the smallest tier that fits the
grid). The only other decode site is `main.rs:255-263`: on `UiControllerEvent::Resized`, a new
document is decoded only when `app.pool_tier != tier`, then
`UiDocumentView::replace_document` (`flamewm-ui-document/src/template.rs:117-123`) assigns the new
document over the old one, dropping it. At most one tier is resident after a frame; the brief
overlap while evaluating `decode_document(UI[tier])` inside `replace_document` is transient and
unavoidable without changing document ownership. The decoded costs in `LOG.md:390-396`
(+1.35 / +4.2 / +12.2 MB; 2 / 9.3 / 22.8 ms) therefore apply to the **one** active tier, never to
the sum of all three.

**Can tiers 1 and 2 be deltas / share sections, or be decoded lazily?** Lazy decode is already the
current behaviour: higher tiers are embedded bytes only until a resize makes them the selected tier,
and `decode_document` (`template.rs:769-774`) uses `decode_static`, which borrows pixels from the
embedded bytes without copying them. Delta storage has no safe foothold in the current format:
`flamewm-render-core/src/codec.rs:21-65` writes a flat, uncompressed stream (magic, version,
`u64 source_fingerprint`, four counts, variables, assets, then one full record per node: tag,
parent/child/sibling indices, id, action, text, image and four style blocks). There is no section
table, no string pool across documents and no compression flag. The three documents have different
node counts (1920 / 6144 / 18432 slots, two nodes each, `build.rs:47-55`), different generated ids
(`r{row}s{slot}`), different sibling indices and different per-node text/style records, so there is
no byte-identical shared section to point at. A delta would require a new wire format, a build-time
diff step and runtime reconstruction into an owned buffer before decode, plus a rebuild and visual
comparison — all forbidden here and unsafe to claim with identical output from static inspection.

**Other levers checked and rejected.** The terminal template has no images (`ui/terminal/terminal.html:1-4`
and no `img`/`url(` hits), so asset sharing cannot reduce these documents. The `-` placeholder in
each run span (`build.rs:50-53`) is load-bearing: `flamewm-render-core/src/model/document.rs:415-428`
requires an element to have exactly one text child for `set_text`, and the compiler drops
whitespace-only text, so removing it breaks every run write. Shrinking `TIERS` would change
`MAX_GRID_ROWS` (`app.rs:49-50`) and run merging (`pool.rs:2-5`), i.e. rendered output. Reusing a
larger tier for a small window would save ~0.7 MB of binary but raise decoded RAM from ~1.35 MB to
~4.2 MB and decode time from 2 to 9.3 ms for that window, the opposite of the brief's decoded-cost
goal; its visual identity would also depend on every extra slot being hidden on first projection
(`ui.rs:159-183,209-236`), which is unproven without a build.

## Rows — file:line, cost, change, risk (zero changes applied)

| file:line | bytes / MB cost | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-terminal/src/main.rs:178-179,255-263`; `crates/flamewm-ui-document/src/template.rs:117-123` | Decoded document of the one active tier: +1.35 / +4.2 / +12.2 MB (`LOG.md:390-396`). | **No change.** The example cut "drop the decoded document of an unused tier" is already implemented: only `UI[tier]` is decoded and the previous document is replaced and dropped. | None introduced. A larger tier is never decoded until the grid grows into it; the resize expression briefly holds two documents before the swap. |
| `crates/flamewm-terminal/src/main.rs:29-35`; `crates/flamewm-terminal/src/app.rs:52-70,257-262` | Higher tiers cost ~2.3 + 6.9 MB of binary disk only, no resident bytes until selected. | **No change.** The example cut "lazy-decode higher tiers" is already implemented: they are embedded byte slices decoded on first use after a resize. | None introduced. With the default 880x540 window (`main.rs:36`, 13 px font at `store.rs:144` gives 8x16 px cells) the startup decode is tier 1, not tier 0, matching `LOG.md:396`. |
| `crates/flamewm-render-core/src/codec.rs:21-65`; `crates/flamewm-terminal/build.rs:46-65` | ~9.2 MB of repeated flat node/style records in tiers 1 and 2. | **No change.** Storing those tiers as deltas or sharing sections needs a new wire format, a build-time diff and runtime reconstruction, and cannot be proven pixel-identical without a build; it also changes build outputs. | High if forced: different node counts, ids, sibling indices and styles mean no existing shared section; a codec change affects every RWR consumer, not just this binary. |
| `crates/flamewm-terminal/build.rs:50-53`; `crates/flamewm-render-core/src/model/document.rs:415-428` | Two nodes per run slot (1920 / 6144 / 18432 slots), the main driver of both binary and decoded size. | **No change.** Removing the one-byte `-` text placeholder would drop the text child that `set_text` requires and whitespace-only text is discarded by the compiler, so run writes would fail. | High if forced: visible run text and geometry would break. |
| `crates/flamewm-terminal/src/pool.rs:15-25,2-5`; `crates/flamewm-terminal/src/app.rs:49-50` | Tier capacity sets maximum grid and run merging. | **No change.** Reducing rows/runs alters `MAX_GRID_ROWS` and how extra cells merge, i.e. rendered output. | High if forced: visual change. |
| `crates/flamewm-terminal/build.rs:59-64`; `docs/swarm/units/u30-binary-size.md:58-66` | Compressing embedded RWR bytes is the only remaining disk-only idea. | **No change.** It needs a new dependency/codec and decompression into owned bytes; the task forbids builds and dependency changes, and the compiler fingerprints raw source bytes into each RWR. | High if forced: build fingerprint/output change and unverified visuals. |

## Verification

UNBUILT by contract: no compiler, test runner, `git`, Python or sleep loop was invoked; no binary size
or RSS was measured. The conclusions are static reads of the current call path plus the recorded
figures in `docs/PERF_HOTLIST.md:16` and `docs/perf-loop/LOG.md:289-290,386-396`. No file other than
this report was written, so there is no pixel, fingerprint or build-output delta for the orchestrator
to verify; it should still run the normal build and tests for the batch as a whole.

PARTIAL — all parts of the brief were answered against current source: all three tiers are embedded,
only the selected tier is ever decoded, and higher tiers are already lazy. Both example cuts are
therefore already in place and cannot be applied again; delta/shared-section storage, compression,
smaller pools and dropping tier 0 were each rejected with the reason and cost recorded above, so
**zero** cuts were applied. The only real remaining lever, lazy run-node allocation inside a tier,
needs a live slot-occupancy RSS measurement and a UI-pipeline redesign outside this no-build unit;
this slice is UNBUILT and awaits the orchestrator.

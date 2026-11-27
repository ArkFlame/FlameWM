# Unit p13-terminal-lazy-nodes
STATUS: PARTIAL

crates/flamewm-terminal: UI node and document allocation at startup (hotlist row 2 tiers): find nodes that can be built lazily when first shown (settings/search/tabs overlays) with identical visuals; implement the smallest safe cut and document RSS expectation.

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-terminal/src/pool.rs:6` (UNBUILT) | TIERS pre-bake 24x80 / 48x128 / 96x192 slots. Only the fitted tier is decoded (`main.rs:179,260`): +1.35 / +4.2 / +12.2 MB RSS and 2 / 9.3 / 22.8 ms for tiers 0 / 1 / 2 (`LOG.md:390-396`); unused tiers stay file-backed. | Added a comment recording the RSS expectation and why per-slot lazy nodes are blocked. | None — comment only, no behaviour or fingerprint change. |
| `crates/flamewm-terminal/build.rs:50-53` (rejected, UNBUILT) | Each slot's `-` text child looks removable, but `set_text_ref` requires a single text child (`render-core/model/document.rs:415-427`) and the compiler drops whitespace-only text (`render-compiler/html.rs:56-58`). | None. | Dropping the placeholder makes every `text_ref` fail with "no text child" — not pixel-identical. |
| `crates/flamewm-terminal/src/app.rs:216`; `app/preferences.rs:19,22` (rejected, UNBUILT) | Settings view, find bar and tab chrome are already built on first use or painted into existing runs; the history trie is read on the first key press. | None. | Deferring the trie changes first-keystroke latency; already declined in u21. |

PARTIAL — no allocation cut was safe in this shared-checkout unit. Tier selection already keeps unused documents file-backed, the settings/search/tabs overlays are built on first use, and the only apparent bulk cut (the per-slot placeholder) is load-bearing for `set_text_ref`. The single edit documents the measured RSS expectation and the blockers; it is UNBUILT and awaits the orchestrator's build and tests. True lazy per-slot nodes need node creation in the document format plus a live slot-occupancy sample; `.vendor/terminator-master` creates GTK widgets on demand but offers no precedent for a compiled fixed-schema document, so that redesign stays deferred.

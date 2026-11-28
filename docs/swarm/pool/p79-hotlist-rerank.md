# Unit p79-hotlist-rerank
STATUS: DONE

Read every Result table in docs/swarm/pool/p*.md (rg '^\|' per file). Rewrite the ranked rows of docs/PERF_HOTLIST.md: group landed cuts (mark landed UNMEASURED), keep the BASELINE section and the correction note as is, add new rows for every finding a worker marked NONE-but-risky or PARTIAL. Docs only.

## Result
DONE. Docs only; 0 production, 0 test edits; no cargo, no build, UNBUILT.
- Read all PARTIAL/NONE result rows from docs/swarm/pool/p*.md (37 files with a PARTIAL or NONE marker).
- docs/PERF_HOTLIST.md: added rows 21-32 (shell catalog heap with TRUE COMPARISON numbers, popup surface retention risk, panic=abort vs panic boundary risk, PARTIAL/UNBUILT edits, idle audits). Added a grouped "Landed cuts (UNMEASURED)" section. BASELINE, POST-WAVE, CORRECTION and TRUE COMPARISON sections untouched.
- TRUE COMPARISON kept: desktop +5.7 MB regression not real (desktop [heap] +0.25 MB, total RSS -0.9 MB); shell +1.8 MB [heap] from catalog warm-up is the only measured delta.
- Open risks to verify next: row 23 (panic boundary under release panic="abort"), row 22 (popup document retention), row 30 (p51 list_stream pool slot).
- Edited: docs/PERF_HOTLIST.md, docs/swarm/pool/p79-hotlist-rerank.md.

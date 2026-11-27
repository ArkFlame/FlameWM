# Unit p79-hotlist-rerank
STATUS: OPEN

Read every Result table in docs/swarm/pool/p*.md (rg '^\|' per file). Rewrite the ranked rows of docs/PERF_HOTLIST.md: group landed cuts (mark landed UNMEASURED), keep the BASELINE section and the correction note as is, add new rows for every finding a worker marked NONE-but-risky or PARTIAL. Docs only.

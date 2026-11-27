# Unit p20-text-cache-bounds
STATUS: OPEN

crates/flamewm-render-core text/glyph caches: confirm each cache has a byte cap and eviction (no unbounded growth with many distinct strings); add the cap if missing, identical visuals.

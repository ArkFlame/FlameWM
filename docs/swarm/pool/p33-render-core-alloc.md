# Unit p33-render-core-alloc
STATUS: OPEN

Crate flamewm-render-core: find per-frame heap allocations in hot paths (rg 'Vec::new|to_vec|clone\(\)|format!' in raster/layout/paint); reuse buffers or borrow. Output pixels must stay identical. Max 3 edits, list the rest.

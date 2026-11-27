# Unit p65-gpu-helper-lazy-rss
STATUS: OPEN

Crate flamewm-gpu-helper (142 MB RSS idle, top baseline RSS row B3): find what it allocates at start (rg 'Vec::with_capacity|vec!\[|Buffer|create_' ), make buffers lazy or sized to the actual output. Pixel output identical. Max 3 edits.

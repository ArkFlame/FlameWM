# Unit p23-warning-sweep
STATUS: OPEN

Dead code sweep: run rg on crates (no cargo) for items the last check flagged never used (see /tmp/claude-1000/check5.txt warnings for crates/flamewm-*, ignore third_party); delete truly dead items or gate them behind cfg(test), max 6 edits.

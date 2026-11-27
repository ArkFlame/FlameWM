// Shared by `build.rs` (which emits one node pool per tier into its compiled UI) and the runtime (which addresses it).
// Each screen row owns `runs` absolutely positioned text nodes; a run is a span of cells with one colour pair.
// Dense full-screen TUIs (OpenCode, htop) alternate colour and block glyphs across a row, so runs per row must cover
// the columns; with fewer slots, ui::fit merges the remaining block rectangles into a text node and discards their
// geometry. Every slot is two document nodes, so tiers trade memory for capacity. The last tier is the maximum grid.
// RSS expectation: decoding the fitted tier adds about 1.35 / 4.2 / 12.2 MB for tiers 0 / 1 / 2
// (2 / 9.3 / 22.8 ms, LOG.md:390-396); only the fitted tier is resident, the others stay file-backed.
// Each slot's "-" text child is load-bearing: `set_text_ref` rejects an element without a single text
// child, and `ui::write_run` writes text before showing the node, so the placeholder is never painted.
// Lazy per-slot allocation needs node creation in the document format and a live slot-occupancy sample;
// both are out of scope until a scoped measurement justifies the redesign.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pool {
    /// Screen rows, including the tab bar.
    pub rows: usize,
    /// Run slots per row.
    pub runs: usize,
}

/// Smallest tier first. A window uses the smallest tier that fits its grid (see `app::pool_tier_for`).
pub const TIERS: [Pool; 3] = [
    Pool { rows: 24, runs: 80 },
    Pool {
        rows: 48,
        runs: 128,
    },
    Pool {
        rows: 96,
        runs: 192,
    },
];

/// Node id of run `slot` on row `row`. Generated ids are `r{row}s{slot}` and must match `build.rs` output.
#[must_use]
pub fn run_id(row: usize, slot: usize) -> String {
    let mut id = String::new();
    run_id_into(&mut id, row, slot);
    id
}

/// Writes the node id of run `slot` on row `row` into `buf`, reusing its allocation for every slot of a row.
pub fn run_id_into(buf: &mut String, row: usize, slot: usize) {
    use std::fmt::Write;
    buf.clear();
    let _ = write!(buf, "r{row}s{slot}");
}
